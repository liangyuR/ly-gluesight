//! 相机组：N 台相机各自配置、状态与重连，共用一条有界帧通道交给检测节拍。
//! 飞拍按触发取图；连续采集的相机只更新缩略图，不送节拍。

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_uint, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use ly_plc::now_ms;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::Sender;

use crate::cycle::{self, CycleHost, Phase};
use crate::frame::{Frame, FrameImage, FramePool};
use crate::mvs::{self, DeviceSummary, FrameInfo};
use crate::recipe::{legacy_camera_id, valid_camera_id, Recipe};
use crate::replay;
use crate::sim::Scenario;
use crate::simimage::{self, PoseError};

/// 帧通道容量：检测节拍处理不过来时丢新帧并计数，不在内存里无限堆积。
pub const FRAME_QUEUE: usize = 64;

/// 模拟相机合成飞拍帧所需的信息：哪个配方的第几个拍照点、什么场景、机器人偏差。
pub struct SimRender {
    pub recipe: Arc<Recipe>,
    pub k: usize,
    pub scenario: Scenario,
    pub pose: PoseError,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraSource {
    Sim,
    Mvs,
    /// 从目录读图（帧录制或现场采的图）
    Replay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Acquisition {
    /// 每个触发出一帧（飞拍）
    Triggered,
    /// 按固定帧率连续采集，只更新缩略图，不进入节拍
    FreeRun,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraConfig {
    /// 相机编号，配方用它引用相机；建相机时分配，之后不变（增删别的相机也不变）
    pub id: String,
    pub name: String,
    pub source: CameraSource,
    pub serial: String,
    pub acquisition: Acquisition,
    pub fps: f32,
    pub trigger_source: String,
    pub trigger_activation: String,
    pub trigger_delay_us: f32,
    pub debouncer_us: i64,
    pub exposure_us: f32,
    pub gain_db: f32,
    pub strobe: bool,
    pub chunk: bool,
    pub replay_dir: String,
    /// 回放通道（从 1 开始），0 表示取目录里的第一个通道
    pub replay_channel: u32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "相机".into(),
            source: CameraSource::Sim,
            serial: String::new(),
            acquisition: Acquisition::Triggered,
            fps: 20.0,
            trigger_source: "Line0".into(),
            trigger_activation: "RisingEdge".into(),
            trigger_delay_us: 0.0,
            debouncer_us: 5,
            exposure_us: 60.0,
            gain_db: 6.0,
            strobe: true,
            chunk: true,
            replay_dir: String::new(),
            replay_channel: 0,
        }
    }
}

impl CameraConfig {
    fn validate(&self) -> Result<(), String> {
        if !(1.0..=1_000_000.0).contains(&self.exposure_us) {
            return Err("曝光时间需在 1–1000000 µs 之间".into());
        }
        if !["Line0", "Software"].contains(&self.trigger_source.as_str()) {
            return Err("触发源只能是 Line0 或 Software".into());
        }
        if self.acquisition == Acquisition::FreeRun && !(1.0..=500.0).contains(&self.fps) {
            return Err("连续采集帧率需在 1–500 fps 之间".into());
        }
        if self.source == CameraSource::Replay && self.replay_dir.trim().is_empty() {
            return Err("回放相机需要填写图片目录".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RigFile {
    cameras: Vec<CameraConfig>,
    /// 下一台新相机的编号 "cam{next_id}"：删掉的相机编号不再分给别的相机
    next_id: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraStatus {
    pub cam: u8,
    pub id: String,
    pub name: String,
    pub source: CameraSource,
    pub acquisition: Acquisition,
    pub ready: bool,
    pub message: String,
    pub device: Option<DeviceSummary>,
    pub sdk_version: Option<String>,
    pub frames: u64,
    pub fps: f32,
    pub max_fps: Option<f32>,
    pub lost_packets: u64,
    /// 检测节拍来不及取走、被丢弃的帧
    pub dropped_frames: u64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DryFrame {
    pub cam: u8,
    pub t_ms: f64,
    pub frame_counter: u64,
    pub trigger_counter: u64,
    pub lost_packets: u32,
}

struct Preview {
    width: u32,
    height: u32,
    full_width: u32,
    full_height: u32,
    data: Vec<u8>,
}

/// 降采样到宽度不超过 960 的缩略图。
fn make_preview(src: &[u8], w: usize, h: usize) -> Preview {
    let step = w.div_ceil(960).max(1);
    let (pw, ph) = (w / step, h / step);
    let mut data = Vec::with_capacity(pw * ph);
    for y in 0..ph {
        let row = &src[y * step * w..];
        data.extend((0..pw).map(|x| row[x * step]));
    }
    Preview { width: pw as u32, height: ph as u32, full_width: w as u32, full_height: h as u32, data }
}

/// 模拟帧并行合成、按帧计数顺序交付：节拍按帧计数的先后推 k，乱序会让后面的帧落错拍照点。
#[derive(Default)]
struct Reorder {
    next: u64,
    pending: BTreeMap<u64, Option<Frame>>,
}

struct ReplayState {
    files: Vec<PathBuf>,
    next: usize,
    /// 帧录制目录：各帧相对工件开始的时刻（ms）。有它时每件从第一张放
    times: Option<Vec<i64>>,
}

/// 相机组共用的部分。
struct RigShared {
    app: AppHandle,
    tx: Sender<Frame>,
    pool: FramePool,
    /// 需要整帧图像（图像测量或帧录制）；模拟测量时不必拷整帧
    capture: AtomicBool,
    dry_run: Mutex<Option<(Instant, Vec<DryFrame>)>>,
}

/// 单台相机的交付通道，取图回调与模拟 / 回放共用。
struct Shared {
    cam: u8,
    rig: Arc<RigShared>,
    free_run: AtomicBool,
    frames: AtomicU64,
    lost_packets: AtomicU64,
    dropped: AtomicU64,
    recent: Mutex<VecDeque<Instant>>,
    preview: Mutex<Option<Preview>>,
    preview_at: Mutex<Option<Instant>>,
    last_full: Mutex<Option<Arc<FrameImage>>>,
    /// last_full 每换一次 +1
    full_seq: AtomicU64,
    /// 下一帧不管要不要整帧都留一份（标定试测、软触发取图）
    grab: AtomicBool,
    /// 还没到的软触发帧数：到了就标成手动取的帧。最后一次软触发 10 s 后还没到的不再等（manual_until，ms）
    manual: AtomicU32,
    manual_until: AtomicI64,
    last_emit: Mutex<Option<Instant>>,
    disconnected: AtomicBool,
    order: Mutex<Reorder>,
}

impl Shared {
    fn capture(&self) -> bool {
        self.rig.capture.load(Ordering::Relaxed)
    }

    /// 连续采集的帧只更新缩略图，不送节拍。
    fn wanted(&self) -> bool {
        !self.free_run.load(Ordering::Relaxed)
    }

    /// 连续采集时缩略图最多每 100 ms 做一张（前端 250 ms 才取一次）；触发采集的帧少，每帧都做，示教裁模板用的就是显示的那帧。
    fn preview_due(&self) -> bool {
        if !self.free_run.load(Ordering::Relaxed) {
            return true;
        }
        let now = Instant::now();
        let mut at = self.preview_at.lock().unwrap();
        if at.is_some_and(|t| now.duration_since(t) < Duration::from_millis(100)) {
            return false;
        }
        *at = Some(now);
        true
    }

    fn set_preview(&self, img: &FrameImage) {
        if self.preview_due() {
            *self.preview.lock().unwrap() = Some(make_preview(&img.pixels, img.width as usize, img.height as usize));
        }
    }

    fn deliver_in_order(&self, seq: u64, frame: Option<Frame>) {
        let mut order = self.order.lock().unwrap();
        // 每台相机的帧序号从 1 开始；不能拿先到的那帧当起点，否则先合成完的第 2 帧会把第 1 帧挤到后面
        if order.next == 0 {
            order.next = 1;
        }
        if seq < order.next {
            drop(order);
            if let Some(f) = frame {
                self.deliver(f);
            }
            return;
        }
        order.pending.insert(seq, frame);
        loop {
            let next = order.next;
            let Some(f) = order.pending.remove(&next) else { break };
            order.next += 1;
            if let Some(f) = f {
                self.deliver(f);
            }
        }
    }

    fn deliver(&self, mut frame: Frame) {
        if let Some(img) = &frame.image {
            *self.last_full.lock().unwrap() = Some(img.clone());
            self.full_seq.fetch_add(1, Ordering::SeqCst);
        }
        if now_ms() > self.manual_until.load(Ordering::SeqCst) {
            self.manual.store(0, Ordering::SeqCst);
        }
        frame.manual = self.manual.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.lost_packets.fetch_add(frame.lost_packets as u64, Ordering::Relaxed);
        let now = Instant::now();
        {
            let mut recent = self.recent.lock().unwrap();
            recent.push_back(now);
            while recent.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3)) {
                recent.pop_front();
            }
        }
        {
            // 连续采集时前端只要看到"有帧在来"，每台相机每秒最多通知 10 次
            let mut last = self.last_emit.lock().unwrap();
            if !self.free_run.load(Ordering::Relaxed) || last.is_none_or(|t| now.duration_since(t) > Duration::from_millis(100)) {
                *last = Some(now);
                let _ = self.rig.app.emit("camera://frame", &frame);
            }
        }
        if let Some((started, frames)) = self.rig.dry_run.lock().unwrap().as_mut() {
            frames.push(DryFrame {
                cam: self.cam,
                t_ms: started.elapsed().as_secs_f64() * 1000.0,
                frame_counter: frame.frame_counter,
                trigger_counter: frame.trigger_counter,
                lost_packets: frame.lost_packets,
            });
            return;
        }
        if !self.wanted() {
            return;
        }
        if self.rig.tx.try_send(frame).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fps(&self) -> f32 {
        let recent = self.recent.lock().unwrap();
        match (recent.front(), recent.back()) {
            (Some(a), Some(b)) if recent.len() > 1 && Instant::now().duration_since(*b) < Duration::from_secs(3) => {
                (recent.len() - 1) as f32 / b.duration_since(*a).as_secs_f32().max(0.001)
            }
            _ => 0.0,
        }
    }
}

extern "system" fn on_image(data: *mut u8, info: *mut FrameInfo, user: *mut c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if info.is_null() || user.is_null() {
            return;
        }
        let shared = unsafe { &*(user as *const Shared) };
        let info = unsafe { &*info };
        let frame_counter = if info.frame_counter != 0 { info.frame_counter } else { info.frame_num } as u64;
        let trigger_counter = if info.trigger_index != 0 { info.trigger_index as u64 } else { frame_counter };
        let mono = info.pixel_type == mvs::PIXEL_MONO8 && !data.is_null();
        let w = if info.extend_width != 0 { info.extend_width } else { info.width as u32 };
        let h = if info.extend_height != 0 { info.extend_height } else { info.height as u32 };
        let src = mono.then(|| unsafe { std::slice::from_raw_parts(data, (w * h) as usize) });
        if let Some(src) = src.filter(|_| shared.preview_due()) {
            *shared.preview.lock().unwrap() = Some(make_preview(src, w as usize, h as usize));
        }
        let keep = src.is_some() && ((shared.capture() && shared.wanted()) | shared.grab.swap(false, Ordering::SeqCst));
        let image = src.filter(|_| keep).map(|src| Arc::new(shared.rig.pool.copy(w, h, src)));
        shared.deliver(Frame { cam: shared.cam, frame_counter, trigger_counter, lost_packets: info.lost_packet, ts: now_ms(), manual: false, image });
    }));
}

extern "system" fn on_exception(msg: c_uint, user: *mut c_void) {
    if msg == mvs::MV_EXCEPTION_DEV_DISCONNECT && !user.is_null() {
        let shared = unsafe { &*(user as *const Shared) };
        shared.disconnected.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct DeviceState {
    message: String,
    warnings: Vec<String>,
    max_fps: Option<f32>,
}

pub struct CameraSlot {
    device: Mutex<Option<mvs::Device>>,
    shared: Arc<Shared>,
    config: Mutex<CameraConfig>,
    state: Mutex<DeviceState>,
    frame_seq: AtomicU64,
    trigger_seq: AtomicU64,
    replay: Mutex<Option<ReplayState>>,
}

impl Drop for CameraSlot {
    fn drop(&mut self) {
        // 先停取流，回调里用的 Shared 才能安全释放
        self.device.lock().unwrap().take();
    }
}

impl CameraSlot {
    fn new(rig: &Arc<RigShared>, cam: u8, config: CameraConfig) -> Self {
        let slot = Self {
            device: Mutex::new(None),
            shared: Arc::new(Shared {
                cam,
                rig: rig.clone(),
                free_run: AtomicBool::new(config.acquisition == Acquisition::FreeRun),
                frames: AtomicU64::new(0),
                lost_packets: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                recent: Mutex::new(VecDeque::new()),
                preview: Mutex::new(None),
                preview_at: Mutex::new(None),
                last_full: Mutex::new(None),
                full_seq: AtomicU64::new(0),
                grab: AtomicBool::new(false),
                manual: AtomicU32::new(0),
                manual_until: AtomicI64::new(0),
                last_emit: Mutex::new(None),
                disconnected: AtomicBool::new(false),
                order: Mutex::new(Reorder::default()),
            }),
            config: Mutex::new(config),
            state: Mutex::new(DeviceState::default()),
            frame_seq: AtomicU64::new(0),
            trigger_seq: AtomicU64::new(0),
            replay: Mutex::new(None),
        };
        if slot.config().source == CameraSource::Replay {
            let _ = slot.load_replay();
        }
        slot
    }

    pub fn config(&self) -> CameraConfig {
        self.config.lock().unwrap().clone()
    }

    fn set_config(&self, config: CameraConfig) {
        self.shared.free_run.store(config.acquisition == Acquisition::FreeRun, Ordering::Relaxed);
        *self.config.lock().unwrap() = config;
        self.state.lock().unwrap().warnings.clear();
    }

    pub fn status(&self) -> CameraStatus {
        let config = self.config();
        let ready = self.is_ready();
        let (state_message, warnings, max_fps) = {
            let s = self.state.lock().unwrap();
            (s.message.clone(), s.warnings.clone(), s.max_fps)
        };
        let device = self.device.lock().unwrap().as_ref().map(|d| d.summary.clone());
        let replay = self.replay.lock().unwrap().as_ref().map(|r| (r.files.len(), r.next));
        let message = match (config.source, replay) {
            (CameraSource::Sim, _) => match config.acquisition {
                Acquisition::Triggered => "模拟相机：收到触发后约 180 ms 交付一帧".to_string(),
                Acquisition::FreeRun => "模拟相机：连续采集不合成画面，飞拍要用触发采集".to_string(),
            },
            (CameraSource::Replay, Some((n, next))) => format!("回放 {n} 帧 · 下一帧第 {} 张", next + 1),
            _ => state_message,
        };
        CameraStatus {
            cam: self.shared.cam,
            id: config.id,
            name: config.name,
            source: config.source,
            acquisition: config.acquisition,
            ready,
            message,
            device,
            sdk_version: mvs::api().ok().map(|a| a.version.clone()),
            frames: self.shared.frames.load(Ordering::Relaxed),
            fps: self.shared.fps(),
            max_fps,
            lost_packets: self.shared.lost_packets.load(Ordering::Relaxed),
            dropped_frames: self.shared.dropped.load(Ordering::Relaxed),
            warnings,
        }
    }

    pub fn last_full(&self) -> Option<Arc<FrameImage>> {
        self.shared.last_full.lock().unwrap().clone()
    }

    /// 等下一帧留一张整帧：海康相机空闲时不一定拷整帧，示教、标定取样要现取。
    pub fn grab_full(&self, timeout: Duration) -> Option<Arc<FrameImage>> {
        let before = self.shared.full_seq.load(Ordering::SeqCst);
        self.shared.grab.store(true, Ordering::SeqCst);
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            if self.shared.full_seq.load(Ordering::SeqCst) != before {
                return self.last_full();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.shared.grab.store(false, Ordering::SeqCst);
        None
    }

    /// 只看就绪与否，不拼状态文字（节拍每 20 ms 查一次）。
    pub fn is_ready(&self) -> bool {
        let source = self.config.lock().unwrap().source;
        match source {
            CameraSource::Sim => true,
            CameraSource::Mvs => self.device.lock().unwrap().is_some() && !self.shared.disconnected.load(Ordering::SeqCst),
            CameraSource::Replay => self.replay.lock().unwrap().is_some(),
        }
    }

    fn next_counters(&self) -> (u64, u64) {
        (self.frame_seq.fetch_add(1, Ordering::SeqCst) + 1, self.trigger_seq.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// 下一张回放图，到末尾后从头再来。
    fn next_replay(&self) -> Option<PathBuf> {
        let mut guard = self.replay.lock().unwrap();
        let r = guard.as_mut()?;
        let p = r.files[r.next % r.files.len()].clone();
        r.next = (r.next + 1) % r.files.len();
        Some(p)
    }

    /// 扫描回放目录。扫描（网络盘上可能很慢）时不持有任何锁。
    fn load_replay(&self) -> Result<String, String> {
        let config = self.config();
        let dir = config.replay_dir.trim().to_string();
        let path = std::path::Path::new(&dir);
        let scanned = replay::scan(path, config.replay_channel).and_then(|files| {
            // 先读一张的文件头：格式解不开（或文件坏了）就不报就绪
            replay::probe(&files[0])?;
            let times = replay::timeline(path, &files);
            Ok((files, times))
        });
        match scanned {
            Ok((files, times)) => {
                let msg = format!("回放目录 {dir}：{} 帧{}", files.len(), if times.is_some() { "（帧录制）" } else { "" });
                *self.replay.lock().unwrap() = Some(ReplayState { files, next: 0, times });
                Ok(msg)
            }
            Err(e) => {
                *self.replay.lock().unwrap() = None;
                self.state.lock().unwrap().message = e.clone();
                Err(e)
            }
        }
    }

    /// 回放的是一件的帧录制时回到第一张：每件都从头放，多一帧少一帧不会错位到下一件。
    fn rewind_recording(&self) {
        if let Some(r) = self.replay.lock().unwrap().as_mut().filter(|r| r.times.is_some()) {
            r.next = 0;
        }
    }

    /// 按触发出一帧。模拟相机合成（render 为空时只有元数据），回放读下一张，海康发软触发。
    /// `lose_in_transfer` 仅模拟相机使用：相机已曝光但帧在传输中丢失，主机侧表现为帧计数跳号。
    pub fn trigger(&self, lose_in_transfer: bool, render: Option<SimRender>) -> bool {
        let config = self.config();
        let cam = self.shared.cam;
        match config.source {
            CameraSource::Sim => {
                let (frame_counter, trigger_counter) = self.next_counters();
                let shared = self.shared.clone();
                if lose_in_transfer {
                    shared.deliver_in_order(frame_counter, None);
                    return true;
                }
                tauri::async_runtime::spawn(async move {
                    let started = Instant::now();
                    let image = match render {
                        Some(r) => tauri::async_runtime::spawn_blocking(move || simimage::render(&r.recipe, r.k, r.scenario, r.pose, r.seed)).await.ok(),
                        None => None,
                    };
                    let transfer = Duration::from_millis(180);
                    if started.elapsed() < transfer {
                        tokio::time::sleep(transfer - started.elapsed()).await;
                    }
                    let image = image.map(|img| {
                        shared.set_preview(&img);
                        Arc::new(img)
                    });
                    let frame = Frame { cam, frame_counter, trigger_counter, lost_packets: 0, ts: now_ms(), manual: false, image };
                    shared.deliver_in_order(frame_counter, Some(frame));
                });
                true
            }
            CameraSource::Mvs => {
                config.trigger_source == "Software" && self.device.lock().unwrap().as_ref().is_some_and(|d| d.command("TriggerSoftware").is_ok())
            }
            CameraSource::Replay => {
                let Some(path) = self.next_replay() else { return false };
                let (frame_counter, trigger_counter) = self.next_counters();
                let shared = self.shared.clone();
                tauri::async_runtime::spawn(async move {
                    let loaded = tauri::async_runtime::spawn_blocking(move || replay::load(&path)).await.map_err(|e| e.to_string());
                    match loaded.and_then(|r| r) {
                        Ok(img) => {
                            shared.set_preview(&img);
                            let image = Some(Arc::new(img));
                            shared.deliver_in_order(frame_counter, Some(Frame { cam, frame_counter, trigger_counter, lost_packets: 0, ts: now_ms(), manual: false, image }));
                        }
                        Err(e) => {
                            cycle::log(&shared.rig.app, "err", "回放", e);
                            shared.deliver_in_order(frame_counter, None);
                        }
                    }
                });
                true
            }
        }
    }

    fn close(&self) {
        self.device.lock().unwrap().take();
        self.shared.disconnected.store(false, Ordering::SeqCst);
    }

    fn open(&self) -> Result<String, String> {
        self.close();
        let result = self.try_open();
        if let Err(e) = &result {
            self.state.lock().unwrap().message = e.clone();
        }
        result
    }

    fn try_open(&self) -> Result<String, String> {
        let config = self.config();
        let device = mvs::Device::open(&config.serial, &self.claimed_serials())?;
        let (warnings, max_fps) = apply(&device, &config);
        device.start(on_image, on_exception, Arc::as_ptr(&self.shared) as *mut c_void)?;
        let mut msg = format!("已连接 {} · {}", device.summary.model, device.summary.serial);
        let serial = device.summary.serial.clone();
        *self.device.lock().unwrap() = Some(device);
        // 序列号留空的相机开到哪台就固定成哪台：重启、增删相机后还是这台，标定跟着它。打开期间用户另选了序列号就不动
        let pinned = config.serial.is_empty() && {
            let mut c = self.config.lock().unwrap();
            let empty = c.serial.is_empty();
            if empty {
                c.serial = serial.clone();
            }
            empty
        };
        if pinned {
            if let Some(host) = self.shared.rig.app.try_state::<CycleHost>() {
                let _ = host.camera.save();
            }
            msg.push_str(&format!("（序列号已固定为 {serial}）"));
        }
        let mut state = self.state.lock().unwrap();
        state.warnings = warnings;
        state.max_fps = max_fps;
        state.message = msg.clone();
        Ok(msg)
    }

    /// 相机组里别的海康相机已经打开或指定了的序列号：序列号留空的相机只能开剩下的。
    fn claimed_serials(&self) -> Vec<String> {
        let Some(host) = self.shared.rig.app.try_state::<CycleHost>() else { return Vec::new() };
        host.camera
            .slots()
            .iter()
            .filter(|s| !Arc::ptr_eq(&s.shared, &self.shared))
            .flat_map(|s| {
                let c = s.config();
                let opened = s.device.lock().unwrap().as_ref().map(|d| d.summary.serial.clone());
                [(c.source == CameraSource::Mvs && !c.serial.is_empty()).then_some(c.serial), opened]
            })
            .flatten()
            .collect()
    }

    /// 应用新配置：海康相机重新打开，回放重新扫描目录。返回相机未接受的参数。
    fn apply_config(&self) -> Result<Vec<String>, String> {
        match self.config().source {
            CameraSource::Sim => {
                self.close();
                Ok(Vec::new())
            }
            CameraSource::Replay => {
                self.close();
                self.load_replay()?;
                Ok(Vec::new())
            }
            CameraSource::Mvs => {
                self.open()?;
                Ok(self.state.lock().unwrap().warnings.clone())
            }
        }
    }
}

/// 写入相机参数。个别型号不支持的节点记为警告，不阻止取流。
fn apply(d: &mvs::Device, c: &CameraConfig) -> (Vec<String>, Option<f32>) {
    let mut warnings = Vec::new();
    let mut must = |r: Result<(), String>| {
        if let Err(e) = r {
            warnings.push(e);
        }
    };
    must(d.set_enum("AcquisitionMode", "Continuous"));
    must(d.set_enum("PixelFormat", "Mono8"));
    match c.acquisition {
        Acquisition::Triggered => {
            let _ = d.set_enum("TriggerSelector", "FrameBurstStart");
            must(d.set_enum("TriggerMode", "On"));
            must(d.set_enum("TriggerSource", &c.trigger_source));
            if c.trigger_source == "Line0" {
                must(d.set_enum("TriggerActivation", &c.trigger_activation));
                must(d.set_float("TriggerDelay", c.trigger_delay_us));
                must(d.set_enum("LineSelector", "Line0"));
                must(d.set_int("LineDebouncerTime", c.debouncer_us));
            }
        }
        Acquisition::FreeRun => {
            must(d.set_enum("TriggerMode", "Off"));
            must(d.set_bool("AcquisitionFrameRateEnable", true));
            must(d.set_float("AcquisitionFrameRate", c.fps));
        }
    }
    must(d.set_enum("ExposureAuto", "Off"));
    must(d.set_float("ExposureTime", c.exposure_us));
    must(d.set_enum("GainAuto", "Off"));
    must(d.set_float("Gain", c.gain_db));
    if d.set_enum("LineSelector", "Line1").is_ok() {
        if c.strobe {
            must(d.set_enum("LineMode", "Strobe"));
            must(d.set_enum("LineSource", "ExposureStartActive"));
            must(d.set_bool("StrobeEnable", true));
        } else {
            let _ = d.set_bool("StrobeEnable", false);
        }
    }
    if c.chunk {
        must(d.set_bool("ChunkModeActive", true));
        match d.enum_entries("ChunkSelector") {
            Ok(entries) => {
                let wanted: Vec<_> = entries
                    .iter()
                    .filter(|e| {
                        let l = e.to_lowercase();
                        (l.contains("frame") && l.contains("count")) || l.contains("trigger") || l.contains("timestamp")
                    })
                    .collect();
                if wanted.is_empty() {
                    must(Err(format!("相机不支持帧计数 / 触发计数 Chunk（可选项：{}）", entries.join("、"))));
                }
                for e in wanted {
                    must(d.set_enum("ChunkSelector", e));
                    must(d.set_bool("ChunkEnable", true));
                }
            }
            Err(e) => must(Err(e)),
        }
    } else {
        let _ = d.set_bool("ChunkModeActive", false);
    }
    if d.summary.transport == "GigE" {
        if let Some(size) = d.optimal_packet_size() {
            must(d.set_int("GevSCPSPacketSize", size));
        }
    }
    let max_fps = d.get_float("ResultingFrameRate").ok();
    (warnings, max_fps)
}

pub struct CameraRig {
    rig: Arc<RigShared>,
    slots: RwLock<Vec<Arc<CameraSlot>>>,
    path: PathBuf,
    next_id: Mutex<u32>,
    /// 写 cameras.json 一次一个：连上相机固定序列号的后台保存会和界面的保存同时来
    save_lock: Mutex<()>,
    /// 增删相机（相机组序号重排）一次 +1：按序号记下的相机就作废了
    generation: AtomicU64,
    /// 启动时读配置遇到的问题，节拍启动后写进日志
    pub notes: Vec<String>,
}

impl CameraRig {
    pub fn new(app: &AppHandle, tx: Sender<Frame>) -> Result<Self, String> {
        let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
        let path = dir.join("cameras.json");
        let mut notes = Vec::new();
        let mut file: RigFile = match crate::fsio::read_json(&path) {
            Ok(f) => f.unwrap_or_default(),
            Err(note) => {
                notes.push(note);
                RigFile::default()
            }
        };
        if file.cameras.is_empty() {
            // 单相机时代的 camera.json 迁移成相机组的第 1 台
            let old = std::fs::read_to_string(dir.join("camera.json")).ok().and_then(|s| serde_json::from_str::<CameraConfig>(&s).ok());
            file.cameras.push(CameraConfig { name: "相机 1".into(), ..old.unwrap_or_default() });
        }
        let assigned = assign_ids(&mut file.cameras, &mut file.next_id);
        let rig = Arc::new(RigShared {
            app: app.clone(),
            tx,
            pool: FramePool::new(48),
            capture: AtomicBool::new(false),
            dry_run: Mutex::new(None),
        });
        let slots = file.cameras.into_iter().enumerate().map(|(i, c)| Arc::new(CameraSlot::new(&rig, i as u8, c))).collect();
        let mut this = Self { rig, slots: RwLock::new(slots), path, next_id: Mutex::new(file.next_id), save_lock: Mutex::new(()), generation: AtomicU64::new(0), notes };
        if assigned {
            if let Err(e) = this.save() {
                this.notes.push(format!("相机编号没能写回 cameras.json：{e}"));
            }
        }
        Ok(this)
    }

    pub fn index_of(&self, id: &str) -> Option<u8> {
        self.slots().iter().position(|s| s.config.lock().unwrap().id == id).map(|i| i as u8)
    }

    /// 编号 → 此刻的相机组序号；不在相机组里时返回给人看的原因。
    pub fn require(&self, id: &str) -> Result<u8, String> {
        self.index_of(id).ok_or_else(|| format!("相机组里没有编号为 {id} 的相机"))
    }

    pub fn resolve(&self, ids: &[String]) -> Result<Vec<u8>, String> {
        ids.iter().map(|id| self.require(id)).collect()
    }

    /// 各相机累计被丢弃的帧。
    pub fn dropped_total(&self) -> u64 {
        self.slots().iter().map(|s| s.shared.dropped.load(Ordering::Relaxed)).sum()
    }

    fn save(&self) -> Result<(), String> {
        let _one = self.save_lock.lock().unwrap();
        let cameras = self.configs();
        let file = RigFile { cameras, next_id: *self.next_id.lock().unwrap() };
        crate::fsio::write_atomic(&self.path, &serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?)
    }

    fn new_id(&self) -> String {
        let configs = self.configs();
        take_id(&configs, &mut self.next_id.lock().unwrap())
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn slots(&self) -> Vec<Arc<CameraSlot>> {
        self.slots.read().unwrap().clone()
    }

    pub fn slot(&self, cam: usize) -> Option<Arc<CameraSlot>> {
        self.slots.read().unwrap().get(cam).cloned()
    }

    pub fn configs(&self) -> Vec<CameraConfig> {
        self.slots().iter().map(|s| s.config()).collect()
    }

    pub fn statuses(&self) -> Vec<CameraStatus> {
        self.slots().iter().map(|s| s.status()).collect()
    }

    pub fn all_ready(&self, cams: &[u8]) -> bool {
        cams.iter().all(|&c| self.slot(c as usize).is_some_and(|s| s.is_ready()))
    }

    /// 这些相机都就绪；否则返回第一台的原因。
    pub fn check_ready_at(&self, cams: &[u8]) -> Result<(), String> {
        for &c in cams {
            let slot = self.slot(c as usize).ok_or("相机组改过了")?;
            if !slot.is_ready() {
                let st = slot.status();
                return Err(format!("{}未就绪：{}", st.name, st.message));
            }
        }
        Ok(())
    }

    /// 开工前：回放帧录制的相机回到第一张；没等到的软触发帧不再算数。
    pub fn begin_part(&self, cams: &[u8]) {
        for &c in cams {
            if let Some(s) = self.slot(c as usize) {
                s.rewind_recording();
                s.shared.manual.store(0, Ordering::SeqCst);
            }
        }
    }

    /// 取图回调是否拷贝整帧。
    pub fn set_capture(&self, on: bool) {
        self.rig.capture.store(on, Ordering::Relaxed);
    }

    pub fn trigger(&self, cam: u8, lose_in_transfer: bool, render: Option<SimRender>) -> bool {
        self.slot(cam as usize).is_some_and(|s| s.trigger(lose_in_transfer, render))
    }

    pub fn last_full(&self, cam: u8) -> Option<Arc<FrameImage>> {
        self.slot(cam as usize).and_then(|s| s.last_full())
    }

    fn rebuild(&self, app: &AppHandle, configs: Vec<CameraConfig>) {
        let slots: Vec<Arc<CameraSlot>> = configs.into_iter().enumerate().map(|(i, c)| Arc::new(CameraSlot::new(&self.rig, i as u8, c))).collect();
        // 旧相机在锁外释放：关海康相机要一会儿，别让别的线程卡在读相机组上
        let old = std::mem::replace(&mut *self.slots.write().unwrap(), slots.clone());
        self.generation.fetch_add(1, Ordering::SeqCst);
        drop(old);
        for s in &slots {
            supervise(app, Arc::downgrade(s));
        }
    }

    pub fn start(app: &AppHandle) {
        for s in app.state::<CycleHost>().camera.slots() {
            supervise(app, Arc::downgrade(&s));
        }
    }
}

/// 每台相机一个后台循环：海康相机掉线或未打开时每 2 s 重连。相机被移出相机组后循环自己结束。
fn supervise(app: &AppHandle, slot: Weak<CameraSlot>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_error: Option<String> = None;
        let mut next_retry = Instant::now();
        loop {
            let Some(s) = slot.upgrade() else { break };
            let config = s.config();
            if config.source == CameraSource::Mvs && Instant::now() >= next_retry {
                next_retry = Instant::now() + Duration::from_secs(2);
                let lost = s.shared.disconnected.load(Ordering::SeqCst);
                if lost || s.device.lock().unwrap().is_none() {
                    if lost {
                        cycle::log(&app, "err", "相机断线", format!("{}：尝试重新连接", config.name));
                    }
                    let s2 = s.clone();
                    match tauri::async_runtime::spawn_blocking(move || s2.open()).await {
                        Ok(Ok(msg)) => {
                            last_error = None;
                            cycle::log(&app, "ok", "相机", format!("{}：{msg}", config.name));
                        }
                        Ok(Err(e)) => {
                            if last_error.as_ref() != Some(&e) {
                                cycle::log(&app, "warn", "相机", format!("{}：{e}", config.name));
                                last_error = Some(e);
                            }
                        }
                        Err(e) => s.state.lock().unwrap().message = e.to_string(),
                    }
                }
            }
            drop(s);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

fn rig(app: &AppHandle) -> &CameraRig {
    &app.state::<CycleHost>().inner().camera
}

#[tauri::command]
pub fn camera_rig_status(cycle: State<'_, CycleHost>) -> Vec<CameraStatus> {
    cycle.camera.statuses()
}

#[tauri::command]
pub fn camera_rig_config(cycle: State<'_, CycleHost>) -> Vec<CameraConfig> {
    cycle.camera.configs()
}

/// 保存并应用一台相机的配置。返回相机未接受的参数。
#[tauri::command]
pub async fn camera_save_config(app: AppHandle, cam: usize, config: CameraConfig) -> Result<Vec<String>, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.lock().await;
    config.validate()?;
    // 海康相机要重新打开，检测中改会打断这一件
    check_idle(&app.state::<CycleHost>())?;
    let slot = rig(&app).slot(cam).ok_or("相机不存在")?;
    // 编号是配方引用相机的依据；陈旧索引不能覆盖另一台相机。
    check_camera_target(&slot.config().id, &config.id)?;
    check_serial(&rig(&app).configs(), cam, &config)?;
    slot.set_config(config);
    let saved = rig(&app).save();
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    let warnings = tauri::async_runtime::spawn_blocking(move || slot.apply_config()).await.map_err(|e| e.to_string())??;
    saved.map_err(|e| format!("已生效，但没能写回 cameras.json：{e}"))?;
    Ok(warnings)
}

fn check_camera_target(current_id: &str, requested_id: &str) -> Result<(), String> {
    if current_id != requested_id {
        return Err("相机组已变化，请刷新配置后重试保存".into());
    }
    Ok(())
}

#[cfg(test)]
mod save_target_tests {
    #[test]
    fn stale_camera_index_cannot_overwrite_another_camera() {
        assert!(super::check_camera_target("cam2", "cam1").is_err());
        assert!(super::check_camera_target("cam1", "").is_err());
        assert!(super::check_camera_target("cam1", "cam1").is_ok());
    }
}

/// 同一个序列号不能给两台海康相机（留空表示"第一台空闲的"）。
fn check_serial(configs: &[CameraConfig], cam: usize, config: &CameraConfig) -> Result<(), String> {
    if config.source != CameraSource::Mvs || config.serial.is_empty() {
        return Ok(());
    }
    match configs.iter().enumerate().find(|(i, c)| *i != cam && c.source == CameraSource::Mvs && c.serial == config.serial) {
        Some((_, c)) => Err(format!("序列号 {} 已经给了{}", config.serial, c.name)),
        None => Ok(()),
    }
}

fn id_number(id: &str) -> Option<u32> {
    id.strip_prefix("cam")?.parse().ok()
}

fn take_id(configs: &[CameraConfig], next_id: &mut u32) -> String {
    let top = configs.iter().filter_map(|c| id_number(&c.id)).max().unwrap_or(0);
    *next_id = (*next_id).max(top + 1);
    let id = format!("cam{next_id}");
    *next_id += 1;
    id
}

/// 没有编号或编号重复的相机补编号。旧文件（还没有 next_id）按位置补 "cam{位置+1}"，旧配方里的相机序号 k 就对应它；
/// 其余从 next_id 取。返回是否改过。
fn assign_ids(configs: &mut [CameraConfig], next_id: &mut u32) -> bool {
    let legacy = *next_id == 0;
    let mut changed = false;
    for i in 0..configs.len() {
        let id = configs[i].id.clone();
        if valid_camera_id(&id) && !configs[..i].iter().any(|c| c.id == id) {
            continue;
        }
        let pos = legacy_camera_id(i as u8);
        configs[i].id = if legacy && !configs.iter().any(|c| c.id == pos) { pos } else { String::new() };
        changed = true;
    }
    for i in 0..configs.len() {
        if configs[i].id.is_empty() {
            configs[i].id = take_id(configs, next_id);
        }
    }
    let top = configs.iter().filter_map(|c| id_number(&c.id)).max().unwrap_or(0);
    *next_id = (*next_id).max(top + 1);
    changed
}

/// 相机组增删只能在空闲或故障时做：相机序号会重排（编号不变）。
fn check_idle(cycle: &CycleHost) -> Result<(), String> {
    if !cycle.busy() && matches!(cycle.phase(), Phase::Idle | Phase::Fault) {
        Ok(())
    } else {
        Err("检测进行中，工件结束后再操作相机".into())
    }
}

#[tauri::command]
pub fn camera_add(app: AppHandle, mut config: CameraConfig) -> Result<usize, String> {
    config.validate()?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_idle(&cycle)?;
    let mut configs = cycle.camera.configs();
    if configs.len() >= 8 {
        return Err("相机组最多 8 台".into());
    }
    check_serial(&configs, usize::MAX, &config)?;
    config.id = cycle.camera.new_id();
    configs.push(config);
    let n = configs.len();
    cycle.camera.rebuild(&app, configs);
    let saved = cycle.camera.save();
    let _ = cycle.tx.send(cycle::Input::Refresh);
    saved?;
    Ok(n - 1)
}

#[tauri::command]
pub fn camera_remove(app: AppHandle, cam: usize) -> Result<(), String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_idle(&cycle)?;
    let mut configs = cycle.camera.configs();
    if cam >= configs.len() {
        return Err("相机不存在".into());
    }
    if configs.len() == 1 {
        return Err("相机组至少保留 1 台".into());
    }
    let id = &configs[cam].id;
    let mut users: Vec<String> = cycle.recipes.list().iter()
        .filter(|r| r.cameras().iter().any(|camera| camera == id)).map(|r| r.id.clone()).collect();
    users.extend(app.state::<crate::workspace::WorkspaceHost>().camera_users(id));
    users.sort(); users.dedup();
    if !users.is_empty() {
        return Err(format!("相机 {id} 正被配方 {} 引用，请先调整配方的相机", users.join("、")));
    }
    configs.remove(cam);
    cycle.camera.rebuild(&app, configs);
    let saved = cycle.camera.save();
    let _ = cycle.tx.send(cycle::Input::Refresh);
    saved
}

#[tauri::command]
pub async fn camera_list_devices() -> Result<Vec<DeviceSummary>, String> {
    tauri::async_runtime::spawn_blocking(mvs::enumerate).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn camera_pick_replay_dir(window: tauri::WebviewWindow, directory: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_parent(&window).set_title("选择回放图片目录");
        let directory = PathBuf::from(directory);
        if directory.is_dir() {
            dialog = dialog.set_directory(directory);
        }
        dialog.pick_folder().map(|path| path.to_string_lossy().into_owned())
    }).await.map_err(|e| e.to_string())
}

/// 最近一帧的缩略图：前 16 字节为缩略图宽、高与原图宽、高（u32 小端），其后为 8 位灰度像素。
#[tauri::command]
pub fn camera_preview(cycle: State<'_, CycleHost>, cam: usize) -> tauri::ipc::Response {
    let mut out = Vec::new();
    if let Some(slot) = cycle.camera.slot(cam) {
        if let Some(p) = slot.shared.preview.lock().unwrap().as_ref() {
            for v in [p.width, p.height, p.full_width, p.full_height] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&p.data);
        }
    }
    tauri::ipc::Response::new(out)
}

#[tauri::command]
pub fn camera_soft_trigger(cycle: State<'_, CycleHost>, cam: usize) -> Result<(), String> {
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_idle(&cycle)?;
    let slot = cycle.camera.slot(cam).ok_or("相机不存在")?;
    let config = slot.config();
    if config.source == CameraSource::Mvs && config.trigger_source != "Software" {
        return Err("触发源为 Line0，软触发前先把触发源改为 Software".into());
    }
    slot.shared.grab.store(config.source == CameraSource::Mvs, Ordering::SeqCst);
    slot.shared.manual_until.store(now_ms() + 10_000, Ordering::SeqCst);
    slot.shared.manual.fetch_add(1, Ordering::SeqCst);
    if slot.trigger(false, None) {
        Ok(())
    } else {
        slot.shared.grab.store(false, Ordering::SeqCst);
        let _ = slot.shared.manual.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        Err("相机未连接".into())
    }
}

/// 空跑测试：机器人不带工件走一遍路径，期间的帧只计数不进入检测节拍。
#[tauri::command]
pub fn camera_dry_run_start(cycle: State<'_, CycleHost>) -> Result<(), String> {
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试空跑")?;
    if cycle.busy() || cycle.phase() != Phase::Idle {
        return Err("检测节拍不在空闲状态，不能开始空跑".into());
    }
    *cycle.camera.rig.dry_run.lock().unwrap() = Some((Instant::now(), Vec::new()));
    Ok(())
}

#[tauri::command]
pub fn camera_dry_run_get(cycle: State<'_, CycleHost>) -> Option<Vec<DryFrame>> {
    cycle.camera.rig.dry_run.lock().unwrap().as_ref().map(|(_, f)| f.clone())
}

#[tauri::command]
pub fn camera_dry_run_stop(cycle: State<'_, CycleHost>) -> Vec<DryFrame> {
    cycle.camera.rig.dry_run.lock().unwrap().take().map(|(_, f)| f).unwrap_or_default()
}

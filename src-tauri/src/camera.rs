//! 相机组：N 台相机各自配置、状态与重连，共用一条有界帧通道交给检测节拍。
//! 飞拍按触发取图；连续采集的相机只更新缩略图，不送节拍。

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_uint, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use ly_plc::now_ms;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::Sender;

use crate::cycle::{self, CycleHost, Phase};
use crate::frame::{CounterSource, Frame, FrameImage, FramePool};
use crate::mvs::{self, DeviceSummary, FrameInfo};
use crate::recipe::{legacy_camera_id, valid_camera_id, Recipe};
use crate::replay;
use crate::sim::Scenario;
use crate::simimage::{self, PoseError};

/// 帧通道容量：检测节拍处理不过来时丢新帧并计数，不在内存里无限堆积。
pub const FRAME_QUEUE: usize = 64;

/// 下一个设备会话号：进程内只增不减，各相机共用，所以会话号不会重复（0 表示还没打开过）。
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn manual_arm_ready(source: CameraSource, pending: u32) -> Result<(), String> {
    if source == CameraSource::Mvs && pending > 0 {
        Err(format!("仍有 {pending} 次手动触发未确认，等待回调或重开相机后才能布防"))
    } else { Ok(()) }
}

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
    pub view_count: u8,
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
    pub counter_after_open: Option<u64>,
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
            view_count: 1,
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
            counter_after_open: None,
            replay_dir: String::new(),
            replay_channel: 0,
        }
    }
}

impl CameraConfig {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.view_count, 1 | 3) {
            return Err("视角数量只能是 1 或 3".into());
        }
        if self.source == CameraSource::Mvs && self.view_count == 3 {
            return Err("海康三目设备的 SDK 图像交付形式待现场确认，目前不能启用三视角取图".into());
        }
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
/// 只排本会话的帧；重新加载前触发、还在合成的帧到了直接交付（会话号旧，不会算进新会话）。
#[derive(Default)]
struct Reorder {
    session: u64,
    next: u64,
    pending: BTreeMap<u64, Option<Frame>>,
}

struct ReplayState {
    files: Vec<Vec<PathBuf>>,
    next: usize,
    /// 帧录制目录：各帧相对工件开始的时刻（ms）。有它时每件从第一张放
    times: Option<Vec<i64>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureTicket {
    session: u64,
    frame_counter: u64,
}

struct CapturedFrame {
    ticket: CaptureTicket,
    images: Vec<Arc<FrameImage>>,
}

impl CapturedFrame {
    fn from_frame(frame: &Frame) -> Self {
        Self { ticket: CaptureTicket { session: frame.session, frame_counter: frame.frame_counter }, images: frame.images.clone() }
    }

    fn images_for(&self, ticket: CaptureTicket) -> Option<Vec<Arc<FrameImage>>> {
        (self.ticket == ticket).then(|| self.images.clone())
    }
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
    preview: Mutex<Vec<Preview>>,
    preview_at: Mutex<Option<Instant>>,
    last_full: Mutex<Option<CapturedFrame>>,
    identity: Mutex<crate::shot_router::Ledgers>,
    /// last_full 每换一次 +1
    full_seq: AtomicU64,
    /// 下一帧不管要不要整帧都留一份（标定试测、软触发取图）
    grab: AtomicBool,
    /// 未完成的手动触发在回调到达或重新开流前阻止布防。
    manual: AtomicU32,
    last_emit: Mutex<Option<Instant>>,
    disconnected: AtomicBool,
    order: Mutex<Reorder>,
    /// 像素格式不支持、没能出灰度图的帧数与最近一次的原因（重新打开时清零）
    unusable: AtomicU64,
    unusable_msg: Mutex<String>,
    /// 当前设备会话号（见 Frame::session）
    session: AtomicU64,
    /// 海康相机本次打开开上了的 Chunk：触发计数、帧计数
    chunk_trigger: AtomicBool,
    chunk_frame: AtomicBool,
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

    fn set_previews(&self, session: u64, images: &[Arc<FrameImage>]) {
        if session != self.session.load(Ordering::SeqCst) {
            return;
        }
        if self.preview_due() {
            *self.preview.lock().unwrap() = images.iter().map(|img| make_preview(&img.pixels, img.width as usize, img.height as usize)).collect();
        }
    }

    /// 海康相机的一帧（像素格式已认过）：缩略图到时或要留整帧时才转灰度，缩略图与整帧用同一张灰度。
    fn gray_image(&self, pixel_type: u32, w: u32, h: u32, data: &[u8]) -> Option<Arc<FrameImage>> {
        let due = self.preview_due();
        let keep = (self.capture() && self.wanted()) | self.grab.swap(false, Ordering::SeqCst);
        if !due && !keep {
            return None;
        }
        let gray = match to_gray(&self.rig.pool, pixel_type, w, h, data) {
            Ok(g) => g,
            Err(e) => {
                self.unusable(e);
                return None;
            }
        };
        if due {
            *self.preview.lock().unwrap() = vec![make_preview(gray.pixels(), w as usize, h as usize)];
        }
        keep.then(|| Arc::new(gray.into_image(&self.rig.pool, w, h)))
    }

    /// 不支持的像素格式不静默丢弃：计数并记下原因，显示在相机状态里。
    fn unusable(&self, e: String) {
        self.unusable.fetch_add(1, Ordering::Relaxed);
        *self.unusable_msg.lock().unwrap() = e;
    }

    /// 有帧没能出灰度图时给状态栏的说明。
    fn unusable_note(&self) -> Option<String> {
        let n = self.unusable.load(Ordering::Relaxed);
        (n > 0).then(|| format!("{}：{n} 帧没有图像", self.unusable_msg.lock().unwrap()))
    }

    fn deliver_in_order(&self, session: u64, seq: u64, frame: Option<Frame>) {
        let mut order = self.order.lock().unwrap();
        // 每台相机的帧序号从 1 开始；不能拿先到的那帧当起点，否则先合成完的第 2 帧会把第 1 帧挤到后面
        if order.next == 0 {
            order.next = 1;
        }
        if session != order.session || seq < order.next {
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
        let current_session = frame.session == self.session.load(Ordering::SeqCst);
        if current_session {
            let mut identity = self.identity.lock().unwrap();
            let advanced = identity.observe(&crate::shot_router::FrameMeta::from(&frame));
            if frame.counter != CounterSource::Synthetic {
                frame.manual = advanced && self.manual.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
            }
        } else if frame.counter != CounterSource::Synthetic {
            frame.manual = false;
        }
        if !frame.images.is_empty() && current_session {
            if frame.counter == CounterSource::Synthetic {
                self.set_previews(frame.session, &frame.images);
            }
            *self.last_full.lock().unwrap() = Some(CapturedFrame::from_frame(&frame));
            self.full_seq.fetch_add(1, Ordering::SeqCst);
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PixelKind {
    Mono8,
    Bayer8,
}

/// SDK 报的像素格式：Mono8 原样用，8 位 Bayer 转灰度（D-9），其他不支持。
fn pixel_kind(pixel_type: u32) -> Result<PixelKind, String> {
    match pixel_type {
        mvs::PIXEL_MONO8 => Ok(PixelKind::Mono8),
        mvs::PIXEL_BAYER_GR8 | mvs::PIXEL_BAYER_RG8 | mvs::PIXEL_BAYER_GB8 | mvs::PIXEL_BAYER_BG8 => Ok(PixelKind::Bayer8),
        t => Err(format!("像素格式 0x{t:08X} 不支持（需要 Mono8 或 8 位 Bayer）")),
    }
}

/// 一帧灰度：Mono8 直接借 SDK 的缓冲（留整帧时才拷），Bayer 转换后的已在缓冲池里。
enum Gray<'a> {
    Raw(&'a [u8]),
    Pooled(FrameImage),
}

impl Gray<'_> {
    fn pixels(&self) -> &[u8] {
        match self {
            Gray::Raw(p) => p,
            Gray::Pooled(img) => &img.pixels,
        }
    }

    fn into_image(self, pool: &FramePool, w: u32, h: u32) -> FrameImage {
        match self {
            Gray::Raw(p) => pool.copy(w, h, p),
            Gray::Pooled(img) => img,
        }
    }
}

/// 取图回调收到的像素转成 8 位灰度。
fn to_gray<'a>(pool: &FramePool, pixel_type: u32, w: u32, h: u32, data: &'a [u8]) -> Result<Gray<'a>, String> {
    match pixel_kind(pixel_type)? {
        PixelKind::Mono8 => Ok(Gray::Raw(data)),
        PixelKind::Bayer8 => pool.bayer8_to_gray(w, h, data).map(Gray::Pooled),
    }
}

/// 海康相机本次打开开上了哪些计数 Chunk（ChunkModeActive 与对应项的 ChunkEnable 都设上了）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Chunks {
    trigger: bool,
    frame: bool,
}

/// Chunk 选项按名字归类（各型号命名不一，按关键字认；W0 台架核对现场相机的 ChunkSelector）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkItem {
    Trigger,
    Frame,
    Timestamp,
}

fn chunk_item(entry: &str) -> Option<ChunkItem> {
    let l = entry.to_lowercase();
    if l.contains("trigger") {
        Some(ChunkItem::Trigger)
    } else if l.contains("frame") && l.contains("count") {
        Some(ChunkItem::Frame)
    } else if l.contains("timestamp") {
        Some(ChunkItem::Timestamp)
    } else {
        None
    }
}

/// 海康一帧的 (帧计数, 触发计数, 来源)。帧计数取 Chunk 帧计数，没有时取 SDK 帧号（同以前）。
/// 开了触发计数 Chunk 就用 Chunk 触发计数，0 也照收（开流后第一个触发记 0 还是 1 待台架确认）；
/// 没开但 SDK 报了非零触发计数也认。都没有时数值仍填帧计数给界面和日志，来源如实标出，不能拿来认拍照点。
fn mvs_counters(frame_num: u32, chunk_frame: u32, trigger_index: u32, chunks: Chunks) -> (u64, u64, CounterSource) {
    let frame_counter = if chunk_frame != 0 { chunk_frame } else { frame_num } as u64;
    if chunks.trigger || trigger_index != 0 {
        return (frame_counter, trigger_index as u64, CounterSource::ChunkTrigger);
    }
    let source = if chunks.frame || chunk_frame != 0 { CounterSource::ChunkFrame } else { CounterSource::SdkFrame };
    (frame_counter, frame_counter, source)
}

/// 模拟 / 回放的一帧：帧计数与触发计数都是本会话的帧号。
fn synthetic_frame(cam: u8, session: u64, n: u64, images: Vec<Arc<FrameImage>>, manual: bool) -> Frame {
    Frame { cam, session, counter: CounterSource::Synthetic, frame_counter: n, trigger_counter: n, lost_packets: 0, ts: now_ms(), manual, images }
}

extern "system" fn on_image(data: *mut u8, info: *mut FrameInfo, user: *mut c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if info.is_null() || user.is_null() {
            return;
        }
        let shared = unsafe { &*(user as *const Shared) };
        let info = unsafe { &*info };
        let chunks = Chunks { trigger: shared.chunk_trigger.load(Ordering::Relaxed), frame: shared.chunk_frame.load(Ordering::Relaxed) };
        let (frame_counter, trigger_counter, counter) = mvs_counters(info.frame_num, info.frame_counter, info.trigger_index, chunks);
        let session = shared.session.load(Ordering::SeqCst);
        let w = if info.extend_width != 0 { info.extend_width } else { info.width as u32 };
        let h = if info.extend_height != 0 { info.extend_height } else { info.height as u32 };
        // 先认像素格式，再按每像素 1 字节取缓冲（Mono8 与 8 位 Bayer 都是）
        let image = match pixel_kind(info.pixel_type) {
            Err(e) => {
                shared.unusable(e);
                None
            }
            Ok(_) if data.is_null() => None,
            Ok(_) => shared.gray_image(info.pixel_type, w, h, unsafe { std::slice::from_raw_parts(data, (w * h) as usize) }),
        };
        shared.deliver(Frame { cam: shared.cam, session, counter, frame_counter, trigger_counter, lost_packets: info.lost_packet, ts: now_ms(), manual: false, images: image.into_iter().collect() });
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
    /// 模拟 / 回放本会话已出的触发数（帧计数与触发计数相同）
    seq: Mutex<u64>,
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
                preview: Mutex::new(Vec::new()),
                preview_at: Mutex::new(None),
                last_full: Mutex::new(None),
                identity: Mutex::new(crate::shot_router::Ledgers::default()),
                full_seq: AtomicU64::new(0),
                grab: AtomicBool::new(false),
                manual: AtomicU32::new(0),
                last_emit: Mutex::new(None),
                disconnected: AtomicBool::new(false),
                order: Mutex::new(Reorder::default()),
                unusable: AtomicU64::new(0),
                unusable_msg: Mutex::new(String::new()),
                session: AtomicU64::new(0),
                chunk_trigger: AtomicBool::new(false),
                chunk_frame: AtomicBool::new(false),
            }),
            config: Mutex::new(config),
            state: Mutex::new(DeviceState::default()),
            seq: Mutex::new(0),
            replay: Mutex::new(None),
        };
        if let Err(e) = slot.config().validate() {
            slot.state.lock().unwrap().message = e;
            return slot;
        }
        match slot.config().source {
            CameraSource::Sim => slot.new_session(),
            CameraSource::Replay => {
                let _ = slot.load_replay();
            }
            // 海康相机打开时才有会话
            CameraSource::Mvs => {}
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
        let message = if let Err(e) = config.validate() { e } else { match (config.source, replay) {
            (CameraSource::Sim, _) => match config.acquisition {
                Acquisition::Triggered => format!("模拟相机：每次触发交付一帧 · {} 个视角", config.view_count),
                Acquisition::FreeRun => "模拟相机：连续采集不合成画面，飞拍要用触发采集".to_string(),
            },
            (CameraSource::Replay, Some((n, next))) => format!("回放 {n} 帧 · 下一帧第 {} 张", next + 1),
            (CameraSource::Mvs, _) => match self.shared.unusable_note() {
                Some(note) => format!("{note} · {state_message}"),
                None => state_message,
            },
            _ => state_message,
        }};
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

    pub fn last_views(&self) -> Vec<Arc<FrameImage>> {
        self.shared.last_full.lock().unwrap().as_ref().map(|frame| frame.images.clone()).unwrap_or_default()
    }

    pub fn routing_state(&self) -> Result<(crate::shot_router::ArmCam, Option<crate::shot_router::Ledger>), String> {
        let config = self.config();
        manual_arm_ready(config.source, self.shared.manual.load(Ordering::SeqCst))?;
        let session = self.shared.session.load(Ordering::SeqCst);
        let observed = if config.source == CameraSource::Mvs {
            self.shared.identity.lock().unwrap().get(self.shared.cam).filter(|ledger| ledger.session == session).cloned()
        } else {
            Some(crate::shot_router::Ledger::snapshot(session, CounterSource::Synthetic, *self.seq.lock().unwrap()))
        };
        Ok((crate::shot_router::ArmCam {
            cam: self.shared.cam, camera: config.id, session,
            source: (config.source != CameraSource::Mvs).then_some(CounterSource::Synthetic),
            counter_after_open: config.counter_after_open,
        }, observed))
    }

    pub fn last_view(&self, view: u8) -> Option<Arc<FrameImage>> {
        let index = view.checked_sub(1)? as usize;
        self.shared.last_full.lock().unwrap().as_ref()?.images.get(index).cloned()
    }

    pub fn grab_views(&self, timeout: Duration) -> Option<Vec<Arc<FrameImage>>> {
        let before = self.shared.full_seq.load(Ordering::SeqCst);
        self.shared.grab.store(true, Ordering::SeqCst);
        let result = self.wait_views(before, timeout);
        self.shared.grab.store(false, Ordering::SeqCst);
        result
    }

    fn wait_views(&self, before: u64, timeout: Duration) -> Option<Vec<Arc<FrameImage>>> {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            if self.shared.full_seq.load(Ordering::SeqCst) != before {
                let images = self.last_views();
                if !images.is_empty() {
                    return Some(images);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn wait_capture(&self, ticket: CaptureTicket, timeout: Duration) -> Result<Option<Vec<Arc<FrameImage>>>, String> {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            if self.shared.session.load(Ordering::SeqCst) != ticket.session {
                return Err("取样期间相机会话已变化，本次取样已取消".into());
            }
            if let Some(images) = self.shared.last_full.lock().unwrap().as_ref().and_then(|frame| frame.images_for(ticket)) {
                return Ok(Some(images));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(None)
    }

    /// 只看就绪与否，不拼状态文字（节拍每 20 ms 查一次）。
    pub fn is_ready(&self) -> bool {
        let config = self.config.lock().unwrap();
        if config.validate().is_err() {
            return false;
        }
        match config.source {
            CameraSource::Sim => true,
            CameraSource::Mvs => self.device.lock().unwrap().is_some() && !self.shared.disconnected.load(Ordering::SeqCst),
            CameraSource::Replay => self.replay.lock().unwrap().is_some(),
        }
    }

    /// 开一个新会话：海康每次打开，模拟 / 回放每次（重新）加载。模拟 / 回放的计数从 0 重来，
    /// 与真机重新开流后计数可能清零一样，靠会话号区分前后两段。
    fn new_session(&self) {
        let mut seq = self.seq.lock().unwrap();
        let session = NEXT_SESSION.fetch_add(1, Ordering::SeqCst);
        *seq = 0;
        *self.shared.order.lock().unwrap() = Reorder { session, ..Reorder::default() };
        self.shared.session.store(session, Ordering::SeqCst);
        self.shared.manual.store(0, Ordering::SeqCst);
        self.shared.grab.store(false, Ordering::SeqCst);
        self.shared.preview.lock().unwrap().clear();
        self.shared.last_full.lock().unwrap().take();
        *self.shared.identity.lock().unwrap() = crate::shot_router::Ledgers::default();
    }

    /// 模拟 / 回放的下一帧：(会话号, 帧号)，帧号即触发计数，本会话从 1 起。
    fn next_counters(&self) -> (u64, u64) {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        (self.shared.session.load(Ordering::SeqCst), *seq)
    }

    /// 下一张回放图，到末尾后从头再来。
    fn next_replay(&self) -> Option<Vec<PathBuf>> {
        let mut guard = self.replay.lock().unwrap();
        let r = guard.as_mut()?;
        let p = r.files[r.next % r.files.len()].clone();
        r.next = (r.next + 1) % r.files.len();
        Some(p)
    }

    /// 扫描回放目录，开一个新会话。扫描（网络盘上可能很慢）时不持有任何锁。
    fn load_replay(&self) -> Result<String, String> {
        self.new_session();
        let config = self.config();
        let dir = config.replay_dir.trim().to_string();
        let path = std::path::Path::new(&dir);
        let scanned = replay::scan_views(path, config.view_count, config.replay_channel).and_then(|files| {
            // 先读一张的文件头：格式解不开（或文件坏了）就不报就绪
            for file in &files[0] {
                replay::probe(file)?;
            }
            let times = replay::timeline_views(path, &files)?;
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
        self.trigger_frame(lose_in_transfer, render, false).is_ok()
    }

    fn trigger_frame(&self, lose_in_transfer: bool, render: Option<SimRender>, manual: bool) -> Result<Option<CaptureTicket>, String> {
        let config = self.config();
        if let Err(e) = config.validate() {
            self.state.lock().unwrap().message = e.clone();
            return Err(e);
        }
        let cam = self.shared.cam;
        match config.source {
            CameraSource::Sim => {
                let (session, n) = self.next_counters();
                let ticket = CaptureTicket { session, frame_counter: n };
                let shared = self.shared.clone();
                if lose_in_transfer {
                    shared.deliver_in_order(session, n, None);
                    return Ok(Some(ticket));
                }
                tauri::async_runtime::spawn(async move {
                    let started = Instant::now();
                    let images = tauri::async_runtime::spawn_blocking(move || match render {
                        Some(r) => simimage::render_views(&r.recipe, r.k, r.scenario, r.pose, r.seed, config.view_count),
                        None if config.view_count == 3 => simimage::background_views(config.view_count, n),
                        None => Ok(Vec::new()),
                    }).await.map_err(|e| e.to_string()).and_then(|r| r);
                    let transfer = Duration::from_millis(180);
                    if started.elapsed() < transfer {
                        tokio::time::sleep(transfer - started.elapsed()).await;
                    }
                    match images {
                        Ok(images) => {
                            shared.deliver_in_order(session, n, Some(synthetic_frame(cam, session, n, images, manual)));
                        }
                        Err(e) => {
                            cycle::log(&shared.rig.app, "err", "模拟取图", e);
                            shared.deliver_in_order(session, n, None);
                        }
                    }
                });
                Ok(Some(ticket))
            }
            CameraSource::Mvs => {
                if config.trigger_source != "Software" {
                    return Err("触发源不是 Software，不能发送软触发".into());
                }
                self.device.lock().unwrap().as_ref().ok_or("相机未连接")?.command("TriggerSoftware")?;
                Ok(None)
            }
            CameraSource::Replay => {
                let paths = self.next_replay().ok_or("回放相机未加载图像")?;
                let (session, n) = self.next_counters();
                let ticket = CaptureTicket { session, frame_counter: n };
                let shared = self.shared.clone();
                tauri::async_runtime::spawn(async move {
                    let loaded = tauri::async_runtime::spawn_blocking(move || replay::load_views(&paths)).await.map_err(|e| e.to_string());
                    match loaded.and_then(|r| r) {
                        Ok(images) => {
                            shared.deliver_in_order(session, n, Some(synthetic_frame(cam, session, n, images, manual)));
                        }
                        Err(e) => {
                            cycle::log(&shared.rig.app, "err", "回放", e);
                            shared.deliver_in_order(session, n, None);
                        }
                    }
                });
                Ok(Some(ticket))
            }
        }
    }

    fn soft_trigger(&self, render: Option<SimRender>) -> Result<Option<CaptureTicket>, String> {
        let config = self.config();
        config.validate()?;
        if config.source == CameraSource::Mvs && config.trigger_source != "Software" {
            return Err("触发源为 Line0，软触发前先把触发源改为 Software".into());
        }
        if config.source == CameraSource::Mvs {
            self.shared.manual.fetch_add(1, Ordering::SeqCst);
        }
        self.shared.grab.store(config.source == CameraSource::Mvs, Ordering::SeqCst);
        match self.trigger_frame(false, render, true) {
            Ok(ticket) => Ok(ticket),
            Err(e) => {
                self.shared.grab.store(false, Ordering::SeqCst);
                Err(if config.source == CameraSource::Mvs { format!("{e}；软触发结果未确认，请重开相机后再布防") } else { e })
            }
        }
    }

    fn close(&self) {
        self.device.lock().unwrap().take();
        self.shared.disconnected.store(false, Ordering::SeqCst);
        self.shared.unusable.store(0, Ordering::Relaxed);
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
        config.validate()?;
        let device = mvs::Device::open(&config.serial, &self.claimed_serials())?;
        let (warnings, max_fps, format, chunks) = apply(&device, &config)?;
        self.shared.chunk_trigger.store(chunks.trigger, Ordering::Relaxed);
        self.shared.chunk_frame.store(chunks.frame, Ordering::Relaxed);
        // 开流前换会话：之后来的帧都算这次打开的
        self.new_session();
        device.start(on_image, on_exception, Arc::as_ptr(&self.shared) as *mut c_void)?;
        let format = if format == "Mono8" { format.to_string() } else { format!("{format} → 灰度") };
        let mut msg = format!("已连接 {} · {} · {format}", device.summary.model, device.summary.serial);
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
                self.new_session();
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

/// 8 位 Bayer 的取用顺序（现场相机是 BG）。
const BAYER8: [&str; 4] = ["BayerBG8", "BayerRG8", "BayerGB8", "BayerGR8"];

/// 相机可选项里的 8 位 Bayer，按 BAYER8 的顺序。
fn bayer8_choices(entries: &[String]) -> Vec<&'static str> {
    BAYER8.into_iter().filter(|f| entries.iter().any(|e| e == f)).collect()
}

/// 先设 Mono8；彩色相机没有 Mono8 时取 8 位 Bayer，取图回调里转灰度（D-9）。都设不上时不能取流。
fn set_pixel_format(d: &mvs::Device) -> Result<&'static str, String> {
    let Err(mut last) = d.set_enum("PixelFormat", "Mono8") else { return Ok("Mono8") };
    let entries = d.enum_entries("PixelFormat").map_err(|e| format!("像素格式设不了 Mono8（{last}），也读不出相机支持的格式：{e}"))?;
    for f in bayer8_choices(&entries) {
        match d.set_enum("PixelFormat", f) {
            Ok(()) => return Ok(f),
            Err(e) => last = e,
        }
    }
    let list = if entries.is_empty() { "无".to_string() } else { entries.join("、") };
    Err(format!("像素格式需要 Mono8 或 8 位 Bayer，相机可选：{list}（{last}）"))
}

/// 写入相机参数。像素格式设不上时返回错误（相机不就绪）；其余个别型号不支持的节点记为警告，不阻止取流。
/// 返回警告、最高帧率、选用的像素格式与开上了的计数 Chunk。
fn apply(d: &mvs::Device, c: &CameraConfig) -> Result<(Vec<String>, Option<f32>, &'static str, Chunks), String> {
    let mut warnings = Vec::new();
    let mut must = |r: Result<(), String>| match r {
        Ok(()) => true,
        Err(e) => {
            warnings.push(e);
            false
        }
    };
    must(d.set_enum("AcquisitionMode", "Continuous"));
    let format = set_pixel_format(d)?;
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
    let mut chunks = Chunks::default();
    if c.chunk {
        let active = must(d.set_bool("ChunkModeActive", true));
        match d.enum_entries("ChunkSelector") {
            Ok(entries) => {
                let wanted: Vec<_> = entries.iter().filter_map(|e| chunk_item(e).map(|item| (e, item))).collect();
                if wanted.is_empty() {
                    must(Err(format!("相机不支持帧计数 / 触发计数 Chunk（可选项：{}）", entries.join("、"))));
                }
                for (e, item) in wanted {
                    let on = must(d.set_enum("ChunkSelector", e)) & must(d.set_bool("ChunkEnable", true));
                    match item {
                        ChunkItem::Trigger => chunks.trigger |= active && on,
                        ChunkItem::Frame => chunks.frame |= active && on,
                        ChunkItem::Timestamp => {}
                    }
                }
            }
            Err(e) => {
                must(Err(e));
            }
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
    Ok((warnings, max_fps, format, chunks))
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
            manual_arm_ready(slot.config().source, slot.shared.manual.load(Ordering::SeqCst))?;
            if !slot.is_ready() {
                let st = slot.status();
                return Err(format!("{}未就绪：{}", st.name, st.message));
            }
        }
        Ok(())
    }

    /// 开工前让回放帧录制回到第一张。
    pub fn begin_part(&self, cams: &[u8]) {
        for &c in cams {
            if let Some(s) = self.slot(c as usize) {
                s.rewind_recording();
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
        self.last_view(cam, 1)
    }

    pub fn last_view(&self, cam: u8, view: u8) -> Option<Arc<FrameImage>> {
        self.slot(cam as usize).and_then(|s| s.last_view(view))
    }

    pub fn capture_views(&self, cam: u8, render: Option<SimRender>) -> Result<Vec<Arc<FrameImage>>, String> {
        let cycle = self.rig.app.state::<CycleHost>();
        let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试取图")?;
        check_idle(&cycle)?;
        let slot = self.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        config.validate()?;
        if !slot.is_ready() {
            return Err(format!("相机未就绪：{}", slot.status().message));
        }
        let images = if config.source == CameraSource::Sim && render.is_none() {
            let images = slot.last_views();
            if images.is_empty() {
                return Err("模拟相机尚未出图，请先运行一个模拟工件".into());
            }
            images
        } else if config.source != CameraSource::Mvs || (config.acquisition == Acquisition::Triggered && config.trigger_source == "Software") {
            let before = slot.shared.full_seq.load(Ordering::SeqCst);
            let captured = match slot.soft_trigger(render)? {
                Some(ticket) => slot.wait_capture(ticket, Duration::from_secs(2))?,
                None => slot.wait_views(before, Duration::from_secs(2)),
            };
            captured.ok_or("2 秒内没有收到本次触发的完整图像；检查触发和回放目录")?
        } else {
            slot.grab_views(Duration::from_secs(2)).ok_or("请在所选位置触发相机；2 秒内未收到完整图像")?
        };
        if images.len() != config.view_count as usize {
            return Err(format!("相机需要 {} 个视角，这一帧只有 {} 幅图像", config.view_count, images.len()));
        }
        Ok(images)
    }

    pub fn capture_view(&self, cam: u8, view: u8, render: Option<SimRender>) -> Result<Arc<FrameImage>, String> {
        let slot = self.slot(cam as usize).ok_or("相机不存在")?;
        if view == 0 || view > slot.config().view_count {
            return Err(format!("相机没有视角 {view}"));
        }
        self.capture_views(cam, render)?.get(view as usize - 1).cloned().ok_or_else(|| format!("这一帧缺少视角 {view}"))
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
    let _ = app.emit("camera://changed", ());
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
    let _ = app.emit("camera://changed", ());
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
    let _ = app.emit("camera://changed", ());
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
pub fn camera_preview(cycle: State<'_, CycleHost>, cam: usize, view: Option<u8>) -> tauri::ipc::Response {
    let mut out = Vec::new();
    if let Some(slot) = cycle.camera.slot(cam) {
        let previews = slot.shared.preview.lock().unwrap();
        if let Some(p) = view.unwrap_or(1).checked_sub(1).and_then(|i| previews.get(i as usize)) {
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
    slot.soft_trigger(None).map(|_| ())
}

#[cfg(test)]
mod view_tests {
    #[test]
    fn unresolved_manual_trigger_blocks_mvs_arming_until_resolved() {
        assert!(super::manual_arm_ready(super::CameraSource::Mvs, 1).is_err());
        assert!(super::manual_arm_ready(super::CameraSource::Mvs, 10).is_err());
        assert!(super::manual_arm_ready(super::CameraSource::Mvs, 0).is_ok());
        assert!(super::manual_arm_ready(super::CameraSource::Sim, 1).is_ok());
    }

    use super::*;

    #[test]
    fn old_camera_configs_keep_one_view_and_only_one_or_three_are_allowed() {
        let mut config: CameraConfig = serde_json::from_str(r#"{"name":"原单目相机"}"#).unwrap();
        assert_eq!(config.view_count, 1);
        assert!(config.validate().is_ok());
        for view_count in [0, 2, 4, u8::MAX] {
            config.view_count = view_count;
            assert!(config.validate().unwrap_err().contains("只能是 1 或 3"));
        }
        config.view_count = 3;
        assert!(config.validate().is_ok());
        assert_eq!(serde_json::to_value(&config).unwrap()["viewCount"], 3);
    }

    #[test]
    fn mvs_three_views_are_rejected_until_sdk_delivery_is_confirmed() {
        let mut config = CameraConfig { source: CameraSource::Mvs, view_count: 3, ..CameraConfig::default() };
        assert!(config.validate().unwrap_err().contains("SDK 图像交付形式待现场确认"));
        config.view_count = 1;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn three_images_share_one_device_frame_and_trigger_counter() {
        let images: Vec<_> = [1, 2, 3].into_iter().map(|value| Arc::new(FrameImage::new(1, 1, vec![value]))).collect();
        let frame = synthetic_frame(0, 72, 19, images, true);
        assert!(frame.manual);
        assert_eq!((frame.cam, frame.session, frame.frame_counter, frame.trigger_counter), (0, 72, 19, 19));
        assert_eq!(frame.counter, CounterSource::Synthetic);
        assert_eq!(frame.images.len(), 3);
        for view in 1..=3 {
            assert_eq!(frame.image(view).unwrap().pixels, [view]);
        }
        assert!(frame.image(0).is_none());
        assert!(frame.image(4).is_none());
    }

    #[test]
    fn prior_trigger_completion_cannot_satisfy_the_current_capture() {
        let frame = |session, counter, value: u8| synthetic_frame(0, session, counter, (1u8..=3).map(|view| Arc::new(FrameImage::new(1, 1, vec![value + view]))).collect(), true);
        let requested = CaptureTicket { session: 12, frame_counter: 8 };
        let mut cached = CapturedFrame::from_frame(&frame(12, 7, 10));
        assert!(cached.images_for(requested).is_none());
        cached = CapturedFrame::from_frame(&frame(11, 8, 20));
        assert!(cached.images_for(requested).is_none());
        cached = CapturedFrame::from_frame(&frame(12, 8, 30));
        let images = cached.images_for(requested).unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images.iter().map(|image| image.pixels[0]).collect::<Vec<_>>(), [31, 32, 33]);
        cached = CapturedFrame::from_frame(&frame(12, 9, 40));
        assert!(cached.images_for(requested).is_none());
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

#[cfg(test)]
mod gray_tests {
    use super::*;

    #[test]
    fn mono8_passes_through_unchanged() {
        let pool = FramePool::new(2);
        let data: Vec<u8> = (0..12).map(|i| i * 20).collect();
        let gray = to_gray(&pool, mvs::PIXEL_MONO8, 4, 3, &data).unwrap();
        assert!(matches!(gray, Gray::Raw(_)));
        assert_eq!(gray.pixels(), &data[..]);
        let img = gray.into_image(&pool, 4, 3);
        assert_eq!((img.width, img.height), (4, 3));
        assert_eq!(img.pixels, data);
    }

    #[test]
    fn bayer8_is_converted() {
        let pool = FramePool::new(2);
        // BG 相位、R = 200、G = 100、B = 40 的纯色块
        let (w, h) = (6, 4);
        let data: Vec<u8> = (0..w * h).map(|i| match (i / w % 2, i % w % 2) {
            (0, 0) => 40,
            (1, 1) => 200,
            _ => 100,
        }).collect();
        let gray = to_gray(&pool, mvs::PIXEL_BAYER_BG8, w as u32, h as u32, &data).unwrap();
        assert!(gray.pixels().iter().all(|&v| v == 110));
        let img = gray.into_image(&pool, w as u32, h as u32);
        assert_eq!((img.width, img.height, img.pixels.len()), (6, 4, 24));
        for t in [mvs::PIXEL_BAYER_GR8, mvs::PIXEL_BAYER_RG8, mvs::PIXEL_BAYER_GB8, mvs::PIXEL_BAYER_BG8] {
            assert_eq!(pixel_kind(t), Ok(PixelKind::Bayer8));
        }
        assert!(to_gray(&pool, mvs::PIXEL_BAYER_BG8, 1, 24, &data).is_err());
    }

    #[test]
    fn unsupported_format_is_an_error() {
        let pool = FramePool::new(2);
        let err = to_gray(&pool, 0x0110_0003, 2, 2, &[0; 8]).err().unwrap();
        assert_eq!(err, "像素格式 0x01100003 不支持（需要 Mono8 或 8 位 Bayer）");
        assert!(pixel_kind(0x0108_000C).is_err());
    }

    #[test]
    fn trigger_counter_source_is_reported_honestly() {
        let none = Chunks::default();
        let both = Chunks { trigger: true, frame: true };
        // 开了触发计数 Chunk：用触发计数，0 也照收
        assert_eq!(mvs_counters(9, 5, 3, both), (5, 3, CounterSource::ChunkTrigger));
        assert_eq!(mvs_counters(9, 5, 0, both), (5, 0, CounterSource::ChunkTrigger));
        // 没开但 SDK 报了非零触发计数（同以前的取值）
        assert_eq!(mvs_counters(9, 5, 3, none), (5, 3, CounterSource::ChunkTrigger));
        // 没有触发计数：数值仍是帧计数（界面、日志照旧），来源不再冒充触发计数
        assert_eq!(mvs_counters(9, 5, 0, Chunks { trigger: false, frame: true }), (5, 5, CounterSource::ChunkFrame));
        assert_eq!(mvs_counters(9, 5, 0, none), (5, 5, CounterSource::ChunkFrame));
        assert_eq!(mvs_counters(9, 0, 0, none), (9, 9, CounterSource::SdkFrame));
    }

    #[test]
    fn chunk_entries_are_classified_by_name() {
        assert_eq!(chunk_item("TriggerID"), Some(ChunkItem::Trigger));
        assert_eq!(chunk_item("Triggercounter"), Some(ChunkItem::Trigger));
        assert_eq!(chunk_item("FrameCounter"), Some(ChunkItem::Frame));
        assert_eq!(chunk_item("Timestamp"), Some(ChunkItem::Timestamp));
        assert_eq!(chunk_item("Exposure"), None);
        assert_eq!(chunk_item("Width"), None);
    }

    #[test]
    fn picks_8bit_bayer_in_preferred_order() {
        let entries = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(bayer8_choices(&entries(&["BayerGR8", "BayerBG10", "BayerBG8"])), ["BayerBG8", "BayerGR8"]);
        assert!(bayer8_choices(&entries(&["BayerBG10", "BayerBG12Packed", "Mono10"])).is_empty());
    }
}

//! 相机组：N 台相机各自配置、状态与重连，共用一条有界帧通道交给检测节拍。
//! 所有相机按触发取图。

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ly_plc::now_ms;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::Sender;

use crate::cycle::{self, CycleHost};
use crate::frame::{BudgetExceeded, CounterSource, Frame, FrameImage, FramePool, GrayError};
use crate::mvs::{self, DeviceSummary, FrameInfo};
use crate::recipe::{legacy_camera_id, valid_camera_id, CameraImaging, Recipe};
use crate::offline::sample_set::SampleSet;
use crate::offline::Scenario;
use crate::simimage::{self, PoseError};

mod delivery;

/// 帧通道容量：检测节拍处理不过来时丢新帧并计数。条数只防失控，图像内存由 IMAGE_BUDGET_PER_CAMERA 封顶（D-SB-32）。
pub const FRAME_QUEUE: usize = 1024;

/// 每台海康相机在途图像的字节上限（FramePool 预算，D-SB-32）：约 190 幅 1.3 MB 图，25 Hz 下 7.6 s 积压。
/// 预算满时取图回调丢弃该帧、按缺帧记账，不中断回调。初值待 P1 实测后回填。合成图、样本集来源不经过这个池。
pub const IMAGE_BUDGET_PER_CAMERA: usize = 256 * 1024 * 1024;

/// 每台海康相机池里留的空闲缓冲块数。
const POOL_KEEP_PER_CAMERA: usize = 16;

/// 一个千兆口按 80% 计的可用带宽（Mbps）：三台相机共用一个触发、同时出图，按三路之和比较（规格 §6）。
pub const PORT_BUDGET_MBPS: f64 = 1000.0 * 0.8;

/// 链路上每像素字节数：取图只支持 Mono8 与 8 位 Bayer（见 pixel_kind），都是 1 字节。
const LINK_BYTES_PER_PIXEL: f64 = 1.0;

/// GigE Vision 链路开销：图像按标准 1500 字节包（不开巨帧）传输，每包 1464 B 图像数据，外加 IP / UDP / GVSP 头 36 B
/// 和以太网帧头、校验、前导码与帧间隔 38 B，线上字节约为图像字节的 1538 / 1464 ≈ 1.05 倍。开巨帧（9000 B）时约 1.008 倍；
/// 估算按不开巨帧取保守值。
const GIGE_OVERHEAD: f64 = 1538.0 / 1464.0;

/// 相机组在某个触发频率下的链路带宽估算。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BandwidthEstimate {
    pub hz: f64,
    /// 已知分辨率的相机之和
    pub mbps_total: f64,
    /// 与输入顺序一致；分辨率未知（还没出过图、离线来源没有物理链路）为 None，不计入合计
    pub per_camera: Vec<Option<f64>>,
    pub budget_mbps: f64,
    pub over_budget: bool,
}

/// 各相机 宽 × 高 × 每像素字节 × 8 × hz × 链路开销（[`GIGE_OVERHEAD`]）求和，超过一个千兆口的 80% 即超预算。
pub fn aggregate_bandwidth(sizes: &[Option<(u32, u32)>], hz: f64) -> BandwidthEstimate {
    let per_camera: Vec<Option<f64>> = sizes.iter()
        .map(|size| size.map(|(w, h)| w as f64 * h as f64 * LINK_BYTES_PER_PIXEL * 8.0 * hz * GIGE_OVERHEAD / 1e6))
        .collect();
    let mbps_total = per_camera.iter().flatten().sum::<f64>();
    BandwidthEstimate { hz, mbps_total, per_camera, budget_mbps: PORT_BUDGET_MBPS, over_budget: mbps_total > PORT_BUDGET_MBPS }
}

/// 下一个设备会话号：进程内只增不减，各相机共用，所以会话号不会重复（0 表示还没打开过）。
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn manual_arm_ready(source: CameraSource, pending: u32) -> Result<(), String> {
    if source == CameraSource::Mvs && pending > 0 {
        Err(format!("仍有 {pending} 次手动触发未确认，等待回调或重开相机后才能布防"))
    } else { Ok(()) }
}

/// 模拟相机合成飞拍帧所需的信息：哪个配方的第几个拍照点、什么场景、机器人偏差。旧拍照点模型；段配方用 [`BurstRender`]。
pub struct SimRender {
    pub recipe: Arc<Recipe>,
    pub k: usize,
    pub scenario: Scenario,
    pub pose: PoseError,
    pub seed: u64,
}

/// 样本集相机取图的身份：哪个样本集的哪一件、哪个拍照点（相机编号与视角取自相机自身配置）。
pub struct SampleTarget {
    pub set: Arc<SampleSet>,
    pub part_id: String,
    pub shot_id: String,
}

impl SampleTarget {
    /// 本身份在指定相机下的全部视角原图；缺任一幅即整帧不可用。
    pub fn images(&self, camera_id: &str, view_count: u8) -> Result<Vec<Arc<FrameImage>>, String> {
        self.set.load_views(&self.part_id, &self.shot_id, camera_id, view_count)
    }
}

/// 段配方模拟相机合成一帧所需的信息（规格 §6）：哪台计划相机、本件第几个脉冲，以及这一帧的真值——
/// 所在段的类型与段内进度（弯道时胶条在视野内转动），按它画直线或弯道，不依赖拍照点示教中线。
pub struct BurstRender {
    pub recipe: Arc<Recipe>,
    /// 计划相机下标（`Recipe.inspect` 的顺序）
    pub camera_idx: usize,
    /// 本件第几个脉冲，从 1 起
    pub seq: u64,
    pub kind: crate::plan::SegmentKind,
    /// 段内进度 0–1
    pub progress: f32,
    pub scenario: Scenario,
    pub seed: u64,
}

/// 触发携带的取图依据：模拟相机按配方合成（旧拍照点配方 `Sim`、段配方 `Burst`），样本集相机按身份取原图。
pub enum TriggerPayload {
    Sim(SimRender),
    Burst(BurstRender),
    Sample(SampleTarget),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraSource {
    /// 离线·合成图：由站点模式生成，不能手动配置
    Sim,
    Mvs,
    /// 离线·样本集：由站点模式按样本集生成，按触发携带的样本件与拍照点取现场原图
    SampleSet,
}

impl CameraSource {
    /// 合成图像（模拟相机）；其余来源都是真实原图，示教资料与标定按真实相机处理。
    pub fn synthetic(self) -> bool {
        self == CameraSource::Sim
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraConfig {
    /// 相机编号，配方用它引用相机；建相机时分配，之后不变（增删别的相机也不变）
    pub id: String,
    pub name: String,
    pub source: CameraSource,
    pub view_count: u8,
    pub composite_layout: Option<crate::recipe_capture::CompositeLayout>,
    pub serial: String,
    pub trigger_source: String,
    pub trigger_activation: String,
    pub frame_rate: f32,
    pub trigger_delay_us: f32,
    pub debouncer_us: i64,
    pub strobe: bool,
    pub chunk: bool,
    pub counter_after_open: Option<u64>,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "相机".into(),
            source: CameraSource::Mvs,
            view_count: 1,
            composite_layout: None,
            serial: String::new(),
            trigger_source: "Line0".into(),
            trigger_activation: "LevelHigh".into(),
            frame_rate: 10.0,
            trigger_delay_us: 0.0,
            debouncer_us: 5,
            strobe: true,
            chunk: true,
            counter_after_open: None,
        }
    }
}

impl CameraConfig {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.view_count, 1 | 3) {
            return Err("视角数量只能是 1 或 3".into());
        }
        if self.source == CameraSource::Mvs && self.view_count == 3 && self.composite_layout.is_none() {
            return Err("三幅拼接图必须明确配置 SDK 交付布局，未确认布局不能启用".into());
        }
        if !["Line0", "Software"].contains(&self.trigger_source.as_str()) {
            return Err("触发源只能是 Line0 或 Software".into());
        }
        if self.trigger_source == "Line0" && self.trigger_activation != "LevelHigh" {
            return Err("Line0 触发必须使用高电平".into());
        }
        if !self.frame_rate.is_finite() || self.frame_rate <= 0.0 {
            return Err("相机帧率必须为有限正数".into());
        }
        Ok(())
    }

    /// 海康相机必须绑定具体设备；新加的相机可以先不绑，但不打开、不能保存。
    fn require_bound(&self) -> Result<(), String> {
        if self.source == CameraSource::Mvs && self.serial.trim().is_empty() {
            return Err("未绑定设备：选择要绑定的相机设备后保存".into());
        }
        Ok(())
    }

    fn require_single_view(&self) -> Result<(), String> {
        if self.view_count != 1 || self.composite_layout.is_some() {
            return Err(format!("相机 {} 是三目拼接配置，分段连拍需要三台独立的单视角相机", self.id));
        }
        Ok(())
    }
}

/// 段配方的计划相机在相机组里都是单视角独立设备（D-SB-11）；缺相机或是三目拼接时返回原因。
/// 台数与重复由 plan::validate_segments 校验。
pub fn check_burst_cameras(rig: &[CameraConfig], cameras: &[String]) -> Result<(), String> {
    let mut frame_rate = None;
    for id in cameras {
        let config = rig.iter().find(|c| &c.id == id).ok_or_else(|| format!("相机组里没有计划相机 {id}"))?;
        config.validate()?;
        config.require_single_view()?;
        if config.trigger_source != "Line0" {
            return Err(format!("相机 {id} 为 Software 触发，请切换为 Line0 高电平后再进行整轮采集或生产"));
        }
        if frame_rate.is_some_and(|rate: f32| (rate - config.frame_rate).abs() > 0.001) {
            return Err("参与整轮采集的相机必须设置相同帧率".into());
        }
        frame_rate = Some(config.frame_rate);
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RigFile {
    cameras: Vec<CameraConfig>,
    /// 下一台新相机的编号 "cam{next_id}"：删掉的相机编号不再分给别的相机
    next_id: u32,
}

fn load_rig_file(path: &Path) -> (RigFile, Vec<String>, bool) {
    let mut notes = Vec::new();
    let mut file = match crate::fsio::read_json::<RigFile>(path) {
        Ok(file) => file.unwrap_or_default(),
        Err(note) => return (RigFile::default(), vec![note], false),
    };
    let old = file.cameras.iter().any(|c| c.trigger_source == "Line0" && c.trigger_activation != "LevelHigh");
    if !old { return (file, notes, false); }
    let backup = path.with_file_name(format!("cameras.level-high-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S%.3f")));
    if let Err(e) = std::fs::copy(path, &backup) {
        notes.push(format!("高电平配置迁移备份失败（{e}），保留原配置且禁止连接，请修复后重启"));
        return (file, notes, false);
    }
    for c in &mut file.cameras {
        if c.trigger_source == "Line0" { c.trigger_activation = "LevelHigh".into(); }
    }
    notes.push(format!("Line0 已改为高电平采集；原配置备份为 {}", backup.display()));
    (file, notes, true)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraStatus {
    pub cam: u8,
    pub id: String,
    pub name: String,
    pub source: CameraSource,
    pub ready: bool,
    /// 已释放给海康 MVS 客户端，不自动重连
    pub released: bool,
    pub message: String,
    pub device: Option<DeviceSummary>,
    pub sdk_version: Option<String>,
    pub frames: u64,
    pub fps: f32,
    pub max_fps: Option<f32>,
    pub configured_frame_rate: Option<f32>,
    pub lost_packets: u64,
    /// 检测节拍来不及取走、被丢弃的帧
    pub dropped_frames: u64,
    /// 本次打开以来因图像内存预算满没拷图、被丢弃的帧（按缺帧处理）
    pub budget_dropped_frames: u64,
    pub warnings: Vec<String>,
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

impl Reorder {
    /// 按帧号排队，返回现在可以依次交付的帧；空位（缺帧）占住帧号但不交付，后面的帧不必等它。
    fn accept(&mut self, session: u64, seq: u64, frame: Option<Frame>) -> Vec<Frame> {
        // 每台相机的帧序号从 1 开始；不能拿先到的那帧当起点，否则先合成完的第 2 帧会把第 1 帧挤到后面
        if self.next == 0 {
            self.next = 1;
        }
        if session != self.session || seq < self.next {
            return frame.into_iter().collect();
        }
        self.pending.insert(seq, frame);
        let mut ready = Vec::new();
        while let Some(frame) = self.pending.remove(&self.next) {
            self.next += 1;
            ready.extend(frame);
        }
        ready
    }
}

/// 样本集相机按触发身份取图后按帧号交付；取不到图（缺图、未选样本集）时本帧按缺帧占位，后面的帧照常交付。
/// 返回取图失败的原因。
#[allow(clippy::too_many_arguments)]
fn sample_set_arrival(order: &Mutex<Reorder>, target: Result<SampleTarget, String>, camera: &str, views: u8, cam: u8, session: u64, n: u64, manual: bool,
    mut deliver: impl FnMut(Frame)) -> Option<String> {
    let loaded = target.and_then(|target| target.images(camera, views));
    let (frame, error) = match loaded {
        Ok(images) => (Some(synthetic_frame(cam, session, n, images, manual)), None),
        Err(error) => (None, Some(error)),
    };
    let mut order = order.lock().unwrap();
    for frame in order.accept(session, n, frame) {
        deliver(frame);
    }
    error
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureTicket {
    session: u64,
    frame_counter: u64,
}

#[derive(Clone, Copy)]
enum CaptureRequest {
    Exact(CaptureTicket),
    After { session: u64, frame: Option<u64> },
}

impl CaptureRequest {
    fn session(self) -> u64 {
        match self { Self::Exact(ticket) => ticket.session, Self::After { session, .. } => session }
    }

    fn images(self, captured: &CapturedFrame) -> Option<Vec<Arc<FrameImage>>> {
        match self {
            Self::Exact(ticket) => captured.images_for(ticket),
            Self::After { session, frame } => (captured.ticket.session == session
                && frame.is_none_or(|last| captured.ticket.frame_counter > last)).then(|| captured.images.clone()),
        }
    }
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
    /// 需要整帧图像（图像测量或帧录制）；模拟测量时不必拷整帧
    capture: AtomicBool,
    recipe_capture: Arc<crate::recipe_capture::RecipeCaptureHost>,
}

/// 单台相机的交付通道，取图回调与模拟 / 样本集共用。
struct Shared {
    cam: u8,
    rig: Arc<RigShared>,
    /// 取图回调的整帧缓冲，按 IMAGE_BUDGET_PER_CAMERA 封顶在途字节
    pool: FramePool,
    frames: AtomicU64,
    lost_packets: AtomicU64,
    dropped: AtomicU64,
    /// 图像内存预算满被丢弃的帧（重新打开时清零）
    budget_dropped: AtomicU64,
    recent: Mutex<VecDeque<Instant>>,
    preview: Mutex<Vec<Preview>>,
    last_full: Mutex<Option<CapturedFrame>>,
    identity: Mutex<crate::shot_router::Ledgers>,
    /// last_full 每换一次 +1
    full_seq: AtomicU64,
    /// 下一帧不管要不要整帧都留一份（标定试测、软触发取图）
    grab: AtomicBool,
    /// 未完成的手动触发在回调到达或重新开流前阻止布防。
    manual: AtomicU32,
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
    /// 旧拍照点模型：三目拼接的拆分布局，段配方不经过（D-SB-11）
    composite_layout: Mutex<Option<crate::recipe_capture::CompositeLayout>>,
}

impl Shared {
    fn capture(&self) -> bool {
        self.rig.capture.load(Ordering::Relaxed) || self.rig.recipe_capture.active()
    }

    /// 每帧都做缩略图：触发采集的帧少，示教裁模板用的就是显示的那帧。
    fn set_previews(&self, session: u64, images: &[Arc<FrameImage>]) {
        if session != self.session.load(Ordering::SeqCst) {
            return;
        }
        *self.preview.lock().unwrap() = images.iter().map(|img| make_preview(&img.pixels, img.width as usize, img.height as usize)).collect();
    }

    /// 海康相机的一帧（像素格式已认过）：缩略图与整帧用同一张灰度。要留整帧而图像内存预算已满时返回 Err，
    /// 这一帧按缺帧处理；不留整帧时预算满只是没有缩略图。
    fn gray_image(&self, pixel_type: u32, w: u32, h: u32, data: &[u8]) -> Result<Option<Arc<FrameImage>>, BudgetExceeded> {
        let keep = self.capture() | self.grab.swap(false, Ordering::SeqCst);
        let gray = match to_gray(&self.pool, pixel_type, w, h, data) {
            Ok(g) => g,
            Err(GrayError::Invalid(e)) => {
                self.unusable(e);
                return Ok(None);
            }
            Err(GrayError::BudgetExceeded(full)) => return if keep { Err(full) } else { Ok(None) },
        };
        *self.preview.lock().unwrap() = vec![make_preview(gray.pixels(), w as usize, h as usize)];
        if !keep {
            return Ok(None);
        }
        gray.into_image(&self.pool, w, h).map(|image| Some(Arc::new(image)))
    }

    /// 图像内存预算满被丢弃的帧给状态栏的说明。
    fn budget_note(&self) -> Option<String> {
        let n = self.budget_dropped.load(Ordering::Relaxed);
        (n > 0).then(|| format!("图像内存预算（{} MiB）已满：{n} 帧没拷图、按缺帧处理，检测处理不过来", IMAGE_BUDGET_PER_CAMERA >> 20))
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
        for frame in order.accept(session, seq, frame) {
            self.deliver(frame);
        }
    }

    fn deliver(&self, frame: Frame) {
        self.deliver_original(frame, None);
    }

    fn deliver_original(&self, mut frame: Frame, original: Option<Arc<FrameImage>>) {
        let current_session = frame.session == self.session.load(Ordering::SeqCst);
        delivery::observe(&self.identity, &self.manual, &mut frame, current_session);
        if !frame.images.is_empty() && current_session {
            if frame.counter == CounterSource::Synthetic || frame.images.len() == 3 {
                self.set_previews(frame.session, &frame.images);
            }
            *self.last_full.lock().unwrap() = Some(CapturedFrame::from_frame(&frame));
            self.full_seq.fetch_add(1, Ordering::SeqCst);
        }
        self.count_arrival(&frame);
        if self.rig.recipe_capture.offer(&frame, original) { return; }
        let _ = delivery::offer(&self.rig.tx, &self.dropped, frame);
    }

    /// 图像内存预算满、没拷图的帧：台账照记、计入帧数，但不交付（节拍按缺帧处理），不中断回调。
    /// 段配方的示教采集轮次按帧号把它记为缺帧并告警（D-SB-8）。
    fn discard_over_budget(&self, mut frame: Frame) {
        let current_session = frame.session == self.session.load(Ordering::SeqCst);
        delivery::discard_over_budget(&self.identity, &self.manual, &self.budget_dropped, &mut frame, current_session);
        self.count_arrival(&frame);
        self.rig.recipe_capture.offer_dropped(&frame);
    }

    /// 帧数、丢包、帧率与界面事件：交付与否都算相机出了这一帧。
    fn count_arrival(&self, frame: &Frame) {
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
        let _ = self.rig.app.emit("camera://frame", frame);
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

impl PixelKind {
    fn bytes_per_pixel(self) -> usize {
        match self {
            PixelKind::Mono8 | PixelKind::Bayer8 => 1,
        }
    }
}

/// SDK 报的帧长够不够 宽 × 高 × 每像素字节数：不够就不能按宽高建切片（越界读）。返回要取的字节数。
fn frame_bytes(kind: PixelKind, w: u32, h: u32, frame_len: u32) -> Result<usize, String> {
    let need = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(kind.bytes_per_pixel()))
        .filter(|&n| n > 0).ok_or_else(|| format!("SDK 帧尺寸 {w}×{h} 无效，丢弃图像"))?;
    if (frame_len as usize) < need {
        return Err(format!("SDK 帧长 {frame_len} 字节不足 {w}×{h} 需要的 {need} 字节（帧不完整或宽高与像素格式不符），丢弃图像"));
    }
    Ok(need)
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

    fn into_image(self, pool: &FramePool, w: u32, h: u32) -> Result<FrameImage, BudgetExceeded> {
        match self {
            Gray::Raw(p) => pool.copy(w, h, p),
            Gray::Pooled(img) => Ok(img),
        }
    }
}

/// 取图回调收到的像素转成 8 位灰度。
fn to_gray<'a>(pool: &FramePool, pixel_type: u32, w: u32, h: u32, data: &'a [u8]) -> Result<Gray<'a>, GrayError> {
    match pixel_kind(pixel_type).map_err(GrayError::Invalid)? {
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

/// 模拟 / 样本集的一帧：帧计数与触发计数都是本会话的帧号。
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
        let frame_counter = u64::from(info.frame_num);
        let trigger_counter = frame_counter;
        let counter = CounterSource::SdkFrame;
        let session = shared.session.load(Ordering::SeqCst);
        let w = if info.extend_width != 0 { info.extend_width } else { info.width as u32 };
        let h = if info.extend_height != 0 { info.extend_height } else { info.height as u32 };
        let frame = |images: Vec<Arc<FrameImage>>| Frame { cam: shared.cam, session, counter, frame_counter, trigger_counter, lost_packets: info.lost_packet, ts: now_ms(), manual: false, images };
        // 先认像素格式、核对 SDK 帧长，再按宽 × 高取缓冲；图像不可用时帧照常交付（触发计数照记），只是不带图
        let image = match pixel_kind(info.pixel_type).and_then(|kind| frame_bytes(kind, w, h, info.frame_len)) {
            Err(e) => {
                shared.unusable(e);
                None
            }
            Ok(_) if data.is_null() => None,
            Ok(len) => match shared.gray_image(info.pixel_type, w, h, unsafe { std::slice::from_raw_parts(data, len) }) {
                Ok(image) => image,
                // 图像内存预算满（D-SB-32）：这一帧不拷图、不交付，台账照记
                Err(_) => return shared.discard_over_budget(frame(Vec::new())),
            },
        };
        // 旧拍照点模型：三目拼接拆成视角 1–3，段配方不经过（D-SB-11）
        let images = match (&image, shared.composite_layout.lock().unwrap().as_ref()) {
            (Some(image), Some(layout)) => match layout.split(image) {
                Ok(images) => images,
                Err(error) => { shared.unusable(error); Vec::new() }
            },
            _ => image.iter().cloned().collect(),
        };
        shared.deliver_original(frame(images), image);
    }));
}

extern "system" fn on_exception(msg: c_uint, user: *mut c_void) {
    if msg == mvs::MV_EXCEPTION_DEV_DISCONNECT && !user.is_null() {
        let shared = unsafe { &*(user as *const Shared) };
        shared.disconnected.store(true, Ordering::SeqCst);
        shared.rig.recipe_capture.disconnected(shared.cam);
    }
}

#[derive(Default)]
struct DeviceState {
    message: String,
    warnings: Vec<String>,
    max_fps: Option<f32>,
    configured_frame_rate: Option<f32>,
}

pub struct CameraSlot {
    device: Mutex<Option<mvs::Device>>,
    shared: Arc<Shared>,
    config: Mutex<CameraConfig>,
    state: Mutex<DeviceState>,
    /// 模拟 / 样本集本会话已出的触发数（帧计数与触发计数相同）
    seq: Mutex<u64>,
    /// 已释放给海康 MVS 客户端（调焦、光圈）：关闭设备且不自动重连，直到重新连接
    released: AtomicBool,
    supervision: CameraSupervision,
    /// 本次打开后已写进海康相机的曝光与增益；关闭、重开时清空，下次必写
    imaging: Mutex<Option<CameraImaging>>,
}

#[derive(Default)]
pub struct CameraSupervision {
    started: AtomicBool,
    paused: AtomicBool,
}

impl CameraSupervision {
    pub fn start(&self, auto_connect: bool) -> bool {
        if self.started.swap(true, Ordering::SeqCst) { return false; }
        self.paused.store(!auto_connect, Ordering::SeqCst);
        true
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn reconnect_due(&self, released: bool, lost: bool, opened: bool) -> bool {
        !self.paused.load(Ordering::SeqCst) && !released && (lost || !opened)
    }
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
                pool: FramePool::with_budget(POOL_KEEP_PER_CAMERA, IMAGE_BUDGET_PER_CAMERA),
                frames: AtomicU64::new(0),
                lost_packets: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                budget_dropped: AtomicU64::new(0),
                recent: Mutex::new(VecDeque::new()),
                preview: Mutex::new(Vec::new()),
                last_full: Mutex::new(None),
                identity: Mutex::new(crate::shot_router::Ledgers::default()),
                full_seq: AtomicU64::new(0),
                grab: AtomicBool::new(false),
                manual: AtomicU32::new(0),
                disconnected: AtomicBool::new(false),
                order: Mutex::new(Reorder::default()),
                unusable: AtomicU64::new(0),
                unusable_msg: Mutex::new(String::new()),
                session: AtomicU64::new(0),
                chunk_trigger: AtomicBool::new(false),
                chunk_frame: AtomicBool::new(false),
                composite_layout: Mutex::new(if config.view_count == 3 { config.composite_layout.clone() } else { None }),
            }),
            config: Mutex::new(config),
            state: Mutex::new(DeviceState::default()),
            seq: Mutex::new(0),
            released: AtomicBool::new(false),
            supervision: CameraSupervision::default(),
            imaging: Mutex::new(None),
        };
        if let Err(e) = slot.config().validate() {
            slot.state.lock().unwrap().message = e;
            return slot;
        }
        // 海康相机打开时才有会话
        if slot.config().source != CameraSource::Mvs {
            slot.new_session();
        }
        slot
    }

    pub fn config(&self) -> CameraConfig {
        self.config.lock().unwrap().clone()
    }

    fn set_config(&self, config: CameraConfig) {
        self.close();
        *self.shared.composite_layout.lock().unwrap() = if config.view_count == 3 { config.composite_layout.clone() } else { None };
        *self.config.lock().unwrap() = config;
        self.state.lock().unwrap().warnings.clear();
    }

    pub fn status(&self) -> CameraStatus {
        let config = self.config();
        let ready = self.is_ready();
        let (state_message, warnings, max_fps, configured_frame_rate) = {
            let s = self.state.lock().unwrap();
            (s.message.clone(), s.warnings.clone(), s.max_fps, s.configured_frame_rate)
        };
        let device = self.device.lock().unwrap().as_ref().map(|d| d.summary.clone());
        let released = self.released.load(Ordering::SeqCst);
        let message = if let Err(e) = config.validate().and_then(|()| config.require_bound()) { e } else { match config.source {
            CameraSource::Sim => format!("离线·合成图：每次触发交付一帧 · {} 个视角", config.view_count),
            CameraSource::SampleSet => match crate::offline::sample_set(&self.shared.rig.app) {
                Some(set) if set.has_camera(&config.id) => format!("离线·样本集 {}：{} 件 · {} 个拍照点 · {} 个视角",
                    set.manifest().name, set.manifest().parts.len(), set.manifest().shots.len(), config.view_count),
                Some(set) => format!("样本集 {} 没有相机 {}", set.manifest().id, config.id),
                None => "样本集相机只在离线·样本集模式下可用".to_string(),
            },
            CameraSource::Mvs if released => "已释放给海康 MVS 客户端，调整完点重新连接".to_string(),
            CameraSource::Mvs => [self.shared.unusable_note(), self.shared.budget_note(), Some(state_message)]
                .into_iter().flatten().collect::<Vec<_>>().join(" · "),
        }};
        CameraStatus {
            cam: self.shared.cam,
            id: config.id,
            name: config.name,
            source: config.source,
            ready,
            released,
            message,
            device,
            sdk_version: mvs::api().ok().map(|a| a.version.clone()),
            frames: self.shared.frames.load(Ordering::Relaxed),
            fps: self.shared.fps(),
            max_fps,
            configured_frame_rate,
            lost_packets: self.shared.lost_packets.load(Ordering::Relaxed),
            dropped_frames: self.shared.dropped.load(Ordering::Relaxed),
            budget_dropped_frames: self.shared.budget_dropped.load(Ordering::Relaxed),
            warnings,
        }
    }

    /// 估算链路带宽用的分辨率：海康相机最近一帧的原图尺寸；离线来源没有物理链路，没出过图时未知。
    fn link_frame_size(&self) -> Option<(u32, u32)> {
        if self.config.lock().unwrap().source != CameraSource::Mvs {
            return None;
        }
        self.shared.preview.lock().unwrap().first().map(|p| (p.full_width, p.full_height))
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
        let result = self.wait_views(before, self.shared.session.load(Ordering::SeqCst), timeout);
        self.shared.grab.store(false, Ordering::SeqCst);
        result
    }

    fn wait_views(&self, before: u64, session: u64, timeout: Duration) -> Option<Vec<Arc<FrameImage>>> {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            if self.shared.session.load(Ordering::SeqCst) != session { return None; }
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

    fn wait_capture(&self, request: CaptureRequest, timeout: Duration) -> Result<Option<Vec<Arc<FrameImage>>>, String> {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            if self.shared.session.load(Ordering::SeqCst) != request.session() {
                return Err("取样期间相机会话已变化，本次取样已取消".into());
            }
            if let Some(images) = self.shared.last_full.lock().unwrap().as_ref().and_then(|frame| request.images(frame)) {
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
            CameraSource::SampleSet => crate::offline::sample_set(&self.shared.rig.app).is_some_and(|set| set.has_camera(&config.id)),
        }
    }

    /// 开一个新会话：海康每次打开，模拟 / 样本集每次（重新）加载。模拟 / 样本集的计数从 0 重来，
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

    /// 模拟 / 样本集的下一帧：(会话号, 帧号)，帧号即触发计数，本会话从 1 起。
    fn next_counters(&self) -> (u64, u64) {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        (self.shared.session.load(Ordering::SeqCst), *seq)
    }

    /// 按触发出一帧。模拟相机合成（没有合成依据时只有元数据），样本集按触发身份取原图，海康发软触发。
    /// `lose_in_transfer` 仅模拟 / 样本集相机使用：相机已曝光但帧在传输中丢失，主机侧表现为帧计数跳号。
    pub fn trigger(&self, lose_in_transfer: bool, payload: Option<TriggerPayload>) -> bool {
        self.trigger_frame(lose_in_transfer, payload, false).is_ok()
    }

    fn trigger_frame(&self, lose_in_transfer: bool, payload: Option<TriggerPayload>, manual: bool) -> Result<Option<CaptureTicket>, String> {
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
                    let images = tauri::async_runtime::spawn_blocking(move || match payload {
                        Some(TriggerPayload::Sim(r)) => simimage::render_views(&r.recipe, r.k, r.scenario, r.pose, r.seed, config.view_count),
                        // 段配方：三台独立单视角相机各出一幅（D-SB-11）
                        Some(TriggerPayload::Burst(r)) if config.view_count == 1 => Ok(vec![Arc::new(simimage::render_burst(&r))]),
                        Some(TriggerPayload::Burst(_)) => Err(format!("段配方的相机必须是单视角，模拟相机 {} 配了 {} 个视角", config.id, config.view_count)),
                        _ if config.view_count == 3 => simimage::background_views(config.view_count, n),
                        _ => Ok(Vec::new()),
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
            CameraSource::SampleSet => {
                let target = match payload {
                    Some(TriggerPayload::Sample(target)) => Ok(target),
                    Some(TriggerPayload::Sim(_) | TriggerPayload::Burst(_)) => Err("样本集相机收到合成图触发，没有样本件身份".to_string()),
                    None => crate::offline::preview_target(&self.shared.rig.app, &config.id)
                        .ok_or_else(|| format!("未选择离线样本集，或样本集里没有相机 {} 的拍照点", config.id)),
                };
                let (session, n) = self.next_counters();
                let ticket = CaptureTicket { session, frame_counter: n };
                let shared = self.shared.clone();
                if lose_in_transfer {
                    shared.deliver_in_order(session, n, None);
                    return Ok(Some(ticket));
                }
                let camera = config.id.clone();
                let views = config.view_count;
                tauri::async_runtime::spawn(async move {
                    let worker = shared.clone();
                    let failed = tauri::async_runtime::spawn_blocking(move || {
                        sample_set_arrival(&worker.order, target, &camera, views, cam, session, n, manual, |frame| worker.deliver(frame))
                    }).await.unwrap_or_else(|e| {
                        shared.deliver_in_order(session, n, None);
                        Some(e.to_string())
                    });
                    if let Some(e) = failed {
                        cycle::log(&shared.rig.app, "err", "样本集取图", format!("{e}；本帧按缺帧处理"));
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
        }
    }

    fn soft_trigger(&self, payload: Option<TriggerPayload>) -> Result<CaptureRequest, String> {
        let config = self.config();
        config.validate()?;
        if config.source == CameraSource::Mvs && config.trigger_source != "Software" {
            return Err("触发源为 Line0，软触发前先把触发源改为 Software".into());
        }
        if config.source == CameraSource::Mvs {
            self.shared.manual.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .map_err(|_| "上一次软触发尚未返回图像，请等待；若已超时请重新连接相机")?;
        }
        let session = self.shared.session.load(Ordering::SeqCst);
        let after = self.shared.identity.lock().unwrap().get(self.shared.cam)
            .filter(|ledger| ledger.session == session).and_then(|ledger| ledger.last_frame);
        self.shared.grab.store(config.source == CameraSource::Mvs, Ordering::SeqCst);
        match self.trigger_frame(false, payload, true) {
            Ok(Some(ticket)) => Ok(CaptureRequest::Exact(ticket)),
            Ok(None) => Ok(CaptureRequest::After { session, frame: after }),
            Err(e) => {
                self.shared.grab.store(false, Ordering::SeqCst);
                Err(if config.source == CameraSource::Mvs { format!("{e}；软触发结果未确认，请重开相机后再布防") } else { e })
            }
        }
    }

    fn close(&self) {
        self.device.lock().unwrap().take();
        self.new_session();
        self.state.lock().unwrap().configured_frame_rate = None;
        self.imaging.lock().unwrap().take();
        self.shared.disconnected.store(false, Ordering::SeqCst);
        self.shared.unusable.store(0, Ordering::Relaxed);
        self.shared.budget_dropped.store(0, Ordering::Relaxed);
    }

    /// 把配方的曝光与增益写进已打开的海康相机，不重开、不换会话；与上次写入相同时不再写。模拟、样本集相机不用写。
    pub fn set_imaging(&self, imaging: &CameraImaging) -> Result<(), String> {
        let config = self.config();
        imaging.validate(&config.id)?;
        if config.source != CameraSource::Mvs { return Ok(()); }
        if self.released.load(Ordering::SeqCst) {
            return Err(format!("{}已释放给海康 MVS 客户端，重新连接后才能下发曝光与增益", config.name));
        }
        let device = self.device.lock().unwrap();
        let d = device.as_ref().filter(|_| !self.shared.disconnected.load(Ordering::SeqCst))
            .ok_or_else(|| format!("{}未连接，不能下发曝光与增益", config.name))?;
        let mut applied = self.imaging.lock().unwrap();
        if applied.as_ref() == Some(imaging) { return Ok(()); }
        *applied = None;
        d.set_float("ExposureTime", imaging.exposure_us).map_err(|e| format!("{}写入曝光时间失败：{e}", config.name))?;
        d.set_float("Gain", imaging.gain_db).map_err(|e| format!("{}写入增益失败：{e}", config.name))?;
        *applied = Some(*imaging);
        Ok(())
    }

    fn open(&self) -> Result<String, String> {
        self.supervision.resume();
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
        config.require_bound()?;
        let device = mvs::Device::open(&config.serial)?;
        let (warnings, max_fps, configured_frame_rate, format, chunks) = apply(&device, &config)?;
        self.shared.chunk_trigger.store(chunks.trigger, Ordering::Relaxed);
        self.shared.chunk_frame.store(chunks.frame, Ordering::Relaxed);
        // 开流前换会话：之后来的帧都算这次打开的
        self.new_session();
        device.start(on_image, on_exception, Arc::as_ptr(&self.shared) as *mut c_void)?;
        let format = if format == "Mono8" { format.to_string() } else { format!("{format} → 灰度") };
        let msg = format!("已连接 {} · {} · {format}", device.summary.model, device.summary.serial);
        *self.device.lock().unwrap() = Some(device);
        let mut state = self.state.lock().unwrap();
        state.warnings = warnings;
        state.max_fps = max_fps;
        state.configured_frame_rate = configured_frame_rate;
        state.message = msg.clone();
        Ok(msg)
    }

    /// 应用新配置：海康相机重新打开。返回相机未接受的参数。
    fn apply_config(&self) -> Result<Vec<String>, String> {
        match self.config().source {
            CameraSource::Sim | CameraSource::SampleSet => {
                self.close();
                self.new_session();
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

fn set_enum_verified(d: &mvs::Device, key: &str, value: &str) -> Result<(), String> {
    d.set_enum(key, value)?;
    let actual = d.get_enum(key)?;
    if actual != value { return Err(format!("{key} 回读为 {actual}，期望 {value}")); }
    Ok(())
}

fn configure_trigger(d: &mvs::Device, c: &CameraConfig) -> Result<Option<f32>, String> {
    set_enum_verified(d, "AcquisitionMode", "Continuous")?;
    let selectors = d.enum_entries("TriggerSelector")?;
    let selector = if c.trigger_source == "Software" {
        ["FrameStart", "FrameBurstStart"]
            .into_iter()
            .find(|name| selectors.iter().any(|s| s == name))
            .ok_or_else(|| {
                format!(
                    "相机 {} 不支持 Software 单帧触发（TriggerSelector: {}）",
                    d.summary.model,
                    selectors.join("、")
                )
            })?
    } else {
        ["FrameBurstActive", "AcquisitionActive"].into_iter()
            .find(|name| selectors.iter().any(|s| s == name))
            .or_else(|| {
                (c.trigger_source == "Line0"
                    && d.summary.model == "JHEM131GC-XS"
                    && selectors.iter().any(|s| s == "FrameBurstStart"))
                .then_some("FrameBurstStart")
            })
            .ok_or_else(|| {
                format!(
                    "相机 {} 未提供当前支持的高电平门控节点（TriggerSelector: {}），不能启用连续采集",
                    d.summary.model,
                    selectors.join("、")
                )
            })?
    };
    for entry in &selectors {
        set_enum_verified(d, "TriggerSelector", entry)?;
        set_enum_verified(d, "TriggerMode", "Off")?;
    }
    set_enum_verified(d, "TriggerSelector", selector)?;
    set_enum_verified(d, "TriggerSource", &c.trigger_source)?;
    if selector == "FrameBurstStart" {
        d.set_int("AcquisitionBurstFrameCount", 1)?;
        let actual = d.get_int("AcquisitionBurstFrameCount")?;
        if actual != 1 {
            return Err(format!(
                "AcquisitionBurstFrameCount 要求为 1，回读为 {actual}"
            ));
        }
    }
    let rate = if c.trigger_source == "Line0" {
        set_enum_verified(d, "TriggerActivation", "LevelHigh")?;
        if selector == "FrameBurstStart" {
            d.set_bool("TriggerCacheEnable", false)?;
            if d.get_bool("TriggerCacheEnable")? {
                return Err("相机未关闭触发缓存".into());
            }
        }
        d.set_trigger_delay(c.trigger_delay_us)?;
        d.set_bool("AcquisitionFrameRateEnable", true)?;
        if !d.get_bool("AcquisitionFrameRateEnable")? { return Err("相机未启用固定帧率".into()); }
        d.set_float("AcquisitionFrameRate", c.frame_rate)?;
        let actual = d.get_float("AcquisitionFrameRate")?;
        if !actual.is_finite() || (actual - c.frame_rate).abs() > (c.frame_rate * 0.001).max(0.001) {
            return Err(format!("相机帧率回读 {actual} fps，与设定 {} fps 不一致", c.frame_rate));
        }
        Some(actual)
    } else { None };
    set_enum_verified(d, "TriggerMode", "On")?;
    Ok(rate)
}

fn apply(d: &mvs::Device, c: &CameraConfig) -> Result<(Vec<String>, Option<f32>, Option<f32>, &'static str, Chunks), String> {
    let configured_frame_rate = configure_trigger(d, c)?;
    let format = set_pixel_format(d)?;
    let mut warnings = Vec::new();
    let mut must = |r: Result<(), String>| match r {
        Ok(()) => true,
        Err(e) => { warnings.push(e); false }
    };
    if c.trigger_source == "Line0" {
        must(d.set_enum("LineSelector", "Line0"));
        must(d.set_int("LineDebouncerTime", c.debouncer_us));
    }
    must(d.set_enum("ExposureAuto", "Off"));
    must(d.set_enum("GainAuto", "Off"));
    if c.strobe {
        must((|| {
            set_enum_verified(d, "LineSelector", "Line1")?;
            set_enum_verified(d, "LineMode", "Strobe")?;
            let sources = d.enum_entries("LineSource")?;
            let source = ["ExposureStartActive", "ExposureActive"]
                .into_iter()
                .find(|name| sources.iter().any(|s| s == name))
                .ok_or_else(|| {
                    format!(
                        "LineSource 不支持曝光频闪输出（可选项：{}）",
                        sources.join("、")
                    )
                })?;
            set_enum_verified(d, "LineSource", source)?;
            d.set_bool("StrobeEnable", true)?;
            if !d.get_bool("StrobeEnable")? {
                return Err("StrobeEnable 回读为关闭，频闪未启用".into());
            }
            Ok(())
        })());
    } else if d.set_enum("LineSelector", "Line1").is_ok() {
        let _ = d.set_bool("StrobeEnable", false);
    }
    let mut chunks = Chunks::default();
    if c.chunk {
        let entries = (|| {
            for node in ["ChunkModeActive", "ChunkSelector", "ChunkEnable"] {
                if !d.has_node(node)? {
                    return Err("该相机不支持 Chunk 附加数据；仍使用 SDK 帧序号进行采集诊断".into());
                }
            }
            d.set_bool("ChunkModeActive", true)?;
            if !d.get_bool("ChunkModeActive")? {
                return Err("ChunkModeActive 回读为关闭，Chunk 未启用".into());
            }
            d.enum_entries("ChunkSelector")
        })();
        match entries {
            Ok(entries) => {
                let wanted: Vec<_> = entries.iter().filter_map(|e| chunk_item(e).map(|item| (e, item))).collect();
                if wanted.is_empty() {
                    must(Err(format!("相机不支持帧计数 / 触发计数 Chunk（可选项：{}）", entries.join("、"))));
                }
                for (e, item) in wanted {
                    let on = must(set_enum_verified(d, "ChunkSelector", e))
                        && must(d.set_bool("ChunkEnable", true));
                    match item {
                        ChunkItem::Trigger => chunks.trigger |= on,
                        ChunkItem::Frame => chunks.frame |= on,
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
    Ok((warnings, max_fps, configured_frame_rate, format, chunks))
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
    pub fn recipe_capture(&self) -> &Arc<crate::recipe_capture::RecipeCaptureHost> { &self.rig.recipe_capture }

    pub fn new(app: &AppHandle, tx: Sender<Frame>) -> Result<Self, String> {
        let dir = crate::storage::config(app);
        let path = dir.join("cameras.json");
        let (mut file, mut notes, retired) = load_rig_file(&path);
        if file.cameras.is_empty() {
            // 单相机时代的 camera.json 迁移成相机组的第 1 台
            let old = std::fs::read_to_string(dir.join("camera.json")).ok().and_then(|s| serde_json::from_str::<CameraConfig>(&s).ok());
            file.cameras.push(CameraConfig { name: "相机 1".into(), ..old.unwrap_or_default() });
        }
        let offline_sources: Vec<String> = file.cameras.iter_mut().filter(|c| c.source != CameraSource::Mvs).map(|c| {
            c.source = CameraSource::Mvs;
            c.id.clone()
        }).collect();
        if !offline_sources.is_empty() {
            notes.push(format!("cameras.json 中的相机 {} 不是海康来源，已改为海康 MVS；合成图与样本集相机只由离线模式临时生成", offline_sources.join("、")));
        }
        let assigned = assign_ids(&mut file.cameras, &mut file.next_id) || !offline_sources.is_empty() || retired;
        let rig = Arc::new(RigShared {
            app: app.clone(),
            tx,
            capture: AtomicBool::new(false),
            recipe_capture: crate::recipe_capture::RecipeCaptureHost::open(crate::storage::root(app).join("recipe-capture"))?,
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
        if crate::offline::enabled(&self.rig.app) {
            return Err("离线期间相机组为临时配置，不写回 cameras.json".into());
        }
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

    /// 把配方的曝光与增益下发到这些相机；有一台写不进去就返回原因。
    pub fn apply_imaging(&self, cams: &[u8], recipe: &Recipe) -> Result<(), String> {
        for &c in cams {
            let slot = self.slot(c as usize).ok_or("相机组改过了")?;
            let id = slot.config().id;
            let imaging = recipe.imaging(&id).ok_or_else(|| format!("配方 {} 没有相机 {id} 的曝光与增益", recipe.id))?;
            slot.set_imaging(&imaging)?;
        }
        Ok(())
    }

    /// 取图回调是否拷贝整帧。
    pub fn set_capture(&self, on: bool) {
        self.rig.capture.store(on, Ordering::Relaxed);
    }

    pub fn trigger(&self, cam: u8, lose_in_transfer: bool, payload: Option<TriggerPayload>) -> bool {
        self.slot(cam as usize).is_some_and(|s| s.trigger(lose_in_transfer, payload))
    }

    pub fn last_view(&self, cam: u8, view: u8) -> Option<Arc<FrameImage>> {
        self.slot(cam as usize).and_then(|s| s.last_view(view))
    }

    pub fn capture_views(&self, cam: u8, payload: Option<TriggerPayload>) -> Result<Vec<Arc<FrameImage>>, String> {
        let cycle = self.rig.app.state::<CycleHost>();
        let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试取图")?;
        check_idle(&cycle)?;
        let slot = self.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        config.validate()?;
        if !slot.is_ready() {
            return Err(format!("相机未就绪：{}", slot.status().message));
        }
        let images = if config.source == CameraSource::Sim && !matches!(payload, Some(TriggerPayload::Sim(_) | TriggerPayload::Burst(_))) {
            let images = slot.last_views();
            if images.is_empty() {
                return Err("模拟相机尚未出图，请先运行一个模拟工件".into());
            }
            images
        } else if config.source != CameraSource::Mvs || config.trigger_source == "Software" {
            let request = slot.soft_trigger(payload)?;
            let captured = slot.wait_capture(request, Duration::from_secs(2))?;
            captured.ok_or("2 秒内没有收到本次触发的完整图像；检查触发与相机连接")?
        } else {
            slot.grab_views(Duration::from_secs(2)).ok_or("请在所选位置触发相机；2 秒内未收到完整图像")?
        };
        if images.len() != config.view_count as usize {
            return Err(format!("相机需要 {} 个视角，这一帧只有 {} 幅图像", config.view_count, images.len()));
        }
        Ok(images)
    }

    pub fn capture_view(&self, cam: u8, view: u8, payload: Option<TriggerPayload>) -> Result<Arc<FrameImage>, String> {
        let slot = self.slot(cam as usize).ok_or("相机不存在")?;
        if view == 0 || view > slot.config().view_count {
            return Err(format!("相机没有视角 {view}"));
        }
        self.capture_views(cam, payload)?.get(view as usize - 1).cloned().ok_or_else(|| format!("这一帧缺少视角 {view}"))
    }

    fn rebuild(&self, app: &AppHandle, configs: Vec<CameraConfig>) {
        // 增删别的相机不把已释放给 MVS 的相机抢回来
        let released: Vec<String> = self.slots().iter().filter(|s| s.released.load(Ordering::SeqCst)).map(|s| s.config().id).collect();
        let slots: Vec<Arc<CameraSlot>> = configs.into_iter().enumerate().map(|(i, c)| {
            let keep = c.source == CameraSource::Mvs && released.contains(&c.id);
            let slot = CameraSlot::new(&self.rig, i as u8, c);
            slot.released.store(keep, Ordering::SeqCst);
            Arc::new(slot)
        }).collect();
        // 旧相机在锁外释放：关海康相机要一会儿，别让别的线程卡在读相机组上
        let old = std::mem::replace(&mut *self.slots.write().unwrap(), slots.clone());
        self.generation.fetch_add(1, Ordering::SeqCst);
        drop(old);
        for s in &slots {
            supervise(app, s, true);
        }
    }

    /// 站点模式切换时整组替换相机，不写回 cameras.json。
    pub fn replace_temporarily(&self, app: &AppHandle, configs: Vec<CameraConfig>) {
        self.rebuild(app, configs);
        let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
        let _ = app.emit("camera://changed", ());
    }

    pub fn start(app: &AppHandle, auto_connect: bool) {
        for s in app.state::<CycleHost>().camera.slots() {
            supervise(app, &s, auto_connect);
        }
    }
}

/// 每台相机一个后台循环：海康相机掉线或未打开时每 2 s 重连（已释放给 MVS 的不连）。相机被移出相机组后循环自己结束。
fn supervise(app: &AppHandle, slot: &Arc<CameraSlot>, auto_connect: bool) {
    if !slot.supervision.start(auto_connect) { return; }
    let slot = Arc::downgrade(slot);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_error: Option<String> = None;
        let mut next_retry = Instant::now();
        loop {
            let Some(s) = slot.upgrade() else { break };
            let config = s.config();
            if config.source == CameraSource::Mvs && Instant::now() >= next_retry {
                next_retry = Instant::now() + Duration::from_secs(2);
                let attempt = {
                    let cycle = app.state::<CycleHost>();
                    let attempt = match cycle.plc_gate.try_lock() {
                        Ok(_gate) => {
                            let config = s.config();
                            let lost = s.shared.disconnected.load(Ordering::SeqCst);
                            let opened = s.device.lock().unwrap().is_some();
                            if config.source == CameraSource::Mvs && !config.serial.trim().is_empty()
                                && s.supervision.reconnect_due(s.released.load(Ordering::SeqCst), lost, opened) {
                                let s2 = s.clone();
                                Some((config, lost, tauri::async_runtime::spawn_blocking(move || s2.open()).await))
                            } else { None }
                        }
                        Err(_) => None,
                    };
                    attempt
                };
                if let Some((config, lost, result)) = attempt {
                    if lost {
                        cycle::log(&app, "err", "相机断线", format!("{}：尝试重新连接", config.name));
                    }
                    match result {
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

/// 相机组在 `hz` 触发频率下的链路带宽估算（规格 §6：按各路之和），按相机组顺序。
#[tauri::command]
pub fn camera_bandwidth(cycle: State<'_, CycleHost>, hz: f64) -> Result<BandwidthEstimate, String> {
    if !(hz.is_finite() && hz > 0.0) {
        return Err(format!("触发频率需为正数（实际 {hz}）"));
    }
    let sizes: Vec<_> = cycle.camera.slots().iter().map(|s| s.link_frame_size()).collect();
    Ok(aggregate_bandwidth(&sizes, hz))
}

/// 保存并应用一台相机的配置。返回相机未接受的参数。
#[tauri::command]
pub async fn camera_save_config(app: AppHandle, cam: usize, config: CameraConfig) -> Result<Vec<String>, String> {
    let cycle = app.state::<CycleHost>();
    check_user_editable(&app, Some(&config))?;
    let _gate = cycle.plc_gate.lock().await;
    check_user_editable(&app, None)?;
    config.validate()?;
    config.require_bound()?;
    // 海康相机要重新打开，检测中改会打断这一件
    check_idle(&app.state::<CycleHost>())?;
    let slot = rig(&app).slot(cam).ok_or("相机不存在")?;
    // 编号是配方引用相机的依据；陈旧索引不能覆盖另一台相机。
    check_camera_target(&slot.config().id, &config.id)?;
    if slot.released.load(Ordering::SeqCst) {
        return Err("相机已释放给海康 MVS 客户端，重新连接后再保存".into());
    }
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

/// 同一个序列号不能给两台海康相机（留空的是刚加、还没绑定设备的相机）。
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

/// 真机模式下用户只能配置海康相机；合成图、样本集相机由站点模式在离线期间临时生成。
fn check_user_editable(app: &AppHandle, config: Option<&CameraConfig>) -> Result<(), String> {
    if crate::offline::enabled(app) {
        return Err("离线模式由站点模式管理相机组，退出离线后再修改".into());
    }
    if config.is_some_and(|c| c.source != CameraSource::Mvs) {
        return Err("真机模式下相机来源只能是海康 MVS；合成图与样本集请在离线控制台进入离线".into());
    }
    Ok(())
}

/// 相机组增删只能在空闲或故障时做：相机序号会重排（编号不变）。
pub(crate) fn check_idle(cycle: &CycleHost) -> Result<(), String> {
    check_access(cycle, CameraAccess::Configuration)
}

pub enum CameraAccess {
    Configuration,
    Maintenance,
}

impl CameraAccess {
    pub fn check(self, in_flight: bool, capture_active: bool, pending: bool) -> Result<(), String> {
        if capture_active {
            return Err("示教整圈采集正在独占设备，请结束本轮采集后操作".into());
        }
        if in_flight {
            return Err("检测进行中，工件结束后再操作相机".into());
        }
        if matches!(self, Self::Configuration) && pending {
            return Err("PLC 仍有未结事务或握手日志故障，请连接 PLC 并复位后再修改相机或采集；可先释放相机给 MVS 排查".into());
        }
        Ok(())
    }
}

fn check_access(cycle: &CycleHost, access: CameraAccess) -> Result<(), String> {
    access.check(
        cycle.in_flight(),
        cycle.camera.recipe_capture().active(),
        cycle.busy(),
    )
}

#[cfg(test)]
mod access_tests {
    use super::CameraAccess;

    #[test]
    fn unresolved_plc_without_active_capture_allows_maintenance_but_keeps_configuration_locked() {
        for (case, in_flight, capture_active, pending, maintenance, configuration) in [
            ("空闲", false, false, false, true, true),
            (
                "初始化故障且有历史未结事务",
                false,
                false,
                true,
                true,
                false,
            ),
            (
                "真实检测在途且界面快照尚未更新",
                true,
                false,
                false,
                false,
                false,
            ),
            ("检测及握手在途", true, false, true, false, false),
            ("示教采集独占", false, true, false, false, false),
            ("示教采集及未结握手", false, true, true, false, false),
        ] {
            assert_eq!(
                CameraAccess::Maintenance
                    .check(in_flight, capture_active, pending)
                    .is_ok(),
                maintenance,
                "{case}"
            );
            assert_eq!(
                CameraAccess::Configuration
                    .check(in_flight, capture_active, pending)
                    .is_ok(),
                configuration,
                "{case}"
            );
        }
    }
}

#[tauri::command]
pub fn camera_add(app: AppHandle, mut config: CameraConfig) -> Result<usize, String> {
    config.validate()?;
    check_user_editable(&app, Some(&config))?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_user_editable(&app, None)?;
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
    check_user_editable(&app, None)?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_user_editable(&app, None)?;
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

/// 调试用：把曝光与增益临时写进相机，不写配方；示教采集和生产布防前仍按配方重新下发。
#[tauri::command]
pub async fn camera_set_imaging(app: AppHandle, cam: usize, id: String, imaging: CameraImaging) -> Result<(), String> {
    check_user_editable(&app, None)?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_idle(&cycle)?;
    let slot = rig(&app).slot(cam).ok_or("相机不存在")?;
    check_camera_target(&slot.config().id, &id)?;
    tauri::async_runtime::spawn_blocking(move || slot.set_imaging(&imaging)).await.map_err(|e| e.to_string())?
}

/// 把海康相机释放给 MVS 客户端（调焦、光圈）：关闭设备，后台不再自动重连，直到重新连接。
#[tauri::command]
pub async fn camera_release(app: AppHandle, cam: usize) -> Result<(), String> {
    check_user_editable(&app, None)?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .plc_gate
        .try_lock()
        .map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_access(&cycle, CameraAccess::Maintenance)?;
    let slot = rig(&app).slot(cam).ok_or("相机不存在")?;
    let config = slot.config();
    if config.source != CameraSource::Mvs {
        return Err("只有海康相机可以释放给 MVS 客户端".into());
    }
    slot.released.store(true, Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || slot.close()).await.map_err(|e| e.to_string())?;
    cycle::log(&app, "ok", "相机", format!("{}：已释放给海康 MVS 客户端", config.name));
    let _ = cycle.tx.send(cycle::Input::Refresh);
    let _ = app.emit("camera://changed", ());
    Ok(())
}

/// 收回已释放的海康相机并重新打开；MVS 客户端还占着时返回打开失败的原因，后台继续每 2 s 重试。
#[tauri::command]
pub async fn camera_reconnect(app: AppHandle, cam: usize) -> Result<String, String> {
    check_user_editable(&app, None)?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .plc_gate
        .try_lock()
        .map_err(|_| "正在处理 PLC 事务，请稍后重试相机操作")?;
    check_access(&cycle, CameraAccess::Maintenance)?;
    let slot = rig(&app).slot(cam).ok_or("相机不存在")?;
    slot.released.store(false, Ordering::SeqCst);
    let opened = tauri::async_runtime::spawn_blocking(move || slot.open()).await.map_err(|e| e.to_string())?;
    let _ = cycle.tx.send(cycle::Input::Refresh);
    let _ = app.emit("camera://changed", ());
    opened
}

#[cfg(test)]
mod rig_file_tests {
    use super::*;

    #[test]
    fn released_camera_is_never_reopened_by_the_supervisor() {
        let supervision = CameraSupervision::default();
        supervision.start(true);
        for (lost, opened) in [(false, false), (true, false), (true, true)] {
            assert!(!supervision.reconnect_due(true, lost, opened), "lost={lost} opened={opened}");
            assert!(supervision.reconnect_due(false, lost, opened), "lost={lost} opened={opened}");
        }
        assert!(!supervision.reconnect_due(false, false, true));
    }
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
    fn burst_plan_camera_missing_or_not_single_view_is_rejected() {
        let single = |id: &str| CameraConfig { id: id.into(), view_count: 1, ..CameraConfig::default() };
        let plan: Vec<String> = ["cam1", "cam2", "cam3"].map(String::from).to_vec();
        let cases: [(&str, CameraConfig, bool); 7] = [
            ("单视角独立相机", single("cam3"), true),
            ("Software 不参与整轮采集", CameraConfig { trigger_source: "Software".into(), ..single("cam3") }, false),
            ("帧率不一致", CameraConfig { frame_rate: 20.0, ..single("cam3") }, false),
            ("旧边沿不可进入生产", CameraConfig { trigger_activation: "RisingEdge".into(), ..single("cam3") }, false),
            ("三视角样本集相机", CameraConfig { source: CameraSource::SampleSet, view_count: 3, ..single("cam3") }, false),
            ("配了三目拼接布局", CameraConfig { composite_layout: Some(crate::recipe_capture::CompositeLayout::Horizontal), ..single("cam3") }, false),
            ("计划相机不在相机组", single("cam9"), false),
        ];
        for (name, third, accepted) in cases {
            let rig = [single("cam1"), single("cam2"), third];
            assert_eq!(check_burst_cameras(&rig, &plan).is_ok(), accepted, "{name}");
        }
    }

    #[test]
    fn software_capture_rejects_the_previous_callback_image_after_its_pending_flag_clears() {
        let image = |session, counter, value| CapturedFrame::from_frame(&synthetic_frame(0, session, counter,
            vec![Arc::new(FrameImage::new(1, 1, vec![value]))], true));
        let requested = CaptureRequest::After { session: 12, frame: Some(7) };
        assert!(requested.images(&image(12, 7, 10)).is_none());
        assert!(requested.images(&image(11, 8, 20)).is_none());
        assert_eq!(requested.images(&image(12, 8, 30)).unwrap()[0].pixels, [30]);
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

#[cfg(test)]
mod gray_tests {
    use super::*;

    #[test]
    fn sdk_frame_length_must_cover_width_height_and_pixel_size() {
        for kind in [PixelKind::Mono8, PixelKind::Bayer8] {
            assert_eq!(frame_bytes(kind, 2448, 2048, 2448 * 2048), Ok(2448 * 2048));
            // SDK 缓冲比宽高需要的长（行尾填充、附带 Chunk）：只取宽 × 高
            assert_eq!(frame_bytes(kind, 4, 3, 64), Ok(12));
            // 差一个字节就是不完整帧
            assert!(frame_bytes(kind, 2448, 2048, 2448 * 2048 - 1).is_err());
            for (w, h) in [(0, 2048), (2448, 0)] {
                assert!(frame_bytes(kind, w, h, u32::MAX).is_err(), "{w}×{h}");
            }
        }
        // 宽高乘积超过 u32：以前 w * h 会溢出回绕成小切片（70000² mod 2³² 小于 u32::MAX 会被放行），现在按实际需要比较
        assert!(frame_bytes(PixelKind::Mono8, 70000, 70000, u32::MAX).is_err());
    }
}

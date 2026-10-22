use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::camera::CameraSource;
use crate::cycle::{CycleHost, Phase};
use crate::frame::{CounterSource, Frame, FrameImage};
use crate::recipe::CameraImaging;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CompositeLayout {
    Horizontal,
    Vertical,
    Rects { rects: [ViewRect; 3] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CompositeLayout {
    pub fn split(&self, image: &FrameImage) -> Result<Vec<Arc<FrameImage>>, String> {
        let (w, h) = (image.width, image.height);
        if w == 0 || h == 0 || (w as usize).checked_mul(h as usize) != Some(image.pixels.len()) {
            return Err("拼接原图的尺寸或像素长度无效".into());
        }
        let rects = match self {
            Self::Horizontal if w % 3 == 0 => std::array::from_fn(|i| ViewRect { x: i as u32 * (w / 3), y: 0, width: w / 3, height: h }),
            Self::Vertical if h % 3 == 0 => std::array::from_fn(|i| ViewRect { x: 0, y: i as u32 * (h / 3), width: w, height: h / 3 }),
            Self::Rects { rects } => rects.clone(),
            _ => return Err("拼接图尺寸不能按已配置方向三等分，请核对 SDK 交付布局".into()),
        };
        for (i, r) in rects.iter().enumerate() {
            if r.width == 0 || r.height == 0 || r.x.checked_add(r.width).is_none_or(|v| v > w) || r.y.checked_add(r.height).is_none_or(|v| v > h) {
                return Err(format!("图 {} 的裁切区域越界或为空", i + 1));
            }
            if rects[..i].iter().any(|other| r.x < other.x + other.width && other.x < r.x + r.width && r.y < other.y + other.height && other.y < r.y + r.height) {
                return Err("三幅图的裁切区域不能重叠".into());
            }
        }
        Ok(rects.iter().map(|r| {
            let mut pixels = Vec::with_capacity(r.width as usize * r.height as usize);
            for y in r.y..r.y + r.height {
                let start = y as usize * w as usize + r.x as usize;
                pixels.extend_from_slice(&image.pixels[start..start + r.width as usize]);
            }
            Arc::new(FrameImage::new(r.width, r.height, pixels))
        }).collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureState { WaitingStart, Receiving, Draining, Complete, Failed }

impl CaptureState {
    fn active(&self) -> bool { matches!(self, Self::WaitingStart | Self::Receiving | Self::Draining) }
}

/// 一轮整圈采集最多同时使用的相机数。
const MAX_CAMERAS: usize = 8;
/// PLC 一圈最多触发次数；每台相机的写盘队列能放下一整圈。
const MAX_TRIGGERS: usize = 64;
/// 多相机时配方拍照点编号为 "S001-{相机编号}"，须在 32 个字符以内。
const MAX_MULTI_CAMERA_ID: usize = 27;
/// 等 PLC 开始后整圈收图、以及 PLC 结束后已收到图像写盘完成的上限。
const ROUND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureCamera {
    pub camera_id: String,
    pub camera_config: serde_json::Value,
    pub imaging: CameraImaging,
    pub device_session: u64,
    pub received_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureView {
    pub view: u8,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// 一台相机在一次触发里交付的帧。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureFrame {
    pub camera_id: String,
    pub frame_counter: u64,
    pub trigger_counter: Option<u64>,
    pub original_path: String,
    pub views: Vec<CaptureView>,
}

/// 一次 PLC 触发：参与本轮的每台相机各一帧，按本轮相机顺序排列。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureShot {
    pub shot_id: String,
    pub ordinal: u32,
    pub cameras: Vec<CaptureFrame>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureRound {
    pub round_id: String,
    pub recipe_id: String,
    pub cameras: Vec<CaptureCamera>,
    pub planned_count: Option<u32>,
    pub plc_planned_count: Option<u32>,
    pub plc_actual_count: Option<u32>,
    pub plc_plan_version: Option<u32>,
    pub state: CaptureState,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    pub frames: Vec<CaptureShot>,
    pub error: Option<String>,
    /// 失败种类与涉及的相机、计数；error 是据此给操作员看的文案。旧记录没有这一项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<CaptureFailure>,
    pub simulated: bool,
}

/// 采集轮次为什么失败。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CaptureFailure {
    /// 这台相机的帧、写盘或设备出错
    Camera { camera_id: String },
    /// PLC 开始/结束信号顺序或数量异常
    Plc,
    /// 超时仍未收到 PLC 开始请求
    StartTimeout,
    /// 超时时到达数量与计划不符的相机，按本轮相机顺序
    ShortCameras { planned: u32, cameras: Vec<CameraArrival> },
    /// PLC 已结束、各相机图像都已到达，但写盘未在时限内完成
    WriteTimeout { planned: u32, written: u32 },
    /// 各相机已收满计划数，但没收到 PLC 结束
    PlcEndMissing { planned: u32 },
    /// 应用退出导致本轮中断
    Interrupted,
    /// 外部中止（操作员停止、PLC 断连、布防失败）或采集记录保存失败
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraArrival {
    pub camera_id: String,
    /// 已到达（进入写盘队列）的帧数
    pub received: u32,
}

/// 开始整圈采集被拒的原因；Display 是给操作员看的文案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureStartRejected {
    DrainTimeout,
    CameraCount,
    InvalidCameraId(String),
    Duplicate(String),
    SessionNotReady(String),
    MissingBaseline(String),
    Busy,
    /// 上一轮失败后可能仍有在途旧帧、须重新连接的相机，按选择顺序
    UncertainSessions(Vec<String>),
    Io(String),
}

impl std::fmt::Display for CaptureStartRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DrainTimeout => write!(f, "等待在途帧时间须为 200–30000 ms"),
            Self::CameraCount => write!(f, "整圈采集须选择 1–{MAX_CAMERAS} 台相机"),
            Self::InvalidCameraId(id) => write!(f, "相机编号 {id:?} 无效；多相机采集时相机编号最长 {MAX_MULTI_CAMERA_ID} 个字符"),
            Self::Duplicate(id) => write!(f, "相机 {id} 被重复选择"),
            Self::SessionNotReady(id) => write!(f, "相机 {id}：设备会话尚未就绪"),
            Self::MissingBaseline(id) => write!(f, "相机 {id}：没有可验证的开流计数基线，请先配置计数起点或取得当前会话计数"),
            Self::Busy => write!(f, "已有示教采集正在占用设备"),
            Self::UncertainSessions(ids) => write!(f, "相机 {}：上一轮采集异常，当前设备会话可能仍有在途旧帧；请重新连接这些相机后整圈重采", ids.join("、")),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// 采用到候选配方时的一个拍照点：第 k 次触发里某台相机的帧。
pub struct RecipeShot<'a> {
    pub k: usize,
    pub shot_id: String,
    pub pose_id: String,
    pub camera: &'a CaptureCamera,
    pub frame: &'a CaptureFrame,
}

impl CaptureRound {
    pub fn camera_ids(&self) -> Vec<String> { self.cameras.iter().map(|c| c.camera_id.clone()).collect() }

    /// 所有相机都已到图的触发数。
    pub fn complete_shots(&self) -> u32 { self.frames.iter().filter(|s| s.cameras.len() == self.cameras.len()).count() as u32 }

    /// 按"触发顺序 → 本轮相机顺序"展开成配方拍照点；同一次触发共用 Pose。
    pub fn recipe_shots(&self) -> Result<Vec<RecipeShot<'_>>, String> {
        let multi = self.cameras.len() > 1;
        let mut shots = Vec::with_capacity(self.frames.len() * self.cameras.len());
        for (k, shot) in self.frames.iter().enumerate() {
            if shot.cameras.len() != self.cameras.len() || shot.cameras.iter().zip(&self.cameras).any(|(f, c)| f.camera_id != c.camera_id) {
                return Err(format!("采集拍照点 {} 缺少部分相机的图像", shot.shot_id));
            }
            let pose_id = format!("S{:03}", k + 1);
            for (camera, frame) in self.cameras.iter().zip(&shot.cameras) {
                let shot_id = if multi { format!("{pose_id}-{}", camera.camera_id) } else { pose_id.clone() };
                shots.push(RecipeShot { k, shot_id, pose_id: pose_id.clone(), camera, frame });
            }
        }
        Ok(shots)
    }
}

/// 开始采集时一台相机的身份与计数起点。
pub struct CaptureDevice {
    pub cam: u8,
    pub camera_id: String,
    pub camera_config: serde_json::Value,
    pub imaging: CameraImaging,
    pub session: u64,
    pub source: Option<CounterSource>,
    pub trigger_baseline: Option<u64>,
}

struct RunCam {
    cam: u8,
    source: Option<CounterSource>,
    last_frame: Option<u64>,
    last_trigger: Option<u64>,
    /// 本轮因这台相机的帧或设备出错而失败
    faulted: bool,
    /// 已从相机到达并进入写盘队列的帧数；received_count 要等写盘线程取出后才累加
    queued: u32,
}

struct Running {
    round: CaptureRound,
    cams: Vec<RunCam>,
    drain_timeout: Duration,
    deadline: Instant,
    /// PLC 结束后，图像均已到达、只差写盘时最多等到这里
    write_deadline: Option<Instant>,
}

impl Running {
    fn camera_of(&self, cam: u8) -> Option<usize> { self.cams.iter().position(|c| c.cam == cam) }
}

struct QueuedFrame { round_id: String, frame: Frame, original: Option<Arc<FrameImage>> }

pub struct RecipeCaptureHost {
    root: PathBuf,
    current: Mutex<Option<Running>>,
    /// 设备会话编号每次启动从 1 重新分配，只封锁本进程内失败轮次的会话，重启后不沿用
    uncertain_sessions: Mutex<std::collections::HashSet<(String, u64)>>,
    /// 按本轮相机顺序，每台相机一个写盘线程
    writers: Vec<mpsc::SyncSender<QueuedFrame>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSummary {
    pub round_id: String,
    pub recipe_id: String,
    pub camera_ids: Vec<String>,
    pub planned_count: Option<u32>,
    pub received_count: u32,
    pub state: CaptureState,
    pub created_at: i64,
    pub error: Option<String>,
}

fn camera_dir(root: &Path, round_id: &str, ordinal: u32, camera_id: &str) -> PathBuf {
    root.join(round_id).join(format!("shot-{ordinal:04}")).join(camera_id)
}

impl RecipeCaptureHost {
    fn safe_round_path(&self, round_id: &str) -> Result<PathBuf, String> {
        if !round_id.starts_with("capture-") || !round_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("无效的采集轮次 ID".into());
        }
        let root = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
        let path = std::fs::canonicalize(self.root.join(round_id)).map_err(|e| e.to_string())?;
        if !path.starts_with(&root) || path == root { return Err("采集目录不在允许的保存位置".into()); }
        Ok(path)
    }

    fn validate_record(&self, expected_id: &str, round: &CaptureRound) -> Result<(), String> {
        let dir = self.safe_round_path(expected_id)?;
        let cameras = &round.cameras;
        if round.round_id != expected_id || round.recipe_id.is_empty() || cameras.is_empty() || cameras.len() > MAX_CAMERAS
            || cameras.iter().enumerate().any(|(i, c)| !crate::recipe::valid_camera_id(&c.camera_id) || c.device_session == 0
                || cameras[..i].iter().any(|o| o.camera_id == c.camera_id))
            || round.planned_count.is_some_and(|n| !(1..=64).contains(&n))
            || cameras.iter().any(|c| round.frames.iter().filter(|s| s.cameras.iter().any(|f| f.camera_id == c.camera_id)).count() > c.received_count as usize) {
            return Err("采集记录身份、相机、计划数量或收图数量无效".into());
        }
        if round.state == CaptureState::WaitingStart && (round.planned_count.is_some() || round.plc_planned_count.is_some()
            || cameras.iter().any(|c| c.received_count != 0) || !round.frames.is_empty() || round.plc_actual_count.is_some()) {
            return Err("等待 PLC 开始时不能预先指定触发数量或存在收图".into());
        }
        if matches!(round.state, CaptureState::Receiving | CaptureState::Draining | CaptureState::Complete)
            && (round.planned_count.is_none() || round.plc_planned_count != round.planned_count) {
            return Err("采集记录缺少有效的 PLC 开始计划数量".into());
        }
        if round.state == CaptureState::Complete && (round.error.is_some() || round.failure.is_some() || round.ended_at.is_none()
            || round.plc_actual_count != round.planned_count
            || cameras.iter().any(|c| Some(c.received_count) != round.planned_count)
            || Some(round.frames.len() as u32) != round.planned_count || round.complete_shots() as usize != round.frames.len()) {
            return Err("完整采集缺少 PLC 结束、各相机触发数量或完整图像证据".into());
        }
        for (k, shot) in round.frames.iter().enumerate() {
            let ordinal = k as u32 + 1;
            if shot.ordinal != ordinal || shot.shot_id != format!("S{ordinal:03}") || shot.cameras.is_empty() {
                return Err("采集拍照点顺序无效".into());
            }
            let mut last = None;
            for frame in &shot.cameras {
                let index = cameras.iter().position(|c| c.camera_id == frame.camera_id).ok_or("采集拍照点引用了本轮之外的相机")?;
                if last.is_some_and(|l| index <= l) { return Err("采集拍照点的相机重复或顺序与本轮不一致".into()); }
                last = Some(index);
                if frame.views.len() != view_count(&cameras[index].camera_config)
                    || frame.views.iter().enumerate().any(|(v, image)| image.view as usize != v + 1 || image.width == 0 || image.height == 0) {
                    return Err(format!("相机 {} 的图像数量与设备视角数不符", frame.camera_id));
                }
                let frame_dir = camera_dir(&self.root, expected_id, ordinal, &frame.camera_id);
                let mut files = vec![(PathBuf::from(&frame.original_path), frame_dir.join("original.png"))];
                files.extend(frame.views.iter().map(|v| (PathBuf::from(&v.path), frame_dir.join(format!("view-{}.png", v.view)))));
                for (stored, expected) in files {
                    if stored != expected { return Err("采集图像路径与轮次目录不一致".into()); }
                    let actual = std::fs::canonicalize(&expected).map_err(|e| format!("采集图像不存在：{e}"))?;
                    let required = dir.join(format!("shot-{ordinal:04}")).join(&frame.camera_id).join(expected.file_name().ok_or("图像文件名无效")?);
                    if actual != required { return Err("采集图像路径越出本轮目录或链接到其他拍照点".into()); }
                }
            }
        }
        Ok(())
    }

    pub fn open(root: PathBuf) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut latest: Option<CaptureRound> = None;
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path().join("round.json");
            if !path.is_file() { continue; }
            if !std::fs::canonicalize(&path).map_err(|e| e.to_string())?.starts_with(&canonical_root) { return Err("采集记录路径越出保存目录".into()); }
            let mut round: CaptureRound = match serde_json::from_str(&crate::fsio::read_text(&path).map_err(|e| e.to_string())?) {
                Ok(round) => round,
                Err(_) => {
                    let backup = path.with_file_name(format!("round.broken-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
                    std::fs::rename(&path, &backup).map_err(|e| format!("采集记录无法读取且备份失败：{e}"))?;
                    continue;
                }
            };
            if round.state.active() {
                round.state = CaptureState::Failed;
                round.error = Some("应用退出导致本轮采集中断，请整圈重采".into());
                round.failure = Some(CaptureFailure::Interrupted);
                crate::fsio::write_atomic(&path, &serde_json::to_string_pretty(&round).map_err(|e| e.to_string())?)?;
            }
            if latest.as_ref().is_none_or(|last| round.created_at >= last.created_at) { latest = Some(round); }
        }
        let (writers, receivers): (Vec<_>, Vec<_>) = (0..MAX_CAMERAS).map(|_| mpsc::sync_channel::<QueuedFrame>(MAX_TRIGGERS)).unzip();
        let current = latest.map(|round| Running { round, cams: Vec::new(), drain_timeout: Duration::from_secs(1), deadline: Instant::now(), write_deadline: None });
        let host = Arc::new(Self { root, current: Mutex::new(current), uncertain_sessions: Mutex::new(std::collections::HashSet::new()), writers });
        for (i, rx) in receivers.into_iter().enumerate() {
            let weak = Arc::downgrade(&host);
            std::thread::spawn(move || loop {
                let item = rx.recv_timeout(Duration::from_millis(50));
                let Some(host) = weak.upgrade() else { break };
                match item {
                    Ok(item) => host.receive(item),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => if i == 0 { host.tick() },
                }
            });
        }
        Ok(host)
    }

    fn save(&self, round: &CaptureRound) -> Result<(), String> {
        crate::fsio::write_atomic(&self.root.join(&round.round_id).join("round.json"), &serde_json::to_string_pretty(round).map_err(|e| e.to_string())?)
    }

    pub fn active(&self) -> bool { self.current.lock().unwrap().as_ref().is_some_and(|r| r.round.state.active()) }

    pub fn current(&self) -> Option<CaptureRound> { self.current.lock().unwrap().as_ref().map(|r| r.round.clone()) }

    pub fn get(&self, round_id: &str) -> Result<CaptureRound, String> {
        let dir = self.safe_round_path(round_id)?;
        let round = match self.current().filter(|r| r.round_id == round_id) {
            Some(round) => round,
            None => {
                let path = std::fs::canonicalize(dir.join("round.json")).map_err(|e| e.to_string())?;
                if !path.starts_with(&dir) { return Err("采集记录路径越出本轮目录".into()); }
                serde_json::from_str(&crate::fsio::read_text(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?
            }
        };
        self.validate_record(round_id, &round)?;
        Ok(round)
    }

    pub fn start(&self, recipe_id: String, devices: Vec<CaptureDevice>, drain_timeout_ms: u64, simulated: bool) -> Result<CaptureRound, CaptureStartRejected> {
        use CaptureStartRejected as Rejected;
        if !(200..=30000).contains(&drain_timeout_ms) {
            return Err(Rejected::DrainTimeout);
        }
        if devices.is_empty() || devices.len() > MAX_CAMERAS { return Err(Rejected::CameraCount); }
        for (i, d) in devices.iter().enumerate() {
            let id = &d.camera_id;
            if !crate::recipe::valid_camera_id(id) || (devices.len() > 1 && id.len() > MAX_MULTI_CAMERA_ID) {
                return Err(Rejected::InvalidCameraId(id.clone()));
            }
            if devices[..i].iter().any(|o| o.camera_id == *id || o.cam == d.cam) { return Err(Rejected::Duplicate(id.clone())); }
            if d.session == 0 { return Err(Rejected::SessionNotReady(id.clone())); }
            if d.trigger_baseline.is_none() { return Err(Rejected::MissingBaseline(id.clone())); }
        }
        let mut guard = self.current.lock().unwrap();
        if guard.as_ref().is_some_and(|r| r.round.state.active()) { return Err(Rejected::Busy); }
        let uncertain: Vec<_> = devices.iter().filter(|d| self.uncertain_sessions.lock().unwrap().contains(&(d.camera_id.clone(), d.session))).map(|d| d.camera_id.clone()).collect();
        if !uncertain.is_empty() {
            return Err(Rejected::UncertainSessions(uncertain));
        }
        let created_at = ly_plc::now_ms();
        let mut suffix = 0u32;
        let round_id = loop {
            let id = format!("capture-{created_at}-{suffix}");
            match std::fs::create_dir(self.root.join(&id)) {
                Ok(()) => break id,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
                Err(e) => return Err(Rejected::Io(e.to_string())),
            }
        };
        let cams = devices.iter().map(|d| RunCam {
            cam: d.cam, source: d.source, last_trigger: d.trigger_baseline, faulted: false, queued: 0,
            last_frame: matches!(d.source, Some(CounterSource::SdkFrame | CounterSource::ChunkFrame)).then_some(d.trigger_baseline).flatten(),
        }).collect();
        let cameras = devices.into_iter().map(|d| CaptureCamera { camera_id: d.camera_id, camera_config: d.camera_config, imaging: d.imaging, device_session: d.session, received_count: 0 }).collect();
        let round = CaptureRound { round_id, recipe_id, cameras, planned_count: None, plc_planned_count: None, plc_actual_count: None, plc_plan_version: None, state: CaptureState::WaitingStart, created_at, ended_at: None, frames: Vec::new(), error: None, failure: None, simulated };
        self.save(&round).map_err(Rejected::Io)?;
        *guard = Some(Running { round: round.clone(), cams, drain_timeout: Duration::from_millis(drain_timeout_ms), deadline: Instant::now() + ROUND_TIMEOUT, write_deadline: None });
        Ok(round)
    }

    /// 失败后，可能还有本轮在途帧的相机要换新会话才能再采：PLC 已报完且与计划一致、这台相机收满计划数且本身没出错时，
    /// 它不会再有本轮的帧，可以直接用当前会话；其余相机（含收图未满、PLC 未报完或报数不符）都记为可疑。
    fn fail_running(&self, run: &mut Running, failure: CaptureFailure, error: String) {
        run.round.state = CaptureState::Failed;
        let round = &run.round;
        let plc_done = round.planned_count.is_some() && round.plc_actual_count == round.planned_count;
        let unsettled = round.cameras.iter().enumerate()
            .filter(|(j, c)| !plc_done || Some(c.received_count) != round.planned_count || run.cams.get(*j).is_some_and(|r| r.faulted))
            .map(|(_, c)| (c.camera_id.clone(), c.device_session));
        self.uncertain_sessions.lock().unwrap().extend(unsettled);
        run.round.error = Some(error);
        run.round.failure = Some(failure);
        if let Err(error) = self.save(&run.round) { run.round.error = Some(format!("{}；异常记录保存失败：{error}", run.round.error.as_deref().unwrap_or_default())); }
    }

    pub fn fail(&self, error: impl Into<String>) {
        if let Some(run) = self.current.lock().unwrap().as_mut().filter(|r| r.round.state.active()) { self.fail_running(run, CaptureFailure::Other, error.into()); }
    }

    pub fn disconnected(&self, cam: u8) {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active()) else { return };
        if let Some(j) = run.camera_of(cam) {
            run.cams[j].faulted = true;
            let camera_id = run.round.cameras[j].camera_id.clone();
            let error = format!("相机 {camera_id} 断连，请整圈重采");
            self.fail_running(run, CaptureFailure::Camera { camera_id }, error);
        }
    }

    pub fn plc_started(&self, planned_count: u32) -> Result<(), String> {
        let mut guard = self.current.lock().unwrap();
        let run = guard.as_mut().ok_or("没有等待开始的示教采集")?;
        if run.round.state != CaptureState::WaitingStart {
            self.fail_running(run, CaptureFailure::Plc, "PLC 开始信号重复或顺序异常".into());
            return Err(run.round.error.clone().unwrap());
        }
        run.round.plc_planned_count = Some(planned_count);
        if !(1..=64).contains(&planned_count) {
            self.fail_running(run, CaptureFailure::Plc, "PLC 计划触发数量须为 1–64".into());
            return Err(run.round.error.clone().unwrap());
        }
        run.round.planned_count = Some(planned_count);
        run.round.state = CaptureState::Receiving;
        run.deadline = Instant::now() + ROUND_TIMEOUT;
        if let Err(e) = self.save(&run.round) { self.fail_running(run, CaptureFailure::Other, e.clone()); return Err(e); }
        Ok(())
    }

    pub fn set_plan_version(&self, version: u32) -> Result<(), String> {
        let mut guard = self.current.lock().unwrap();
        let run = guard.as_mut().ok_or("没有等待开始的示教采集")?;
        if version == 0 || run.round.state != CaptureState::WaitingStart { return Err("计划版本须为正数，且只能在 PLC 开始前记录".into()); }
        run.round.plc_plan_version = Some(version);
        if let Err(e) = self.save(&run.round) { self.fail_running(run, CaptureFailure::Other, e.clone()); return Err(e); }
        Ok(())
    }

    pub fn plc_ended(&self, actual_count: u32) -> Result<(), String> {
        let mut guard = self.current.lock().unwrap();
        let run = guard.as_mut().ok_or("没有正在进行的示教采集")?;
        run.round.plc_actual_count = Some(actual_count);
        if run.round.state != CaptureState::Receiving || Some(actual_count) != run.round.planned_count {
            self.fail_running(run, CaptureFailure::Plc, "PLC 结束顺序异常或实际触发数量与计划不一致".into());
            return Err(run.round.error.clone().unwrap());
        }
        run.round.state = CaptureState::Draining;
        run.round.ended_at = Some(ly_plc::now_ms());
        run.deadline = Instant::now() + run.drain_timeout;
        run.write_deadline = Some(Instant::now() + ROUND_TIMEOUT);
        if let Err(e) = self.save(&run.round) { self.fail_running(run, CaptureFailure::Other, e.clone()); return Err(e); }
        Ok(())
    }

    pub fn offer(&self, frame: &Frame, original: Option<Arc<FrameImage>>) -> bool {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active()) else { return false; };
        let Some(j) = run.camera_of(frame.cam) else { return true; };
        if run.round.state == CaptureState::WaitingStart {
            run.cams[j].faulted = true;
            let camera_id = run.round.cameras[j].camera_id.clone();
            let error = format!("相机 {camera_id}：PLC 开始前收到帧，无法确定本轮归属");
            self.fail_running(run, CaptureFailure::Camera { camera_id }, error);
            return true;
        }
        if self.writers[j].try_send(QueuedFrame { round_id: run.round.round_id.clone(), frame: frame.clone(), original }).is_err() {
            run.cams[j].faulted = true;
            let camera_id = run.round.cameras[j].camera_id.clone();
            let error = format!("相机 {camera_id}：示教采集写盘队列已满或关闭，不能保证完整收图，请整圈重采");
            self.fail_running(run, CaptureFailure::Camera { camera_id }, error);
        } else {
            run.cams[j].queued += 1;
        }
        true
    }

    fn receive(&self, item: QueuedFrame) {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.round_id == item.round_id && r.round.state.active()) else { return; };
        let f = item.frame;
        let Some(j) = run.camera_of(f.cam) else { return; };
        run.round.cameras[j].received_count += 1;
        let camera = &run.round.cameras[j];
        let state = &run.cams[j];
        let ordinal = camera.received_count;
        let first = run.round.frames.iter().find_map(|s| s.cameras.iter().find(|c| c.camera_id == camera.camera_id));
        let error = if run.round.state == CaptureState::WaitingStart { Some("PLC 开始前收到帧，无法确定本轮归属") }
            else if f.session != camera.device_session { Some("设备会话已变化，不能混用不同会话的图像") }
            else if f.manual { Some("本轮收到手动触发图像") }
            else if f.lost_packets != 0 { Some("图像存在丢包") }
            else if run.round.planned_count.is_none() { Some("尚未取得 PLC 计划触发数量，不能接收本轮图像") }
            else if run.round.planned_count.is_some_and(|n| ordinal > n) { Some("实际收图数量超过 PLC 计划触发数量") }
            else if f.images.len() != view_count(&camera.camera_config) { Some("一帧必须完整交付设备配置的全部视角图像") }
            else if f.images.iter().any(|i| i.width == 0 || i.height == 0 || (i.width as usize).checked_mul(i.height as usize) != Some(i.pixels.len())) { Some("图像尺寸或像素长度无效") }
            else if first.is_some_and(|shot| shot.views.iter().zip(&f.images).any(|(v, i)| v.width != i.width || v.height != i.height)) { Some("本轮图像尺寸发生变化") }
            else if state.source.is_some_and(|source| source != f.counter) { Some("帧计数来源发生变化") }
            else if state.source.is_none() && !matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic) && state.last_trigger.is_some_and(|last| last.checked_add(1) != Some(f.frame_counter)) { Some("首帧计数与开流基线不连续") }
            else if state.last_frame.is_some_and(|last| last.checked_add(1) != Some(f.frame_counter)) { Some("SDK 帧号重复、跳号或顺序异常") }
            else if matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic) && state.last_trigger.is_some_and(|last| last.checked_add(1) != Some(f.trigger_counter)) { Some("触发计数重复、跳号或顺序异常") }
            else { None };
        if let Some(error) = error {
            let camera_id = camera.camera_id.clone();
            let error = format!("相机 {camera_id}：{error}");
            run.cams[j].faulted = true;
            self.fail_running(run, CaptureFailure::Camera { camera_id }, error);
            return;
        }
        let camera_id = camera.camera_id.clone();
        let state = &mut run.cams[j];
        state.source = Some(f.counter);
        state.last_frame = Some(f.frame_counter);
        let trigger = matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic).then_some(f.trigger_counter);
        state.last_trigger = trigger;
        let dir = camera_dir(&self.root, &run.round.round_id, ordinal, &camera_id);
        let round_id = run.round.round_id.clone();
        drop(guard);
        let saved = (|| -> Result<CaptureFrame, String> {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let original = match item.original { Some(image) => image, None => compose_views(&f.images)? };
            let original_path = dir.join("original.png");
            save_image(&original_path, &original)?;
            let views = f.images.iter().enumerate().map(|(i, image)| {
                let path = dir.join(format!("view-{}.png", i + 1));
                save_image(&path, image)?;
                Ok(CaptureView { view: i as u8 + 1, path: path.to_string_lossy().into_owned(), width: image.width, height: image.height })
            }).collect::<Result<Vec<_>, String>>()?;
            Ok(CaptureFrame { camera_id: camera_id.clone(), frame_counter: f.frame_counter, trigger_counter: trigger, original_path: original_path.to_string_lossy().into_owned(), views })
        })();
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.round_id == round_id && r.round.state.active()) else { return; };
        match saved {
            Ok(frame) => {
                let index = ordinal as usize - 1;
                if run.round.frames.len() == index {
                    run.round.frames.push(CaptureShot { shot_id: format!("S{ordinal:03}"), ordinal, cameras: Vec::new() });
                }
                let order = |id: &str| run.round.cameras.iter().position(|c| c.camera_id == id);
                let Some(shot) = run.round.frames.get_mut(index) else {
                    run.cams[j].faulted = true;
                    self.fail_running(run, CaptureFailure::Camera { camera_id: camera_id.clone() }, format!("相机 {camera_id}：前一次触发的图像缺失，无法确定拍照点归属"));
                    return;
                };
                let at = shot.cameras.iter().position(|c| order(&c.camera_id) > Some(j)).unwrap_or(shot.cameras.len());
                shot.cameras.insert(at, frame);
                if let Err(e) = self.save(&run.round) { self.fail_running(run, CaptureFailure::Other, e); }
            }
            Err(e) => { run.cams[j].faulted = true; self.fail_running(run, CaptureFailure::Camera { camera_id: camera_id.clone() }, format!("相机 {camera_id}：保存采集图像失败：{e}")) }
        }
    }

    fn tick(&self) {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active() && Instant::now() >= r.deadline) else { return; };
        let round = &run.round;
        let plc_ended = round.state == CaptureState::Draining && round.plc_actual_count == round.planned_count;
        let arrived = round.planned_count.is_some_and(|n| run.cams.iter().all(|c| c.queued == n));
        let written = round.planned_count.is_some_and(|n| round.frames.len() == n as usize && round.complete_shots() == n
            && round.cameras.iter().all(|c| c.received_count == n));
        if plc_ended && arrived && written {
            run.round.state = CaptureState::Complete;
            if let Err(e) = self.save(&run.round) { self.fail_running(run, CaptureFailure::Other, e); }
            return;
        }
        if plc_ended && arrived && run.write_deadline.is_some_and(|d| Instant::now() < d) { return; }
        let (failure, error) = match round.planned_count {
            None => (CaptureFailure::StartTimeout, "采集超时：仍在等待 PLC 开始请求，请整圈重采".to_string()),
            Some(n) => {
                let short: Vec<_> = round.cameras.iter().zip(&run.cams).filter(|(_, r)| r.queued != n)
                    .map(|(c, r)| CameraArrival { camera_id: c.camera_id.clone(), received: r.queued }).collect();
                if !short.is_empty() {
                    let named: Vec<_> = short.iter().map(|c| format!("相机 {} 收到 {}/{n} 张", c.camera_id, c.received)).collect();
                    let error = format!("采集超时：计划触发 {n} 次，{}；必须收到 PLC 结束且各相机数量一致，请整圈重采", named.join("、"));
                    (CaptureFailure::ShortCameras { planned: n, cameras: short }, error)
                } else if plc_ended {
                    let written = round.complete_shots();
                    (CaptureFailure::WriteTimeout { planned: n, written }, format!("采集超时：PLC 已结束且各相机均已收到 {n} 张，但 {} s 内只写完 {written}/{n} 个拍照点；请检查磁盘后整圈重采", ROUND_TIMEOUT.as_secs()))
                } else { (CaptureFailure::PlcEndMissing { planned: n }, format!("采集超时：各相机均已收到 {n} 张，但未收到 PLC 结束；请整圈重采")) }
            }
        };
        self.fail_running(run, failure, error);
    }

    pub fn list(&self, recipe_id: &str) -> Result<Vec<CaptureSummary>, String> {
        let mut rounds = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().join("round.json").is_file() { continue; }
            let r = self.get(&id)?;
            if r.recipe_id == recipe_id {
                rounds.push(CaptureSummary { camera_ids: r.camera_ids(), received_count: r.complete_shots(), round_id: r.round_id, recipe_id: r.recipe_id, planned_count: r.planned_count, state: r.state, created_at: r.created_at, error: r.error });
            }
        }
        rounds.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.round_id.cmp(&a.round_id)));
        Ok(rounds)
    }
}

/// 采集设备每帧交付的视角数（1 或 3），取自本轮记下的设备配置。
fn view_count(camera_config: &serde_json::Value) -> usize {
    camera_config.get("viewCount").and_then(serde_json::Value::as_u64).filter(|n| matches!(n, 1 | 3)).unwrap_or(3) as usize
}

fn save_image(path: &Path, image: &FrameImage) -> Result<(), String> {
    image::save_buffer_with_format(path, &image.pixels, image.width, image.height, image::ColorType::L8, image::ImageFormat::Png).map_err(|e| e.to_string())
}

fn compose_views(images: &[Arc<FrameImage>]) -> Result<Arc<FrameImage>, String> {
    let height = images.first().ok_or("没有图像")?.height;
    if images.iter().any(|i| i.height != height) { return Err("模拟／样本集图像高度不一致，无法保留拼接原图".into()); }
    let width = images.iter().try_fold(0u32, |w, i| w.checked_add(i.width)).ok_or("图像宽度溢出")?;
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height as usize { for image in images { let start = y * image.width as usize; pixels.extend_from_slice(&image.pixels[start..start + image.width as usize]); } }
    Ok(Arc::new(FrameImage::new(width, height, pixels)))
}

#[tauri::command]
pub async fn recipe_capture_start(app: AppHandle, recipe_id: String, camera_ids: Vec<String>, drain_timeout_ms: Option<u64>) -> Result<CaptureRound, String> {
    tauri::async_runtime::spawn_blocking(move || start_capture(app, recipe_id, camera_ids, drain_timeout_ms)).await.map_err(|e| e.to_string())?
}

fn start_capture(app: AppHandle, recipe_id: String, camera_ids: Vec<String>, drain_timeout_ms: Option<u64>) -> Result<CaptureRound, String> {
    let cycle = app.state::<CycleHost>();
    if recipe_id.trim().is_empty() { return Err("请先创建配方草稿".into()); }
    if camera_ids.is_empty() { return Err("请至少选择一台采集相机".into()); }
    let Some(_gate) = crate::workspace::wait_idle_plc_gate(&cycle, "开始整圈采集")? else { return Err("工件正在检测，结束后再开始整圈采集".into()) };
    if !matches!(cycle.phase(), Phase::Idle | Phase::Fault) { return Err("节拍尚未回到空闲，稍后再试".into()); }
    let mut slots = Vec::with_capacity(camera_ids.len());
    for camera_id in &camera_ids {
        let cam = cycle.camera.require(camera_id)?;
        let slot = cycle.camera.slot(cam as usize).ok_or_else(|| format!("相机 {camera_id} 不存在"))?;
        let config = slot.config();
        if !slot.is_ready() { return Err(format!("相机 {}（{camera_id}）未就绪", config.name)); }
        if config.source == CameraSource::Mvs && config.trigger_source != "Line0" { return Err(format!("相机 {}（{camera_id}）必须使用 PLC Line0 硬触发", config.name)); }
        slots.push((cam, slot, config));
    }
    let simulated = slots.iter().any(|(_, _, config)| config.source != CameraSource::Mvs);
    app.state::<crate::workspace::WorkspaceHost>().capture_for_candidate(&recipe_id, |workspace| {
        let mut devices = Vec::with_capacity(slots.len());
        for (cam, slot, config) in &slots {
            let imaging = workspace.imaging(&config.id);
            slot.set_imaging(&imaging)?;
            let (arm, ledger) = slot.routing_state().map_err(|e| format!("相机 {}：{e}", config.id))?;
            devices.push(CaptureDevice {
                cam: *cam, camera_id: config.id.clone(), camera_config: serde_json::to_value(config).map_err(|e| e.to_string())?, imaging,
                session: arm.session, source: ledger.as_ref().and_then(|l| l.source).or(arm.source),
                trigger_baseline: ledger.as_ref().and_then(|l| l.last).or(arm.counter_after_open),
            });
        }
        cycle.camera.recipe_capture().start(recipe_id.clone(), devices, drain_timeout_ms.unwrap_or(1000), simulated).map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub fn recipe_capture_get(app: AppHandle, round_id: Option<String>) -> Result<Option<CaptureRound>, String> {
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    match round_id { Some(id) => capture.get(&id).map(Some), None => capture.current().map(|r| capture.get(&r.round_id)).transpose() }
}

#[tauri::command]
pub fn recipe_capture_stop(app: AppHandle) -> Option<CaptureRound> {
    let cycle = app.state::<CycleHost>();
    cycle.camera.recipe_capture().fail("操作员中止本轮采集，请整圈重采");
    cycle.camera.recipe_capture().current()
}

#[tauri::command]
pub fn recipe_capture_list(app: AppHandle, recipe_id: String) -> Result<Vec<CaptureSummary>, String> {
    app.state::<CycleHost>().camera.recipe_capture().list(&recipe_id)
}

#[tauri::command]
pub fn recipe_capture_image(app: AppHandle, round_id: String, k: usize, camera_id: String, view: Option<u8>) -> Result<tauri::ipc::Response, String> {
    let cycle = app.state::<CycleHost>();
    let round = cycle.camera.recipe_capture().get(&round_id)?;
    let shot = round.frames.get(k).ok_or("拍照点不存在")?;
    let frame = shot.cameras.iter().find(|f| f.camera_id == camera_id).ok_or_else(|| format!("拍照点 {} 没有相机 {camera_id} 的图像", shot.shot_id))?;
    let path = match view {
        None => &frame.original_path,
        Some(view) => &frame.views.iter().find(|v| v.view == view).ok_or("图像不存在")?.path,
    };
    let image = crate::image_file::load(Path::new(path))?;
    let scale = (image.width as f32 / 900.0).max(image.height as f32 / 650.0).max(1.0);
    let w = (image.width as f32 / scale).max(1.0) as u32;
    let h = (image.height as f32 / scale).max(1.0) as u32;
    let mut bytes = Vec::with_capacity(16 + (w * h) as usize);
    for n in [w, h, image.width, image.height] { bytes.extend(n.to_le_bytes()); }
    for y in 0..h { for x in 0..w {
        let iy = ((y as f32 * scale) as u32).min(image.height - 1);
        let ix = ((x as f32 * scale) as u32).min(image.width - 1);
        bytes.push(image.pixels[(iy * image.width + ix) as usize]);
    } }
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);
    /// 后台写盘线程每 50 ms 会 tick；测试手动推进各步，只由 drain() 结束在途等待，避免机器高负载时被后台提前判定。
    const TEST_DRAIN_MS: u64 = 30000;

    fn host() -> Arc<RecipeCaptureHost> {
        let path = std::env::temp_dir().join(format!("gluesight-capture-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_TEST.fetch_add(1, Ordering::Relaxed)));
        RecipeCaptureHost::open(path).unwrap()
    }

    fn device(cam: u8, id: &str, config: serde_json::Value, session: u64, source: Option<CounterSource>, baseline: Option<u64>) -> CaptureDevice {
        CaptureDevice { cam, camera_id: id.into(), camera_config: config, imaging: crate::recipe::DEFAULT_IMAGING, session, source, trigger_baseline: baseline }
    }

    fn single(host: &RecipeCaptureHost, config: serde_json::Value, session: u64, source: Option<CounterSource>, baseline: Option<u64>, simulated: bool) -> Result<CaptureRound, CaptureStartRejected> {
        host.start("r".into(), vec![device(0, "cam1", config, session, source, baseline)], TEST_DRAIN_MS, simulated)
    }

    fn start(host: &RecipeCaptureHost) -> CaptureRound {
        let session = host.current().map_or(9, |r| r.cameras[0].device_session + 1);
        host.start("recipe-a".into(), vec![device(0, "cam1", serde_json::json!({"source":"sim"}), session, Some(CounterSource::Synthetic), Some(0))], TEST_DRAIN_MS, true).unwrap()
    }

    /// 三台单视角相机，各自会话 10/11/12、触发计数基线 0/100/200。
    fn start_three(host: &RecipeCaptureHost) -> CaptureRound {
        let devices = (0..3u8).map(|i| device(i, &format!("cam{}", i + 1), serde_json::json!({"viewCount": 1}), 10 + i as u64, Some(CounterSource::Synthetic), Some(i as u64 * 100))).collect();
        host.start("recipe-a".into(), devices, TEST_DRAIN_MS, true).unwrap()
    }

    fn frame(n: u64) -> Frame {
        Frame { cam: 0, session: 9, counter: CounterSource::Synthetic, frame_counter: n, trigger_counter: n, lost_packets: 0, ts: 0, manual: false,
            images: (1..=3).map(|v| Arc::new(FrameImage::new(2, 2, vec![v; 4]))).collect() }
    }

    /// 三相机轮次里第 cam 台相机的第 n 次触发。
    fn cam_frame(cam: u8, n: u64) -> Frame {
        let counter = cam as u64 * 100 + n;
        Frame { cam, session: 10 + cam as u64, counter: CounterSource::Synthetic, frame_counter: counter, trigger_counter: counter, lost_packets: 0, ts: 0, manual: false,
            images: vec![Arc::new(FrameImage::new(2, 2, vec![cam; 4]))] }
    }

    /// 帧已到达并进入写盘队列（offer 的计数），尚未写盘。
    fn arrive(host: &RecipeCaptureHost, round: &CaptureRound, f: &Frame) {
        let mut guard = host.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.round_id == round.round_id && r.round.state.active()) else { return };
        if let Some(j) = run.camera_of(f.cam) { run.cams[j].queued += 1; }
    }

    fn write(host: &RecipeCaptureHost, round: &CaptureRound, f: Frame) {
        host.receive(QueuedFrame { round_id: round.round_id.clone(), frame: f, original: None });
    }

    fn receive(host: &RecipeCaptureHost, round: &CaptureRound, f: Frame) {
        arrive(host, round, &f);
        write(host, round, f);
    }

    fn drain(host: &RecipeCaptureHost) {
        host.current.lock().unwrap().as_mut().unwrap().deadline = Instant::now();
        host.tick();
    }

    fn complete_three(host: &RecipeCaptureHost) -> CaptureRound {
        let round = start_three(host);
        host.plc_started(2).unwrap();
        for n in 1..=2 { for cam in 0..3 { receive(host, &round, cam_frame(cam, n)); } }
        host.plc_ended(2).unwrap();
        drain(host);
        host.get(&round.round_id).unwrap()
    }

    #[test]
    fn plc_start_latches_only_a_valid_count_once_and_preserves_it_on_duplicate_start() {
        for count in [1, 20, 64] {
            let host = host();
            let round = start(&host);
            host.plc_started(count).unwrap();
            let receiving = host.get(&round.round_id).unwrap();
            assert_eq!(receiving.state, CaptureState::Receiving);
            assert_eq!(receiving.planned_count, Some(count));
            assert_eq!(receiving.plc_planned_count, Some(count));
            assert_eq!(host.list("recipe-a").unwrap()[0].planned_count, Some(count));
            assert!(host.plc_started(if count == 1 { 2 } else { 1 }).is_err());
            let failed = host.get(&round.round_id).unwrap();
            assert_eq!(failed.state, CaptureState::Failed);
            assert_eq!(failed.planned_count, Some(count));
            assert_eq!(failed.plc_planned_count, Some(count));
        }
        for invalid in [0, 65, u32::MAX] {
            let host = host();
            let round = start(&host);
            assert!(host.plc_started(invalid).is_err());
            let failed = host.get(&round.round_id).unwrap();
            assert_eq!(failed.state, CaptureState::Failed);
            assert_eq!(failed.planned_count, None);
            assert_eq!(failed.plc_planned_count, Some(invalid));
        }
    }

    #[test]
    fn plc_actual_count_must_match_the_latched_start_request() {
        for actual in [0, 19, 21] {
            let host = host();
            let round = start(&host);
            host.plc_started(20).unwrap();
            assert!(host.plc_ended(actual).is_err());
            let failed = host.get(&round.round_id).unwrap();
            assert_eq!(failed.state, CaptureState::Failed);
            assert_eq!(failed.planned_count, Some(20));
            assert_eq!(failed.plc_actual_count, Some(actual));
        }
        let host = host();
        let round = start(&host);
        assert!(host.plc_ended(0).is_err());
        assert_eq!(host.get(&round.round_id).unwrap().planned_count, None);
        let host = self::host();
        let round = start(&host);
        host.plc_started(2).unwrap();
        receive(&host, &round, frame(1));
        host.plc_ended(2).unwrap();
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
    }

    #[test]
    fn first_real_frame_requires_a_baseline_and_cannot_be_old_or_missing() {
        for source in [None, Some(CounterSource::SdkFrame), Some(CounterSource::ChunkFrame), Some(CounterSource::ChunkTrigger)] {
            let host = host();
            assert!(single(&host, serde_json::json!({}), 9, source, None, false).is_err());
            for first in [10, 12] {
                let session = first;
                let round = single(&host, serde_json::json!({}), session, source, Some(10), false).unwrap();
                host.plc_started(1).unwrap();
                let mut f = frame(first);
                f.session = session;
                f.counter = source.unwrap_or(CounterSource::SdkFrame);
                receive(&host, &round, f);
                assert_eq!(host.current().unwrap().state, CaptureState::Failed);
                assert!(host.current().unwrap().frames.is_empty());
            }
            let round = single(&host, serde_json::json!({}), 9, source, Some(10), false).unwrap();
            host.plc_started(1).unwrap();
            let mut f = frame(11); f.counter = source.unwrap_or(CounterSource::SdkFrame);
            receive(&host, &round, f);
            assert_eq!(host.current().unwrap().frames.len(), 1);
        }
    }

    #[test]
    fn failed_round_requires_a_new_session_and_rejects_late_old_session_frames_for_all_sources() {
        for counter in [CounterSource::Synthetic, CounterSource::SdkFrame, CounterSource::ChunkTrigger] {
        let host = host();
        let simulated = counter == CounterSource::Synthetic;
        let round = single(&host, serde_json::json!({}), 9, Some(counter), Some(0), simulated).unwrap();
        host.plc_started(20).unwrap();
        for n in 1..=19 { let mut f = frame(n); f.counter = counter; receive(&host, &round, f); }
        host.plc_ended(20).unwrap();
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        assert!(matches!(single(&host, serde_json::json!({}), 9, Some(counter), Some(19), simulated), Err(CaptureStartRejected::UncertainSessions(_))));
        let next = single(&host, serde_json::json!({}), 10, Some(counter), Some(0), simulated).unwrap();
        host.plc_started(20).unwrap();
        receive(&host, &round, frame(20));
        assert!(host.current().unwrap().frames.is_empty());
        let mut late = frame(20); late.counter = counter;
        receive(&host, &next, late);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        assert!(host.current().unwrap().frames.is_empty());
        }
    }

    #[test]
    fn twenty_frames_are_twenty_shots_and_require_plc_end() {
        let host = host();
        let round = start(&host);
        host.plc_started(20).unwrap();
        for n in 1..=20 { receive(&host, &round, frame(n)); }
        let result = host.current().unwrap();
        assert_eq!(result.state, CaptureState::Receiving);
        assert_eq!(result.planned_count, Some(20));
        assert_eq!(result.frames.len(), 20);
        assert!(result.frames.iter().all(|s| s.cameras.len() == 1 && s.cameras[0].views.len() == 3 && Path::new(&s.cameras[0].original_path).is_file()));
        host.plc_ended(20).unwrap();
        assert_eq!(host.current().unwrap().state, CaptureState::Draining);
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Complete);
    }

    #[test]
    fn three_cameras_one_trigger_is_one_shot_with_an_image_per_camera_and_completes_only_together() {
        let host = host();
        let round = start_three(&host);
        host.plc_started(2).unwrap();
        for (cam, n) in [(1, 1), (0, 1), (2, 1), (0, 2), (2, 2)] { receive(&host, &round, cam_frame(cam, n)); }
        let receiving = host.current().unwrap();
        assert_eq!(receiving.state, CaptureState::Receiving);
        assert_eq!(receiving.complete_shots(), 1);
        assert_eq!(receiving.frames[0].cameras.iter().map(|f| f.camera_id.as_str()).collect::<Vec<_>>(), ["cam1", "cam2", "cam3"]);
        host.plc_ended(2).unwrap();
        receive(&host, &round, cam_frame(1, 2));
        drain(&host);
        let done = host.get(&round.round_id).unwrap();
        assert_eq!(done.state, CaptureState::Complete);
        assert_eq!(done.frames[1].cameras.iter().map(|f| (f.camera_id.as_str(), f.trigger_counter)).collect::<Vec<_>>(),
            [("cam1", Some(2)), ("cam2", Some(102)), ("cam3", Some(202))]);
    }

    #[test]
    fn drain_timeout_names_only_the_camera_that_is_short() {
        let host = host();
        let round = start_three(&host);
        host.plc_started(2).unwrap();
        for (cam, n) in [(0, 1), (1, 1), (2, 1), (0, 2), (2, 2)] { receive(&host, &round, cam_frame(cam, n)); }
        host.plc_ended(2).unwrap();
        drain(&host);
        let failed = host.current().unwrap();
        assert_eq!(failed.state, CaptureState::Failed);
        assert_eq!(failed.failure, Some(CaptureFailure::ShortCameras { planned: 2, cameras: vec![CameraArrival { camera_id: "cam2".into(), received: 1 }] }));
    }

    /// 现场 debug 构建写 PNG 慢：PLC 已结束、各相机图像都已到达，只差写盘时，在途等待到期不能误报未收到 PLC 结束。
    #[test]
    fn plc_ended_and_all_frames_arrived_but_not_written_waits_for_writes_then_completes() {
        let host = host();
        let round = start_three(&host);
        host.plc_started(2).unwrap();
        for n in 1..=2 { for cam in 0..3 { arrive(&host, &round, &cam_frame(cam, n)); } }
        for (cam, n) in [(0, 1), (1, 1), (2, 1), (0, 2)] { write(&host, &round, cam_frame(cam, n)); }
        host.plc_ended(2).unwrap();
        drain(&host);
        let waiting = host.current().unwrap();
        assert_eq!(waiting.state, CaptureState::Draining, "{:?}", waiting.error);
        for cam in 1..3 { write(&host, &round, cam_frame(cam, 2)); }
        host.tick();
        assert_eq!(host.get(&round.round_id).unwrap().state, CaptureState::Complete);
    }

    #[test]
    fn writes_still_pending_at_the_write_limit_fail_as_a_write_timeout_not_a_missing_plc_end() {
        let host = host();
        let round = start_three(&host);
        host.plc_started(2).unwrap();
        for n in 1..=2 { for cam in 0..3 { arrive(&host, &round, &cam_frame(cam, n)); } }
        for cam in 0..3 { write(&host, &round, cam_frame(cam, 1)); }
        host.plc_ended(2).unwrap();
        host.current.lock().unwrap().as_mut().unwrap().write_deadline = Some(Instant::now());
        drain(&host);
        let failed = host.current().unwrap();
        assert_eq!(failed.state, CaptureState::Failed);
        assert_eq!(failed.failure, Some(CaptureFailure::WriteTimeout { planned: 2, written: 1 }));
    }

    #[test]
    fn trigger_counts_are_checked_per_camera_not_across_cameras() {
        for (gap_on, accepted) in [(None, true), (Some(1u8), false)] {
            let host = host();
            let round = start_three(&host);
            host.plc_started(3).unwrap();
            for n in 1..=2 { for cam in [2, 0, 1] { receive(&host, &round, cam_frame(cam, n)); } }
            if let Some(cam) = gap_on { receive(&host, &round, cam_frame(cam, 4)); }
            let state = host.current().unwrap();
            assert_eq!(state.state == CaptureState::Receiving, accepted, "{:?}", state.error);
            if !accepted { assert_eq!(state.failure, Some(CaptureFailure::Camera { camera_id: "cam2".into() })); }
        }
    }

    #[test]
    fn recipe_shots_expand_trigger_major_with_a_shared_pose_per_trigger() {
        let host = host();
        let three = complete_three(&host);
        let shots = three.recipe_shots().unwrap();
        assert_eq!(shots.iter().map(|s| (s.shot_id.as_str(), s.pose_id.as_str(), s.camera.camera_id.as_str(), s.frame.trigger_counter)).collect::<Vec<_>>(), [
            ("S001-cam1", "S001", "cam1", Some(1)), ("S001-cam2", "S001", "cam2", Some(101)), ("S001-cam3", "S001", "cam3", Some(201)),
            ("S002-cam1", "S002", "cam1", Some(2)), ("S002-cam2", "S002", "cam2", Some(102)), ("S002-cam3", "S002", "cam3", Some(202)),
        ]);
        let host = self::host();
        let round = start(&host);
        host.plc_started(2).unwrap();
        for n in 1..=2 { receive(&host, &round, frame(n)); }
        assert_eq!(host.current().unwrap().recipe_shots().unwrap().iter().map(|s| s.shot_id.clone()).collect::<Vec<_>>(), ["S001", "S002"]);
    }

    /// 收满计划且 PLC 报数一致的相机不会再有本轮帧，不必重连；其余相机一次全部点名。
    #[test]
    fn failure_requires_reconnecting_only_cameras_that_may_still_have_frames_and_names_them_all() {
        let restart = |host: &RecipeCaptureHost| host.start("recipe-a".into(), (0..3u8).map(|i| device(i, &format!("cam{}", i + 1), serde_json::json!({"viewCount": 1}), 10 + i as u64, Some(CounterSource::Synthetic), Some(i as u64 * 100))).collect(), TEST_DRAIN_MS, true);
        let host = host();
        let round = start_three(&host);
        host.plc_started(2).unwrap();
        for (cam, n) in [(0, 1), (1, 1), (2, 1), (0, 2), (2, 2)] { receive(&host, &round, cam_frame(cam, n)); }
        host.plc_ended(2).unwrap();
        drain(&host);
        assert_eq!(restart(&host).unwrap_err(), CaptureStartRejected::UncertainSessions(vec!["cam2".into()]));
        let host = self::host();
        let round = start_three(&host);
        host.plc_started(2).unwrap();
        receive(&host, &round, cam_frame(0, 1));
        host.fail("操作员中止");
        assert_eq!(restart(&host).unwrap_err(), CaptureStartRejected::UncertainSessions(vec!["cam1".into(), "cam2".into(), "cam3".into()]));
    }

    #[test]
    fn start_rejects_a_camera_selected_twice_and_names_the_camera_without_baseline() {
        let host = host();
        let config = serde_json::json!({"viewCount": 1});
        let twice = vec![device(0, "cam1", config.clone(), 9, None, Some(0)), device(0, "cam2", config.clone(), 9, None, Some(0))];
        assert_eq!(host.start("r".into(), twice, 200, false).unwrap_err(), CaptureStartRejected::Duplicate("cam2".into()));
        let twice = vec![device(0, "cam1", config.clone(), 9, None, Some(0)), device(1, "cam1", config.clone(), 10, None, Some(0))];
        assert_eq!(host.start("r".into(), twice, 200, false).unwrap_err(), CaptureStartRejected::Duplicate("cam1".into()));
        let missing = vec![device(0, "cam1", config.clone(), 9, None, Some(0)), device(1, "cam2", config, 10, None, None)];
        assert_eq!(host.start("r".into(), missing, 200, false).unwrap_err(), CaptureStartRejected::MissingBaseline("cam2".into()));
        assert!(host.current().is_none());
    }

    #[test]
    fn each_frame_must_carry_exactly_the_device_view_count() {
        for (device_views, delivered, accepted) in [(1, 1, true), (1, 3, false), (3, 3, true), (3, 1, false)] {
            let host = host();
            let round = host.start("recipe-a".into(), vec![device(0, "cam1", serde_json::json!({"viewCount": device_views}), 9, Some(CounterSource::Synthetic), Some(0))], TEST_DRAIN_MS, true).unwrap();
            host.plc_started(1).unwrap();
            let mut f = frame(1);
            f.images.truncate(delivered);
            receive(&host, &round, f);
            let _ = host.plc_ended(1);
            drain(&host);
            let state = host.get(&round.round_id).unwrap().state;
            assert_eq!(state == CaptureState::Complete, accepted, "{device_views} 视角设备收到 {delivered} 幅：{state:?}");
        }
    }

    #[test]
    fn trailing_frames_are_accepted_but_extra_during_drain_fails() {
        let host = host();
        let round = start(&host);
        host.plc_started(2).unwrap();
        receive(&host, &round, frame(1));
        host.plc_ended(2).unwrap();
        receive(&host, &round, frame(2));
        assert_eq!(host.current().unwrap().state, CaptureState::Draining);
        receive(&host, &round, frame(3));
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
    }

    #[test]
    fn extra_frame_still_in_write_queue_at_drain_end_fails_instead_of_completing() {
        let host = host();
        let round = start(&host);
        host.plc_started(1).unwrap();
        receive(&host, &round, frame(1));
        host.plc_ended(1).unwrap();
        arrive(&host, &round, &frame(2));
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
    }

    #[test]
    fn duplicate_gap_session_manual_and_packet_loss_fail() {
        for bad in [0, 1, 2, 3, 4] {
            let host = host();
            let round = start(&host);
            host.plc_started(3).unwrap();
            receive(&host, &round, frame(1));
            let mut next = frame(2);
            match bad { 0 => next.frame_counter = 1, 1 => next.trigger_counter = 3, 2 => next.session = 10, 3 => next.manual = true, _ => next.lost_packets = 1 }
            receive(&host, &round, next);
            assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        }
    }

    /// 设备会话编号每次启动从 1 分配：退出时中断的轮次不能封锁新进程里同编号的会话。
    #[test]
    fn restart_marks_interrupted_round_failed_restores_latest_and_frees_its_session_number() {
        let host = host();
        let interrupted = start(&host);
        let session = interrupted.cameras[0].device_session;
        let reopened = RecipeCaptureHost::open(host.root.clone()).unwrap();
        assert_eq!(reopened.current().unwrap().round_id, interrupted.round_id);
        assert_eq!(reopened.current().unwrap().state, CaptureState::Failed);
        assert!(!reopened.active());
        let next = reopened.start("recipe-a".into(), vec![device(0, "cam1", serde_json::json!({"source":"sim"}), session, Some(CounterSource::Synthetic), Some(0))], TEST_DRAIN_MS, true);
        assert_eq!(next.map(|round| round.state), Ok(CaptureState::WaitingStart));
    }

    #[test]
    fn old_single_camera_round_record_is_backed_up_instead_of_blocking_startup() {
        let host = host();
        let round = start(&host);
        let dir = host.root.join(&round.round_id);
        let mut old: serde_json::Value = serde_json::from_str(&crate::fsio::read_text(&dir.join("round.json")).unwrap()).unwrap();
        let object = old.as_object_mut().unwrap();
        object.remove("cameras");
        object.insert("cameraId".into(), "cam1".into());
        crate::fsio::write_atomic(&dir.join("round.json"), &old.to_string()).unwrap();
        let reopened = RecipeCaptureHost::open(host.root.clone()).unwrap();
        assert!(reopened.current().is_none());
        assert!(!dir.join("round.json").exists());
        assert!(std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("round.broken-")));
    }

    #[test]
    fn explicit_layout_splits_without_guessing_or_overlapping() {
        let image = FrameImage::new(6, 3, (0..18).collect());
        let horizontal = CompositeLayout::Horizontal.split(&image).unwrap();
        assert_eq!(horizontal[1].pixels, [2, 3, 8, 9, 14, 15]);
        let vertical = CompositeLayout::Vertical.split(&image).unwrap();
        assert_eq!(vertical[2].pixels, [12, 13, 14, 15, 16, 17]);
        assert!(CompositeLayout::Horizontal.split(&FrameImage::new(5, 1, vec![0; 5])).is_err());
        let rect = ViewRect { x: 0, y: 0, width: 2, height: 2 };
        assert!(CompositeLayout::Rects { rects: [rect.clone(), rect.clone(), rect] }.split(&image).is_err());
        assert!(CompositeLayout::Vertical.split(&FrameImage::new(6, 3, vec![0; 1])).is_err());
    }

    #[test]
    fn persisted_record_cannot_redirect_images_or_fake_complete_evidence() {
        let host = host();
        let round = start(&host);
        host.plc_started(1).unwrap();
        receive(&host, &round, frame(1));
        host.plc_ended(1).unwrap();
        drain(&host);
        let valid = host.get(&round.round_id).unwrap();
        let mut forged = valid.clone();
        forged.frames[0].cameras[0].views[0].path = host.root.join("external.png").to_string_lossy().into_owned();
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        forged = valid.clone(); forged.plc_actual_count = None;
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        for plan in [None, Some(0), Some(2)] {
            forged = valid.clone(); forged.planned_count = plan;
            assert!(host.validate_record(&round.round_id, &forged).is_err());
        }
        forged = valid.clone(); forged.frames[0].cameras[0].views[0].view = 2;
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        forged = valid; forged.round_id = "capture-other".into();
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        assert!(host.get("capture-../../outside").is_err());
        let host = self::host();
        let three = complete_three(&host);
        let mut forged = three.clone();
        forged.frames[1].cameras.remove(1);
        assert!(host.validate_record(&three.round_id, &forged).is_err());
        forged = three.clone(); forged.frames[0].cameras.swap(0, 1);
        assert!(host.validate_record(&three.round_id, &forged).is_err());
    }
}

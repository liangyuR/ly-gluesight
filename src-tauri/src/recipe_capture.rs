use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::camera::CameraSource;
use crate::cycle::{CycleHost, Phase};
use crate::frame::{CounterSource, Frame, FrameImage};
use crate::recipe::CameraImaging;

mod burst;

pub use burst::{CaptureFailureV2, CaptureRoundV2};

/// 三目拼接图的拆分布局。旧拍照点模型：段配方只用单视角相机，不经过这里（D-SB-11）。
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

/// 开始整圈采集被拒的原因；Display 是给操作员看的文案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureStartRejected {
    DrainTimeout,
    CameraCount,
    InvalidCameraId(String),
    Duplicate(String),
    SessionNotReady(String),
    Busy,
    /// 采集相机不是单视角
    MultiView(String),
    Io(String),
}

impl std::fmt::Display for CaptureStartRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DrainTimeout => write!(f, "等待在途帧时间须为 200–30000 ms"),
            Self::CameraCount => write!(f, "整圈采集须选择 1–{MAX_CAMERAS} 台相机"),
            Self::InvalidCameraId(id) => write!(f, "相机编号 {id:?} 无效"),
            Self::Duplicate(id) => write!(f, "相机 {id} 被重复选择"),
            Self::SessionNotReady(id) => write!(f, "相机 {id}：设备会话尚未就绪"),
            Self::Busy => write!(f, "已有示教采集正在占用设备"),
            Self::MultiView(id) => write!(f, "相机 {id}：段配方用三台独立的单视角相机共用一个触发，不能用三目拼接或多视角设备采集"),
            Self::Io(e) => write!(f, "{e}"),
        }
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
    /// 这台相机本会话见到的最高 SDK 帧计数（没见过帧时取开流计数起点），seq = frame_counter − 它（D-SB-10）；
    /// 两者都没有时为空，本轮按这台相机收到的第一帧定基线
    pub frame_baseline: Option<u64>,
}

pub struct RecipeCaptureHost {
    root: PathBuf,
    /// 最近一轮（burst 模块）
    burst: Mutex<Option<burst::BurstRun>>,
    /// frames.jsonl 追加句柄；写盘线程只单独持有它，不与 burst 锁嵌套
    journal: Mutex<Option<burst::Journal>>,
    /// 写盘线程，按本轮相机顺序；排队条数由 MAX_FRAMES_PER_CAMERA 限制
    burst_writers: Vec<mpsc::Sender<burst::BurstItem>>,
}

/// 一轮示教采集（段配方的采集轮次，序列化为 CaptureRoundV2 的形状）。
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum AnyRound {
    Burst(CaptureRoundV2),
}

impl AnyRound {
    pub fn round_id(&self) -> &str {
        let Self::Burst(r) = self;
        &r.round_id
    }

    pub fn camera_ids(&self) -> Vec<String> {
        let Self::Burst(r) = self;
        r.camera_ids()
    }

    pub fn state(&self) -> &CaptureState {
        let Self::Burst(r) = self;
        &r.state
    }

    /// 交给 PLC 的采集完成码：完成 1/0；失败 90 配异常码，按原因报 98（相机断连、会话变化）或 99（其余），
    /// 计数不符不是失败（D-SB-33）。未结束为 None。
    pub fn plc_report(&self) -> Option<(u16, u16)> {
        let Self::Burst(r) = self;
        match r.state {
            CaptureState::Complete => Some((1, 0)),
            CaptureState::Failed => Some((
                90,
                r.failure.as_ref().map_or(
                    crate::judge::fault::PROCESS_TIMEOUT,
                    CaptureFailureV2::fault_code,
                ),
            )),
            _ => None,
        }
    }
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

    pub fn open(root: PathBuf) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut latest_burst: Option<CaptureRoundV2> = None;
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
        let backup_broken = |path: &Path| -> Result<(), String> {
            let backup = path.with_file_name(format!("round.broken-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
            std::fs::rename(path, &backup).map_err(|e| format!("采集记录无法读取且备份失败：{e}"))
        };
        for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path().join("round.json");
            if !path.is_file() { continue; }
            if !std::fs::canonicalize(&path).map_err(|e| e.to_string())?.starts_with(&canonical_root) { return Err("采集记录路径越出保存目录".into()); }
            let text = crate::fsio::read_text(&path).map_err(|e| e.to_string())?;
            if !burst::is_v2(&text) {
                continue;
            }
            match serde_json::from_str::<CaptureRoundV2>(&text) {
                Ok(round) => {
                    let round = burst::restore(&path, round)?;
                    if latest_burst
                        .as_ref()
                        .is_none_or(|last| round.created_at >= last.created_at)
                    {
                        latest_burst = Some(round);
                    }
                }
                Err(_) => backup_broken(&path)?,
            }
        }
        let (burst_writers, burst_receivers): (Vec<_>, Vec<_>) = (0..MAX_CAMERAS).map(|_| mpsc::channel::<burst::BurstItem>()).unzip();
        let host = Arc::new(Self {
            root,
            burst: Mutex::new(latest_burst.map(burst::restored_run)),
            journal: Mutex::new(None),
            burst_writers,
        });
        let weak = Arc::downgrade(&host);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(50));
            let Some(host) = weak.upgrade() else { break };
            host.tick_burst();
        });
        for rx in burst_receivers {
            let weak = Arc::downgrade(&host);
            std::thread::spawn(move || while let Ok(item) = rx.recv() {
                let Some(host) = weak.upgrade() else { break };
                host.write_burst(item);
            });
        }
        Ok(host)
    }

    pub fn active(&self) -> bool { self.burst_active() }

    /// 最近的一轮。
    pub fn current_any(&self) -> Option<AnyRound> {
        self.current_burst().map(AnyRound::Burst)
    }

    /// 按轮次 ID 读一轮，经记录核对；旧拍照点配方的采集轮次已停用，按无效记录报错。
    pub fn get_any(&self, round_id: &str) -> Result<AnyRound, String> {
        let dir = self.safe_round_path(round_id)?;
        if self.burst_round_id().as_deref() != Some(round_id)
            && crate::fsio::read_text(&dir.join("round.json"))
                .is_ok_and(|text| !burst::is_v2(&text))
        {
            return Err(format!(
                "采集轮次 {round_id} 是旧拍照点配方的采集记录，已停用"
            ));
        }
        self.get_burst(round_id).map(AnyRound::Burst)
    }

    pub fn fail(&self, error: impl Into<String>) {
        self.fail_burst_active(CaptureFailureV2::Other, error.into());
    }

    pub fn interrupt(&self, error: impl Into<String>) {
        self.fail_burst_active(CaptureFailureV2::Interrupted, error.into());
    }

    pub fn disconnected(&self, cam: u8) {
        self.disconnected_burst(cam);
    }

    pub fn offer(&self, frame: &Frame, _original: Option<Arc<FrameImage>>) -> bool {
        self.offer_burst(frame)
    }

    pub fn list(&self, recipe_id: &str) -> Result<Vec<CaptureSummary>, String> {
        let mut rounds = Vec::new();
        let live_burst = self.burst_round_id();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().join("round.json").is_file() { continue; }
            let burst = live_burst.as_deref() == Some(id.as_str())
                || crate::fsio::read_text(&entry.path().join("round.json"))
                    .is_ok_and(|text| burst::is_v2(&text));
            if !burst {
                continue;
            }
            let r = self.burst_summary(&id)?;
            if r.recipe_id == recipe_id {
                rounds.push(CaptureSummary {
                    camera_ids: r.camera_ids(),
                    received_count: u32::try_from(r.group_count).unwrap_or(u32::MAX),
                    round_id: r.round_id,
                    recipe_id: r.recipe_id,
                    planned_count: r.plc_shot_count.map(u32::from),
                    state: r.state,
                    created_at: r.created_at,
                    error: r.error,
                });
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

/// 开始一轮段配方整圈采集。
#[tauri::command]
pub async fn recipe_capture_start(
    app: AppHandle,
    recipe_id: String,
    camera_ids: Vec<String>,
    drain_timeout_ms: Option<u64>,
) -> Result<AnyRound, String> {
    let mut data = serde_json::json!({"recipeId": recipe_id, "cameraIds": camera_ids});
    let result =
        recipe_capture_start_inner(app.clone(), recipe_id, camera_ids, drain_timeout_ms).await;
    data["roundId"] = serde_json::json!(result.as_ref().ok().map(|round| round.round_id()));
    crate::cycle::log_op(&app, &crate::applog::codes::OP_CAPTURE_START, data, &result);
    result
}

async fn recipe_capture_start_inner(
    app: AppHandle,
    recipe_id: String,
    camera_ids: Vec<String>,
    drain_timeout_ms: Option<u64>,
) -> Result<AnyRound, String> {
    let cycle = app.state::<CycleHost>();
    let _link = cycle.link_gate.lock().await;
    let _config = cycle.config_gate.lock().await;
    if cycle.busy() {
        return Err("在途事务尚未结束，请等待握手释放或复位".into());
    }
    if crate::cycle::production_start_pending(app.state::<crate::plc::PlcHost>().engine()).await? {
        return Err("PLC 已发出生产请求，请等待生产完成".into());
    }
    let mut teaching = cycle.teaching.lock().await;
    let status = teaching.view();
    if !status.available || status.busy {
        return Err(status.message);
    }
    let capture_app = app.clone();
    let round = tauri::async_runtime::spawn_blocking(move || {
        start_capture(capture_app, recipe_id, camera_ids, drain_timeout_ms)
    })
    .await
    .map_err(|e| e.to_string())??;
    let result = async {
        let id = teaching.prepare_request(round.round_id())?;
        cycle.store_teaching_busy(teaching.busy(), teaching.blocks_site_switch());
        cycle.camera.recipe_capture().burst_ready(id)?;
        teaching
            .commit_request(app.state::<crate::plc::PlcHost>().engine())
            .await?;
        Ok::<(), String>(())
    }
    .await;
    cycle.store_teaching_busy(teaching.busy(), teaching.blocks_site_switch());
    if let Err(error) = result {
        cycle.camera.recipe_capture().fail(error.clone());
        return Err(error);
    }
    cycle
        .camera
        .recipe_capture()
        .current_any()
        .ok_or("采集轮次不存在".into())
}

fn start_capture(app: AppHandle, recipe_id: String, camera_ids: Vec<String>, drain_timeout_ms: Option<u64>) -> Result<AnyRound, String> {
    let cycle = app.state::<CycleHost>();
    if recipe_id.trim().is_empty() {
        return Err("请先创建配方草稿".into());
    }
    if camera_ids.is_empty() {
        return Err("请至少选择一台采集相机".into());
    }
    if cycle.phase() != Phase::Idle {
        return Err("节拍尚未回到空闲，请先复位".into());
    }
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
        if !workspace.doc.is_burst() { return Err("旧拍照点配方已停用，请创建段配方".into()); }
        crate::camera::check_burst_cameras(&cycle.camera.configs(), &camera_ids)?;
        let mut devices = Vec::with_capacity(slots.len());
        for (cam, slot, config) in &slots {
            let imaging = workspace.imaging(&config.id);
            slot.set_imaging(&imaging)?;
            let (arm, ledger) = slot.routing_state().map_err(|e| format!("相机 {}：{e}", config.id))?;
            devices.push(CaptureDevice {
                cam: *cam, camera_id: config.id.clone(), camera_config: serde_json::to_value(config).map_err(|e| e.to_string())?, imaging,
                session: arm.session, source: ledger.as_ref().and_then(|l| l.source).or(arm.source),
                frame_baseline: ledger.as_ref().and_then(|l| l.last_frame).or(arm.counter_after_open),
            });
        }
        let drain = drain_timeout_ms.unwrap_or(1000);
        let capture = cycle.camera.recipe_capture();
        capture.start_burst(recipe_id.clone(), devices, drain, simulated).map(AnyRound::Burst).map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub fn recipe_capture_get(app: AppHandle, round_id: Option<String>) -> Result<Option<AnyRound>, String> {
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    match round_id { Some(id) => capture.get_any(&id).map(Some), None => capture.current_any().map(|r| capture.get_any(r.round_id())).transpose() }
}

#[tauri::command]
pub async fn recipe_capture_stop(app: AppHandle) -> Result<Option<AnyRound>, String> {
    let result = recipe_capture_stop_inner(app.clone()).await;
    crate::cycle::log_op(
        &app,
        &crate::applog::codes::OP_CAPTURE_STOP,
        serde_json::json!({}),
        &result,
    );
    result
}

async fn recipe_capture_stop_inner(app: AppHandle) -> Result<Option<AnyRound>, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.link_gate.lock().await;
    let mut teaching = cycle.teaching.lock().await;
    let result = teaching
        .cancel(app.state::<crate::plc::PlcHost>().engine(), 1)
        .await;
    cycle.store_teaching_busy(teaching.busy(), teaching.blocks_site_switch());
    cycle
        .camera
        .recipe_capture()
        .interrupt("操作员中止本轮采集，请整圈重采");
    result?;
    Ok(cycle.camera.recipe_capture().current_any())
}

#[tauri::command]
pub async fn recipe_capture_plc_status(app: AppHandle) -> crate::teaching::TeachingView {
    app.state::<CycleHost>()
        .teaching
        .lock()
        .await
        .monitor_view(app.state::<crate::plc::PlcHost>().engine())
}

#[tauri::command]
pub fn recipe_capture_list(app: AppHandle, recipe_id: String) -> Result<Vec<CaptureSummary>, String> {
    app.state::<CycleHost>().camera.recipe_capture().list(&recipe_id)
}

/// 某台相机第 seq 帧（帧组序号，从 1 起）的原图，格式见 `preview_bytes`；这台相机缺这一帧时报错。
#[tauri::command]
pub fn recipe_capture_frame_image(app: AppHandle, round_id: String, camera_id: String, seq: u64) -> Result<tauri::ipc::Response, String> {
    let path = app.state::<CycleHost>().camera.recipe_capture().burst_frame_path(&round_id, &camera_id, seq)?;
    let image = crate::image_file::load(&path)?;
    Ok(tauri::ipc::Response::new(preview_bytes(&image)))
}

/// 缩略图下采样倍数：1280×1024 → 160×128。
const THUMB_FACTOR: u32 = 8;

/// 新轮次一台相机全部帧组的缩略图条带（格式见 `frame_align::thumb_strip`），分段标定工作台的时间轴用。
/// 进行中的轮次没有完整帧记录，按 `load_burst` 的核对报错；读不了的帧按缺帧给出。
#[tauri::command]
pub async fn recipe_capture_round_thumbs(
    app: AppHandle,
    round_id: String,
    camera_id: String,
) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = app
            .state::<CycleHost>()
            .camera
            .recipe_capture()
            .burst_frame_paths(&round_id, &camera_id)?;
        let frames: Vec<Option<FrameImage>> = paths
            .iter()
            .map(|path| {
                path.as_ref()
                    .and_then(|path| crate::image_file::load(path).ok())
                    .map(|image| crate::frame_align::downsample(&image, THUMB_FACTOR))
            })
            .collect();
        Ok(tauri::ipc::Response::new(crate::frame_align::thumb_strip(
            &frames,
        )))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 读图命令的原始字节：前 16 字节依次是预览宽、高、原图宽、高（u32 小端），其后是最近邻缩到 900×650 以内的 8 位灰度。
fn preview_bytes(image: &FrameImage) -> Vec<u8> {
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
    bytes
}

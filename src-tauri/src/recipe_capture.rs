use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::camera::{Acquisition, CameraSource};
use crate::cycle::{CycleHost, Phase};
use crate::frame::{CounterSource, Frame, FrameImage};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureView {
    pub view: u8,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureShot {
    pub shot_id: String,
    pub ordinal: u32,
    pub frame_counter: u64,
    pub trigger_counter: Option<u64>,
    pub original_path: String,
    pub views: Vec<CaptureView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureRound {
    pub round_id: String,
    pub recipe_id: String,
    pub camera_id: String,
    pub camera_config: serde_json::Value,
    pub device_session: u64,
    pub planned_count: u32,
    pub plc_planned_count: Option<u32>,
    pub plc_actual_count: Option<u32>,
    pub plc_plan_version: Option<u32>,
    pub received_count: u32,
    pub state: CaptureState,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    pub frames: Vec<CaptureShot>,
    pub error: Option<String>,
    pub simulated: bool,
}

struct Running {
    round: CaptureRound,
    cam: u8,
    source: Option<CounterSource>,
    last_frame: Option<u64>,
    last_trigger: Option<u64>,
    drain_timeout: Duration,
    deadline: Instant,
}

struct QueuedFrame { round_id: String, frame: Frame, original: Option<Arc<FrameImage>> }

pub struct RecipeCaptureHost {
    root: PathBuf,
    current: Mutex<Option<Running>>,
    uncertain_sessions: Mutex<std::collections::HashSet<(String, u64)>>,
    tx: mpsc::SyncSender<QueuedFrame>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSummary {
    pub round_id: String,
    pub recipe_id: String,
    pub camera_id: String,
    pub planned_count: u32,
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

    fn validate_record(&self, expected_id: &str, round: &CaptureRound) -> Result<(), String> {
        let dir = self.safe_round_path(expected_id)?;
        if round.round_id != expected_id || round.recipe_id.is_empty() || round.camera_id.is_empty()
            || round.device_session == 0 || !(1..=64).contains(&round.planned_count)
            || round.frames.len() > round.received_count as usize {
            return Err("采集记录身份、计划数量或收图数量无效".into());
        }
        if round.state == CaptureState::Complete && (round.error.is_some() || round.ended_at.is_none()
            || round.plc_planned_count != Some(round.planned_count) || round.plc_actual_count != Some(round.planned_count)
            || round.received_count != round.planned_count || round.frames.len() != round.planned_count as usize) {
            return Err("完整采集缺少 PLC 结束、触发数量或完整图像证据".into());
        }
        for (k, shot) in round.frames.iter().enumerate() {
            let ordinal = k as u32 + 1;
            if shot.ordinal != ordinal || shot.shot_id != format!("S{ordinal:04}") || shot.views.len() != 3
                || shot.views.iter().enumerate().any(|(v, image)| image.view as usize != v + 1 || image.width == 0 || image.height == 0) {
                return Err("采集拍照点顺序或三幅图结构无效".into());
            }
            let shot_dir = self.root.join(expected_id).join(format!("shot-{ordinal:04}"));
            let mut files = vec![(PathBuf::from(&shot.original_path), shot_dir.join("original.png"))];
            files.extend(shot.views.iter().map(|v| (PathBuf::from(&v.path), shot_dir.join(format!("view-{}.png", v.view)))));
            for (stored, expected) in files {
                if stored != expected { return Err("采集图像路径与轮次目录不一致".into()); }
                let actual = std::fs::canonicalize(&expected).map_err(|e| format!("采集图像不存在：{e}"))?;
                let required = dir.join(format!("shot-{ordinal:04}")).join(expected.file_name().ok_or("图像文件名无效")?);
                if actual != required { return Err("采集图像路径越出本轮目录或链接到其他拍照点".into()); }
            }
        }
        Ok(())
    }

    pub fn open(root: PathBuf) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut latest: Option<CaptureRound> = None;
        let mut uncertain_sessions = std::collections::HashSet::new();
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path().join("round.json");
            if !path.is_file() { continue; }
            if !std::fs::canonicalize(&path).map_err(|e| e.to_string())?.starts_with(&canonical_root) { return Err("采集记录路径越出保存目录".into()); }
            let mut round: CaptureRound = serde_json::from_str(&crate::fsio::read_text(&path).map_err(|e| e.to_string())?).map_err(|e| format!("采集记录无法读取：{e}"))?;
            if round.state.active() {
                round.state = CaptureState::Failed;
                round.error = Some("应用退出导致本轮采集中断，请整圈重采".into());
                crate::fsio::write_atomic(&path, &serde_json::to_string_pretty(&round).map_err(|e| e.to_string())?)?;
            }
            if round.state == CaptureState::Failed {
                uncertain_sessions.insert((round.camera_id.clone(), round.device_session));
            }
            if latest.as_ref().is_none_or(|last| round.created_at >= last.created_at) { latest = Some(round); }
        }
        let (tx, rx) = mpsc::sync_channel::<QueuedFrame>(64);
        let current = latest.map(|round| Running { round, cam: 0, source: None, last_frame: None, last_trigger: None, drain_timeout: Duration::from_secs(1), deadline: Instant::now() });
        let host = Arc::new(Self { root, current: Mutex::new(current), uncertain_sessions: Mutex::new(uncertain_sessions), tx });
        let weak = Arc::downgrade(&host);
        std::thread::spawn(move || loop {
            let item = rx.recv_timeout(Duration::from_millis(50));
            let Some(host) = weak.upgrade() else { break };
            match item {
                Ok(item) => host.receive(item),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => host.tick(),
            }
        });
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

    pub fn start(&self, recipe_id: String, cam: u8, camera_id: String, camera_config: serde_json::Value, session: u64, source: Option<CounterSource>, trigger_baseline: Option<u64>, planned_count: u32, drain_timeout_ms: u64, simulated: bool) -> Result<CaptureRound, String> {
        if planned_count == 0 || planned_count > 64 || !(200..=30000).contains(&drain_timeout_ms) || session == 0 {
            return Err("计划数量须为 1–64，等待在途帧时间须为 200–30000 ms，且设备会话必须就绪".into());
        }
        if trigger_baseline.is_none() { return Err("设备没有可验证的开流计数基线，请先配置计数起点或取得当前会话计数".into()); }
        let mut guard = self.current.lock().unwrap();
        if guard.as_ref().is_some_and(|r| r.round.state.active()) { return Err("已有示教采集正在占用设备".into()); }
        if self.uncertain_sessions.lock().unwrap().contains(&(camera_id.clone(), session)) {
            return Err("上一轮采集异常，当前设备会话可能仍有在途旧帧；请重新连接设备后整圈重采".into());
        }
        let created_at = ly_plc::now_ms();
        let mut suffix = 0u32;
        let round_id = loop {
            let id = format!("capture-{created_at}-{suffix}");
            match std::fs::create_dir(self.root.join(&id)) {
                Ok(()) => break id,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
                Err(e) => return Err(e.to_string()),
            }
        };
        let round = CaptureRound { round_id, recipe_id, camera_id, camera_config, device_session: session, planned_count, plc_planned_count: None, plc_actual_count: None, plc_plan_version: None, received_count: 0, state: CaptureState::WaitingStart, created_at, ended_at: None, frames: Vec::new(), error: None, simulated };
        self.save(&round)?;
        let frame_baseline = matches!(source, Some(CounterSource::SdkFrame | CounterSource::ChunkFrame)).then_some(trigger_baseline).flatten();
        *guard = Some(Running { round: round.clone(), cam, source, last_frame: frame_baseline, last_trigger: trigger_baseline, drain_timeout: Duration::from_millis(drain_timeout_ms), deadline: Instant::now() + Duration::from_secs(120) });
        Ok(round)
    }

    fn fail_running(&self, run: &mut Running, error: String) {
        run.round.state = CaptureState::Failed;
        self.uncertain_sessions.lock().unwrap().insert((run.round.camera_id.clone(), run.round.device_session));
        run.round.error = Some(error);
        if let Err(error) = self.save(&run.round) { run.round.error = Some(format!("{}；异常记录保存失败：{error}", run.round.error.as_deref().unwrap_or_default())); }
    }

    pub fn fail(&self, error: impl Into<String>) {
        if let Some(run) = self.current.lock().unwrap().as_mut().filter(|r| r.round.state.active()) { self.fail_running(run, error.into()); }
    }

    pub fn disconnected(&self, cam: u8) {
        if let Some(run) = self.current.lock().unwrap().as_mut().filter(|r| r.cam == cam && r.round.state.active()) { self.fail_running(run, "采集设备断连，请整圈重采".into()); }
    }

    pub fn plc_started(&self, planned_count: u32) -> Result<(), String> {
        let mut guard = self.current.lock().unwrap();
        let run = guard.as_mut().ok_or("没有等待开始的示教采集")?;
        run.round.plc_planned_count = Some(planned_count);
        if run.round.state != CaptureState::WaitingStart || planned_count != run.round.planned_count {
            self.fail_running(run, "PLC 开始信号重复或计划触发数量与采集设置不一致".into());
            return Err(run.round.error.clone().unwrap());
        }
        run.round.state = CaptureState::Receiving;
        run.deadline = Instant::now() + Duration::from_secs(120);
        if let Err(e) = self.save(&run.round) { self.fail_running(run, e.clone()); return Err(e); }
        Ok(())
    }

    pub fn set_plan_version(&self, version: u32) -> Result<(), String> {
        let mut guard = self.current.lock().unwrap();
        let run = guard.as_mut().ok_or("没有等待开始的示教采集")?;
        if version == 0 || run.round.state != CaptureState::WaitingStart { return Err("计划版本须为正数，且只能在 PLC 开始前记录".into()); }
        run.round.plc_plan_version = Some(version);
        if let Err(e) = self.save(&run.round) { self.fail_running(run, e.clone()); return Err(e); }
        Ok(())
    }

    pub fn plc_ended(&self, actual_count: u32) -> Result<(), String> {
        let mut guard = self.current.lock().unwrap();
        let run = guard.as_mut().ok_or("没有正在进行的示教采集")?;
        run.round.plc_actual_count = Some(actual_count);
        if run.round.state != CaptureState::Receiving || actual_count != run.round.planned_count {
            self.fail_running(run, "PLC 结束顺序异常或实际触发数量与计划不一致".into());
            return Err(run.round.error.clone().unwrap());
        }
        run.round.state = CaptureState::Draining;
        run.round.ended_at = Some(ly_plc::now_ms());
        run.deadline = Instant::now() + run.drain_timeout;
        if let Err(e) = self.save(&run.round) { self.fail_running(run, e.clone()); return Err(e); }
        Ok(())
    }

    pub fn offer(&self, frame: &Frame, original: Option<Arc<FrameImage>>) -> bool {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active()) else { return false; };
        if frame.cam != run.cam { return true; }
        if run.round.state == CaptureState::WaitingStart {
            self.fail_running(run, "PLC 开始前收到帧，无法确定本轮归属".into());
            return true;
        }
        if self.tx.try_send(QueuedFrame { round_id: run.round.round_id.clone(), frame: frame.clone(), original }).is_err() {
            self.fail_running(run, "示教采集写盘队列已满或关闭，不能保证完整收图，请整圈重采".into());
        }
        true
    }

    fn receive(&self, item: QueuedFrame) {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.round_id == item.round_id && r.round.state.active()) else { return; };
        let f = item.frame;
        run.round.received_count += 1;
        let error = if run.round.state == CaptureState::WaitingStart { Some("PLC 开始前收到帧，无法确定本轮归属") }
            else if f.session != run.round.device_session { Some("设备会话已变化，不能混用不同会话的图像") }
            else if f.manual { Some("本轮收到手动触发图像") }
            else if f.lost_packets != 0 { Some("图像存在丢包") }
            else if run.round.received_count > run.round.planned_count { Some("实际收图数量超过 PLC 计划数量") }
            else if f.images.len() != 3 { Some("一张拼接图必须完整交付图 1／2／3") }
            else if f.images.iter().any(|i| i.width == 0 || i.height == 0 || (i.width as usize).checked_mul(i.height as usize) != Some(i.pixels.len())) { Some("图像尺寸或像素长度无效") }
            else if run.round.frames.first().is_some_and(|shot| shot.views.iter().zip(&f.images).any(|(v, i)| v.width != i.width || v.height != i.height)) { Some("本轮图像尺寸发生变化") }
            else if run.source.is_some_and(|source| source != f.counter) { Some("帧计数来源发生变化") }
            else if run.source.is_none() && !matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic) && run.last_trigger.is_some_and(|last| last.checked_add(1) != Some(f.frame_counter)) { Some("首帧计数与开流基线不连续") }
            else if run.last_frame.is_some_and(|last| last.checked_add(1) != Some(f.frame_counter)) { Some("SDK 帧号重复、跳号或顺序异常") }
            else if matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic) && run.last_trigger.is_some_and(|last| last.checked_add(1) != Some(f.trigger_counter)) { Some("触发计数重复、跳号或顺序异常") }
            else { None };
        if let Some(error) = error { self.fail_running(run, error.into()); return; }
        run.source = Some(f.counter);
        run.last_frame = Some(f.frame_counter);
        let trigger = matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic).then_some(f.trigger_counter);
        run.last_trigger = trigger;
        let ordinal = run.round.received_count;
        let dir = self.root.join(&run.round.round_id).join(format!("shot-{ordinal:04}"));
        let round_id = run.round.round_id.clone();
        drop(guard);
        let saved = (|| -> Result<CaptureShot, String> {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let original = match item.original { Some(image) => image, None => compose_views(&f.images)? };
            let original_path = dir.join("original.png");
            save_image(&original_path, &original)?;
            let views = f.images.iter().enumerate().map(|(i, image)| {
                let path = dir.join(format!("view-{}.png", i + 1));
                save_image(&path, image)?;
                Ok(CaptureView { view: i as u8 + 1, path: path.to_string_lossy().into_owned(), width: image.width, height: image.height })
            }).collect::<Result<Vec<_>, String>>()?;
            Ok(CaptureShot { shot_id: format!("S{ordinal:04}"), ordinal, frame_counter: f.frame_counter, trigger_counter: trigger, original_path: original_path.to_string_lossy().into_owned(), views })
        })();
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.round_id == round_id && r.round.state.active()) else { return; };
        match saved {
            Ok(shot) => { run.round.frames.push(shot); if let Err(e) = self.save(&run.round) { self.fail_running(run, e); } }
            Err(e) => self.fail_running(run, format!("保存采集图像失败：{e}")),
        }
    }

    fn tick(&self) {
        let mut guard = self.current.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active() && Instant::now() >= r.deadline) else { return; };
        if run.round.state == CaptureState::Draining && run.round.frames.len() == run.round.planned_count as usize {
            run.round.state = CaptureState::Complete;
            if let Err(e) = self.save(&run.round) { self.fail_running(run, e); }
        } else {
            let error = format!("采集超时：计划 {} 张，已收到 {} 张；必须收到 PLC 结束且数量一致，请整圈重采", run.round.planned_count, run.round.received_count);
            self.fail_running(run, error);
        }
    }

    pub fn list(&self, recipe_id: &str) -> Result<Vec<CaptureSummary>, String> {
        let mut rounds = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().join("round.json").is_file() { continue; }
            let r = self.get(&id)?;
            if r.recipe_id == recipe_id {
                rounds.push(CaptureSummary { round_id: r.round_id, recipe_id: r.recipe_id, camera_id: r.camera_id, planned_count: r.planned_count, received_count: r.received_count, state: r.state, created_at: r.created_at, error: r.error });
            }
        }
        rounds.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.round_id.cmp(&a.round_id)));
        Ok(rounds)
    }
}

fn save_image(path: &Path, image: &FrameImage) -> Result<(), String> {
    image::save_buffer_with_format(path, &image.pixels, image.width, image.height, image::ColorType::L8, image::ImageFormat::Png).map_err(|e| e.to_string())
}

fn compose_views(images: &[Arc<FrameImage>]) -> Result<Arc<FrameImage>, String> {
    let height = images.first().ok_or("没有图像")?.height;
    if images.iter().any(|i| i.height != height) { return Err("模拟／回放图像高度不一致，无法保留拼接原图".into()); }
    let width = images.iter().try_fold(0u32, |w, i| w.checked_add(i.width)).ok_or("图像宽度溢出")?;
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height as usize { for image in images { let start = y * image.width as usize; pixels.extend_from_slice(&image.pixels[start..start + image.width as usize]); } }
    Ok(Arc::new(FrameImage::new(width, height, pixels)))
}

#[tauri::command]
pub fn recipe_capture_start(app: AppHandle, recipe_id: String, camera_id: String, planned_count: u32, drain_timeout_ms: Option<u64>) -> Result<CaptureRound, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务")?;
    if recipe_id.trim().is_empty() { return Err("请先创建配方草稿".into()); }
    if cycle.busy() || cycle.phase() != Phase::Idle || cycle.camera.dry_run_active() { return Err("设备正被检测或空跑占用".into()); }
    let cam = cycle.camera.require(&camera_id)?;
    let slot = cycle.camera.slot(cam as usize).ok_or("设备不存在")?;
    let config = slot.config();
    if !slot.is_ready() || config.acquisition != Acquisition::Triggered || config.view_count != 3 { return Err("采集要求已就绪的单个 device、触发模式及三幅图交付".into()); }
    if config.source == CameraSource::Mvs && config.trigger_source != "Line0" { return Err("真实示教采集必须使用 PLC Line0 硬触发".into()); }
    let (arm, ledger) = slot.routing_state()?;
    let source = ledger.as_ref().and_then(|l| l.source).or(arm.source);
    let baseline = ledger.as_ref().and_then(|l| l.last).or(arm.counter_after_open);
    app.state::<crate::workspace::WorkspaceHost>().capture_for_candidate(&recipe_id, || {
        cycle.camera.recipe_capture().start(recipe_id.clone(), cam, camera_id, serde_json::to_value(&config).map_err(|e| e.to_string())?, arm.session, source, baseline, planned_count, drain_timeout_ms.unwrap_or(1000), config.source != CameraSource::Mvs)
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
pub fn recipe_capture_image(app: AppHandle, round_id: String, k: usize, view: Option<u8>) -> Result<tauri::ipc::Response, String> {
    let cycle = app.state::<CycleHost>();
    let round = cycle.camera.recipe_capture().get(&round_id)?;
    let shot = round.frames.get(k).ok_or("拍照点不存在")?;
    let path = match view {
        None => &shot.original_path,
        Some(view) => &shot.views.iter().find(|v| v.view == view).ok_or("图像不存在")?.path,
    };
    let image = crate::replay::load(Path::new(path))?;
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

    fn host() -> Arc<RecipeCaptureHost> {
        let path = std::env::temp_dir().join(format!("gluesight-capture-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_TEST.fetch_add(1, Ordering::Relaxed)));
        RecipeCaptureHost::open(path).unwrap()
    }

    fn start(host: &RecipeCaptureHost, count: u32) -> CaptureRound {
        let session = host.current().map_or(9, |r| r.device_session + 1);
        host.start("recipe-a".into(), 0, "cam1".into(), serde_json::json!({"source":"sim"}), session, Some(CounterSource::Synthetic), Some(0), count, 200, true).unwrap()
    }

    fn frame(n: u64) -> Frame {
        Frame { cam: 0, session: 9, counter: CounterSource::Synthetic, frame_counter: n, trigger_counter: n, lost_packets: 0, ts: 0, manual: false,
            images: (1..=3).map(|v| Arc::new(FrameImage::new(2, 2, vec![v; 4]))).collect() }
    }

    fn receive(host: &RecipeCaptureHost, round: &CaptureRound, f: Frame) {
        host.receive(QueuedFrame { round_id: round.round_id.clone(), frame: f, original: None });
    }

    fn drain(host: &RecipeCaptureHost) {
        host.current.lock().unwrap().as_mut().unwrap().deadline = Instant::now();
        host.tick();
    }

    #[test]
    fn first_real_frame_requires_a_baseline_and_cannot_be_old_or_missing() {
        for source in [None, Some(CounterSource::SdkFrame), Some(CounterSource::ChunkFrame), Some(CounterSource::ChunkTrigger)] {
            let host = host();
            assert!(host.start("r".into(), 0, "cam1".into(), serde_json::json!({}), 9, source, None, 1, 200, false).is_err());
            for first in [10, 12] {
                let session = first;
                let round = host.start("r".into(), 0, "cam1".into(), serde_json::json!({}), session, source, Some(10), 1, 200, false).unwrap();
                host.plc_started(1).unwrap();
                let mut f = frame(first);
                f.session = session;
                f.counter = source.unwrap_or(CounterSource::SdkFrame);
                receive(&host, &round, f);
                assert_eq!(host.current().unwrap().state, CaptureState::Failed);
                assert!(host.current().unwrap().frames.is_empty());
            }
            let round = host.start("r".into(), 0, "cam1".into(), serde_json::json!({}), 9, source, Some(10), 1, 200, false).unwrap();
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
        let round = host.start("r".into(), 0, "cam1".into(), serde_json::json!({}), 9,
            Some(counter), Some(0), 20, 200, simulated).unwrap();
        host.plc_started(20).unwrap();
        for n in 1..=19 { let mut f = frame(n); f.counter = counter; receive(&host, &round, f); }
        host.plc_ended(20).unwrap();
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        assert!(host.start("r".into(), 0, "cam1".into(), serde_json::json!({}), 9,
            Some(counter), Some(19), 20, 200, simulated).unwrap_err().contains("重新连接"));
        let next = host.start("r".into(), 0, "cam1".into(), serde_json::json!({}), 10,
            Some(counter), Some(0), 20, 200, simulated).unwrap();
        host.plc_started(20).unwrap();
        let mut late = frame(20); late.counter = counter;
        receive(&host, &next, late);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        assert!(host.current().unwrap().frames.is_empty());
        }
    }

    #[test]
    fn twenty_frames_are_twenty_shots_and_require_plc_end() {
        let host = host();
        let round = start(&host, 20);
        host.plc_started(20).unwrap();
        for n in 1..=20 { receive(&host, &round, frame(n)); }
        let result = host.current().unwrap();
        assert_eq!(result.state, CaptureState::Receiving);
        assert_eq!(result.frames.len(), 20);
        assert!(result.frames.iter().all(|s| s.views.len() == 3 && Path::new(&s.original_path).is_file()));
        host.plc_ended(20).unwrap();
        assert_eq!(host.current().unwrap().state, CaptureState::Draining);
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Complete);
    }

    #[test]
    fn missing_or_extra_frames_are_failed_rounds() {
        for received in [19, 21] {
            let host = host();
            let round = start(&host, 20);
            host.plc_started(20).unwrap();
            for n in 1..=received { receive(&host, &round, frame(n)); }
            if received == 19 { host.plc_ended(20).unwrap(); drain(&host); }
            assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        }
    }

    #[test]
    fn trailing_frames_are_accepted_but_extra_during_drain_fails() {
        let host = host();
        let round = start(&host, 2);
        host.plc_started(2).unwrap();
        receive(&host, &round, frame(1));
        host.plc_ended(2).unwrap();
        receive(&host, &round, frame(2));
        assert_eq!(host.current().unwrap().state, CaptureState::Draining);
        receive(&host, &round, frame(3));
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
    }

    #[test]
    fn duplicate_gap_session_manual_and_packet_loss_fail() {
        for bad in [0, 1, 2, 3, 4] {
            let host = host();
            let round = start(&host, 3);
            host.plc_started(3).unwrap();
            receive(&host, &round, frame(1));
            let mut next = frame(2);
            match bad { 0 => next.frame_counter = 1, 1 => next.trigger_counter = 3, 2 => next.session = 10, 3 => next.manual = true, _ => next.lost_packets = 1 }
            receive(&host, &round, next);
            assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        }
    }

    #[test]
    fn stale_queued_frames_cannot_enter_a_new_round() {
        let host = host();
        let old = start(&host, 1);
        host.fail("中止");
        let new = start(&host, 1);
        assert_ne!(old.round_id, new.round_id);
        host.plc_started(1).unwrap();
        receive(&host, &old, frame(1));
        assert_eq!(host.current().unwrap().received_count, 0);
        let mut first = frame(1); first.session = new.device_session;
        receive(&host, &new, first);
        host.plc_ended(1).unwrap();
        drain(&host);
        assert_eq!(host.current().unwrap().state, CaptureState::Complete);
        assert_eq!(host.get(&old.round_id).unwrap().state, CaptureState::Failed);
    }

    #[test]
    fn restart_marks_interrupted_round_failed_and_restores_latest() {
        let host = host();
        let round = start(&host, 2);
        let reopened = RecipeCaptureHost::open(host.root.clone()).unwrap();
        assert_eq!(reopened.current().unwrap().round_id, round.round_id);
        assert_eq!(reopened.current().unwrap().state, CaptureState::Failed);
        assert!(!reopened.active());
    }

    #[test]
    fn plc_plan_mismatch_start_order_and_disconnect_fail() {
        let host = host();
        start(&host, 2);
        assert!(host.plc_started(3).is_err());
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        start(&host, 2);
        assert!(host.offer(&frame(1), None));
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
        start(&host, 2);
        host.plc_started(2).unwrap();
        host.disconnected(0);
        assert_eq!(host.current().unwrap().state, CaptureState::Failed);
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
        let round = start(&host, 1);
        host.plc_started(1).unwrap();
        receive(&host, &round, frame(1));
        host.plc_ended(1).unwrap();
        drain(&host);
        let valid = host.get(&round.round_id).unwrap();
        let mut forged = valid.clone();
        forged.frames[0].views[0].path = host.root.join("external.png").to_string_lossy().into_owned();
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        forged = valid.clone(); forged.plc_actual_count = None;
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        forged = valid.clone(); forged.frames[0].views[0].view = 2;
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        forged = valid; forged.round_id = "capture-other".into();
        assert!(host.validate_record(&round.round_id, &forged).is_err());
        assert!(host.get("capture-../../outside").is_err());
        assert!(host.start("recipe-a".into(), 0, "cam1".into(), serde_json::json!({}), 9, None, None, 65, 200, true).is_err());
    }
}

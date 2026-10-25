use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::frame::{CounterSource, Frame, FrameImage};
use crate::judge::Verdict;
use crate::recipe::{valid_camera_id, Recipe};
use crate::settings::{CycleSettings, RecordMode};
use crate::store::{FrameKey, HistoryScope, RecordSource};

/// 旧拍照点模型的录制队列：按组（一个拍照点的 1 或 3 个视角）计，行为不变。
const QUEUE: usize = 48;
/// 段配方的录制队列：按帧计（D-SB-32），只防失控；图像内存由 FramePool 的每相机字节预算封顶。
/// 队满时该帧原图不录，part.json 记 rawDropped，证据状态 Incomplete，不影响结论。
const BURST_QUEUE: usize = 1024;
/// 段配方逐帧写盘错误写进录制结果的条数上限；其余只在 part.json 的 frames 里。
const BURST_FRAME_ERRORS: usize = 8;
const PENDING_SWEEP_LIMIT: usize = 64;
const PENDING_FILE_LIMIT: usize = 256;

pub type PendingProtection = Arc<dyn Fn() -> Result<Vec<PathBuf>, String> + Send + Sync>;
pub type RetentionWarning = Arc<dyn Fn(&[String]) + Send + Sync>;
/// 滚动清理删除旧原图前，先让历史记录不再引用这些目录；失败则本次不删。
pub type RecordingRelease = Arc<dyn Fn(&[PathBuf]) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct Cleanup {
    pub protection: PendingProtection,
    pub warning: RetentionWarning,
    pub release: RecordingRelease,
}

pub type RecordingCallback = Arc<dyn Fn(RecordingOutcome) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    Off,
    NotRetained,
    Complete,
    Incomplete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedRawFile {
    pub k: usize,
    pub view: u8,
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// 段配方的帧身份 (计划相机下标, seq)；有值时 k、view 不用（为 0）。旧拍照点模型为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<FrameKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingOutcome {
    pub cycle_id: String,
    pub directory: Option<PathBuf>,
    pub files: Vec<RecordedRawFile>,
    pub errors: Vec<String>,
    pub retention_errors: Vec<String>,
    pub available: bool,
    pub state: RecordingState,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameMeta {
    cycle_id: String,
    bundle_id: Option<String>,
    recipe_revision: String,
    session: u64,
    ordinal: u64,
    k: usize,
    shot_id: String,
    selected_view: u8,
    view: u8,
    cam: u8,
    camera: String,
    seq: u64,
    file: String,
    ts: i64,
    counter: CounterSource,
    frame_counter: u64,
    trigger_counter: u64,
    lost_packets: u32,
    manual: bool,
    width: u32,
    height: u32,
    queued: bool,
    available: bool,
    error: Option<String>,
}

struct FrameGroup {
    cycle_id: String,
    pending: PathBuf,
    images: Vec<(FrameMeta, Arc<FrameImage>)>,
}

/// 段配方一帧的录制元数据（part.json 的 frames）。camera 是计划相机下标，与 IPC 的 FrameRecord.camera 同一口径。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BurstFrameMeta {
    cycle_id: String,
    bundle_id: Option<String>,
    recipe_revision: String,
    session: u64,
    camera: usize,
    camera_id: String,
    cam: u8,
    seq: u64,
    file: String,
    ts: i64,
    counter: CounterSource,
    frame_counter: u64,
    trigger_counter: u64,
    lost_packets: u32,
    manual: bool,
    width: u32,
    height: u32,
    queued: bool,
    available: bool,
    error: Option<String>,
}

struct BurstFrameJob {
    cycle_id: String,
    pending: PathBuf,
    meta: BurstFrameMeta,
    image: Arc<FrameImage>,
}

/// 段配方录制的帧状态：按 (计划相机下标, seq) 记，缺帧在收尾时按“1..=最大 seq 内没到的帧”列出。
struct BurstFrames {
    cameras: Vec<String>,
    frames: Vec<BurstFrameMeta>,
    seen: BTreeSet<FrameKey>,
    /// 录制队满或写入线程停止而没录的帧
    dropped: Vec<FrameKey>,
    drop_reason: Option<String>,
}

struct FinishJob {
    recording: Recording,
    target: Option<PathBuf>,
    verdict: Verdict,
    reason: String,
    keep: u32,
    max_bytes: u64,
    in_use: Vec<PathBuf>,
}

enum Msg {
    Frames(FrameGroup),
    BurstFrame(BurstFrameJob),
    Finish(FinishJob),
    Prune { keep: u32, max_bytes: u64 },
}

pub struct Recording {
    scope: HistoryScope,
    started: i64,
    dir: PathBuf,
    name: String,
    cycle_id: String,
    bundle_id: Option<String>,
    sn: String,
    recipe: Arc<Recipe>,
    mode: RecordMode,
    frames: Vec<FrameMeta>,
    seen: HashSet<usize>,
    dropped: u32,
    errors: Vec<String>,
    /// 段配方（配方 segments 非空）按相机帧录制；旧拍照点配方为 None
    burst: Option<BurstFrames>,
    /// 收尾时并入 part.json 的附加字段（件的模式、相机设备、PLC 请求）
    extra: serde_json::Map<String, serde_json::Value>,
}

/// 维护采集件的录制目录里放这个标记：示教与标定的样本，不参与按件数与大小的自动清理，由现场清理。
pub const KEEP_MARKER: &str = "maintenance-collect";

impl Recording {
    pub fn annotate(&mut self, key: &str, value: serde_json::Value) {
        self.extra.insert(key.to_owned(), value);
    }

    fn keep(&self) -> bool {
        self.extra.get("partMode").and_then(serde_json::Value::as_str) == Some("maintenanceCollect")
    }
}

pub struct Recorder {
    root: PathBuf,
    tx: Sender<Msg>,
    queued: Arc<AtomicUsize>,
    callback: Option<RecordingCallback>,
    startup_error: Option<String>,
    active: Arc<Mutex<HashSet<PathBuf>>>,
    cleanup: Option<Cleanup>,
    scope: HistoryScope,
}

/// 真机与离线各用一个录制根目录、各自滚动清理：离线连续跑多少件都不会挤掉真机原图。
pub struct Recorders {
    real: Recorder,
    offline: Recorder,
}

impl Recorders {
    pub fn new(real: Recorder, offline: Recorder) -> Self {
        Self { real, offline: Recorder { scope: HistoryScope::Offline, ..offline } }
    }

    pub fn get(&self, scope: HistoryScope) -> &Recorder {
        match scope {
            HistoryScope::Real => &self.real,
            HistoryScope::Offline | HistoryScope::Debug => &self.offline,
        }
    }

    pub fn root(&self, source: RecordSource) -> &Path {
        self.get(HistoryScope::of(source)).root()
    }

    pub fn begin(&self, source: RecordSource, mode: RecordMode, sn: &str, recipe: Arc<Recipe>, cycle_id: &str, bundle_id: Option<&str>) -> Option<Recording> {
        self.get(HistoryScope::of(source)).begin(mode, sn, recipe, cycle_id, bundle_id)
    }

    pub fn frame(&self, rec: &mut Recording, frame: &Frame, camera: &str, k: usize, ordinal: u64) {
        self.get(rec.scope).frame(rec, frame, camera, k, ordinal)
    }

    /// 段配方：按 (计划相机下标, seq) 录一帧。
    pub fn burst_frame(&self, rec: &mut Recording, frame: &Frame, camera: usize, seq: u64) {
        self.get(rec.scope).burst_frame(rec, frame, camera, seq)
    }

    pub fn finish(&self, rec: Recording, verdict: Verdict, reason: &str, settings: &CycleSettings) {
        let (keep, max_bytes) = retention(rec.scope, settings);
        self.get(rec.scope).finish(rec, verdict, reason, keep, max_bytes, Vec::new())
    }

    /// 按当前保留上限立即滚动清理两个录制根目录，不等下一件收尾（资料盘不足暂停接件时没有下一件）。
    pub fn apply_retention(&self, settings: &CycleSettings) {
        for scope in [HistoryScope::Real, HistoryScope::Offline] {
            let (keep, max_bytes) = retention(scope, settings);
            let _ = self.get(scope).tx.send(Msg::Prune { keep, max_bytes });
        }
    }
}

fn retention(scope: HistoryScope, settings: &CycleSettings) -> (u32, u64) {
    let (keep, max_gb) = match scope {
        HistoryScope::Offline | HistoryScope::Debug => (settings.offline_record_keep, settings.offline_record_max_gb),
        HistoryScope::Real => (settings.record_keep, settings.record_max_gb),
    };
    (keep, (max_gb as f64 * 1e9) as u64)
}

impl Recorder {
    #[cfg(test)]
    pub fn new(root: PathBuf, callback: Option<RecordingCallback>) -> Self {
        Self::start(root, callback, None)
    }

    pub fn guarded(root: PathBuf, callback: Option<RecordingCallback>, cleanup: Cleanup) -> Self {
        Self::start(root, callback, Some(cleanup))
    }

    fn start(root: PathBuf, callback: Option<RecordingCallback>, cleanup: Option<Cleanup>) -> Self {
        let (tx, rx) = channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(Mutex::new(HashSet::new()));
        let (writer_root, writer_queue, writer_callback, writer_active, writer_cleanup) = (root.clone(), queued.clone(), callback.clone(), active.clone(), cleanup.clone());
        let startup_error = std::thread::Builder::new()
            .name("frame-recorder".into())
            .spawn(move || writer(writer_root, rx, writer_queue, writer_callback, writer_active, writer_cleanup))
            .err()
            .map(|e| format!("录制线程启动失败：{e}"));
        Self { root, tx, queued, callback, startup_error, active, cleanup, scope: HistoryScope::Real }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn begin(&self, mode: RecordMode, sn: &str, recipe: Arc<Recipe>, cycle_id: &str, bundle_id: Option<&str>) -> Option<Recording> {
        if mode == RecordMode::Off {
            notify(
                &self.callback,
                RecordingOutcome {
                    cycle_id: cycle_id.into(),
                    directory: None,
                    files: Vec::new(),
                    errors: Vec::new(),
                    retention_errors: Vec::new(),
                    available: false,
                    state: RecordingState::Off,
                },
            );
            return None;
        }
        let error = if !safe_cycle_id(cycle_id) {
            Some("录制 cycleId 只能包含字母、数字、- 和 _，长度为 1–128".to_string())
        } else {
            self.startup_error.clone()
        };
        if let Some(error) = error {
            notify(
                &self.callback,
                RecordingOutcome {
                    cycle_id: cycle_id.into(),
                    directory: None,
                    files: Vec::new(),
                    errors: vec![error],
                    retention_errors: Vec::new(),
                    available: false,
                    state: RecordingState::Failed,
                },
            );
            return None;
        }
        let name = Local::now().format("%Y%m%d_%H%M%S_%3f").to_string();
        let dir = self.root.join("_pending").join(format!("{name}_cycle_{cycle_id}"));
        if !self.active.lock().unwrap().insert(dir.clone()) {
            notify(&self.callback, RecordingOutcome {
                cycle_id: cycle_id.into(), directory: None, files: Vec::new(),
                errors: vec!["录制目录仍在使用或清理，拒绝覆盖".into()], retention_errors: Vec::new(),
                available: false, state: RecordingState::Failed,
            });
            return None;
        }
        let burst = recipe.is_burst().then(|| BurstFrames {
            cameras: recipe.plan_cameras(),
            frames: Vec::new(),
            seen: BTreeSet::new(),
            dropped: Vec::new(),
            drop_reason: None,
        });
        Some(Recording {
            scope: self.scope,
            started: ly_plc::now_ms(),
            dir,
            name,
            cycle_id: cycle_id.into(),
            bundle_id: bundle_id.map(str::to_string),
            sn: sn.into(),
            recipe,
            mode,
            frames: Vec::new(),
            seen: HashSet::new(),
            dropped: 0,
            errors: Vec::new(),
            burst,
            extra: serde_json::Map::new(),
        })
    }

    /// 段配方：按 (计划相机下标, seq) 录一帧，文件名 `{相机}_s{seq:06}.pgm`。同一帧重复到达时保留先到的原图；
    /// 录制队满（BURST_QUEUE 帧）或写入线程停止时该帧原图不录，记进 rawDropped，不影响检测结论。
    pub fn burst_frame(&self, rec: &mut Recording, frame: &Frame, camera: usize, seq: u64) {
        let Some(burst) = rec.burst.as_mut() else {
            rec.errors.push("旧拍照点配方的录制不能按相机帧录制".into());
            return;
        };
        let Some(camera_id) = burst.cameras.get(camera).filter(|id| valid_camera_id(id)).cloned() else {
            rec.errors.push(format!("录制相机下标 {camera} 不在原配方的计划相机中"));
            return;
        };
        let key = FrameKey { camera, seq };
        if seq == 0 {
            rec.errors.push(format!("相机 {camera_id} 的帧组序号为 0，不录制"));
            return;
        }
        if !burst.seen.insert(key) {
            rec.errors.push(format!("相机 {camera_id} 第 {seq} 帧重复录制，拒绝覆盖原图"));
            return;
        }
        let Some(image) = frame.single_image() else {
            rec.errors.push(format!("相机 {camera_id} 第 {seq} 帧原图不完整：实际 {} 幅图像", frame.images.len()));
            return;
        };
        let mut meta = BurstFrameMeta {
            cycle_id: rec.cycle_id.clone(),
            bundle_id: rec.bundle_id.clone(),
            recipe_revision: rec.recipe.revision_id.clone(),
            session: frame.session,
            camera,
            cam: frame.cam,
            seq,
            file: format!("{camera_id}_s{seq:06}.pgm"),
            camera_id,
            ts: frame.ts,
            counter: frame.counter,
            frame_counter: frame.frame_counter,
            trigger_counter: frame.trigger_counter,
            lost_packets: frame.lost_packets,
            manual: frame.manual,
            width: image.width,
            height: image.height,
            queued: true,
            available: false,
            error: None,
        };
        let queue_error = if self.queued.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < BURST_QUEUE).then_some(n + 1)).is_err() {
            Some(format!("录制队列已满（{BURST_QUEUE} 帧）"))
        } else {
            let job = BurstFrameJob { cycle_id: rec.cycle_id.clone(), pending: rec.dir.clone(), meta: meta.clone(), image: image.clone() };
            if self.tx.send(Msg::BurstFrame(job)).is_err() {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                Some("录制写入线程已停止".to_string())
            } else {
                None
            }
        };
        if let Some(error) = queue_error {
            meta.queued = false;
            meta.error = Some(format!("{error}，本帧原图未录"));
            burst.drop_reason.get_or_insert(error);
            burst.dropped.push(key);
        }
        burst.frames.push(meta);
    }

    pub fn frame(&self, rec: &mut Recording, frame: &Frame, camera: &str, k: usize, ordinal: u64) {
        let Some(shot) = rec.recipe.shots.get(k) else {
            rec.errors.push(format!("录制拍照点 k={k} 不在原配方中"));
            return;
        };
        if !valid_camera_id(camera) || !valid_camera_id(&shot.id) || camera != shot.camera {
            rec.errors.push(format!("录制拍照点 {} 的相机或编号与原配方不一致", shot.id));
            return;
        }
        if !rec.seen.insert(k) {
            rec.errors.push(format!("拍照点 {} 重复录制，拒绝覆盖原图", shot.id));
            return;
        }
        if !matches!(frame.images.len(), 1 | 3) || frame.image(shot.view).is_none() {
            rec.errors.push(format!("拍照点 {} 原图不完整：实际 {} 个视角，所选视角 {}", shot.id, frame.images.len(), shot.view));
            return;
        }
        let mut metadata: Vec<_> = frame
            .images
            .iter()
            .enumerate()
            .map(|(i, image)| {
                let view = i as u8 + 1;
                FrameMeta {
                    cycle_id: rec.cycle_id.clone(),
                    bundle_id: rec.bundle_id.clone(),
                    recipe_revision: rec.recipe.revision_id.clone(),
                    session: frame.session,
                    ordinal,
                    k,
                    shot_id: shot.id.clone(),
                    selected_view: shot.view,
                    view,
                    cam: frame.cam,
                    camera: camera.into(),
                    seq: ordinal,
                    file: format!("k{k:03}_{}_{camera}_v{view}.pgm", shot.id),
                    ts: frame.ts,
                    counter: frame.counter,
                    frame_counter: frame.frame_counter,
                    trigger_counter: frame.trigger_counter,
                    lost_packets: frame.lost_packets,
                    manual: frame.manual,
                    width: image.width,
                    height: image.height,
                    queued: true,
                    available: false,
                    error: None,
                }
            })
            .collect();
        let queue_error = if self.queued.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < QUEUE).then_some(n + 1)).is_err() {
            Some(format!("录制队列已满（{QUEUE} 组），拍照点 {} 的 {} 个视角整组未入队", shot.id, frame.images.len()))
        } else {
            let images = metadata.iter().cloned().zip(frame.images.iter().cloned()).collect();
            if self.tx.send(Msg::Frames(FrameGroup { cycle_id: rec.cycle_id.clone(), pending: rec.dir.clone(), images })).is_err() {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                Some(format!("录制写入线程已停止，拍照点 {} 整组未入队", shot.id))
            } else {
                None
            }
        };
        if let Some(error) = queue_error {
            rec.dropped += 1;
            rec.errors.push(error.clone());
            for meta in &mut metadata {
                meta.queued = false;
                meta.error = Some(error.clone());
            }
        }
        rec.frames.extend(metadata);
    }

    pub fn finish(&self, rec: Recording, verdict: Verdict, reason: &str, keep: u32, max_bytes: u64, in_use: Vec<PathBuf>) {
        let failed = !matches!(verdict, Verdict::Ok | Verdict::OkWithExcursion);
        let keep_this = rec.mode == RecordMode::All || failed;
        let tag = serde_json::to_value(verdict).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let target = keep_this.then(|| self.root.join(&rec.name[..8]).join(format!("{}_{tag}_cycle_{}", rec.name, rec.cycle_id)));
        let job = FinishJob { recording: rec, target, verdict, reason: reason.into(), keep, max_bytes, in_use };
        if let Err(error) = self.tx.send(Msg::Finish(job)) {
            if let Msg::Finish(mut job) = error.0 {
                job.recording.errors.push("录制写入线程已停止，收尾改为同步保存并报告".into());
                let pending = job.recording.dir.clone();
                let mut state = WriteState::default();
                protect_retention(&mut job, &mut state, self.cleanup.as_ref().map(|c| &c.protection));
                let outcome = finish_recording(&self.root, job, state, self.cleanup.as_ref().map(|c| &c.release));
                if !pending.exists() { self.active.lock().unwrap().remove(&pending); }
                notify(&self.callback, outcome);
            }
        }
    }
}

const WRITE_FAILURES: &[&str] = &[
    "创建录制目录失败", "创建原图失败", "写入原图失败", "创建 part.json 失败", "写入 part.json 失败", "提交 part.json 失败",
    "创建收尾目录失败", "录制目录收尾失败", "录制目标目录已存在", "NG-only 合格件临时录制清理失败",
    "录制线程启动失败", "录制写入线程已停止",
    "原图未取得写入成功确认", "已写入原图不可用", "原图路径不能是 reparse point", "原图路径不是普通文件或文件长度与录制尺寸不符",
    "原图解码失败", "原图解码尺寸与录制尺寸不符",
];

pub fn is_write_error(error: &str) -> bool {
    WRITE_FAILURES.iter().any(|marker| error.contains(marker))
}

fn safe_cycle_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn notify(callback: &Option<RecordingCallback>, outcome: RecordingOutcome) {
    let fields = crate::applog::Fields { cycle_id: Some(outcome.cycle_id.clone()), stage: Some("recording".into()), ..Default::default() };
    if let Some(callback) = callback {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(outcome))).is_err() {
            crate::applog::emit(crate::applog::Level::Error, "录制", "录制结果回调发生异常", fields);
        }
    } else if !outcome.errors.is_empty() {
        crate::applog::emit(crate::applog::Level::Error, "录制", &format!("{} 失败：{}", outcome.cycle_id, outcome.errors.join("；")), fields);
    }
}

#[derive(Default)]
struct WriteState {
    results: HashMap<(usize, u8), Result<(), String>>,
    /// 段配方逐帧写盘结果；失败原因只在这里，收尾时汇总，不逐帧进 errors
    frame_results: HashMap<FrameKey, Result<(), String>>,
    errors: Vec<String>,
    retention_errors: Vec<String>,
    retention_blocked: bool,
}

fn protect_retention(job: &mut FinishJob, state: &mut WriteState, protection: Option<&PendingProtection>) {
    if let Some(protection) = protection {
        match protection() {
            Ok(paths) => job.in_use.extend(paths),
            Err(error) => {
                state.retention_blocked = true;
                state.retention_errors.push(format!("无法确认待入库原图保护，跳过滚动清理：{error}"));
            }
        }
    }
}

fn writer(root: PathBuf, rx: Receiver<Msg>, queued: Arc<AtomicUsize>, callback: Option<RecordingCallback>,
    active: Arc<Mutex<HashSet<PathBuf>>>, cleanup: Option<Cleanup>) {
    let mut cleaner = PendingCleaner::default();
    if let Some(cleanup) = &cleanup {
        let errors = cleaner.sweep(&root, &active, &cleanup.protection, &[]);
        if !errors.is_empty() { (cleanup.warning)(&errors); }
    }
    let mut cycles: HashMap<String, WriteState> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Frames(group) => {
                let state = cycles.entry(group.cycle_id).or_default();
                for (meta, image) in group.images {
                    let result = save_raw(&group.pending.join(&meta.file), &image).map_err(|error| format!("{}：{error}", meta.file));
                    if let Err(error) = &result {
                        state.errors.push(error.clone());
                    }
                    state.results.insert((meta.k, meta.view), result);
                }
                queued.fetch_sub(1, Ordering::AcqRel);
            }
            Msg::BurstFrame(job) => {
                let state = cycles.entry(job.cycle_id).or_default();
                let result = save_raw(&job.pending.join(&job.meta.file), &job.image);
                state.frame_results.insert(FrameKey { camera: job.meta.camera, seq: job.meta.seq }, result);
                queued.fetch_sub(1, Ordering::AcqRel);
            }
            Msg::Finish(mut job) => {
                let mut state = cycles.remove(&job.recording.cycle_id).unwrap_or_default();
                if let Some(cleanup) = &cleanup {
                    state.retention_errors = cleaner.sweep(&root, &active, &cleanup.protection, &job.in_use);
                }
                protect_retention(&mut job, &mut state, cleanup.as_ref().map(|c| &c.protection));
                let pending = job.recording.dir.clone();
                let outcome = finish_recording(&root, job, state, cleanup.as_ref().map(|c| &c.release));
                if !pending.exists() { active.lock().unwrap().remove(&pending); }
                notify(&callback, outcome);
            }
            Msg::Prune { keep, max_bytes } => {
                let errors = match cleanup.as_ref().map(|c| (c.protection)()).transpose() {
                    Ok(protected) => prune(&root, keep as usize, max_bytes, &protected.unwrap_or_default(), cleanup.as_ref().map(|c| &c.release)),
                    Err(error) => vec![format!("无法确认待入库原图保护，跳过滚动清理：{error}")],
                };
                if !errors.is_empty() {
                    match &cleanup {
                        Some(cleanup) => (cleanup.warning)(&errors),
                        None => crate::applog::write("warn", "原图滚动清理失败", &errors.join("；")),
                    }
                }
            }
        }
    }
}

fn save_raw(path: &Path, image: &FrameImage) -> Result<(), String> {
    let expected = (image.width as usize).checked_mul(image.height as usize).filter(|n| *n > 0).ok_or("图像尺寸无效")?;
    if image.pixels.len() != expected {
        return Err("原图像素缓冲长度与尺寸不一致".into());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建录制目录失败：{e}"))?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("创建原图失败：{e}"))?;
    let header = format!("P5\n{} {}\n255\n", image.width, image.height);
    file.write_all(header.as_bytes()).and_then(|_| file.write_all(&image.pixels)).and_then(|_| file.sync_all()).map_err(|e| format!("写入原图失败：{e}"))?;
    Ok(())
}

fn write_metadata(path: &Path, meta: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| format!("创建 part.json 失败：{e}"))?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| format!("写入 part.json 失败：{e}"))?;
    drop(file);
    std::fs::rename(temp, path).map_err(|e| format!("提交 part.json 失败：{e}"))
}

/// 写盘后核对一张原图：是普通文件（不是 reparse point）、长度与录制尺寸相符、能解码且解码尺寸一致。
fn verify_raw(path: &Path, width: u32, height: u32) -> Result<(), String> {
    std::fs::symlink_metadata(path).map_err(|e| format!("已写入原图不可用：{e}"))
        .and_then(|meta| {
            #[cfg(windows)]
            { use std::os::windows::fs::MetadataExt;
              if meta.file_attributes() & 0x400 != 0 { return Err("原图路径不能是 reparse point".into()); } }
            let header = format!("P5\n{width} {height}\n255\n");
            let expected = header.len() as u64 + u64::from(width) * u64::from(height);
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != expected {
                Err("原图路径不是普通文件或文件长度与录制尺寸不符".into())
            } else {
                let image = image::open(path).map_err(|error| format!("原图解码失败：{error}"))?;
                if image.width() != width || image.height() != height {
                    return Err("原图解码尺寸与录制尺寸不符".into());
                }
                Ok(())
            }
        })
}

/// 段配方的 part.json：缺帧（1..=最大 seq 内某台相机没到的帧，即相机没出图）列进 missingFrames，只供追溯、不算录制错误；
/// 录制队满没录的帧列进 rawDropped，记一条录制错误（证据 Incomplete，不影响检测结论）。
fn burst_metadata(rec: &mut Recording, burst: &BurstFrames, verdict: Verdict, reason: &str) -> serde_json::Value {
    let groups = burst.seen.iter().map(|key| key.seq).max().unwrap_or(0);
    let missing: Vec<FrameKey> = (1..=groups)
        .flat_map(|seq| (0..burst.cameras.len()).map(move |camera| FrameKey { camera, seq }))
        .filter(|key| !burst.seen.contains(key))
        .collect();
    if let Some(drop_reason) = &burst.drop_reason {
        rec.errors.push(format!("{drop_reason}：{} 帧原图未录，见 part.json 的 rawDropped", burst.dropped.len()));
    }
    json!({
        "cycleId": rec.cycle_id, "bundleId": rec.bundle_id, "recipeRevision": rec.recipe.revision_id,
        "startedTs": rec.started, "sn": rec.sn, "mode": rec.mode, "verdict": verdict, "reason": reason,
        "recipe": &*rec.recipe, "cameras": burst.cameras, "groupCount": groups,
        "missingFrames": missing, "rawDropped": burst.dropped, "droppedFrames": burst.dropped.len(),
        "frames": burst.frames, "available": false, "errors": rec.errors,
    })
}

/// 段配方逐帧核对写盘结果，写成的帧按 (camera, seq) 列进录制结果。没入队的帧已在 rawDropped 里汇总；
/// 写盘失败的帧只把前 BURST_FRAME_ERRORS 条写进录制错误，其余汇总成一条，逐帧原因留在 part.json 的 frames。
fn burst_files(root: &Path, actual_dir: &Path, burst: &mut BurstFrames, results: &mut HashMap<FrameKey, Result<(), String>>,
    files: &mut Vec<RecordedRawFile>, errors: &mut Vec<String>) {
    let mut failures = 0usize;
    for frame in burst.frames.iter_mut().filter(|frame| frame.queued) {
        let key = FrameKey { camera: frame.camera, seq: frame.seq };
        let path = actual_dir.join(&frame.file);
        let result = match results.remove(&key) {
            Some(Ok(())) => verify_raw(&path, frame.width, frame.height).and_then(|()| {
                path.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).map_err(|error| format!("原图路径不在录制根目录：{error}"))
            }),
            Some(Err(error)) => Err(error),
            None => Err("原图未取得写入成功确认".into()),
        };
        match result {
            Ok(file) => {
                frame.available = true;
                files.push(RecordedRawFile { k: 0, view: 0, file, width: frame.width, height: frame.height, frame: Some(key) });
            }
            Err(error) => {
                failures += 1;
                if failures <= BURST_FRAME_ERRORS {
                    errors.push(format!("{}：{error}", frame.file));
                }
                frame.error = Some(error);
            }
        }
    }
    if failures > BURST_FRAME_ERRORS {
        errors.push(format!("另有 {} 帧原图未确认写入，见 part.json 的 frames", failures - BURST_FRAME_ERRORS));
    }
}

fn finish_recording(root: &Path, job: FinishJob, mut state: WriteState, release: Option<&RecordingRelease>) -> RecordingOutcome {
    let FinishJob { recording: mut rec, target, verdict, reason, keep, max_bytes, in_use } = job;
    rec.errors.append(&mut state.errors);
    if target.is_none() {
        if rec.dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&rec.dir) {
                rec.errors.push(format!("NG-only 合格件临时录制清理失败：{error}"));
            }
        }
        return RecordingOutcome {
            cycle_id: rec.cycle_id,
            directory: rec.dir.exists().then_some(rec.dir),
            files: Vec::new(),
            errors: rec.errors,
            retention_errors: state.retention_errors,
            available: false,
            state: RecordingState::NotRetained,
        };
    }
    let mut burst = rec.burst.take();
    let mut meta = match burst.as_ref() {
        Some(burst) => burst_metadata(&mut rec, burst, verdict, &reason),
        None => {
            let planned: Vec<_> =
                rec.recipe.shots.iter().enumerate().map(|(k, shot)| json!({"k": k, "shotId": shot.id, "camera": shot.camera, "selectedView": shot.view})).collect();
            let missing: Vec<_> = rec
                .recipe
                .shots
                .iter()
                .enumerate()
                .filter(|(k, _)| !rec.frames.iter().any(|frame| frame.k == *k))
                .map(|(k, shot)| json!({"k": k, "shotId": shot.id}))
                .collect();
            if !missing.is_empty() {
                rec.errors.push(format!("{} 个计划拍照点没有录制原图", missing.len()));
            }
            json!({
                "cycleId": rec.cycle_id, "bundleId": rec.bundle_id, "recipeRevision": rec.recipe.revision_id,
                "startedTs": rec.started, "sn": rec.sn, "mode": rec.mode, "verdict": verdict, "reason": reason,
                "recipe": &*rec.recipe, "plannedShots": planned, "missingShots": missing,
                "frames": rec.frames, "droppedFrames": rec.dropped, "available": false, "errors": rec.errors,
            })
        }
    };
    for (key, value) in &rec.extra {
        meta[key] = value.clone();
    }
    match std::fs::create_dir_all(&rec.dir) {
        Ok(()) => {
            if rec.keep() {
                if let Err(error) = std::fs::write(rec.dir.join(KEEP_MARKER), b"") {
                    rec.errors.push(format!("写入维护采集保留标记失败：{error}"));
                }
            }
            if let Err(error) = write_metadata(&rec.dir.join("part.json"), &meta) {
                rec.errors.push(error);
            }
        }
        Err(error) => rec.errors.push(format!("创建收尾目录失败：{error}")),
    }
    let target = target.unwrap();
    let promoted = if target.exists() {
        rec.errors.push(format!("录制目标目录已存在，拒绝覆盖：{}", target.display()));
        false
    } else {
        let result = target.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::rename(&rec.dir, &target));
        match result {
            Ok(()) => true,
            Err(error) => {
                rec.errors.push(format!("录制目录收尾失败：{error}"));
                false
            }
        }
    };
    let actual_dir = if promoted { target } else { rec.dir.clone() };
    let directory = actual_dir.is_dir().then_some(actual_dir.clone());
    let mut files = Vec::new();
    if let Some(burst) = burst.as_mut() {
        burst_files(root, &actual_dir, burst, &mut state.frame_results, &mut files, &mut rec.errors);
    }
    for frame in &mut rec.frames {
        let result = match state.results.remove(&(frame.k, frame.view)) {
            Some(Ok(())) => verify_raw(&actual_dir.join(&frame.file), frame.width, frame.height),
            Some(Err(error)) => Err(error),
            None => Err(frame.error.clone().unwrap_or_else(|| "原图未取得写入成功确认".into())),
        };
        match result {
            Ok(()) => {
                let path = actual_dir.join(&frame.file);
                let relative = path.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/"));
                match relative {
                    Ok(file) => {
                        frame.available = true;
                        files.push(RecordedRawFile { k: frame.k, view: frame.view, file, width: frame.width, height: frame.height, frame: None });
                    }
                    Err(error) => {
                        frame.error = Some(format!("原图路径不在录制根目录：{error}"));
                    }
                }
            }
            Err(error) => {
                frame.error = Some(error);
            }
        }
        if let Some(error) = &frame.error {
            let error = format!("{}：{error}", frame.file);
            if !rec.errors.contains(&error) {
                rec.errors.push(error);
            }
        }
    }
    if files.is_empty() {
        rec.errors.push("本件没有已确认写入的原图".into());
    }
    let mut protected = in_use;
    protected.push(actual_dir.clone());
    let mut retention_errors = state.retention_errors;
    if !state.retention_blocked { retention_errors.extend(prune(root, keep as usize, max_bytes, &protected, release)); }
    let mut available = promoted && directory.is_some() && !files.is_empty() && rec.errors.is_empty();
    meta["frames"] = match &burst {
        Some(burst) => serde_json::to_value(&burst.frames).unwrap(),
        None => serde_json::to_value(&rec.frames).unwrap(),
    };
    meta["available"] = json!(available);
    meta["errors"] = json!(rec.errors);
    meta["retentionErrors"] = json!(retention_errors);
    if directory.is_some() {
        if let Err(error) = write_metadata(&actual_dir.join("part.json"), &meta) {
            rec.errors.push(error);
            available = false;
        }
    }
    let outcome_state = if available {
        RecordingState::Complete
    } else if files.is_empty() {
        RecordingState::Failed
    } else {
        RecordingState::Incomplete
    };
    RecordingOutcome { cycle_id: rec.cycle_id, directory, files, errors: rec.errors, retention_errors, available, state: outcome_state }
}

struct PendingClaim<'a> {
    active: &'a Mutex<HashSet<PathBuf>>,
    path: PathBuf,
}

impl<'a> PendingClaim<'a> {
    fn acquire(active: &'a Mutex<HashSet<PathBuf>>, path: &Path) -> Option<Self> {
        active.lock().unwrap().insert(path.to_path_buf()).then(|| Self { active, path: path.to_path_buf() })
    }
}

impl Drop for PendingClaim<'_> {
    fn drop(&mut self) { self.active.lock().unwrap().remove(&self.path); }
}

#[derive(Default)]
struct PendingCleaner {
    entries: Option<std::fs::ReadDir>,
}

fn pending_name(name: &str) -> bool {
    let Some((stamp, cycle)) = name.split_once("_cycle_") else { return false };
    stamp.len() == 19 && stamp.bytes().enumerate().all(|(i, b)| if i == 8 || i == 15 { b == b'_' } else { b.is_ascii_digit() })
        && safe_cycle_id(cycle)
}

fn pending_files_safe(path: &Path) -> Result<bool, String> {
    let entries = std::fs::read_dir(path).map_err(|e| format!("读取孤立录制失败 {}：{e}", path.display()))?;
    for (i, entry) in entries.enumerate() {
        if i == PENDING_FILE_LIMIT { return Ok(false); }
        let entry = entry.map_err(|e| e.to_string())?;
        let meta = entry.path().symlink_metadata().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 { return Ok(false); }
        }
        if !meta.is_file() || meta.file_type().is_symlink() { return Ok(false); }
    }
    Ok(true)
}

impl PendingCleaner {
    fn sweep(&mut self, root: &Path, active: &Mutex<HashSet<PathBuf>>, protection: &PendingProtection, in_use: &[PathBuf]) -> Vec<String> {
        let pending = root.join("_pending");
        if !pending.exists() { self.entries = None; return Vec::new(); }
        if !plain_directory(root) || !plain_directory(&pending) {
            self.entries = None;
            return vec!["孤立录制清理拒绝包含链接的 records/_pending 目录".into()];
        }
        let checked_root = match root.canonicalize() {
            Ok(path) => path,
            Err(error) => return vec![format!("录制根目录无法解析：{error}")],
        };
        let checked_pending = match pending.canonicalize() {
            Ok(path) if path.parent() == Some(checked_root.as_path()) => path,
            _ => return vec!["孤立录制清理路径越过 records 根目录".into()],
        };
        if self.entries.is_none() {
            match std::fs::read_dir(&pending) {
                Ok(entries) => self.entries = Some(entries),
                Err(error) => return vec![format!("扫描孤立录制失败：{error}")],
            }
        }
        let mut errors = Vec::new();
        let mut candidates = Vec::new();
        for _ in 0..PENDING_SWEEP_LIMIT {
            let Some(entry) = self.entries.as_mut().unwrap().next() else { self.entries = None; break };
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => { errors.push(format!("扫描孤立录制目录项失败：{error}")); continue; },
            };
            if !path.file_name().and_then(|n| n.to_str()).is_some_and(pending_name) || !plain_directory(&path) { continue; }
            if !active.lock().unwrap().contains(&path) { candidates.push(path); }
        }
        if candidates.is_empty() { return errors; }
        let mut protected = match protection() {
            Ok(paths) => paths,
            Err(error) => { self.entries = None; errors.push(format!("读取录制历史引用失败，暂停孤立录制清理：{error}")); return errors; },
        };
        protected.extend_from_slice(in_use);
        let mut canonical_protected = Vec::new();
        for path in protected {
            match path.canonicalize() {
                Ok(path) => canonical_protected.push(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return vec![format!("录制引用路径无法解析，暂停孤立录制清理 {}：{error}", path.display())],
            }
        }
        for path in candidates {
            let Some(_claim) = PendingClaim::acquire(active, &path) else { continue };
            let checked = match path.canonicalize() {
                Ok(checked) if checked.parent() == Some(checked_pending.as_path()) => checked,
                _ => { errors.push(format!("孤立录制路径无效，保留 {}", path.display())); continue; },
            };
            if canonical_protected.iter().any(|p| p.starts_with(&checked) || checked.starts_with(p)) { continue; }
            match pending_files_safe(&path) {
                Ok(true) => {
                    if let Err(error) = std::fs::remove_dir_all(&path) {
                        errors.push(format!("清理孤立录制失败 {}：{error}", path.display()));
                    }
                },
                Ok(false) => errors.push(format!("孤立录制包含链接、子目录或过多文件，保留 {}", path.display())),
                Err(error) => errors.push(error),
            }
        }
        errors
    }
}

fn dir_bytes(dir: &Path) -> Result<u64, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取录制大小失败 {}：{e}", dir.display()))?;
    let mut size = 0u64;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_file() {
            size = size.saturating_add(meta.len());
        }
    }
    Ok(size)
}

fn prune(root: &Path, keep: usize, max_bytes: u64, in_use: &[PathBuf], release: Option<&RecordingRelease>) -> Vec<String> {
    let mut errors = Vec::new();
    let mut protected = Vec::new();
    for path in in_use {
        match path.canonicalize() {
            Ok(path) => protected.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return vec![format!("保留目录无法解析，暂停清理录制 {}：{error}", path.display())],
        }
    }
    let days = match std::fs::read_dir(root) {
        Ok(days) => days,
        Err(error) => return vec![format!("扫描录制目录失败：{error}")],
    };
    let mut parts = Vec::new();
    for day in days {
        let day = match day {
            Ok(day) => day.path(),
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        let name = day.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if name.len() != 8 || !name.bytes().all(|b| b.is_ascii_digit()) || !plain_directory(&day) {
            continue;
        }
        match std::fs::read_dir(&day) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) if plain_directory(&entry.path()) && !entry.path().join(KEEP_MARKER).exists() => parts.push(entry.path()),
                        Ok(_) => {}
                        Err(error) => errors.push(error.to_string()),
                    }
                }
            }
            Err(error) => errors.push(format!("扫描每日录制目录失败 {}：{error}", day.display())),
        }
    }
    parts.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    let mut total = 0u64;
    let mut victims = Vec::new();
    for (i, path) in parts.iter().enumerate() {
        match dir_bytes(path) {
            Ok(bytes) => total = total.saturating_add(bytes),
            Err(error) => {
                errors.push(error);
                continue;
            }
        }
        if i == 0 || (i < keep && total <= max_bytes) {
            continue;
        }
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                errors.push(format!("旧录制路径无法解析，保留 {}：{error}", path.display()));
                continue;
            }
        };
        if !protected.contains(&canonical) {
            victims.push(path.clone());
        }
    }
    if victims.is_empty() {
        return errors;
    }
    if let Some(release) = release {
        if let Err(error) = release(&victims) {
            errors.push(format!("历史记录的原图引用未能同步，本次不清理 {} 件旧录制：{error}", victims.len()));
            return errors;
        }
    }
    for path in victims {
        if let Err(error) = std::fs::remove_dir_all(&path) {
            errors.push(format!("清理旧录制失败 {}：{error}", path.display()));
        }
    }
    errors
}

fn plain_directory(path: &Path) -> bool {
    let Ok(meta) = path.symlink_metadata() else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    meta.is_dir() && !meta.file_type().is_symlink()
}

#[cfg(test)]
#[path = "recorder/tests.rs"]
mod tests;

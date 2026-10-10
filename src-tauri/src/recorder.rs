use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use chrono::Local;
use serde::Serialize;
use serde_json::json;

use crate::frame::{CounterSource, Frame, FrameImage};
use crate::judge::Verdict;
use crate::recipe::{valid_camera_id, Recipe};
use crate::settings::RecordMode;

const QUEUE: usize = 48;

pub type RecordingCallback = Arc<dyn Fn(RecordingOutcome) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    Off,
    NotRetained,
    Complete,
    Incomplete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedRawFile {
    pub k: usize,
    pub view: u8,
    pub file: String,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingOutcome {
    pub cycle_id: String,
    pub directory: Option<PathBuf>,
    pub files: Vec<RecordedRawFile>,
    pub errors: Vec<String>,
    pub available: bool,
    pub state: RecordingState,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameMeta {
    cycle_id: String,
    bundle_hash: Option<String>,
    recipe_hash: String,
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
    hash: Option<String>,
    error: Option<String>,
}

struct FrameGroup {
    cycle_id: String,
    pending: PathBuf,
    images: Vec<(FrameMeta, Arc<FrameImage>)>,
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
    Finish(FinishJob),
}

pub struct Recording {
    started: i64,
    dir: PathBuf,
    name: String,
    cycle_id: String,
    bundle_hash: Option<String>,
    sn: u32,
    recipe: Arc<Recipe>,
    mode: RecordMode,
    frames: Vec<FrameMeta>,
    seen: HashSet<usize>,
    dropped: u32,
    errors: Vec<String>,
}

pub struct Recorder {
    root: PathBuf,
    tx: Sender<Msg>,
    queued: Arc<AtomicUsize>,
    callback: Option<RecordingCallback>,
    startup_error: Option<String>,
}

impl Recorder {
    pub fn new(root: PathBuf, callback: Option<RecordingCallback>) -> Self {
        let (tx, rx) = channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let (writer_root, writer_queue, writer_callback) = (root.clone(), queued.clone(), callback.clone());
        let startup_error = std::thread::Builder::new()
            .name("frame-recorder".into())
            .spawn(move || writer(writer_root, rx, writer_queue, writer_callback))
            .err()
            .map(|e| format!("录制线程启动失败：{e}"));
        Self { root, tx, queued, callback, startup_error }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn begin(&self, mode: RecordMode, sn: u32, recipe: Arc<Recipe>, cycle_id: &str, bundle_hash: Option<&str>) -> Option<Recording> {
        if mode == RecordMode::Off {
            notify(
                &self.callback,
                RecordingOutcome {
                    cycle_id: cycle_id.into(),
                    directory: None,
                    files: Vec::new(),
                    errors: Vec::new(),
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
                    available: false,
                    state: RecordingState::Failed,
                },
            );
            return None;
        }
        let name = Local::now().format("%Y%m%d_%H%M%S_%3f").to_string();
        Some(Recording {
            started: ly_plc::now_ms(),
            dir: self.root.join("_pending").join(format!("{name}_cycle_{cycle_id}")),
            name,
            cycle_id: cycle_id.into(),
            bundle_hash: bundle_hash.map(str::to_string),
            sn,
            recipe,
            mode,
            frames: Vec::new(),
            seen: HashSet::new(),
            dropped: 0,
            errors: Vec::new(),
        })
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
                    bundle_hash: rec.bundle_hash.clone(),
                    recipe_hash: rec.recipe.hash.clone(),
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
                    hash: None,
                    error: None,
                }
            })
            .collect();
        let queue_error = if self.queued.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < QUEUE).then_some(n + 1)).is_err() {
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
                notify(&self.callback, finish_recording(&self.root, job, WriteState::default()));
            }
        }
    }
}

fn safe_cycle_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn notify(callback: &Option<RecordingCallback>, outcome: RecordingOutcome) {
    if let Some(callback) = callback {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(outcome))).is_err() {
            eprintln!("录制结果回调发生异常");
        }
    } else if !outcome.errors.is_empty() {
        eprintln!("录制 {} 失败：{}", outcome.cycle_id, outcome.errors.join("；"));
    }
}

#[derive(Default)]
struct WriteState {
    results: HashMap<(usize, u8), Result<String, String>>,
    errors: Vec<String>,
}

fn writer(root: PathBuf, rx: Receiver<Msg>, queued: Arc<AtomicUsize>, callback: Option<RecordingCallback>) {
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
            Msg::Finish(job) => {
                let state = cycles.remove(&job.recording.cycle_id).unwrap_or_default();
                notify(&callback, finish_recording(&root, job, state));
            }
        }
    }
}

fn save_raw(path: &Path, image: &FrameImage) -> Result<String, String> {
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
    let expected_hash = hash_bytes(header.bytes().chain(image.pixels.iter().copied()));
    let actual = file_hash(path)?;
    if actual != expected_hash {
        return Err("原图落盘校验失败".into());
    }
    Ok(actual)
}

fn hash_bytes(bytes: impl Iterator<Item = u8>) -> String {
    let hash = bytes.fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x100000001b3));
    format!("fnv1a64:{hash:016x}")
}

fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取已写原图失败：{e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("原图路径不是普通文件".into());
    }
    let mut hash = 0xcbf29ce484222325u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| format!("校验原图失败：{e}"))?;
        if n == 0 {
            break;
        }
        for byte in &buffer[..n] {
            hash = (hash ^ *byte as u64).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}

fn write_metadata(path: &Path, meta: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| format!("创建 part.json 失败：{e}"))?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| format!("写入 part.json 失败：{e}"))?;
    drop(file);
    std::fs::rename(temp, path).map_err(|e| format!("提交 part.json 失败：{e}"))
}

fn finish_recording(root: &Path, job: FinishJob, mut state: WriteState) -> RecordingOutcome {
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
            available: false,
            state: RecordingState::NotRetained,
        };
    }
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
    let mut meta = json!({
        "cycleId": rec.cycle_id, "bundleHash": rec.bundle_hash, "recipeHash": rec.recipe.hash,
        "startedTs": rec.started, "sn": rec.sn, "mode": rec.mode, "verdict": verdict, "reason": reason,
        "recipe": &*rec.recipe, "plannedShots": planned, "missingShots": missing,
        "frames": rec.frames, "droppedFrames": rec.dropped, "available": false, "errors": rec.errors,
    });
    match std::fs::create_dir_all(&rec.dir) {
        Ok(()) => {
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
    for frame in &mut rec.frames {
        let result = match state.results.remove(&(frame.k, frame.view)) {
            Some(Ok(expected)) => {
                file_hash(&actual_dir.join(&frame.file)).and_then(|actual| if actual == expected { Ok(actual) } else { Err("收尾时原图哈希不一致".into()) })
            }
            Some(Err(error)) => Err(error),
            None => Err(frame.error.clone().unwrap_or_else(|| "原图未取得写入成功确认".into())),
        };
        match result {
            Ok(hash) => {
                let path = actual_dir.join(&frame.file);
                let relative = path.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/"));
                match relative {
                    Ok(file) => {
                        frame.available = true;
                        frame.hash = Some(hash.clone());
                        files.push(RecordedRawFile { k: frame.k, view: frame.view, file, hash });
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
        rec.errors.push("本件没有已校验的落盘原图".into());
    }
    let mut protected = in_use;
    protected.push(actual_dir.clone());
    rec.errors.extend(prune(root, keep as usize, max_bytes, &protected));
    let mut available = promoted && directory.is_some() && !files.is_empty() && rec.errors.is_empty();
    meta["frames"] = serde_json::to_value(&rec.frames).unwrap();
    meta["available"] = json!(available);
    meta["errors"] = json!(rec.errors);
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
    RecordingOutcome { cycle_id: rec.cycle_id, directory, files, errors: rec.errors, available, state: outcome_state }
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

fn prune(root: &Path, keep: usize, max_bytes: u64, in_use: &[PathBuf]) -> Vec<String> {
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
                        Ok(entry) if plain_directory(&entry.path()) => parts.push(entry.path()),
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
        if protected.contains(&canonical) {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(path) {
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

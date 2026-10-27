use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::Semaphore;

use crate::burst_judge::FrameState;
use crate::cycle::{CycleHost, Input};
use crate::frame::FrameImage;
use crate::recipe::Recipe;
use crate::release::{ReleaseBundle, ValidationSite, RELEASE_SCHEMA_BURST};
use crate::vision::{self, Engine, FrameMeasurement, VisionHost};

/// 段配方的测量图版本（每段每台检测相机一条走廊，D-SB-45）；发布包 schema 5 的 `versions.graph`。
pub const GRAPH_VERSION_BURST: &str = "corridor-2";
/// 用到逐帧定向（D-SB-47）或找零件边（D-SB-48）的配方的测量图版本：graph.json 多了定向图与找边节点。
pub const GRAPH_VERSION_BURST_HEADING: &str = "corridor-3";

/// 配方对应的测量图版本：都不用逐帧定向与找边时保持 `corridor-2`，旧发布包照常加载。
pub fn graph_version(recipe: &Recipe) -> &'static str {
    let edge = recipe
        .path_segments
        .iter()
        .any(|s| s.offset_ref.as_ref().is_some_and(|o| o.edge.is_some()));
    if edge || recipe.path_segments.iter().any(|s| s.heading.is_some()) {
        GRAPH_VERSION_BURST_HEADING
    } else {
        GRAPH_VERSION_BURST
    }
}

/// 段配方的测量图（发布包 graph.json）：每段每台检测相机一张走廊测量图，开启定向的（段, 相机）另有搜索图与回退图；
/// sizes 是各相机示教帧尺寸。
pub fn burst_graphs(recipe: &Recipe, sizes: &[[u32; 2]]) -> Result<Value, String> {
    if sizes.len() != recipe.inspect.len() {
        return Err(format!("段配方有 {} 台检测相机，示教帧尺寸却有 {} 份", recipe.inspect.len(), sizes.len()));
    }
    let set = vision::CorridorSet::build(recipe, sizes)?;
    let graphs: Vec<Value> = set.iter()
        .map(|c| json!({"index": c.camera_idx, "camera": c.camera, "segment": c.segment, "size": c.size, "graph": c.graph}))
        .collect();
    let mut doc = json!({"schemaVersion": 4, "version": graph_version(recipe), "graphs": graphs});
    let headings: Vec<Value> = set.headings()
        .map(|h| json!({"index": h.camera_idx, "camera": h.camera, "segment": h.segment, "size": h.size, "graph": h.graph, "fallback": h.fallback}))
        .collect();
    if !headings.is_empty() {
        doc["headings"] = json!(headings);
    }
    Ok(doc)
}

/// 同一 主.次 系列的引擎兼容运行：发布时 1.1.0 的版本可以在 1.1.x 上生产。
fn engine_version_matches(published: &str, loaded: &str) -> bool {
    let published = vision::series_of(published.split(':').next().unwrap_or_default());
    published.is_some() && published == vision::series_of(loaded)
}

/// 引擎与发布时不是同一份（同系列另一补丁版本，或同版本号但文件大小、修改时间变了）：照常生产，但要让人知道结果不是验证时那份引擎量的。
fn engine_file_note(published: &str, engine: &Engine) -> Option<String> {
    if published == engine.identity { return None; }
    let version = published.split(':').next().unwrap_or_default();
    Some(if version != engine.version {
        format!("发布时引擎 lyFlow {version}，当前 {}：同系列兼容运行，建议择机重新验证并发布", engine.version)
    } else {
        format!("当前核心库 {} 与发布时的文件不同（发布时 {published}，现在 {}）：同版本号但文件大小或修改时间变了，建议重新验证并发布",
            engine.path.display(), engine.identity)
    })
}

pub struct Prepared {
    pub bundle: ReleaseBundle,
    pub recipe: Arc<Recipe>,
    /// 核心库文件与发布时不同的提示（见 engine_file_note）
    pub engine_note: Option<String>,
    engine: Arc<Engine>,
    /// 按（计划相机下标, 段号）的走廊测量图（D-SB-45）
    corridors: vision::CorridorSet,
}

impl Prepared {
    /// 按段与检测相机建走廊测量图。发布包与所选生产版本的配方逐项一致、测量图版本与引擎系列匹配、
    /// 包内测量图等于按配方和各相机示教帧尺寸重算的结果、各相机发布标定的当量等于示教当量。
    pub fn load(
        bundle: ReleaseBundle,
        engine: Arc<Engine>,
        expected: &Recipe,
    ) -> Result<Self, String> {
        bundle.verify()?;
        if bundle.manifest.schema_version != RELEASE_SCHEMA_BURST {
            return Err(format!("段配方需要按段与相机组织的发布包（格式版本 {RELEASE_SCHEMA_BURST}），请重新示教、验证并发布"));
        }
        let recipe = Arc::new(bundle.recipe.build()?);
        if recipe.revision_id != expected.revision_id || recipe.id != expected.id || recipe.version != expected.version
            || recipe.product_code != expected.product_code || recipe.trigger_mode != expected.trigger_mode || recipe.schema_version != expected.schema_version
            || recipe.spacing != expected.spacing || recipe.detect != expected.detect || recipe.filter_window != expected.filter_window
            || recipe.teaching_id != expected.teaching_id || recipe.plan_version != expected.plan_version || recipe.camera_imaging != expected.camera_imaging
            || recipe.path_segments != expected.path_segments || recipe.inspect != expected.inspect || recipe.part_calib != expected.part_calib {
            return Err("发布包配方与所选生产版本不一致，请重新发布".into());
        }
        if bundle.manifest.versions.graph != graph_version(&recipe)
            || !engine_version_matches(&bundle.manifest.versions.engine, &engine.version)
        {
            return Err("发布包的算法图或引擎版本与当前引擎不一致，请重新验证并发布".into());
        }
        let mut sizes = Vec::with_capacity(recipe.inspect.len());
        for (idx, inspect) in recipe.inspect.iter().enumerate() {
            let resource = bundle.camera(idx)?;
            sizes.push(resource.size);
            let calibration: Value = serde_json::from_str(&crate::fsio::read_text(&resource.calibration).map_err(|e| e.to_string())?)
                .map_err(|e| format!("发布包标定无效：{e}"))?;
            let scale = calibration["mmPerPx"].as_f64().ok_or("发布包标定没有有效像素当量")?;
            if !scale.is_finite() || inspect.mm_per_px.is_none_or(|taught| (scale - taught).abs() > 1e-9) {
                return Err(format!("相机 {} 的发布标定与示教像素当量不一致", inspect.camera));
            }
        }
        let stored: Value = serde_json::from_str(
            &crate::fsio::read_text(&bundle.root.join(&bundle.manifest.graph))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("发布包算法图无效：{e}"))?;
        if stored != burst_graphs(&recipe, &sizes)? {
            return Err("发布包算法图与配方走廊不一致".into());
        }
        let corridors = vision::CorridorSet::build(&recipe, &sizes)?;
        let engine_note = engine_file_note(&bundle.manifest.versions.engine, &engine);
        Ok(Self {
            bundle,
            recipe,
            engine_note,
            engine,
            corridors,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        self.bundle.verify()?;
        Ok(())
    }

    /// 段配方第 camera_idx 台计划相机、用第 segment 段走廊量一帧（seq 为帧组序号）：图像尺寸必须与这台相机的发布示教帧相同。
    pub fn measure_frame(
        &self,
        camera_idx: usize,
        segment: u32,
        seq: u64,
        image: &FrameImage,
        run_id: &str,
    ) -> Result<FrameMeasurement, String> {
        let corridor = self.corridors.get(camera_idx, segment).ok_or_else(|| {
            format!(
                "发布包没有第 {segment} 段、第 {} 台计划相机的走廊测量图",
                camera_idx + 1
            )
        })?;
        vision::measure_frame(&self.engine, corridor, image, seq, run_id)
    }

    /// 预热：每台相机在它每一段的发布示教帧上用该段走廊量一次，必须测成（Measured）——现场证明发布包、走廊与运行库接线正确。
    pub(crate) fn warm(&self) -> Result<(), String> {
        for idx in 0..self.recipe.inspect.len() {
            let resource = self.bundle.camera(idx)?;
            for corridor in &resource.corridors {
                let image = crate::image_file::load(&corridor.image)?;
                let m = self.measure_frame(
                    idx,
                    corridor.segment,
                    0,
                    &image,
                    &format!(
                        "warm-{}-c{idx}-s{}-{}",
                        self.bundle.id,
                        corridor.segment,
                        ly_plc::now_ms()
                    ),
                )?;
                if m.metrics.state != FrameState::Measured {
                    return Err(format!(
                        "相机 {} 第 {} 段：发布包走廊在自己的示教帧上没有测成（{}），不能布防",
                        resource.camera,
                        corridor.segment,
                        m.metrics.error.as_deref().unwrap_or("状态异常")
                    ));
                }
            }
        }
        self.verify()
    }
}

const WARMUP_RETRY_DELAY: Duration = Duration::from_secs(1);

enum Entry {
    Preparing,
    Ready(Arc<Prepared>),
    Retryable { error: String, retry_at: Instant },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WarmupFailure {
    QueueTimeout(String),
    Failed(String),
}

impl WarmupFailure {
    fn message(&self) -> &str {
        match self { Self::QueueTimeout(error) | Self::Failed(error) => error }
    }

    fn into_entry(self, now: Instant) -> Entry {
        match self {
            Self::QueueTimeout(error) => Entry::Retryable { error, retry_at: now + WARMUP_RETRY_DELAY },
            Self::Failed(error) => Entry::Failed(error),
        }
    }
}

#[derive(Clone, Copy)]
enum WarmupPhase { Waiting, Started { execution_deadline: Instant }, Expired }

fn clock() -> Instant { tokio::time::Instant::now().into_std() }

async fn run_warmup<T, F>(slot: Arc<Semaphore>, timeout: Duration, operation: F) -> Result<T, WarmupFailure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let queue_deadline = clock().checked_add(timeout).ok_or_else(|| WarmupFailure::Failed("图像引擎排队截止时间溢出".into()))?;
    let queued = format!("图像引擎预热排队超过 {} ms；引擎尚未开始，稍后重试", timeout.as_millis());
    let running = format!("图像引擎预热执行超过 {} ms；尚未返回的引擎继续占用预热线程，请排查核心库或重启", timeout.as_millis());
    let phase = Arc::new(Mutex::new(WarmupPhase::Waiting));
    let worker_phase = phase.clone();
    let queued_error = queued.clone();
    let running_error = running.clone();
    let task = async move {
        let permit = slot.acquire_owned().await.map_err(|error| WarmupFailure::Failed(format!("图像引擎预热线程已关闭：{error}")))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let execution_deadline = {
                let mut phase = worker_phase.lock().unwrap();
                let started = clock();
                if !matches!(*phase, WarmupPhase::Waiting) || started >= queue_deadline {
                    *phase = WarmupPhase::Expired;
                    return Err(WarmupFailure::QueueTimeout(queued_error));
                }
                let execution_deadline = started.checked_add(timeout).ok_or_else(|| WarmupFailure::Failed("图像引擎执行截止时间溢出".into()))?;
                *phase = WarmupPhase::Started { execution_deadline };
                execution_deadline
            };
            let result = operation().map_err(WarmupFailure::Failed);
            if clock() > execution_deadline { Err(WarmupFailure::Failed(running_error)) } else { result }
        }).await.map_err(|error| WarmupFailure::Failed(format!("图像引擎预热异常：{error}")))?
    };
    tokio::pin!(task);
    let execution_deadline = match tokio::time::timeout_at(tokio::time::Instant::from_std(queue_deadline), &mut task).await {
        Ok(result) => return result,
        Err(_) => {
            let mut phase = phase.lock().unwrap();
            match *phase {
                WarmupPhase::Started { execution_deadline } => execution_deadline,
                WarmupPhase::Waiting | WarmupPhase::Expired => {
                    *phase = WarmupPhase::Expired;
                    return Err(WarmupFailure::QueueTimeout(queued));
                }
            }
        }
    };
    match tokio::time::timeout_at(tokio::time::Instant::from_std(execution_deadline), &mut task).await {
        Ok(result) => result,
        Err(_) => Err(WarmupFailure::Failed(running)),
    }
}

pub struct ProductionHost {
    entries: Mutex<HashMap<String, Entry>>,
    slot: Arc<Semaphore>,
}

impl Default for ProductionHost {
    fn default() -> Self { Self { entries: Mutex::new(HashMap::new()), slot: Arc::new(Semaphore::new(1)) } }
}

impl ProductionHost {
    pub fn clear(&self) {
        self.entries.lock().unwrap().retain(|_, entry| matches!(entry, Entry::Preparing));
    }

    fn begin(&self, key: &str, now: Instant) -> Result<Option<Arc<Prepared>>, String> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get(key) {
            Some(Entry::Ready(prepared)) => return Ok(Some(prepared.clone())),
            Some(Entry::Failed(error)) => return Err(error.clone()),
            Some(Entry::Retryable { error, retry_at }) if now < *retry_at => return Err(error.clone()),
            Some(Entry::Preparing) => return Err("发布资源与图像引擎正在预热，完成前不能布防".into()),
            _ => {}
        }
        entries.insert(key.to_owned(), Entry::Preparing);
        Ok(None)
    }

    fn ensure(&self, app: &AppHandle, recipe: Arc<Recipe>) -> Result<Arc<Prepared>, String> {
        let key = warmup_key(&recipe);
        if let Some(prepared) = self.begin(&key, Instant::now())? { return Ok(prepared); }
        let app = app.clone();
        let slot = self.slot.clone();
        tauri::async_runtime::spawn(async move {
            let run_app = app.clone();
            let started = Instant::now();
            let result = run_warmup(slot, Duration::from_secs(30), move || {
                let bundle = crate::workspace::published_bundle(&run_app, &recipe)?;
                let engine = run_app.state::<VisionHost>().engine().ok_or("图像核心库未加载")?;
                let prepared = Prepared::load(bundle, engine, &recipe)?;
                prepared.warm()?;
                Ok(Arc::new(prepared))
            }).await;
            let state = match result {
                Ok(prepared) => {
                    crate::cycle::log(&app, "ok", "生产预热", format!("发布包 {} 已就绪，耗时 {} ms", prepared.bundle.id, started.elapsed().as_millis()));
                    if let Some(note) = &prepared.engine_note { crate::cycle::log(&app, "warn", "生产预热", note.clone()); }
                    Entry::Ready(prepared)
                }
                Err(error) => {
                    let level = if matches!(&error, WarmupFailure::QueueTimeout(_)) { "warn" } else { "err" };
                    crate::cycle::log(&app, level, "生产预热", error.message().to_owned());
                    error.into_entry(Instant::now())
                }
            };
            let retry_at = match &state { Entry::Retryable { retry_at, .. } => Some(*retry_at), _ => None };
            app.state::<ProductionHost>().entries.lock().unwrap().insert(key, state);
            if let Some(retry_at) = retry_at { tokio::time::sleep_until(tokio::time::Instant::from_std(retry_at)).await; }
            let _ = app.state::<CycleHost>().tx.send(Input::Refresh);
        });
        Err("发布资源与图像引擎正在预热，完成前不能布防".into())
    }
}

fn warmup_key(recipe: &Recipe) -> String {
    format!("{}:{}", recipe.id, recipe.revision_id)
}

/// 生产预热的只读状态，查询不触发预热。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warmup {
    /// 尚未开始，或可重试的失败已到重试时间
    Pending,
    Preparing,
    Ready,
    /// 引擎排队超时，稍后自动重试
    Retryable(String),
    Failed(String),
}

impl ProductionHost {
    fn warmup(&self, key: &str, now: Instant) -> Warmup {
        match self.entries.lock().unwrap().get(key) {
            None => Warmup::Pending,
            Some(Entry::Preparing) => Warmup::Preparing,
            Some(Entry::Ready(_)) => Warmup::Ready,
            Some(Entry::Retryable { error, retry_at }) if now < *retry_at => Warmup::Retryable(error.clone()),
            Some(Entry::Retryable { .. }) => Warmup::Pending,
            Some(Entry::Failed(error)) => Warmup::Failed(error.clone()),
        }
    }

    fn prepared(&self, key: &str) -> Option<Arc<Prepared>> {
        match self.entries.lock().unwrap().get(key) { Some(Entry::Ready(prepared)) => Some(prepared.clone()), _ => None }
    }
}

pub fn warmup(app: &AppHandle, recipe: &Recipe) -> Warmup {
    if !crate::site::image_measurement(app) { return Warmup::Ready; }
    let key = warmup_key(recipe);
    app.state::<ProductionHost>().warmup(&key, Instant::now())
}

/// 这一配方的发布资源与图像引擎正在预热（尚未就绪也未失败）。
pub fn warming(app: &AppHandle, recipe: &Recipe) -> bool {
    warmup(app, recipe) == Warmup::Preparing
}

pub const OFFLINE_VALIDATED: &str = "该版本在离线环境验证发布，需在真机重新验证并发布";

/// 离线验证发布的版本只能在离线模式运行。
pub fn site_admits(site: ValidationSite, offline: bool) -> Result<(), String> {
    if site == ValidationSite::Offline && !offline { Err(OFFLINE_VALIDATED.into()) } else { Ok(()) }
}

pub const COLLECT_ONLY: &str = "配方只有采集版，仅维护模式可用";

/// 采集版没有发布包，也没示教、标定，正常生产不能当正式检测运行；维护模式在调用 `ready` 之前自行分流。
pub fn stage_admits(recipe: &Recipe) -> Result<(), String> {
    if recipe.is_collect() {
        Err(COLLECT_ONLY.into())
    } else {
        Ok(())
    }
}

pub fn acquisition_tag(config: &crate::camera::CameraConfig) -> Value {
    serde_json::json!({ "triggerSource": config.trigger_source, "triggerActivation": config.trigger_activation, "frameRate": config.frame_rate })
}

fn check_acquisition(bundle: &ReleaseBundle, configs: &[crate::camera::CameraConfig]) -> Result<(), String> {
    for (index, inspect) in bundle.recipe.inspect.iter().enumerate() {
        let config = configs.iter().find(|c| c.id == inspect.camera).ok_or_else(|| format!("相机 {} 不存在", inspect.camera))?;
        let calibration: Value = serde_json::from_str(&crate::fsio::read_text(&bundle.camera(index)?.calibration).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if calibration.get("acquisition") != Some(&acquisition_tag(config)) {
            return Err(format!("相机 {} 的触发方式或帧率与发布标定不一致，请恢复参数或重新标定、验证并发布", inspect.camera));
        }
    }
    Ok(())
}

pub fn ready(app: &AppHandle, recipe: &Recipe) -> Result<Option<Arc<Prepared>>, String> {
    if crate::offline::candidate(app).is_some_and(|c| c.matches(recipe)) { return Ok(None); }
    stage_admits(recipe)?;
    if !crate::site::image_measurement(app) { return Ok(None); }
    let host = app.state::<ProductionHost>();
    let site = match host.prepared(&warmup_key(recipe)) {
        Some(prepared) => prepared.bundle.manifest.validation_site,
        None => crate::workspace::published_manifest(app, recipe)?.validation_site,
    };
    site_admits(site, crate::offline::enabled(app))?;
    let prepared = host.ensure(app, Arc::new(recipe.clone()))?;
    check_acquisition(&prepared.bundle, &app.state::<crate::cycle::CycleHost>().camera.configs())?;
    Ok(Some(prepared))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod warmup_tests;

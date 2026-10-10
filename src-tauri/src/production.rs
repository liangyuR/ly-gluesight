use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::Semaphore;

use crate::cycle::{CycleHost, Input};
use crate::frame::FrameImage;
use crate::recipe::Recipe;
use crate::release::ReleaseBundle;
use crate::vision::{self, Engine, ShotMeasurement, VisionHost};

pub const GRAPH_VERSION: &str = "taught-path-2-multiview";

pub fn graphs(recipe: &Recipe) -> Result<Value, String> {
    let mut shots = Vec::new();
    for (k, shot) in recipe.shots.iter().enumerate() {
        for view in shot.enabled_views() {
            let projected = recipe.for_view(k, view)?;
            shots.push(json!({"k": k, "shotId": shot.id, "camera": shot.camera, "view": view,
                "graph": vision::build_taught_graph(&projected, k)?}));
        }
    }
    Ok(json!({"schemaVersion": 2, "version": GRAPH_VERSION, "shots": shots}))
}

fn engine_version_matches(published: &str, loaded: &str) -> bool {
    published.split(':').next() == Some(loaded)
}

/// 版本号相同但核心库文件（大小或修改时间）与发布时不同：照常生产，但要让人知道结果不是验证时那份文件量的。
fn engine_file_note(published: &str, engine: &Engine) -> Option<String> {
    (published != engine.identity).then(|| format!(
        "当前核心库 {} 与发布时的文件不同（发布时 {published}，现在 {}）：同版本号但文件大小或修改时间变了，建议重新验证并发布",
        engine.path.display(), engine.identity))
}

pub struct Prepared {
    pub bundle: ReleaseBundle,
    pub recipe: Arc<Recipe>,
    /// 核心库文件与发布时不同的提示（见 engine_file_note）
    pub engine_note: Option<String>,
    engine: Arc<Engine>,
    graphs: HashMap<(usize, u8), Value>,
    sizes: HashMap<(usize, u8), [u32; 2]>,
}

impl Prepared {
    pub fn load(bundle: ReleaseBundle, engine: Arc<Engine>, expected: &Recipe) -> Result<Self, String> {
        bundle.verify()?;
        let recipe = Arc::new(bundle.recipe.build()?);
        if recipe.revision_id != expected.revision_id || recipe.id != expected.id || recipe.version != expected.version
            || recipe.product_code != expected.product_code || recipe.trigger_mode != expected.trigger_mode || recipe.schema_version != expected.schema_version
            || recipe.shots != expected.shots || recipe.spacing != expected.spacing || recipe.detect != expected.detect
            || recipe.limits != expected.limits || recipe.filter_window != expected.filter_window || recipe.teaching_id != expected.teaching_id
            || recipe.plan_version != expected.plan_version { return Err("发布包配方与所选生产版本不一致，请重新发布".into()); }
        if bundle.manifest.versions.graph != GRAPH_VERSION || !engine_version_matches(&bundle.manifest.versions.engine, &engine.version) {
            return Err("发布包的算法图或引擎版本与当前引擎不一致，请重新验证并发布".into());
        }
        let stored: Value = serde_json::from_str(&crate::fsio::read_text(&bundle.root.join(&bundle.manifest.graph)).map_err(|e| e.to_string())?)
            .map_err(|e| format!("发布包算法图无效：{e}"))?;
        if stored != graphs(&recipe)? { return Err("发布包算法图与配方测点不一致".into()); }
        let mut values = HashMap::new();
        let mut sizes = HashMap::new();
        for (k, shot) in recipe.shots.iter().enumerate() {
            for view in shot.enabled_views() {
                let resource = bundle.view(k, view)?;
                let projected = recipe.for_view(k, view)?;
                values.insert((k, view), vision::build_taught_graph(&projected, k)?);
                sizes.insert((k, view), resource.size.ok_or("发布原图缺少尺寸")?);
                let calibration: Value = serde_json::from_str(&crate::fsio::read_text(resource.calibration.as_ref().ok_or("发布包缺少标定")?).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("发布包标定无效：{e}"))?;
                let scale = calibration["mmPerPx"].as_f64().ok_or("发布包标定没有有效像素当量")?;
                if !scale.is_finite() || (scale - projected.shots[k].mm_per_px.unwrap_or_default() as f64).abs() > 1e-6 {
                    return Err(format!("拍照点 {} 图 {view} 的发布标定与示教像素当量不一致", shot.id));
                }
            }
        }
        let engine_note = engine_file_note(&bundle.manifest.versions.engine, &engine);
        Ok(Self { bundle, recipe, engine_note, engine, graphs: values, sizes })
    }

    pub fn verify(&self) -> Result<(), String> {
        self.bundle.verify()?;
        Ok(())
    }

    pub fn measure(&self, k: usize, image: &FrameImage, run_id: &str) -> Result<ShotMeasurement, String> {
        let view = self.recipe.shots.get(k).ok_or("拍照点不存在")?.view;
        self.measure_view(k, view, image, run_id)
    }

    pub fn measure_view(&self, k: usize, view: u8, image: &FrameImage, run_id: &str) -> Result<ShotMeasurement, String> {
        if self.sizes.get(&(k, view)).copied() != Some([image.width, image.height]) {
            return Err(format!("拍照点 {} 图 {view} 的原图尺寸与发布示教图不一致", k + 1));
        }
        let graph = self.graphs.get(&(k, view)).ok_or("拍照点图像没有已发布测量图")?;
        let projected = self.recipe.for_view(k, view)?;
        vision::measure_shot_with_graph(&self.engine, &projected, k, image, run_id, "", graph)
    }

    pub fn measure_views(&self, k: usize, images: &[(u8, Arc<FrameImage>)], run_id: &str) -> Result<vision::MultiViewMeasurement, String> {
        for (view, image) in images {
            if self.recipe.shots[k].enabled_views().contains(view) && self.sizes.get(&(k, *view)).copied() != Some([image.width, image.height]) {
                return Err(format!("拍照点 {} 图 {view} 的收图尺寸与发布资源不一致", k + 1));
            }
        }
        vision::measure_views_with(&self.recipe, k, images, |view, image| {
            self.measure_view(k, view, image, &format!("{run_id}-v{view}"))
        })
    }

    fn warm(&self) -> Result<(), String> {
        for (k, shot) in self.recipe.shots.iter().enumerate() {
            for view in shot.enabled_views() {
                let resource = self.bundle.view(k, view)?;
                let image = crate::replay::load(resource.image.as_ref().ok_or("发布包缺少示教原图")?)?;
                self.measure_view(k, view, &image, &format!("warm-{}-{k}-v{view}-{}", self.bundle.id, ly_plc::now_ms()))?;
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

async fn run_warmup<T, F>(slot: Arc<Semaphore>, timeout: Duration, operation: F) -> Result<T, WarmupFailure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let queue_deadline = Instant::now().checked_add(timeout).ok_or_else(|| WarmupFailure::Failed("图像引擎排队截止时间溢出".into()))?;
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
                let started = Instant::now();
                if !matches!(*phase, WarmupPhase::Waiting) || started >= queue_deadline {
                    *phase = WarmupPhase::Expired;
                    return Err(WarmupFailure::QueueTimeout(queued_error));
                }
                let execution_deadline = started.checked_add(timeout).ok_or_else(|| WarmupFailure::Failed("图像引擎执行截止时间溢出".into()))?;
                *phase = WarmupPhase::Started { execution_deadline };
                execution_deadline
            };
            let result = operation().map_err(WarmupFailure::Failed);
            if Instant::now() > execution_deadline { Err(WarmupFailure::Failed(running_error)) } else { result }
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

    fn ensure(&self, app: &AppHandle, recipe: Arc<Recipe>, core: Option<String>) -> Result<Arc<Prepared>, String> {
        let key = format!("{}:{}:{}", recipe.id, recipe.revision_id, core.as_deref().unwrap_or_default());
        if let Some(prepared) = self.begin(&key, Instant::now())? { return Ok(prepared); }
        let app = app.clone();
        let slot = self.slot.clone();
        tauri::async_runtime::spawn(async move {
            let run_app = app.clone();
            let started = Instant::now();
            let result = run_warmup(slot, Duration::from_secs(30), move || {
                let bundle = crate::workspace::published_bundle(&run_app, &recipe)?;
                let engine = run_app.state::<VisionHost>().engine(core.as_deref()).ok_or("图像核心库未加载，检查系统设置")?;
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

pub fn ready(app: &AppHandle, recipe: &Recipe) -> Result<Option<Arc<Prepared>>, String> {
    let settings = app.state::<CycleHost>().settings();
    if !settings.vision { return Ok(None); }
    app.state::<ProductionHost>().ensure(app, Arc::new(recipe.clone()), settings.lyflow_core).map(Some)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod warmup_tests;

#[cfg(test)]
mod regression;

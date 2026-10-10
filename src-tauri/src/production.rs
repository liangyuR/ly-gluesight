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

pub const GRAPH_VERSION: &str = "taught-path-1";

pub fn graphs(recipe: &Recipe) -> Result<Value, String> {
    let shots = recipe.shots.iter().enumerate().map(|(k, shot)| {
        let graph = if shot.measured() { Some(vision::build_taught_graph(recipe, k)?) } else { None };
        Ok(json!({"k":k,"shotId":shot.id,"camera":shot.camera,"view":shot.view,"graph":graph}))
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(json!({"schemaVersion":1,"version":GRAPH_VERSION,"shots":shots}))
}

pub struct Prepared {
    pub bundle: ReleaseBundle,
    pub recipe: Arc<Recipe>,
    engine: Arc<Engine>,
    graphs: Vec<Option<Value>>,
    sizes: Vec<Option<[u32; 2]>>,
}

impl Prepared {
    pub fn load(bundle: ReleaseBundle, engine: Arc<Engine>, expected: &Recipe) -> Result<Self, String> {
        bundle.verify()?;
        let recipe = Arc::new(bundle.recipe.build()?);
        if recipe.hash != expected.hash { return Err("发布包配方与所选生产版本不一致，请重新发布".into()); }
        if bundle.manifest.versions.graph != GRAPH_VERSION || bundle.manifest.versions.engine != engine.identity {
            return Err("发布包的算法图或引擎版本与当前引擎不一致，请重新验证并发布".into());
        }
        let stored: Value = serde_json::from_str(&crate::fsio::read_text(&bundle.root.join(&bundle.manifest.graph)).map_err(|e| e.to_string())?)
            .map_err(|e| format!("发布包算法图无效：{e}"))?;
        if stored != graphs(&recipe)? { return Err("发布包算法图与配方测点不一致".into()); }
        let mut values = Vec::new();
        let mut sizes = Vec::new();
        for (k, shot) in recipe.shots.iter().enumerate() {
            let resource = bundle.shot(k)?;
            values.push(if shot.measured() { Some(stored["shots"][k]["graph"].clone()) } else { None });
            sizes.push(resource.size);
            if shot.measured() {
                let calibration: Value = serde_json::from_str(&crate::fsio::read_text(resource.calibration.as_ref().ok_or("发布包缺少标定")?).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("发布包标定无效：{e}"))?;
                let scale = calibration["mmPerPx"].as_f64().ok_or("发布包标定没有有效像素当量")?;
                if !scale.is_finite() || (scale - shot.mm_per_px.unwrap_or_default() as f64).abs() > 1e-6 {
                    return Err(format!("拍照点 {} 的发布标定与示教像素当量不一致", shot.id));
                }
            }
        }
        Ok(Self { bundle, recipe, engine, graphs: values, sizes })
    }

    pub fn verify(&self) -> Result<(), String> {
        self.bundle.verify()?;
        let bytes = std::fs::read(&self.engine.path).map_err(|e| format!("无法核验算法核心库：{e}"))?;
        if self.engine.identity != format!("{}:{}", self.engine.version, crate::release::fnv_hex(&bytes)) {
            return Err("算法核心库文件已变化，需要重新启动并验证发布版本".into());
        }
        Ok(())
    }

    pub fn measure(&self, k: usize, image: &FrameImage, run_id: &str) -> Result<ShotMeasurement, String> {
        if self.sizes.get(k).copied().flatten() != Some([image.width, image.height]) {
            return Err(format!("拍照点 {} 的原图尺寸与发布示教图不一致", k + 1));
        }
        let graph = self.graphs.get(k).and_then(Option::as_ref).ok_or("拍照点没有已发布测量图")?;
        vision::measure_shot_with_graph(&self.engine, &self.recipe, k, image, run_id, "", graph)
    }

    fn warm(&self) -> Result<(), String> {
        for (k, shot) in self.recipe.shots.iter().enumerate().filter(|(_, shot)| shot.measured()) {
            let resource = self.bundle.shot(k)?;
            let image = crate::replay::load(resource.image.as_ref().ok_or("发布包缺少示教原图")?)?;
            let reading = self.measure(k, &image, &format!("warm-{}-{k}-{}", self.bundle.hash, ly_plc::now_ms()))?;
            if reading.coverage < 0.8 { return Err(format!("拍照点 {} 发布原图预热量成比例不足 80%", shot.id)); }
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
enum WarmupPhase { Waiting, Started, Expired }

async fn run_warmup<T, F>(slot: Arc<Semaphore>, timeout: Duration, operation: F) -> Result<T, WarmupFailure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| WarmupFailure::Failed("图像引擎预热截止时间溢出".into()))?;
    let prefix = format!("图像引擎排队或预热超过 {} ms", timeout.as_millis());
    let queued = format!("{prefix}；引擎尚未开始，稍后重试");
    let running = format!("{prefix}；尚未返回的引擎继续占用预热线程，请排查核心库或重启");
    let phase = Arc::new(Mutex::new(WarmupPhase::Waiting));
    let worker_phase = phase.clone();
    let queued_error = queued.clone();
    let running_error = running.clone();
    let task = async move {
        let permit = slot.acquire_owned().await.map_err(|error| WarmupFailure::Failed(format!("图像引擎预热线程已关闭：{error}")))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            {
                let mut phase = worker_phase.lock().unwrap();
                if !matches!(*phase, WarmupPhase::Waiting) || Instant::now() >= deadline {
                    *phase = WarmupPhase::Expired;
                    return Err(WarmupFailure::QueueTimeout(queued_error));
                }
                *phase = WarmupPhase::Started;
            }
            let result = operation().map_err(WarmupFailure::Failed);
            if Instant::now() > deadline { Err(WarmupFailure::Failed(running_error)) } else { result }
        }).await.map_err(|error| WarmupFailure::Failed(format!("图像引擎预热异常：{error}")))?
    };
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), task).await {
        Ok(result) => result,
        Err(_) => {
            let mut phase = phase.lock().unwrap();
            match *phase {
                WarmupPhase::Started => Err(WarmupFailure::Failed(running)),
                WarmupPhase::Waiting | WarmupPhase::Expired => {
                    *phase = WarmupPhase::Expired;
                    Err(WarmupFailure::QueueTimeout(queued))
                }
            }
        }
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
        let key = format!("{}:{}:{}", recipe.id, recipe.hash, core.as_deref().unwrap_or_default());
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
                    crate::cycle::log(&app, "ok", "生产预热", format!("发布包 {} 已就绪，耗时 {} ms", prepared.bundle.hash, started.elapsed().as_millis()));
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

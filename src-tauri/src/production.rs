use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::Semaphore;

use crate::cycle::{CycleHost, Input};
use crate::frame::FrameImage;
use crate::recipe::Recipe;
use crate::release::{ReleaseBundle, ValidationSite, RELEASE_SCHEMA_BURST};
use crate::vision::{self, Engine, FrameMeasurement, VisionHost};

/// 段配方的测量图版本（LF-4）：测量图模板加每（相机, 段）的全部有效参数；发布包 schema 5 的 `versions.graph`。
/// `flow-3` 冻结彩色 extent 观测与站级证据；旧图发布包需重新验证发布。
pub const GRAPH_VERSION_BURST: &str = "flow-3";

/// 段配方的测量图（发布包 graph.json）：用到的模板原文，加每段每台检测相机的走廊参数与判定门限，开启定向的（段, 相机）另有定向图的；
/// sizes 是各相机示教帧尺寸。
pub fn burst_graphs(recipe: &Recipe, sizes: &[[u32; 2]]) -> Result<Value, String> {
    if sizes.len() != recipe.inspect.len() {
        return Err(format!("段配方有 {} 台检测相机，示教帧尺寸却有 {} 份", recipe.inspect.len(), sizes.len()));
    }
    let set = vision::CorridorSet::build(recipe, sizes)?;
    // 灰度图沿用原来的形状（不记模板 id），胶色图记下用的是哪张模板
    let entry = |template: &vision::template::Template, mut v: Value| {
        if template.id.ends_with("_color") {
            v["template"] = json!(template.id);
        }
        v
    };
    let mut templates =
        json!({ vision::template::corridor().id: vision::template::corridor().doc });
    let graphs: Vec<Value> = set.iter()
        .map(|c| {
            templates[c.template.id] = c.template.doc.clone();
            entry(c.template, json!({"index": c.camera_idx, "camera": c.camera, "segment": c.segment, "size": c.size, "params": c.params, "judge": c.judge}))
        })
        .collect();
    let mut doc = json!({"schemaVersion": 5, "version": GRAPH_VERSION_BURST, "graphs": graphs});
    let headings: Vec<Value> = set.headings()
        .map(|h| {
            templates[h.template.id] = h.template.doc.clone();
            entry(h.template, json!({"index": h.camera_idx, "camera": h.camera, "segment": h.segment, "size": h.size, "params": h.params, "judge": h.judge}))
        })
        .collect();
    if !headings.is_empty() {
        doc["headings"] = json!(headings);
    }
    doc["templates"] = templates;
    Ok(doc)
}

pub struct Prepared {
    pub bundle: ReleaseBundle,
    pub recipe: Arc<Recipe>,
    engine: Arc<Engine>,
    /// 按（计划相机下标, 段号）的走廊测量图（D-SB-45）
    corridors: vision::CorridorSet,
    /// 预热时在自己的示教帧上没测成的（计划相机下标, 段号）与原因：生产不测，落在里面的帧记未测（D-SB-59）
    untested: BTreeMap<(usize, u32), String>,
    /// 预热测成的（相机, 段）数
    measurable: usize,
}

impl Prepared {
    /// 按段与检测相机建走廊测量图。发布包与所选生产版本的配方逐项一致、各相机发布标定的当量等于示教当量；
    /// 不按包内测量图或发布时 lyFlow 版本拒绝，走廊按配方和各相机示教帧尺寸重建。
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
        if recipe.revision_id != expected.revision_id
            || recipe.id != expected.id
            || recipe.version != expected.version
            || recipe.product_code != expected.product_code
            || recipe.trigger_mode != expected.trigger_mode
            || recipe.schema_version != expected.schema_version
            || recipe.spacing != expected.spacing
            || recipe.detect != expected.detect
            || recipe.filter_window != expected.filter_window
            || recipe.teaching_id != expected.teaching_id
            || recipe.path_segments != expected.path_segments
            || recipe.inspect != expected.inspect
            || recipe.part_calib != expected.part_calib
        {
            return Err("发布包配方与所选生产版本不一致，请重新发布".into());
        }
        let mut sizes = Vec::with_capacity(recipe.inspect.len());
        let mut channels = Vec::with_capacity(recipe.inspect.len());
        for (idx, inspect) in recipe.inspect.iter().enumerate() {
            let resource = bundle.camera(idx)?;
            sizes.push(resource.size);
            channels.push(match resource.corridors.first() {
                Some(corridor) => crate::image_file::load_channels(&corridor.image)?,
                None => 1,
            });
            let calibration: Value = serde_json::from_str(&crate::fsio::read_text(&resource.calibration).map_err(|e| e.to_string())?)
                .map_err(|e| format!("发布包标定无效：{e}"))?;
            let scale = calibration["mmPerPx"].as_f64().ok_or("发布包标定没有有效像素当量")?;
            if !scale.is_finite() || inspect.mm_per_px.is_none_or(|taught| (scale - taught).abs() > 1e-9) {
                return Err(format!("相机 {} 的发布标定与示教像素当量不一致", inspect.camera));
            }
        }
        let corridors = vision::CorridorSet::build(&recipe, &sizes)?.with_channels(&channels);
        Ok(Self {
            bundle,
            recipe,
            engine,
            corridors,
            untested: BTreeMap::new(),
            measurable: 0,
        })
    }

    /// 这一（相机, 段）预热没测成、生产不测时的原因。
    pub fn untested(&self, camera_idx: usize, segment: u32) -> Option<&str> {
        self.untested
            .get(&(camera_idx, segment))
            .map(String::as_str)
    }

    /// 预热没测成、生产不测的各（相机, 段）原因；全部测成时为空。
    pub fn untested_reasons(&self) -> Vec<&str> {
        self.untested.values().map(String::as_str).collect()
    }

    /// 整件说明里不测的段：“第 2 段相机 cam1、第 5 段相机 cam3”；全部测成时为 None。
    pub fn untested_label(&self) -> Option<String> {
        (!self.untested.is_empty()).then(|| {
            self.untested
                .keys()
                .map(|&(idx, segment)| {
                    let camera = self
                        .recipe
                        .inspect
                        .get(idx)
                        .map_or("?", |i| i.camera.as_str());
                    format!("第 {segment} 段相机 {camera}")
                })
                .collect::<Vec<_>>()
                .join("、")
        })
    }

    /// 至少有一（相机, 段）预热测成；一段都测不了时来件只录原图、回检测错误。
    pub fn measurable(&self) -> bool {
        self.measurable > 0
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

    /// 预热：每台相机在它每一段的发布示教帧上用该段走廊量一次——现场证明发布包、走廊与运行库接线正确。
    /// 没测成（无可用测量证据或运行失败）的（相机, 段）记进不测清单，其余段照常检测（D-SB-59）；示教帧读不出、发布包核验失败仍整包失败。
    pub(crate) fn warm(&mut self) -> Result<(), String> {
        let mut untested = BTreeMap::new();
        let mut measurable = 0;
        for idx in 0..self.recipe.inspect.len() {
            let resource = self.bundle.camera(idx)?;
            for corridor in &resource.corridors {
                let image = crate::image_file::load(&corridor.image)?;
                let run = format!(
                    "warm-{}-c{idx}-s{}-{}",
                    self.bundle.id,
                    corridor.segment,
                    ly_plc::now_ms()
                );
                let problem = match self.measure_frame(idx, corridor.segment, 0, &image, &run) {
                    Ok(m) if m.metrics.has_usable_evidence() => None,
                    Ok(m) => Some(m.metrics.error.unwrap_or_else(|| "没有可用测量证据".into())),
                    Err(error) => Some(error),
                };
                match problem {
                    None => measurable += 1,
                    Some(error) => {
                        untested.insert(
                            (idx, corridor.segment),
                            format!(
                                "第 {} 段相机 {}：示教帧上没测成（{error}）",
                                corridor.segment, resource.camera
                            ),
                        );
                    }
                }
            }
        }
        self.untested = untested;
        self.measurable = measurable;
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
            Some(Entry::Retryable { error, retry_at }) if now < *retry_at => {
                return Err(error.clone())
            }
            Some(Entry::Preparing) => {
                return Err("发布资源与图像引擎正在预热，完成前不能接件".into())
            }
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
                let mut prepared = Prepared::load(bundle, engine, &recipe)?;
                prepared.warm()?;
                Ok(Arc::new(prepared))
            }).await;
            let state = match result {
                Ok(prepared) => {
                    let elapsed = started.elapsed().as_millis();
                    let untested = prepared.untested_reasons();
                    if untested.is_empty() {
                        crate::cycle::log(
                            &app,
                            "ok",
                            "生产预热",
                            format!("发布包 {} 已就绪，耗时 {elapsed} ms", prepared.bundle.id),
                        );
                    } else if prepared.measurable() {
                        crate::cycle::log(&app, "warn", "生产预热", format!("发布包 {} 已就绪，耗时 {elapsed} ms；这些段不测（帧记未测，结论只覆盖其余段）：{}", prepared.bundle.id, untested.join("；")));
                    } else {
                        crate::cycle::log(
                            &app,
                            "err",
                            "生产预热",
                            format!(
                                "发布包 {} 每一段都没测成，来件只录原图、回检测错误：{}",
                                prepared.bundle.id,
                                untested.join("；")
                            ),
                        );
                    }
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
        Err("发布资源与图像引擎正在预热，完成前不能接件".into())
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

    /// 已预热好的这个发布包（历史叠图复用，不另加载）。
    pub fn ready_bundle(&self, bundle_id: &str) -> Option<Arc<Prepared>> {
        self.entries
            .lock()
            .unwrap()
            .values()
            .find_map(|entry| match entry {
                Entry::Ready(prepared) if prepared.bundle.id == bundle_id => Some(prepared.clone()),
                _ => None,
            })
    }
}

/// 已预热好的发布包里不测的段（D-SB-59）：部分不测返回说明；一段都测不了返回 Err；没预热好或全部能测为 Ok(None)。
pub fn coverage(app: &AppHandle, recipe: &Recipe) -> Result<Option<String>, String> {
    let Some(prepared) = app.state::<ProductionHost>().prepared(&warmup_key(recipe)) else {
        return Ok(None);
    };
    let untested = prepared.untested_reasons();
    if untested.is_empty() {
        Ok(None)
    } else if prepared.measurable() {
        Ok(Some(format!(
            "部分段不测（结论只覆盖其余段）：{}",
            untested.join("；")
        )))
    } else {
        Err(format!(
            "发布包每一段在示教帧上都没测成，来件只录原图、回检测错误：{}",
            untested.join("；")
        ))
    }
}

pub fn warmup(app: &AppHandle, recipe: &Recipe) -> Warmup {
    let key = warmup_key(recipe);
    app.state::<ProductionHost>().warmup(&key, Instant::now())
}

pub const OFFLINE_VALIDATED: &str = "该版本在离线环境验证发布，需在真机重新验证并发布";

/// 拒绝旧离线环境发布的版本，必须重新验证发布。
pub fn site_admits(site: ValidationSite) -> Result<(), String> {
    if site == ValidationSite::Offline {
        Err(OFFLINE_VALIDATED.into())
    } else {
        Ok(())
    }
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
    stage_admits(recipe)?;
    let host = app.state::<ProductionHost>();
    let site = match host.prepared(&warmup_key(recipe)) {
        Some(prepared) => prepared.bundle.manifest.validation_site,
        None => crate::workspace::published_manifest(app, recipe)?.validation_site,
    };
    site_admits(site)?;
    let prepared = host.ensure(app, Arc::new(recipe.clone()))?;
    check_acquisition(&prepared.bundle, &app.state::<crate::cycle::CycleHost>().camera.configs())?;
    Ok(Some(prepared))
}


#[cfg(test)]
mod warmup_tests;

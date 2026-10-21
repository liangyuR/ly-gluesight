//! lyFlow 视觉引擎：加载 core DLL，逐帧注入完整图像，沿拍照点示教中线量胶。
//! 图只量不判，判定在 judge 模块（设计稿 §9）。

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lyflow_client::{Core, RunHandle, RunImageInput, RunSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::camera::CameraSource;
use crate::cycle::CycleHost;
pub use crate::frame::FrameImage;
use crate::measure::{Job, Measured, Measurer};
use crate::recipe::Recipe;

mod fixture;
mod taught;
pub use taught::{build_taught_graph, measure_shot, measure_shot_with_graph, ShotMeasurement, MultiViewMeasurement, ViewMeasurement, measure_views, measure_views_with};
#[cfg(test)]
use taught::measure_shot_patched;

unsafe extern "C" fn ignore_event(_: *const c_char, _: *mut c_void) {}

pub(crate) fn unique_run_id(label: &str) -> String {
    static NEXT_RUN: AtomicU64 = AtomicU64::new(0);
    format!("{label}-{}-{}", std::process::id(), NEXT_RUN.fetch_add(1, Ordering::Relaxed))
}

/// 文件大小与修改时间（毫秒）。
pub(crate) fn file_identity(path: &Path) -> Result<(u64, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取核心库文件信息失败：{e}"))?;
    let modified = meta.modified().map_err(|e| format!("读取核心库修改时间失败：{e}"))?;
    let ms = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64);
    Ok((meta.len(), ms))
}

pub struct Engine {
    core: Arc<Core>,
    pub path: PathBuf,
    pub version: String,
    /// 加载时核心库文件的大小与修改时间：同版本号重新编译的 DLL 也能分辨（不算文件摘要）
    pub bytes: u64,
    pub modified_ms: u64,
    /// `版本:文件大小:修改时间`，写进发布包，生产加载时核对
    pub identity: String,
}

impl Engine {
    pub fn load(path: &Path) -> Result<Self, String> {
        let resolved = std::fs::canonicalize(path).map_err(|e| format!("加载 lyFlow 失败（{}；客户端 ABI {}）：无法解析核心库路径：{e}", path.display(), lyflow_client::ABI_VERSION))?;
        let core = Core::load_from(&resolved).map_err(|e| format!("加载 lyFlow 失败（{}；客户端 ABI {}）：{e}", path.display(), lyflow_client::ABI_VERSION))?;
        core.self_check()?;
        check_operators(&serde_json::from_str(&core.manifest_json().map_err(|e| e.to_string())?)
            .map_err(|e| format!("核心库算子清单无效：{e}"))?)?;
        let version = core.version();
        let (bytes, modified_ms) = file_identity(&resolved)?;
        let identity = format!("{version}:{bytes}:{modified_ms}");
        Ok(Self { core: Arc::new(core), path: path.to_path_buf(), version, bytes, modified_ms, identity })
    }

    /// 跑一次图，返回 run summary 与图级命名输出（都已解析成 JSON）。
    pub fn run(&self, graph: &str, run_id: &str, base_dir: &str, image: &FrameImage, params: &Value) -> Result<RunResult, String> {
        let images = [image_input(image)?];
        let params_json = params.to_string();
        let run_id = unique_run_id(run_id);
        let mut spec = RunSpec::new(graph, &run_id, base_dir, &[]).with_params_json(&params_json);
        // 正式测量只注入原始全分辨率像素。core 在 start 内拷贝，images 活到 start 返回。
        spec.image_inputs = &images;
        let handle = unsafe { RunHandle::start(self.core.clone(), spec, ignore_event, Box::new(())) }.map_err(|e| e.to_string())?;
        handle.join();
        let summary_json = self
            .core
            .run_summary(&run_id)
            .map_err(|e| e.to_string())?
            .ok_or("算法没有返回运行摘要")?;
        let summary: Value = serde_json::from_str(&summary_json).map_err(|e| format!("算法运行摘要无效：{e}"))?;
        let outputs: Value = serde_json::from_str(&self.core.run_outputs(&run_id).map_err(|e| e.to_string())?)
            .map_err(|e| format!("算法输出无效：{e}"))?;
        drop(handle);
        Ok(RunResult { summary, outputs })
    }
}

fn check_operators(manifest: &Value) -> Result<(), String> {
    let required = ["io.load_image", "image.board_calib", "image.load_calib", "glue.taught_path", "glue.bead_width"];
    let ops = manifest["operators"].as_array().ok_or("核心库没有算子清单")?;
    let missing: Vec<_> = required.into_iter().filter(|id| !ops.iter().any(|op| op["id"].as_str() == Some(id))).collect();
    if missing.is_empty() { Ok(()) } else { Err(format!("核心库缺少示教胶路/标定算子：{}。请选择包含 glue.taught_path 的核心库，旧版飞拍核心库不能用于当前配方。", missing.join("、"))) }
}

fn image_input(image: &FrameImage) -> Result<RunImageInput, String> {
    let count = (image.width as usize).checked_mul(image.height as usize).filter(|&n| n > 0)
        .ok_or("图像尺寸无效")?;
    if image.pixels.len() != count { return Err("图像像素缓冲与尺寸不一致".into()); }
    Ok(RunImageInput { node_id: "n_load".into(), port: "image".into(), width: image.width, height: image.height,
        channels: 1, depth: 1, pixels: image.pixels.clone() })
}

pub struct RunResult {
    pub summary: Value,
    pub outputs: Value,
}

/// lyFlow 作为测量后端：沿拍照点的示教中线量胶。
pub struct LyFlowMeasurer;

impl Measurer for LyFlowMeasurer {
    fn measure(&self, job: &Job, image: &FrameImage) -> Result<Measured, String> {
        if let Some(candidate) = &job.candidate { return candidate.measure(job, image); }
        let prepared = job.production.as_ref().ok_or("图像检测没有已核验并预热的发布包")?;
        if job.bundle_id.as_deref() != Some(prepared.bundle.id.as_str()) || job.recipe.revision_id != prepared.recipe.revision_id {
            return Err("测量任务与已冻结发布包不一致".into());
        }
        let run_id = format!("shot-{}-{}", job.cycle_id, job.k);
        if job.images.is_empty() {
            Ok(prepared.measure(job.k, image, &run_id)?.into_measured(job))
        } else {
            Ok(prepared.measure_views(job.k, &job.images, &run_id)?.into_measured(job))
        }
    }
}

impl RunResult {
    pub fn status(&self) -> &str {
        self.summary.get("status").and_then(|s| s.as_str()).unwrap_or("failed")
    }

    /// 命名输出里 Record 的 data。
    pub fn record(&self, name: &str) -> Option<&Value> {
        let v = self.outputs.get(name)?.get("value")?;
        v.get("data").or(Some(v))
    }

    /// 运行失败时给人看的原因：失败输出的 root 节点与错误。
    pub fn failure(&self) -> String {
        let mut parts = Vec::new();
        if let Some(outs) = self.summary.get("outputs").and_then(|o| o.as_object()) {
            for (name, o) in outs {
                if o.get("state").and_then(|s| s.as_str()) == Some("failed") {
                    let root = o.get("root").and_then(|r| r.as_str()).unwrap_or("?");
                    let code = o.get("rootCode").and_then(|r| r.as_str()).unwrap_or("?");
                    parts.push(format!("{name}：{root}（{code}）"));
                }
            }
        }
        if parts.is_empty() {
            format!("lyFlow 运行状态 {}", self.status())
        } else {
            parts.join("；")
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub loaded: bool,
    /// 运行库目录
    pub path: Option<String>,
    pub version: Option<String>,
    /// 开发构建临时指向本机 lyFlow 运行库（联合开发）
    pub overridden: bool,
    pub message: String,
}

/// 本版本锁定的 lyFlow 系列（packaging/runtime.json 的 lyflow.series，编译时带入）。
pub const LYFLOW_SERIES: &str = env!("GLUESIGHT_LYFLOW_SERIES");
/// 仅开发构建读取：联合开发时由 pnpm dev:joint -LyFlowRuntime 设置，只对本次启动有效，不落盘。
const RUNTIME_OVERRIDE: &str = "GLUESIGHT_LYFLOW_RUNTIME";

struct RuntimeDir {
    dir: PathBuf,
    overridden: bool,
}

impl RuntimeDir {
    fn core(&self) -> PathBuf {
        self.dir.join("lyflow_core.dll")
    }
}

fn override_dir() -> Option<String> {
    if cfg!(debug_assertions) { std::env::var(RUNTIME_OVERRIDE).ok() } else { None }
}

/// 运行库只在程序同目录的 runtime/lyflow；开发构建可被联合开发覆盖。
fn runtime_dir(override_dir: Option<String>, executable: impl FnOnce() -> std::io::Result<PathBuf>) -> Result<RuntimeDir, String> {
    if let Some(dir) = override_dir.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) {
        return Ok(RuntimeDir { dir: PathBuf::from(dir), overridden: true });
    }
    let executable = executable().map_err(|e| format!("无法确定 lyFlow 运行库位置：{e}"))?;
    let dir = executable.parent().ok_or("无法确定程序所在目录")?;
    Ok(RuntimeDir { dir: dir.join("runtime").join("lyflow"), overridden: false })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDescriptor {
    engine_version: String,
    abi_version: String,
    architecture: String,
    #[serde(default)]
    local_validation_only: bool,
}

pub(crate) fn series_of(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let (major, minor) = (parts.next()?, parts.next()?);
    (major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok()).then(|| format!("{major}.{minor}"))
}

/// 运行库自带的描述文件：引擎与 ABI 版本属于锁定系列、x64、正式构建。只看版本字段，不看文件摘要。
fn check_runtime(dir: &Path, series: &str) -> Result<(), String> {
    let path = dir.join("lyflow-runtime.json");
    let text = crate::fsio::read_text(&path).map_err(|e| format!("lyFlow 运行库缺少描述文件 {}（{e}）", path.display()))?;
    let descriptor: RuntimeDescriptor = serde_json::from_str(&text).map_err(|e| format!("lyFlow 运行库描述文件无效：{e}"))?;
    if descriptor.architecture != "x64" {
        return Err(format!("lyFlow 运行库架构为 {}，需要 x64", descriptor.architecture));
    }
    if descriptor.local_validation_only {
        return Err("lyFlow 运行库是本地验证构建，不能用于 GlueSight".into());
    }
    if [&descriptor.engine_version, &descriptor.abi_version].iter().any(|v| series_of(v).as_deref() != Some(series)) {
        return Err(format!("lyFlow 运行库 {}（ABI {}）不属于本版本锁定的 {series}.x 系列", descriptor.engine_version, descriptor.abi_version));
    }
    Ok(())
}

/// lyFlow 引擎：只从程序同目录的 runtime/lyflow 加载，加载前核对运行库描述文件。
#[derive(Default)]
pub struct VisionHost {
    engine: Mutex<Option<Arc<Engine>>>,
    error: Mutex<Option<String>>,
    failed: Mutex<Option<(PathBuf, Instant)>>,
}

const STATUS_RELOAD_BACKOFF: Duration = Duration::from_secs(5);

impl VisionHost {
    pub fn engine(&self) -> Option<Arc<Engine>> {
        match runtime_dir(override_dir(), std::env::current_exe) {
            Ok(runtime) => self.engine_at(&runtime),
            Err(message) => {
                *self.error.lock().unwrap() = Some(message);
                None
            }
        }
    }

    fn engine_at(&self, runtime: &RuntimeDir) -> Option<Arc<Engine>> {
        let path = runtime.core();
        let mut guard = self.engine.lock().unwrap();
        if let Some(e) = guard.as_ref().filter(|e| e.path == path) {
            return Some(e.clone());
        }
        let loaded = if !path.is_file() {
            Err(if runtime.overridden {
                format!("联合开发指定的 lyFlow 运行库目录缺少 lyflow_core.dll：{}", runtime.dir.display())
            } else {
                format!("缺少 lyFlow 运行库：{}。安装版请重新安装完整安装包；开发请先执行 pnpm deps:lyflow:dev 准备锁定系列的运行库。", runtime.dir.display())
            })
        } else if runtime.overridden {
            Engine::load(&path)
        } else {
            check_runtime(&runtime.dir, LYFLOW_SERIES).and_then(|()| Engine::load(&path))
        };
        let path = path.as_path();
        match loaded {
            Ok(e) => {
                let e = Arc::new(e);
                *guard = Some(e.clone());
                *self.error.lock().unwrap() = None;
                *self.failed.lock().unwrap() = None;
                Some(e)
            }
            Err(msg) => {
                *guard = None;
                let mut error = self.error.lock().unwrap();
                if error.as_deref() != Some(msg.as_str()) { crate::applog::write("err", "lyFlow 加载", &msg); }
                *error = Some(msg);
                drop(error);
                *self.failed.lock().unwrap() = Some((path.to_path_buf(), Instant::now()));
                None
            }
        }
    }

    pub fn status(&self) -> EngineStatus {
        self.status_at(override_dir(), Instant::now())
    }

    /// 未加载时同一路径的失败在退避期内直接报上次原因，不反复加载核心库。
    fn status_at(&self, override_dir: Option<String>, now: Instant) -> EngineStatus {
        let runtime = match runtime_dir(override_dir, std::env::current_exe) {
            Ok(runtime) => runtime,
            Err(message) => return EngineStatus { message, ..EngineStatus::default() },
        };
        let path = runtime.core();
        let loaded = self.engine.lock().unwrap().as_ref().filter(|e| e.path == path).cloned();
        let backing_off = self.failed.lock().unwrap().as_ref()
            .is_some_and(|(failed, at)| *failed == path && now.saturating_duration_since(*at) < STATUS_RELOAD_BACKOFF);
        let engine = loaded.or_else(|| if backing_off { None } else { self.engine_at(&runtime) });
        EngineStatus {
            loaded: engine.is_some(),
            path: Some(runtime.dir.display().to_string()),
            version: engine.as_ref().map(|e| e.version.clone()),
            overridden: runtime.overridden,
            message: match &engine {
                Some(_) if runtime.overridden => "已加载（联合开发覆盖）".into(),
                Some(_) => "已加载".into(),
                None => self.error.lock().unwrap().clone().unwrap_or_else(|| "加载失败".into()),
            },
        }
    }

}


pub fn station_calib_path(app: &AppHandle, cam_id: &str) -> Result<PathBuf, String> {
    let name = format!("plane_calib_{cam_id}.json");
    Ok(crate::storage::root(app).join("calib").join(name))
}

/// 拍照点 k 的标定文件：按它的标定引用（缺省为相机编号）找工位标定。
pub fn shot_calib_path(app: &AppHandle, recipe: &Recipe, k: usize) -> Result<PathBuf, String> {
    station_calib_path(app, &recipe.shots.get(k).ok_or("拍照点不存在")?.calib_ref())
}

/// 配方各相机的来源：要么全是模拟合成，要么全是真实原图（海康 / 离线样本集），示教资料与标定的来路不同。
pub fn recipe_source(app: &AppHandle, recipe: &Recipe) -> Result<CameraSource, String> {
    let rig = &app.state::<CycleHost>().camera;
    let mut found: Option<(CameraSource, String)> = None;
    for id in recipe.cameras() {
        let source = rig.slot(rig.require(&id)? as usize).ok_or("相机不存在")?.config().source;
        match &found {
            Some((first, first_id)) if first.synthetic() != source.synthetic() => {
                return Err(format!("配方 {} 的相机来源不一致：{first_id} 是 {first:?}，{id} 是 {source:?}；要么全用模拟相机，要么全用真实原图（海康 / 样本集）相机", recipe.id));
            }
            Some(_) => {}
            None => found = Some((source, id)),
        }
    }
    found.map(|(s, _)| s).ok_or_else(|| format!("配方 {} 没有拍照点", recipe.id))
}

fn station_view_key(device: &str, view: u8) -> Result<String, String> {
    if !(1..=3).contains(&view) { return Err("标定图编号必须为 1、2 或 3".into()); }
    Ok(format!("{device}-v{view}"))
}

fn camera_id(app: &AppHandle, cam: u8) -> Result<String, String> {
    Ok(app.state::<CycleHost>().camera.slot(cam as usize).ok_or("相机不存在")?.config().id)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibInfo {
    pub path: String,
    pub rms: Option<f64>,
    pub mm_per_px: Option<f64>,
    pub max_error: Option<f64>,
    pub pattern: Option<Vec<f64>>,
    pub square: Option<f64>,
    pub ts: Option<i64>,
}

pub fn calib_info(path: &Path) -> Option<CalibInfo> {
    let doc: Value = serde_json::from_str(&crate::fsio::read_text(path).ok()?).ok()?;
    let d = doc.get("data").unwrap_or(&doc);
    let f = |k: &str| d.get(k).and_then(|v| v.as_f64());
    Some(CalibInfo {
        path: path.display().to_string(),
        rms: f("rms"),
        mm_per_px: f("mmPerPx"),
        max_error: f("maxError"),
        pattern: d.get("pattern").and_then(|v| serde_json::from_value(v.clone()).ok()),
        square: f("square"),
        ts: doc.get("ts").and_then(|v| v.as_i64()),
    })
}

#[tauri::command]
pub fn vision_calib_info(app: AppHandle, cam: Option<u8>, view: Option<u8>) -> Result<Option<CalibInfo>, String> {
    let key = station_view_key(&camera_id(&app, cam.unwrap_or(0))?, view.unwrap_or(1))?;
    Ok(calib_info(&station_calib_path(&app, &key)?))
}

/// 工位标定：用飞拍相机最近一帧整图跑 image.board_calib，结果存成工位标定文件。
#[tauri::command]
pub async fn vision_calibrate(app: AppHandle, pattern: [f64; 2], square: f64, cam: Option<u8>, view: Option<u8>, image_id: Option<String>) -> Result<CalibInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let view = view.unwrap_or(1);
        let key = station_view_key(&camera_id(&app, cam.unwrap_or(0))?, view)?;
        if app.state::<CycleHost>().busy() || app.state::<CycleHost>().camera.recipe_capture().active() { return Err("设备正在采集或检测，结束后再标定".into()); }
        if pattern.iter().any(|n|!n.is_finite()||*n<2.0||*n>100.0||n.fract()!=0.0)||!square.is_finite()||square<=0.0 { return Err("棋盘格内角点与格长无效".into()); }
        let engine = app.state::<VisionHost>().engine().ok_or("lyFlow 核心库未加载")?;
        let image = if let Some(id)=image_id.as_deref() {
            if crate::workspace::station_image_view(&app, cam.unwrap_or(0), id)? != view { return Err("冻结标定样本的图编号与当前选择不一致".into()); }
            crate::workspace::station_image_ref(&app,cam.unwrap_or(0),id)? } else { app
            .state::<CycleHost>()
            .camera
            .last_view(cam.unwrap_or(0), view)
            .ok_or("还没有整帧图像：打开图像测量后软触发一帧（标定板放在内边所在高度）")? };
        let graph = json!({
            "schemaVersion": 1,
            "id": "01TUJIAOBOARDCALIB00000000",
            "nodes": [
                {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
                {"id": "n_calib", "op": "image.board_calib", "params": {"pattern": pattern, "square": square}}
            ],
            "edges": [{"id": "e0", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_calib", "port": "image"}}],
            "outputs": {"calib": {"node": "n_calib", "port": "calib"}}
        });
        let run_id = format!("calib-{}", ly_plc::now_ms());
        let r = engine.run(&graph.to_string(), &run_id, "", &image, &json!({}))?;
        if r.status() == "failed" {
            let why = r.failure();
            return Err(if why.contains("no_board") {
                format!("没找到 {}×{} 个内角点的棋盘格：确认 pattern 是内角点数、整块板都在图里", pattern[0], pattern[1])
            } else {
                why
            });
        }
        let data = r.record("calib").cloned().ok_or("标定没有输出")?;
        let path = station_calib_path(&app, &key)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if let Some(id)=image_id.as_deref() {
            if crate::workspace::station_image_view(&app, cam.unwrap_or(0), id)? != view { return Err("标定过程中样本视图已改变".into()); }
            crate::workspace::station_image_ref(&app,cam.unwrap_or(0),id)?;
        }
        let doc = json!({"kind": "Record", "type": "image.PlaneCalib", "data": data, "ts": ly_plc::now_ms(), "sampleId":image_id,"cameraId":camera_id(&app,cam.unwrap_or(0))?,"view":view});
        crate::fsio::write_atomic(&path, &serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?)?;
        let _ = app.emit("calibration://changed", ());
        calib_info(&path).ok_or_else(|| "标定文件写入后读不回来".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod eval;

#[cfg(test)]
mod run_identity_tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Barrier;

    #[test]
    fn concurrent_identical_labels_get_unique_run_ids() {
        let barrier = Barrier::new(8);
        let ids = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8).map(|_| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    (0..1024).map(|_| unique_run_id("same-label")).collect::<Vec<_>>()
                })
            }).collect();
            workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
        });
        assert_eq!(ids.len(), 8192);
        assert!(ids.iter().all(|id| id.starts_with("same-label-")));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    }
}

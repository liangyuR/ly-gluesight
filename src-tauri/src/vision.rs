//! lyFlow 视觉引擎：加载 core DLL，逐帧注入完整图像，沿段配方各相机的检测走廊量胶。
//! 图只量不判，判定在 burst_judge 模块。

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
use crate::measure::FrameJob;
use crate::recipe::{CameraInspect, Recipe};

mod heading;
mod judge;
mod taught;
pub mod template;
#[allow(unused_imports)] // D2 标定轮次逐帧测量接入后去掉
pub use taught::measure_frame_with;
pub use taught::{corridor_inside, measure_frame, CorridorSet, FrameMeasurement};

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
        let manifest: Value = serde_json::from_str(&core.manifest_json().map_err(|e| e.to_string())?)
            .map_err(|e| format!("核心库算子清单无效：{e}"))?;
        check_operators(&manifest)?;
        let version = core.version();
        let (bytes, modified_ms) = file_identity(&resolved)?;
        let identity = format!("{version}:{bytes}:{modified_ms}");
        Ok(Self {
            core: Arc::new(core),
            path: path.to_path_buf(),
            version,
            bytes,
            modified_ms,
            identity,
        })
    }

    /// 流程调试页直接驱动运行库（编计划、按节点取输出）用。
    pub(crate) fn core(&self) -> Arc<Core> {
        self.core.clone()
    }

    /// 跑一次图，返回 run summary 与图级命名输出（都已解析成 JSON）。
    pub fn run(&self, graph: &str, run_id: &str, base_dir: &str, image: &FrameImage, params: &Value) -> Result<RunResult, String> {
        self.run_spec(graph, run_id, base_dir, image, params, &[])
    }

    /// 按顶层图参数跑测量图模板，只跑到 `targets` 这几个节点（空为整张图）。
    pub fn run_targets(&self, graph: &str, run_id: &str, image: &FrameImage, params: &Value, targets: &[String]) -> Result<RunResult, String> {
        self.run_spec(graph, run_id, "", image, params, targets)
    }

    fn run_spec(&self, graph: &str, run_id: &str, base_dir: &str, image: &FrameImage, params: &Value, targets: &[String]) -> Result<RunResult, String> {
        let images = [image_input(image)?];
        let params_json = params.to_string();
        let run_id = unique_run_id(run_id);
        let mut spec = RunSpec::new(graph, &run_id, base_dir, targets).with_params_json(&params_json);
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

/// 必需算子：packaging/runtime.json 的 requiredOperators（build.rs 编译时带入，逗号分隔），打包与加载同一份清单。
pub(crate) const REQUIRED_OPERATORS: &str = env!("GLUESIGHT_REQUIRED_OPERATORS");

fn check_operators(manifest: &Value) -> Result<(), String> {
    let ops = manifest["operators"].as_array().ok_or("核心库没有算子清单")?;
    let missing: Vec<_> = REQUIRED_OPERATORS.split(',').filter(|id| !ops.iter().any(|op| op["id"].as_str() == Some(id))).collect();
    if missing.is_empty() { Ok(()) } else { Err(format!("核心库缺少必需算子：{}。请使用本版本锁定系列的 lyFlow 运行库。", missing.join("、"))) }
}

pub(crate) fn image_input(image: &FrameImage) -> Result<RunImageInput, String> {
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

/// lyFlow 作为测量后端：沿检测走廊逐帧量胶。
pub struct LyFlowMeasurer;

impl LyFlowMeasurer {
    /// 段配方一台相机一帧：候选试运行用候选的走廊测量图，生产用已核验并预热的发布包。
    pub fn measure_frame(&self, job: &FrameJob, image: &FrameImage) -> Result<FrameMeasurement, String> {
        if let Some(candidate) = &job.candidate { return candidate.measure_frame(job, image); }
        let prepared = job.production.as_ref().ok_or("图像检测没有已核验并预热的发布包")?;
        if job.bundle_id.as_deref() != Some(prepared.bundle.id.as_str()) || job.recipe.revision_id != prepared.recipe.revision_id {
            return Err("测量任务与已冻结发布包不一致".into());
        }
        if prepared.recipe.inspect.get(job.camera_idx).map(|i| i.camera.as_str()) != Some(job.camera.as_str()) {
            return Err(format!("测量任务的相机 {} 不是发布包的第 {} 台计划相机", job.camera, job.camera_idx + 1));
        }
        prepared.measure_frame(
            job.camera_idx,
            job.segment,
            job.seq,
            image,
            &format!("frame-{}-{}-{}", job.cycle_id, job.camera_idx, job.seq),
        )
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

pub(crate) fn runtime_directory() -> Result<PathBuf, String> {
    Ok(runtime_dir(override_dir(), std::env::current_exe)?.dir)
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

/// 段配方缺省设备标定所在的子目录（相对 `calib/`）。
const DEVICE_CALIB_DIR: &str = "device";

/// 工位标定的键。
/// - `Station`：旧拍照点配方的 `{device}-v{view}` 与显式标定引用，文件 `calib/plane_calib_{键}.json`；
/// - `Device`：段配方缺省的设备标定（D-SB-30），文件在单独子目录 `calib/device/plane_calib_{device}.json`。
///
/// 两者不能共用扁平文件名：相机编号允许 '-'，相机 "cam1-v1" 的设备键会等于相机 "cam1" 图 1 的旧键，一方标定就覆盖另一方；
/// 加前缀也不够（相机 "dev_cam1" 的图 1 = "dev_cam1-v1" = 相机 "cam1-v1" 加前缀 "dev_"）。分目录后永不同名：
/// Station 键是相机编号或标定引用，只含字母、数字、- 和 _，不带路径分隔符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibKey {
    Station(String),
    Device(String),
}

impl CalibKey {
    /// 标定根目录 `calib_root`（数据根目录下的 `calib/`）里这个键的文件。
    fn file(&self, calib_root: &Path) -> PathBuf {
        match self {
            Self::Station(key) => calib_root.join(format!("plane_calib_{key}.json")),
            Self::Device(device) => calib_root.join(DEVICE_CALIB_DIR).join(format!("plane_calib_{device}.json")),
        }
    }

    /// 发布包 `calibration.json` 里记的标定引用：Station 为键本身，Device 为 `device/{device}`。
    pub fn reference(&self) -> String {
        match self {
            Self::Station(key) => key.clone(),
            Self::Device(device) => format!("{DEVICE_CALIB_DIR}/{device}"),
        }
    }
}

/// 标定键对应的文件（`calib/` 在当前项目目录下）。
pub fn calib_key_path(app: &AppHandle, key: &CalibKey) -> Result<PathBuf, String> {
    Ok(key.file(&crate::storage::project(app).join("calib")))
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
    found
        .map(|(s, _)| s)
        .ok_or_else(|| format!("配方 {} 没有检测相机", recipe.id))
}

/// 旧拍照点配方的工位标定键：设备 + 图编号（三目拼接相机每幅图各一份标定）。
fn station_view_key(device: &str, view: u8) -> Result<String, String> {
    if !(1..=3).contains(&view) { return Err("标定图编号必须为 1、2 或 3".into()); }
    Ok(format!("{device}-v{view}"))
}

/// 段配方的设备标定键 `CalibKey::Device`（D-SB-30，三台独立单视角相机，没有图编号）。
/// 与旧配方的 `{device}-v{view}` 并存、文件在不同目录、永不同名（见 `CalibKey`）；标定页对单视角相机的图 1 同时写两份，见 `calib_keys`。
pub fn station_key(device: &str) -> Result<CalibKey, String> {
    if !crate::recipe::valid_camera_id(device) { return Err(format!("相机编号 {device:?} 只能用字母、数字、- 和 _")); }
    Ok(CalibKey::Device(device.to_string()))
}

/// 段配方一台检测相机的工位标定键：它的标定引用（`Station`），缺省为设备标定 `Device(device)`。
pub fn camera_calib_key(inspect: &CameraInspect) -> Result<CalibKey, String> {
    match &inspect.calib { Some(reference) => Ok(CalibKey::Station(reference.clone())), None => station_key(&inspect.camera) }
}

/// 段配方一台检测相机的标定文件：按它的标定引用（缺省为设备标定）找工位标定。
pub fn camera_calib_path(app: &AppHandle, inspect: &CameraInspect) -> Result<PathBuf, String> {
    calib_key_path(app, &camera_calib_key(inspect)?)
}

fn camera_id(app: &AppHandle, cam: u8) -> Result<String, String> {
    Ok(app.state::<CycleHost>().camera.slot(cam as usize).ok_or("相机不存在")?.config().id)
}

/// 标定命令的键：第一个是 `vision_calib_info` 读的键，全部是 `vision_calibrate` 写的键。
/// - 给了图编号（标定页总会给，单视角相机为 1）：旧配方的 `{device}-v{view}`。单视角相机的图 1 就是这台设备的整幅图，
///   同时写段配方读的设备标定 `device/{device}`（D-SB-30），标定页做的标定段配方才用得上；三目拼接相机的各图只写 `{device}-v{view}`。
/// - 没给：只写段配方的设备标定，这台相机必须是单视角（D-SB-11）。
fn calib_keys(config: &crate::camera::CameraConfig, view: Option<u8>) -> Result<Vec<CalibKey>, String> {
    let single_view = || crate::camera::check_burst_cameras(std::slice::from_ref(config), std::slice::from_ref(&config.id));
    match view {
        Some(view) => {
            let mut keys = vec![CalibKey::Station(station_view_key(&config.id, view)?)];
            if view == 1 && single_view().is_ok() { keys.push(station_key(&config.id)?); }
            Ok(keys)
        }
        None => {
            single_view()?;
            Ok(vec![station_key(&config.id)?])
        }
    }
}

fn camera_calib_keys(app: &AppHandle, cam: u8, view: Option<u8>) -> Result<Vec<CalibKey>, String> {
    calib_keys(&app.state::<CycleHost>().camera.slot(cam as usize).ok_or("相机不存在")?.config(), view)
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

/// `view` 给了读旧配方的 `{device}-v{view}`，没给读段配方的设备标定 `device/{device}`（见 `calib_keys`，单视角相机的图 1 两份同时写）。
#[tauri::command]
pub fn vision_calib_info(app: AppHandle, cam: Option<u8>, view: Option<u8>) -> Result<Option<CalibInfo>, String> {
    let keys = camera_calib_keys(&app, cam.unwrap_or(0), view)?;
    Ok(calib_info(&calib_key_path(&app, &keys[0])?))
}

fn save_calibration(
    project: &Path,
    paths: &[PathBuf],
    data: Value,
    image: &FrameImage,
    image_id: Option<&str>,
    camera_id: &str,
    view: u8,
) -> Result<(), String> {
    static NEXT_IMAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let generated = format!(
        "station-latest-{}-{}-{}",
        std::process::id(),
        ly_plc::now_ms(),
        NEXT_IMAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let sample_id = image_id.unwrap_or(&generated);
    if sample_id.is_empty()
        || !sample_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("标定样本编号无效".into());
    }
    let relative = format!("calib/images/{sample_id}.pgm");
    let image_path = project
        .join("calib")
        .join("images")
        .join(format!("{sample_id}.pgm"));
    std::fs::create_dir_all(image_path.parent().unwrap())
        .map_err(|e| format!("创建标定图目录失败：{e}"))?;
    crate::image_file::checked_source_path(image_path.parent().unwrap())?;
    crate::image_file::save_pgm_new(&image_path, image)?;
    let doc = json!({"kind": "Record", "type": "image.PlaneCalib", "data": data, "ts": ly_plc::now_ms(), "sampleId": sample_id, "samplePath": relative, "cameraId": camera_id, "view": view});
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    paths
        .iter()
        .try_for_each(|path| crate::fsio::write_atomic(path, &text))
}

/// 工位标定：用飞拍相机最近一帧整图跑 image.board_calib，结果存成工位标定文件。
/// `view` 给了存旧配方的 `{device}-v{view}`，单视角相机的图 1 还同时存段配方的设备标定 `device/{device}`；没给只存设备标定
/// （单视角相机，样本与最近一帧都是图 1）。键见 `calib_keys`。
#[tauri::command]
pub async fn vision_calibrate(app: AppHandle, pattern: [f64; 2], square: f64, cam: Option<u8>, view: Option<u8>, image_id: Option<String>) -> Result<CalibInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        let _gate = cycle.config_gate.try_lock().map_err(|_| "正在采集、标定或接件，请稍后重试")?;
        crate::camera::check_idle(&cycle)?;
        let keys = camera_calib_keys(&app, cam.unwrap_or(0), view)?;
        let view = view.unwrap_or(1);
        if app.state::<CycleHost>().busy() { return Err("设备正在检测，结束后再标定".into()); }
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
        let paths = keys.iter().map(|key| calib_key_path(&app, key)).collect::<Result<Vec<_>, _>>()?;
        for dir in paths.iter().filter_map(|path| path.parent()) {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if let Some(id)=image_id.as_deref() {
            if crate::workspace::station_image_view(&app, cam.unwrap_or(0), id)? != view { return Err("标定过程中样本视图已改变".into()); }
            crate::workspace::station_image_ref(&app,cam.unwrap_or(0),id)?;
        }
        let written = save_calibration(&crate::storage::project(&app), &paths, data, &image, image_id.as_deref(), &camera_id(&app,cam.unwrap_or(0))?, view);
        let _ = app.emit("calibration://changed", ());
        written?;
        calib_info(&paths[0]).ok_or_else(|| "标定文件写入后读不回来".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;

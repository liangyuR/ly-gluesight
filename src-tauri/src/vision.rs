//! lyFlow 视觉引擎：加载 core DLL，逐帧注入图像跑飞拍检测图（定位 + 逐点卡尺），读回 glue.Pose2D 与 glue.StationMeasure。
//! 图只量不判，判定在 judge 模块（设计稿 §9）。

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lyflow_client::{Core, RunHandle, RunImageInput, RunSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::camera::CameraSource;
use crate::cycle::CycleHost;
pub use crate::frame::FrameImage;
use crate::measure::{Job, Measured, Measurer};
use crate::recipe::Recipe;

/// 沿示教中线量胶的 lyFlow 流程接入前（P0 步 L），图像测量与示教试测都报这一句，不用模拟值顶替实物。
pub const TAUGHT_PATH_PENDING: &str = "沿示教中线量胶的 lyFlow 流程尚未接入（P0 步 L），图像测量暂不可用";


unsafe extern "C" fn ignore_event(_: *const c_char, _: *mut c_void) {}

pub struct Engine {
    core: Arc<Core>,
    pub path: PathBuf,
    pub version: String,
}

impl Engine {
    pub fn load(path: &Path) -> Result<Self, String> {
        let core = Core::load_from(path).map_err(|e| e.to_string())?;
        core.self_check()?;
        check_operators(&serde_json::from_str(&core.manifest_json().map_err(|e| e.to_string())?)
            .map_err(|e| format!("核心库算子清单无效：{e}"))?)?;
        let version = core.version();
        Ok(Self { core: Arc::new(core), path: path.to_path_buf(), version })
    }

    /// 跑一次图，返回 run summary 与图级命名输出（都已解析成 JSON）。
    pub fn run(&self, graph: &str, run_id: &str, base_dir: &str, image: &FrameImage, params: &Value) -> Result<RunResult, String> {
        let images = [image_input(image)?];
        let params_json = params.to_string();
        let mut spec = RunSpec::new(graph, run_id, base_dir, &[]).with_params_json(&params_json);
        // 正式测量只注入原始全分辨率像素。core 在 start 内拷贝，images 活到 start 返回。
        spec.image_inputs = &images;
        let handle = unsafe { RunHandle::start(self.core.clone(), spec, ignore_event, Box::new(())) }.map_err(|e| e.to_string())?;
        handle.join();
        let summary_json = self
            .core
            .run_summary(run_id)
            .map_err(|e| e.to_string())?
            .ok_or("算法没有返回运行摘要")?;
        let summary: Value = serde_json::from_str(&summary_json).map_err(|e| format!("算法运行摘要无效：{e}"))?;
        let outputs: Value = serde_json::from_str(&self.core.run_outputs(run_id).map_err(|e| e.to_string())?)
            .map_err(|e| format!("算法输出无效：{e}"))?;
        drop(handle);
        Ok(RunResult { summary, outputs })
    }
}

fn check_operators(manifest: &Value) -> Result<(), String> {
    let required = ["io.load_image", "image.board_calib", "image.load_calib", "glue.locate", "glue.station_calipers"];
    let ops = manifest["operators"].as_array().ok_or("核心库没有算子清单")?;
    let missing: Vec<_> = required.into_iter().filter(|id| !ops.iter().any(|op| op["id"].as_str() == Some(id))).collect();
    if missing.is_empty() { Ok(()) } else { Err(format!("核心库缺少飞拍/标定算子：{}。请选择包含胶路检测功能的核心库。", missing.join("、"))) }
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
pub struct LyFlowMeasurer {
    pub app: AppHandle,
}

impl Measurer for LyFlowMeasurer {
    fn measure(&self, job: &Job, _image: &FrameImage) -> Result<Measured, String> {
        let settings = self.app.state::<CycleHost>().settings();
        self.app.state::<VisionHost>().engine(settings.lyflow_core.as_deref()).ok_or("lyFlow 核心库未加载")?;
        let shot = job.recipe.shots.get(job.k).ok_or("拍照点不存在")?;
        if !shot.taught() {
            return Err(format!("拍照点 {} 尚未示教胶路", shot.id));
        }
        Err(TAUGHT_PATH_PENDING.into())
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
    pub path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

fn resolve_core_path(path: Option<&str>, executable: impl FnOnce() -> std::io::Result<PathBuf>) -> Result<Option<PathBuf>, String> {
    if let Some(path) = path.map(str::trim).filter(|p| !p.is_empty()) {
        return Ok(Some(PathBuf::from(if cfg!(windows) { path.replace('/', "\\") } else { path.to_string() })));
    }
    if cfg!(windows) {
        let executable = executable().map_err(|e| format!("无法确定内置 lyFlow 核心库路径：{e}"))?;
        let dir = executable.parent().ok_or("无法确定程序安装目录")?;
        Ok(Some(dir.join("runtime").join("lyflow").join("lyflow_core.dll")))
    } else {
        Ok(None)
    }
}

/// lyFlow 引擎。优先按设置路径加载，否则使用安装目录内的核心库。
#[derive(Default)]
pub struct VisionHost {
    engine: Mutex<Option<Arc<Engine>>>,
    error: Mutex<Option<String>>,
}

impl VisionHost {
    pub fn engine(&self, path: Option<&str>) -> Option<Arc<Engine>> {
        match resolve_core_path(path, std::env::current_exe) {
            Ok(Some(resolved)) => self.engine_at(&resolved, path.map(str::trim).filter(|p| !p.is_empty()).is_none()),
            Ok(None) => None,
            Err(message) => {
                *self.error.lock().unwrap() = Some(message);
                None
            }
        }
    }

    fn engine_at(&self, path: &Path, bundled: bool) -> Option<Arc<Engine>> {
        let mut guard = self.engine.lock().unwrap();
        if let Some(e) = guard.as_ref().filter(|e| e.path == path) {
            return Some(e.clone());
        }
        let loaded = if bundled && !path.is_file() {
            Err(format!("安装目录缺少内置 lyFlow 核心库：{}。请重新安装完整安装包，或在系统设置中指定核心库路径。", path.display()))
        } else {
            Engine::load(path)
        };
        match loaded {
            Ok(e) => {
                let e = Arc::new(e);
                *guard = Some(e.clone());
                *self.error.lock().unwrap() = None;
                Some(e)
            }
            Err(msg) => {
                *guard = None;
                *self.error.lock().unwrap() = Some(msg);
                None
            }
        }
    }

    pub fn status(&self, path: Option<&str>) -> EngineStatus {
        let resolved = match resolve_core_path(path, std::env::current_exe) {
            Ok(path) => path,
            Err(message) => return EngineStatus { message, ..EngineStatus::default() },
        };
        let bundled = path.map(str::trim).filter(|p| !p.is_empty()).is_none();
        let engine = resolved.as_deref().and_then(|path| self.engine_at(path, bundled));
        EngineStatus {
            loaded: engine.is_some(),
            path: resolved.as_ref().map(|path| path.display().to_string()),
            version: engine.as_ref().map(|e| e.version.clone()),
            message: match (&engine, &resolved) {
                (Some(_), _) => "已加载".into(),
                (None, None) => "未配置核心库路径，飞拍件会判 ERR（不用 lyFlow 时在系统设置里把飞拍改为模拟测量）".into(),
                (None, Some(_)) => self.error.lock().unwrap().clone().unwrap_or_else(|| "加载失败".into()),
            },
        }
    }

}

/// 工位标定文件（标定属于相机工位，换型不重标）。按相机编号存，增删别的相机也跟着这台相机走；cam1 沿用单相机时代的文件名。
pub fn station_calib_path(app: &AppHandle, cam_id: &str) -> Result<PathBuf, String> {
    let name = if cam_id == crate::recipe::legacy_camera_id(0) { "plane_calib.json".to_string() } else { format!("plane_calib_{cam_id}.json") };
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("calib").join(name))
}

/// 拍照点 k 的标定文件：按它的标定引用（缺省为相机编号）找工位标定。
pub fn shot_calib_path(app: &AppHandle, recipe: &Recipe, k: usize) -> Result<PathBuf, String> {
    station_calib_path(app, recipe.shots.get(k).ok_or("拍照点不存在")?.calib_ref())
}

/// 配方各相机的来源：要么全是模拟，要么全是真实 / 回放（示教资料与标定的来路不同）。
pub fn recipe_source(app: &AppHandle, recipe: &Recipe) -> Result<CameraSource, String> {
    let rig = &app.state::<CycleHost>().camera;
    let mut found: Option<(CameraSource, String)> = None;
    for id in recipe.cameras() {
        let source = rig.slot(rig.require(&id)? as usize).ok_or("相机不存在")?.config().source;
        match &found {
            Some((first, first_id)) if (*first == CameraSource::Sim) != (source == CameraSource::Sim) => {
                return Err(format!("配方 {} 的相机来源不一致：{first_id} 是 {first:?}，{id} 是 {source:?}；要么全用模拟相机，要么全用真实 / 回放相机", recipe.id));
            }
            Some(_) => {}
            None => found = Some((source, id)),
        }
    }
    found.map(|(s, _)| s).ok_or_else(|| format!("配方 {} 没有拍照点", recipe.id))
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
pub fn vision_calib_info(app: AppHandle, cam: Option<u8>) -> Result<Option<CalibInfo>, String> {
    Ok(calib_info(&station_calib_path(&app, &camera_id(&app, cam.unwrap_or(0))?)?))
}

/// 工位标定：用飞拍相机最近一帧整图跑 image.board_calib，结果存成工位标定文件。
#[tauri::command]
pub async fn vision_calibrate(app: AppHandle, pattern: [f64; 2], square: f64, cam: Option<u8>, image_id: Option<String>) -> Result<CalibInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if app.state::<CycleHost>().busy() { return Err("工件正在检测，结束后再标定".into()); }
        if pattern.iter().any(|n|!n.is_finite()||*n<2.0||*n>100.0||n.fract()!=0.0)||!square.is_finite()||square<=0.0 { return Err("棋盘格内角点与格长无效".into()); }
        let settings = app.state::<CycleHost>().settings();
        let engine = app.state::<VisionHost>().engine(settings.lyflow_core.as_deref()).ok_or("lyFlow 核心库未加载")?;
        let image = if let Some(id)=image_id.as_deref() { crate::workspace::station_image_ref(&app,cam.unwrap_or(0),id)? } else { app
            .state::<CycleHost>()
            .camera
            .last_full(cam.unwrap_or(0))
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
        let path = station_calib_path(&app, &camera_id(&app, cam.unwrap_or(0))?)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if let Some(id)=image_id.as_deref() { crate::workspace::station_image_ref(&app,cam.unwrap_or(0),id)?; }
        let doc = json!({"kind": "Record", "type": "image.PlaneCalib", "data": data, "ts": ly_plc::now_ms(), "sampleId":image_id});
        std::fs::write(&path, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        calib_info(&path).ok_or_else(|| "标定文件写入后读不回来".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;

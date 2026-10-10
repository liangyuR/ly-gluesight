//! lyFlow 视觉引擎：加载 core DLL，逐帧注入完整图像，沿拍照点示教中线量胶。
//! 图只量不判，判定在 judge 模块（设计稿 §9）。

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lyflow_client::{Core, RunHandle, RunImageInput, RunSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::camera::CameraSource;
use crate::cycle::CycleHost;
pub use crate::frame::FrameImage;
use crate::measure::{Job, Measured, Measurer};
use crate::recipe::Recipe;

mod taught;
pub use taught::{build_taught_graph, measure_shot, measure_shot_with_graph, ShotMeasurement};

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
        let core = Core::load_from(path).map_err(|e| e.to_string())?;
        core.self_check()?;
        check_operators(&serde_json::from_str(&core.manifest_json().map_err(|e| e.to_string())?)
            .map_err(|e| format!("核心库算子清单无效：{e}"))?)?;
        let version = core.version();
        let (bytes, modified_ms) = file_identity(path)?;
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
        let prepared = job.production.as_ref().ok_or("图像检测没有已核验并预热的发布包")?;
        if job.bundle_id.as_deref() != Some(prepared.bundle.id.as_str()) || job.recipe.revision_id != prepared.recipe.revision_id {
            return Err("测量任务与已冻结发布包不一致".into());
        }
        let run_id = format!("shot-{}-{}", job.cycle_id, job.k);
        Ok(prepared.measure(job.k, image, &run_id)?.into_measured(job))
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
        let _ = app.emit("calibration://changed", ());
        calib_info(&path).ok_or_else(|| "标定文件写入后读不回来".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;

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

    #[test]
    #[ignore = "requires LYFLOW_CORE_DLL with glue.taught_path; concurrent same-label pixel isolation"]
    fn native_concurrent_identical_labels_keep_each_pixel_measurement() {
        let dll = std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL");
        let engine = Engine::load(Path::new(&dll)).unwrap();
        let mut doc = crate::recipe::samples().remove(0);
        doc.shots.truncate(1);
        doc.shots[0].path = vec![[40.0, 80.0], [200.0, 80.0]];
        doc.shots[0].mm_per_px = Some(0.25);
        doc.spacing = 1.0;
        doc.detect = crate::recipe::DetectParams {
            search_mm: 8.0,
            polarity: crate::recipe::Polarity::Dark,
            width_range: [2.0, 6.0],
        };
        let graph = build_taught_graph(&doc.build().unwrap(), 0).unwrap().to_string();
        let cases = [(74, 10), (78, 14), (84, 18), (90, 22)];
        let barrier = Barrier::new(cases.len());
        let readings = std::thread::scope(|scope| {
            let workers: Vec<_> = cases.into_iter().map(|(center, width)| {
                let (engine, graph, barrier) = (&engine, &graph, &barrier);
                scope.spawn(move || {
                    let mut pixels = vec![220; 256 * 160];
                    for y in center - width / 2..center + width / 2 {
                        for x in 16..240 { pixels[y * 256 + x] = 28; }
                    }
                    let image = FrameImage::new(256, 160, pixels);
                    let runs = (0..12).map(|_| {
                        barrier.wait();
                        engine.run(graph, "same-run-label", "", &image, &json!({}))
                    }).collect::<Vec<_>>();
                    (center, width, runs)
                })
            }).collect();
            workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
        });
        let mut run_ids = HashSet::new();
        for (center, width, runs) in readings {
            for result in runs {
                let result = result.unwrap();
                assert_eq!(result.status(), "ok", "{}", result.failure());
                let run_id = result.summary["runId"].as_str().unwrap();
                assert!(run_ids.insert(run_id.to_string()), "duplicate native run: {run_id}");
                let stations = result.record("stations").unwrap();
                assert_eq!(stations["count"], 41);
                for i in 0..41 {
                    assert_eq!(stations["present"][i], true);
                    let actual_width = stations["widthPx"][i].as_f64().unwrap();
                    assert!((actual_width - width as f64).abs() < 1.6, "width {actual_width} != {width}");
                    let lo = stations["lo"][i].as_f64().unwrap();
                    let hi = stations["hi"][i].as_f64().unwrap();
                    let offset = (lo + hi) * 0.5;
                    let expected = center as f64 - 80.5;
                    assert!((offset - expected).abs() < 1.2, "offset {offset} != {expected}");
                }
            }
        }
        assert_eq!(run_ids.len(), 48);
    }
}

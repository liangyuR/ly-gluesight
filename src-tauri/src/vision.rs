//! lyFlow 视觉引擎：加载 core DLL，逐帧注入图像跑飞拍检测图（定位 + 逐点卡尺），读回 glue.Pose2D 与 glue.StationMeasure。
//! 图只量不判，判定在 judge 模块（设计稿 §9）。

use std::collections::HashMap;
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
use crate::measure::{Job, JobKind, Measured, Measurer, ST_GAP, ST_INVALID, ST_OK};
use crate::recipe::Recipe;
use crate::simimage;

/// 与 LyFlow packs/glue/graphs/flyshot.lyflow.json 同一份；本程序固定用它。
pub const FLYSHOT_GRAPH: &str = include_str!("../resources/flyshot.lyflow.json");


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

/// lyFlow 飞拍流程（定位 + 逐点卡尺）作为测量后端。
pub struct LyFlowMeasurer {
    pub app: AppHandle,
}

impl Measurer for LyFlowMeasurer {
    fn measure(&self, job: &Job, image: &FrameImage) -> Result<Measured, String> {
        let JobKind::Shot { k } = job.kind else {
            return Err("lyFlow 流程目前只有飞拍版本；随动配方请在系统设置里改用本程序卡尺".into());
        };
        let app = &self.app;
        let settings = app.state::<CycleHost>().settings();
        let engine = app.state::<VisionHost>().engine(settings.lyflow_core.as_deref()).ok_or("lyFlow 核心库未加载")?;
        let assets = assets_for(app, &job.recipe)?;
        if !assets.calib.exists() {
            return Err("工位未标定：先在图像源页用标定板标定".into());
        }
        let params = assets.params(k).ok_or_else(|| format!("拍照点 k={k} 没有示教资料"))?;
        let base = assets.calib.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let run_id = format!("{}-k{k}-{}", job.sn, ly_plc::now_ms());
        let graph = assets.shots.get(k).map(|s| crate::workspace::station_graph(&s.stations)).unwrap_or_else(|| FLYSHOT_GRAPH.to_string());
        let r = engine.run(&graph, &run_id, &base, image, &params)?;
        if r.status() == "failed" {
            return Err(r.failure());
        }
        let pose: Pose = r.record("pose").and_then(|v| serde_json::from_value(v.clone()).ok()).ok_or("图没有输出 pose")?;
        let sm: StationMeasure = r.record("measure").and_then(|v| serde_json::from_value(v.clone()).ok()).ok_or("图没有输出 measure")?;
        sm.into_measured(job, pose)
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

#[derive(Clone, Debug, Deserialize)]
pub struct Pose {
    pub ok: bool,
    #[serde(default)]
    pub score: f64,
}

/// glue.StationMeasure 里本程序用到的字段。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationMeasure {
    pub unit: String,
    pub ids: Vec<Value>,
    pub status: Vec<String>,
    pub inner_center: Vec<Option<f64>>,
    pub width: Vec<Option<f64>>,
    pub point: Vec<[f64; 2]>,
}

impl StationMeasure {
    fn into_measured(self, job: &Job, pose: Pose) -> Result<Measured, String> {
        if self.unit != "mm" { return Err(format!("测量单位是 {}，标定文件不是毫米", self.unit)); }
        let n = self.ids.len();
        if n == 0 || [self.status.len(), self.inner_center.len(), self.width.len(), self.point.len()].iter().any(|&len| len != n) {
            return Err("算法点表为空或各列长度不一致".into());
        }
        if n != job.recipe.owned_points(job.k).count() { return Err("算法点表未包含本帧的全部测量点".into()); }
        let mut seen = std::collections::HashSet::new();
        let mut m = Measured { located: pose.ok, score: pose.score as f32, ..Measured::empty(job) };
        for (i, id) in self.ids.iter().enumerate() {
            let j = id.as_u64().and_then(|j| usize::try_from(j).ok())
                .filter(|&j| j < job.recipe.point_count() && job.recipe.points.k[j] as usize == job.k)
                .ok_or("算法输出了不属于本帧的测量点")?;
            if !seen.insert(j) { return Err("算法输出了重复测量点".into()); }
            let valid = |v: f64| v.is_finite() && (v as f32).is_finite();
            if self.point[i].iter().any(|&v| !valid(v)) { return Err("算法输出了无效的图像坐标".into()); }
            let (d, w, st) = match (self.status[i].as_str(), self.inner_center[i], self.width[i]) {
                ("ok", Some(d), Some(w)) if pose.ok && valid(d) && valid(w) && w > 0.0 => (d as f32, w as f32, ST_OK),
                ("no_bead", _, _) if pose.ok => (0.0, f32::NAN, ST_GAP),
                _ => (0.0, f32::NAN, ST_INVALID),
            };
            m.idx.push(j as u32);
            m.d.push(d);
            m.w.push(w);
            m.st.push(st);
            m.px.push(self.point[i].map(|v| v as f32));
        }
        Ok(m)
    }
}

/// 一个拍照点的视觉资料（示教产物）：模板、模板锚点、测量点文件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotAssets {
    pub template: PathBuf,
    pub anchor: [f64; 2],
    pub stations: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionAssets {
    pub recipe_id: String,
    pub recipe_hash: String,
    /// 模拟相机的像素当量；真实相机为 None（用工位标定）
    pub sim_mm_per_px: Option<f64>,
    pub calib: PathBuf,
    pub shots: Vec<ShotAssets>,
    /// 示教时的飞拍相机编号（旧文件没有，不核对）
    #[serde(default)]
    pub camera: String,
}

impl VisionAssets {
    pub fn params(&self, k: usize) -> Option<Value> {
        let s = self.shots.get(k).filter(|s| !s.template.as_os_str().is_empty())?;
        let p = |p: &Path| p.to_string_lossy().replace('\\', "/");
        Some(json!({
            "template": p(&s.template),
            "anchor": s.anchor,
            "stations": p(&s.stations),
            "calib": p(&self.calib),
        }))
    }

    /// 模板与测量点文件按清单所在目录找：配方改名时整个目录跟着搬。
    pub fn load(path: &Path) -> Option<Self> {
        let mut a: Self = crate::fsio::read_text(path).ok().and_then(|s| serde_json::from_str(&s).ok())?;
        let dir = path.parent()?;
        for s in &mut a.shots {
            for p in [&mut s.template, &mut s.stations] {
                if let Some(name) = p.file_name() {
                    *p = dir.join(name);
                }
            }
        }
        Some(a)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::fsio::write_atomic(path, &serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
    }

    /// 真实相机的示教资料是否还对得上配方：胶路几何、拍照点没改，飞拍相机没换。
    pub fn fits(&self, recipe: &Recipe) -> Result<(), String> {
        if self.recipe_hash != recipe.geometry_hash() {
            return Err(format!("配方 {} 的胶路或拍照点改过，需要重新示教", recipe.id));
        }
        if !self.camera.is_empty() && self.camera != recipe.camera {
            return Err(format!("配方 {} 换了飞拍相机（示教时是 {}），需要重新示教", recipe.id, self.camera));
        }
        Ok(())
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

/// 引擎与各配方视觉资料的缓存。优先按设置路径加载，否则使用安装目录内的核心库。
#[derive(Default)]
pub struct VisionHost {
    engine: Mutex<Option<Arc<Engine>>>,
    error: Mutex<Option<String>>,
    assets: Mutex<HashMap<String, Arc<VisionAssets>>>,
    generating: Mutex<()>,
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

    pub fn assets(&self, key: &str) -> Option<Arc<VisionAssets>> {
        self.assets.lock().unwrap().get(key).cloned()
    }

    pub fn set_assets(&self, key: String, assets: Arc<VisionAssets>) {
        self.assets.lock().unwrap().insert(key, assets);
    }

    /// 重新示教后丢掉该配方的缓存。
    pub fn forget(&self, recipe_id: &str) {
        self.assets.lock().unwrap().retain(|k, _| !k.contains(&format!(":{recipe_id}:")));
    }
}

/// 工位标定文件（标定属于相机工位，换型不重标）。按相机编号存，增删别的相机也跟着这台相机走；cam1 沿用单相机时代的文件名。
pub fn station_calib_path(app: &AppHandle, cam_id: &str) -> Result<PathBuf, String> {
    let name = if cam_id == crate::recipe::legacy_camera_id(0) { "plane_calib.json".to_string() } else { format!("plane_calib_{cam_id}.json") };
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("calib").join(name))
}

fn camera_id(app: &AppHandle, cam: u8) -> Result<String, String> {
    Ok(app.state::<CycleHost>().camera.slot(cam as usize).ok_or("相机不存在")?.config().id)
}

/// 示教资料的清单文件名（在 taught_dir 里）。
pub const ASSETS_FILE: &str = "vision.json";

/// 真实相机的飞拍示教资料目录：按配方编号，示教向导写、测量时读。
pub fn taught_dir(app: &AppHandle, recipe_id: &str) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("vision").join("taught").join(recipe_id))
}

/// 拍照点 k 的测量点文件（lyFlow 飞拍图的 stations 输入）。
pub fn save_stations(dir: &Path, k: usize, points: Vec<Value>, normals: Vec<Value>, ids: Vec<usize>) -> Result<PathBuf, String> {
    let path = dir.join(format!("k{k}.stations.json"));
    std::fs::write(&path, json!({"points": points, "normals": normals, "ids": ids}).to_string()).map_err(|e| format!("写测量点文件失败：{e}"))?;
    Ok(path)
}

/// 配方在当前相机下的视觉资料。模拟相机按名义几何自动生成（按配方哈希缓存）；
/// 真实相机用示教向导的产物，标定取工位标定文件。
pub fn assets_for(app: &AppHandle, recipe: &Recipe) -> Result<Arc<VisionAssets>, String> {
    let rig = &app.state::<CycleHost>().camera;
    let slot = rig.slot(rig.require(&recipe.camera)? as usize).ok_or("相机不存在")?;
    let source = slot.config().source;
    let host = app.state::<VisionHost>();
    let key = format!("{source:?}:{}:{}", recipe.id, recipe.hash);
    if let Some(a) = host.assets(&key) {
        return Ok(a);
    }
    let _generating = host.generating.lock().unwrap();
    if let Some(a) = host.assets(&key) {
        return Ok(a);
    }
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?.join("vision");
    let name = format!("{}-{}", recipe.id, &recipe.hash[..8.min(recipe.hash.len())]);
    let assets = match source {
        CameraSource::Sim => {
            if recipe.teaching_hash.is_some() {
                if let Some(a) = VisionAssets::load(&taught_dir(app, &recipe.id)?.join(ASSETS_FILE))
                    .filter(|a| a.fits(recipe).is_ok() && a.calib.exists() && a.shots.iter().all(|s| s.template.exists() && s.stations.exists())) {
                    let assets = Arc::new(a);
                    host.set_assets(key, assets.clone());
                    return Ok(assets);
                }
            }
            if !matches!(recipe.path, Some(crate::recipe::PathSpec::RoundedRect { .. })) {
                return Err("模拟相机只会合成圆角矩形胶路的飞拍图像".into());
            }
            let dir = root.join("sim").join(name);
            let file = dir.join(ASSETS_FILE);
            match VisionAssets::load(&file).filter(|a| a.recipe_hash == recipe.hash && a.shots.iter().all(|s| s.template.exists())) {
                Some(a) => a,
                None => {
                    let a = simimage::teach(recipe, &dir)?;
                    a.save(&file)?;
                    a
                }
            }
        }
        CameraSource::Mvs | CameraSource::Replay => {
            let mut a = VisionAssets::load(&taught_dir(app, &recipe.id)?.join(ASSETS_FILE)).ok_or_else(|| format!("配方 {} 尚未示教", recipe.id))?;
            a.fits(recipe)?;
            a.calib = station_calib_path(app, &recipe.camera)?;
            a
        }
    };
    let assets = Arc::new(assets);
    host.set_assets(key, assets.clone());
    Ok(assets)
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

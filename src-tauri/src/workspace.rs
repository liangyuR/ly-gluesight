use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::camera::CameraSource;
use crate::cycle::{self, CycleHost};
use crate::frame::FrameImage;
use crate::judge::{self, PointState, Verdict};
use crate::recipe::{InspectMode, Recipe, RecipeDoc};
use crate::store::Store;
use crate::vision::{self, ShotAssets, VisionAssets, VisionHost};

fn fingerprint(value: &impl Serialize) -> String {
    let mut h = DefaultHasher::new();
    serde_json::to_string(value)
        .unwrap_or_default()
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

fn safe_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 32
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("配方编号只能包含字母、数字、下划线和短横线，最多 32 位".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameParams {
    pub rect: [u32; 4],
    pub dx: f32,
    pub dy: f32,
    pub deg: f32,
    pub mm_per_px: f32,
    pub search_mm: f32,
    pub min_contrast: f32,
    pub min_score: f32,
}

impl Default for FrameParams {
    fn default() -> Self {
        Self {
            rect: [0, 0, 0, 0],
            dx: 0.0,
            dy: 0.0,
            deg: 0.0,
            mm_per_px: 0.04,
            search_mm: 4.0,
            min_contrast: 25.0,
            min_score: 0.6,
        }
    }
}

impl FrameParams {
    fn validate(&self, size: [u32; 2]) -> Result<(), String> {
        let [x, y, w, h] = self.rect;
        if w < 16
            || h < 16
            || x.checked_add(w).is_none_or(|n| n > size[0])
            || y.checked_add(h).is_none_or(|n| n > size[1])
        {
            return Err("请在冻结图像内框选至少 16×16 px 的定位模板".into());
        }
        if ![
            self.dx,
            self.dy,
            self.deg,
            self.mm_per_px,
            self.search_mm,
            self.min_contrast,
            self.min_score,
        ]
        .iter()
        .all(|v| v.is_finite())
            || self.mm_per_px <= 0.0
            || self.search_mm <= 0.0
            || !(0.0..=255.0).contains(&self.min_contrast)
            || !(0.0..=1.0).contains(&self.min_score)
        {
            return Err("像素当量、搜索余量、对比度或定位分数无效".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenImage {
    pub id: String,
    pub source: String,
    pub captured_at: i64,
    pub size: [u32; 2],
    pub camera: String,
    pub camera_tag: String,
    pub calib_tag: String,
    pub geometry_tag: String,
    pub exposure_us: Option<f32>,
    pub gain_db: Option<f32>,
    pub history_id: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    pub image_id: String,
    pub params_tag: String,
    pub geometry_tag: String,
    pub passed: bool,
    pub score: f64,
    pub coverage: f64,
    pub elapsed_ms: u128,
    pub reason: String,
    pub measurement: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Teaching {
    pub k: usize,
    pub image: Option<FrozenImage>,
    pub params: FrameParams,
    pub trial: Option<Trial>,
    pub saved: bool,
    pub backup: Option<Box<Teaching>>,
}

impl Teaching {
    fn empty(k: usize) -> Self {
        Self {
            k,
            image: None,
            params: FrameParams::default(),
            trial: None,
            saved: false,
            backup: None,
        }
    }

    fn checked(&self, geometry: &str) -> Result<(), String> {
        let image = self.image.as_ref().ok_or("本帧尚未冻结原图")?;
        let trial = self.trial.as_ref().ok_or("本帧尚未试测")?;
        if image.geometry_tag != geometry
            || !trial.passed
            || trial.image_id != image.id
            || trial.geometry_tag != geometry
            || trial.params_tag != fingerprint(&self.params)
        {
            return Err("图像或参数已变更，需要重新试测".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub background: Option<String>,
    pub positions: Vec<[f32; 2]>,
    pub saved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub history_id: Option<i64>,
    #[serde(default)]
    pub sample_id: Option<String>,
    pub expected: Verdict,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BankSample {
    pub id: String,
    pub name: String,
    pub geometry_tag: String,
    pub expected: Verdict,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleResult {
    pub history_id: Option<i64>,
    pub sample_id: Option<String>,
    pub name: String,
    pub sn: u32,
    pub expected: Verdict,
    pub actual: Option<Verdict>,
    pub passed: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    pub revision: u64,
    pub passed: bool,
    pub checked_at: i64,
    pub checks: Vec<Check>,
    pub samples: Vec<SampleResult>,
    pub environment_tag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub doc: RecipeDoc,
    pub base_hash: Option<String>,
    pub revision: u64,
    pub frames: Vec<Teaching>,
    pub overview: Overview,
    pub validation: Validation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub doc: RecipeDoc,
    pub base_hash: Option<String>,
    pub revision: u64,
    pub frames: Vec<Teaching>,
    pub overview: Overview,
    pub samples: Vec<Sample>,
    #[serde(default)]
    pub sample_bank: Vec<BankSample>,
    pub validation: Option<Validation>,
    pub pending: Option<Box<Release>>,
    pub publish_error: Option<String>,
    pub updated_at: i64,
}

impl Workspace {
    fn new(mut doc: RecipeDoc, base_hash: Option<String>) -> Self {
        if base_hash.is_some() {
            doc.version += 1;
        }
        let count = if doc.mode == InspectMode::FlyShot {
            doc.shots.len()
        } else {
            0
        };
        Self {
            doc,
            base_hash,
            revision: 1,
            frames: (0..count).map(Teaching::empty).collect(),
            overview: Overview::default(),
            samples: Vec::new(),
            sample_bank: Vec::new(),
            validation: None,
            pending: None,
            publish_error: None,
            updated_at: ly_plc::now_ms(),
        }
    }

    fn expect(&self, revision: u64) -> Result<(), String> {
        if self.revision != revision {
            return Err("候选配置已更新，请重新加载后操作".into());
        }
        Ok(())
    }

    fn changed(&mut self) {
        self.revision += 1;
        self.validation = None;
        self.updated_at = ly_plc::now_ms();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    pub workspace: Workspace,
    pub layout: Recipe,
    pub production_version: Option<u32>,
    pub coverage: f32,
}

pub struct WorkspaceHost {
    root: PathBuf,
    items: Mutex<HashMap<String, Workspace>>,
    live: Mutex<LiveFrames>,
    station: Mutex<HashMap<u8, (FrozenImage, Arc<FrameImage>)>>,
    capture_seq: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct LiveFrames {
    key: Option<(u32, String)>,
    frames: VecDeque<(usize, Arc<FrameImage>)>,
    bytes: usize,
}

impl LiveFrames {
    fn insert(&mut self, sn: u32, hash: &str, k: usize, image: Arc<FrameImage>) {
        if self
            .key
            .as_ref()
            .is_none_or(|key| key.0 != sn || key.1 != hash)
        {
            *self = Self {
                key: Some((sn, hash.into())),
                ..Self::default()
            };
        }
        if self.frames.iter().any(|(index, _)| *index == k) {
            return;
        }
        const LIMIT: usize = 128 * 1024 * 1024;
        if image.pixels.len() > LIMIT {
            return;
        }
        while self.bytes + image.pixels.len() > LIMIT {
            let Some((_, old)) = self.frames.pop_front() else {
                break;
            };
            self.bytes -= old.pixels.len();
        }
        self.bytes += image.pixels.len();
        self.frames.push_back((k, image));
    }
}

impl WorkspaceHost {
    /// 候选与已提交的发布快照都保留相机引用，删除相机前核对。
    pub fn camera_users(&self, camera: &str) -> Vec<String> {
        let uses = |doc: &RecipeDoc| match doc.mode {
            InspectMode::FlyShot => doc.camera == camera,
            InspectMode::Follow => doc.follow.as_ref().is_some_and(|f| f.cameras.iter().any(|id| id == camera)),
        };
        self.items.lock().unwrap().values()
            .filter(|w| uses(&w.doc) || w.pending.as_ref().is_some_and(|p| uses(&p.doc)))
            .map(|w| w.doc.id.clone()).collect()
    }

    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("workspaces");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut items = HashMap::new();
        for entry in std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let file = entry.path().join("workspace.json");
            if let Ok(text) = crate::fsio::read_text(&file) {
                if let Ok(w) = serde_json::from_str::<Workspace>(&text) {
                    if safe_id(&w.doc.id).is_ok()
                        && w.doc.id == entry.file_name().to_string_lossy()
                        && w.doc.build().is_ok()
                    {
                        items.insert(w.doc.id.clone(), w);
                    }
                }
            }
        }
        Ok(Self {
            root,
            items: Mutex::new(items),
            live: Mutex::new(LiveFrames::default()),
            station: Mutex::new(HashMap::new()),
            capture_seq: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn save(&self, w: &Workspace) -> Result<(), String> {
        safe_id(&w.doc.id)?;
        let dir = self.dir(&w.doc.id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        crate::fsio::write_atomic(
            &dir.join("workspace.json"),
            &serde_json::to_string_pretty(w).map_err(|e| e.to_string())?,
        )
    }
}

fn coverage(recipe: &Recipe, frames: &[Teaching]) -> f32 {
    if recipe.mode == InspectMode::Follow {
        return 100.0;
    }
    let n = recipe.point_count();
    if n == 0 {
        return 0.0;
    }
    let found = (0..n)
        .filter(|&j| {
            let k = recipe.points.k[j] as usize;
            let Some(c) = recipe.shots.get(k) else {
                return false;
            };
            let margin = frames.get(k).map_or(4.0, |f| f.params.search_mm);
            (recipe.points.x[j] - c[0]).abs() + margin <= recipe.fov[0] / 2.0
                && (recipe.points.y[j] - c[1]).abs() + margin <= recipe.fov[1] / 2.0
        })
        .count();
    100.0 * found as f32 / n as f32
}

fn view(app: &AppHandle, w: Workspace) -> Result<WorkspaceView, String> {
    let layout = w.doc.build()?;
    let production_version = app
        .state::<CycleHost>()
        .recipe(&w.doc.id)
        .map(|r| r.version);
    let coverage = coverage(&layout, &w.frames);
    Ok(WorkspaceView {
        workspace: w,
        layout,
        production_version,
        coverage,
    })
}

fn environment(app: &AppHandle, recipe: &Recipe) -> Result<String, String> {
    let cycle = app.state::<CycleHost>();
    let mut values = Vec::new();
    for id in recipe.cameras() {
        let cam = cycle.camera.require(&id)?;
        let config = cycle
            .camera
            .slot(cam as usize)
            .ok_or("相机不存在")?
            .config();
        let calib = vision::station_calib_path(app, &id)?;
        values.push(json!([config, crate::fsio::read_text(&calib).ok()]));
    }
    let settings = cycle.settings();
    values.push(json!([
        settings.vision,
        settings.follow_vision,
        settings.lyflow_core
    ]));
    Ok(fingerprint(&values))
}

fn tags(app: &AppHandle, recipe: &Recipe) -> Result<(String, String), String> {
    let cycle = app.state::<CycleHost>();
    let cam = cycle.camera.require(&recipe.camera)?;
    let config = cycle
        .camera
        .slot(cam as usize)
        .ok_or("相机不存在")?
        .config();
    let calib = vision::station_calib_path(app, &recipe.camera)?;
    Ok((
        fingerprint(&config),
        fingerprint(&crate::fsio::read_text(&calib).ok()),
    ))
}

#[tauri::command]
pub fn workspace_list(host: State<'_, WorkspaceHost>) -> Vec<Workspace> {
    let mut out: Vec<_> = host.items.lock().unwrap().values().cloned().collect();
    out.sort_by(|a, b| a.doc.id.cmp(&b.doc.id));
    out
}

#[tauri::command]
pub fn workspace_get(app: AppHandle, id: String) -> Result<WorkspaceView, String> {
    safe_id(&id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    if !items.contains_key(&id) {
        let cycle = app.state::<CycleHost>();
        let doc = cycle.recipes.doc(&id).ok_or("配方不存在")?;
        let w = Workspace::new(doc, cycle.recipe(&id).map(|r| r.hash.clone()));
        host.save(&w)?;
        items.insert(id.clone(), w);
    }
    view(&app, items[&id].clone())
}

#[tauri::command]
pub fn workspace_create(app: AppHandle, doc: RecipeDoc) -> Result<WorkspaceView, String> {
    safe_id(&doc.id)?;
    doc.build()?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let production = app.state::<CycleHost>().recipes.list();
    check_new_identity(&doc, items.values().map(|w| (w.doc.id.as_str(), w.doc.product_code))
        .chain(production.iter().map(|r| (r.id.as_str(), r.product_code))))?;
    // Windows 路径不区分大小写，未登记的旧目录也不能被新候选覆盖。
    if host.dir(&doc.id).exists() { return Err("配方目录已经存在，请换一个编号".into()); }
    let w = Workspace::new(doc, None);
    host.save(&w)?;
    items.insert(w.doc.id.clone(), w.clone());
    view(&app, w)
}

fn check_new_identity<'a>(doc: &RecipeDoc, existing: impl Iterator<Item = (&'a str, u16)>) -> Result<(), String> {
    if doc.product_code == 0 { return Err("产品代码需在 1–65535 之间".into()); }
    for (id, code) in existing {
        if id.eq_ignore_ascii_case(&doc.id) { return Err("配方编号已经存在（不区分大小写）".into()); }
        if code == doc.product_code { return Err(format!("产品代码 {} 已被配方 {} 使用", code, id)); }
    }
    Ok(())
}

#[tauri::command]
pub fn workspace_delete(app: AppHandle, id: String) -> Result<(), String> {
    safe_id(&id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    if items.get(&id).is_some_and(|w| w.pending.is_some()) {
        return Err("已有待生效版本，不能删除".into());
    }
    if app.state::<CycleHost>().recipe(&id).is_some() {
        crate::recipe_api::recipe_delete(app.clone(), app.state::<CycleHost>(), id.clone())?;
    }
    let dir = host.dir(&id);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    items.remove(&id);
    Ok(())
}

#[tauri::command]
pub fn workspace_save_doc(
    app: AppHandle,
    id: String,
    revision: u64,
    doc: RecipeDoc,
) -> Result<WorkspaceView, String> {
    if doc.id != id {
        return Err("编辑候选配置时不能改变配方编号；请在配方库复制".into());
    }
    let recipe = doc.build()?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    if w.doc != doc {
        let old = w.doc.build()?;
        if old.geometry_hash() != recipe.geometry_hash()
            || old.camera != recipe.camera
            || old.mode != recipe.mode
        {
            w.frames = (0..recipe.shot_count()).map(Teaching::empty).collect();
            w.overview.saved = false;
        }
        w.doc = doc;
        w.changed();
    }
    host.save(w)?;
    view(&app, w.clone())
}

fn image_file(host: &WorkspaceHost, id: &str, image_id: &str) -> Result<PathBuf, String> {
    if image_id.is_empty()
        || !image_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("图像编号无效".into());
    }
    Ok(host.dir(id).join("images").join(format!("{image_id}.pgm")))
}

fn keep_image(
    app: &AppHandle,
    id: &str,
    revision: u64,
    k: usize,
    image: FrameImage,
    source: String,
    history_id: Option<i64>,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let r = w.doc.build()?;
    let (camera_tag, calib_tag) = tags(app, &r)?;
    let config = app
        .state::<CycleHost>()
        .camera
        .configs()
        .into_iter()
        .find(|c| c.id == r.camera)
        .ok_or("相机不存在")?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    let imported = source == "import";
    if (history_id.is_some() || imported) && frame.backup.is_none() && frame.image.is_some() {
        let mut backup = frame.clone();
        backup.backup = None;
        frame.backup = Some(Box::new(backup));
    }
    let image_id = format!("{}-{}-{}", ly_plc::now_ms(), w.revision, k);
    let path = image_file(&host, id, &image_id)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    crate::replay::save_pgm(&path, &image)?;
    frame.image = Some(FrozenImage {
        id: image_id,
        source,
        captured_at: ly_plc::now_ms(),
        size: [image.width, image.height],
        camera: r.camera.clone(),
        camera_tag,
        calib_tag,
        geometry_tag: r.geometry_hash(),
        exposure_us: (history_id.is_none() && !imported).then_some(config.exposure_us),
        gain_db: (history_id.is_none() && !imported).then_some(config.gain_db),
        history_id,
    });
    if config.source == CameraSource::Sim && !imported {
        frame.params.mm_per_px = crate::simimage::SIM_MM_PER_PX as f32;
    } else if let Some(info) = vision::calib_info(&vision::station_calib_path(app, &r.camera)?) {
        if let Some(mm) = info.mm_per_px {
            frame.params.mm_per_px = mm as f32;
        }
    }
    frame.trial = None;
    frame.saved = false;
    w.changed();
    host.save(w)?;
    view(app, w.clone())
}

fn decode_imported_image(bytes: Vec<u8>) -> Result<FrameImage, String> {
    if bytes.is_empty() || bytes.len() > 15_000_000 {
        return Err("原图不得为空，单图不得超过 15 MB".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format().map_err(|e| format!("原图格式无法识别：{e}"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128_000_000);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| format!("原图无法读取：{e}"))?.into_luma8();
    if image.width() as u64 * image.height() as u64 > 40_000_000 {
        return Err("单图像素数量超过 4000 万".into());
    }
    Ok(FrameImage::new(image.width(), image.height(), image.into_raw()))
}

#[tauri::command]
pub async fn workspace_import_image(app: AppHandle, id: String, revision: u64, k: usize, bytes: Vec<u8>) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() { return Err("工件正在检测，结束后再导入示教原图".into()); }
        {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            if w.doc.build()?.mode != InspectMode::FlyShot || k >= w.frames.len() {
                return Err("请选择有效的飞拍拍照点".into());
            }
        }
        let image = decode_imported_image(bytes)?;
        if cycle.busy() { return Err("导入期间工件开始检测，原图已丢弃".into()); }
        keep_image(&app, &id, revision, k, image, "import".into(), None)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn workspace_capture(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() {
            return Err("工件正在检测，结束后再取示教图像".into());
        }
        let r = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.doc.build()?
        };
        if r.mode != InspectMode::FlyShot || k >= r.shot_count() {
            return Err("请选择飞拍配方中的拍照点".into());
        }
        let cam = cycle.camera.require(&r.camera)?;
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        let image = if config.source == CameraSource::Sim {
            crate::simimage::render(
                &r,
                k,
                crate::sim::Scenario::Normal,
                crate::simimage::PoseError::default(),
                0,
            )
        } else {
            let before = slot.last_full();
            if config.source == CameraSource::Replay || config.trigger_source == "Software" {
                crate::camera::camera_soft_trigger(app.state::<CycleHost>(), cam as usize)?;
                let until = Instant::now() + Duration::from_millis(2000);
                loop {
                    if let Some(image) = slot
                        .last_full()
                        .filter(|i| before.as_ref().is_none_or(|old| !Arc::ptr_eq(old, i)))
                    {
                        break FrameImage::new(image.width, image.height, image.pixels.clone());
                    }
                    if Instant::now() >= until {
                        return Err("2 秒内没有收到新的完整图像；检查触发和回放目录".into());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            } else {
                let image = slot
                    .grab_full(Duration::from_millis(2000))
                    .ok_or("请让机器人在所选拍照点触发相机；2 秒内未收到完整图像")?;
                FrameImage::new(image.width, image.height, image.pixels.clone())
            }
        };
        if cycle.busy() {
            return Err("取样期间工件已开始检测，这次取样已丢弃".into());
        }
        keep_image(
            &app,
            &id,
            revision,
            k,
            image,
            format!("{:?}", config.source),
            None,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

fn preview_response(image: &FrameImage) -> tauri::ipc::Response {
    let scale = (image.width as f32 / 900.0)
        .max(image.height as f32 / 650.0)
        .max(1.0);
    let w = (image.width as f32 / scale).max(1.0) as u32;
    let h = (image.height as f32 / scale).max(1.0) as u32;
    let mut bytes = Vec::with_capacity(16 + (w * h) as usize);
    for n in [w, h, image.width, image.height] {
        bytes.extend(n.to_le_bytes());
    }
    for y in 0..h {
        for x in 0..w {
            let iy = ((y as f32 * scale) as u32).min(image.height - 1);
            let ix = ((x as f32 * scale) as u32).min(image.width - 1);
            bytes.push(image.pixels[(iy * image.width + ix) as usize]);
        }
    }
    tauri::ipc::Response::new(bytes)
}

#[tauri::command]
pub fn workspace_image(
    host: State<'_, WorkspaceHost>,
    id: String,
    image_id: String,
) -> Result<tauri::ipc::Response, String> {
    safe_id(&id)?;
    let image = crate::replay::load(&image_file(&host, &id, &image_id)?)?;
    Ok(preview_response(&image))
}

pub fn measurement_graph(params: &FrameParams) -> Result<String, String> {
    let mut graph: Value =
        serde_json::from_str(vision::FLYSHOT_GRAPH).map_err(|e| e.to_string())?;
    for node in graph["nodes"].as_array_mut().ok_or("检测图没有节点")? {
        match node["id"].as_str() {
            Some("n_locate") => {
                node["params"]["minScore"] = json!(params.min_score);
            }
            Some("n_calipers") => {
                node["params"]["contrastMin"] = json!(params.min_contrast);
                node["params"]["searchHalf"] = json!(params.search_mm / params.mm_per_px);
            }
            _ => {}
        }
    }
    Ok(graph.to_string())
}

pub fn station_graph(path: &Path) -> String {
    crate::fsio::read_text(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| serde_json::from_value::<FrameParams>(v["workspaceParams"].clone()).ok())
        .and_then(|p| measurement_graph(&p).ok())
        .unwrap_or_else(|| vision::FLYSHOT_GRAPH.to_string())
}

fn assets(
    app: &AppHandle,
    w: &Workspace,
    frame: &Teaching,
    dir: &Path,
) -> Result<(Value, String), String> {
    let r = w.doc.build()?;
    let frozen = frame.image.as_ref().ok_or("本帧没有冻结图像")?;
    frame.params.validate(frozen.size)?;
    let img = crate::replay::load(&image_file(
        &app.state::<WorkspaceHost>(),
        &r.id,
        &frozen.id,
    )?)?;
    let [x, y, width, height] = frame.params.rect;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let template = dir.join("template.pgm");
    let mut px = Vec::with_capacity((width * height) as usize);
    for row in y..y + height {
        let first = (row * img.width + x) as usize;
        px.extend_from_slice(&img.pixels[first..first + width as usize]);
    }
    crate::replay::save_pgm(&template, &FrameImage::new(width, height, px))?;
    let [cx, cy] = r.shots[frame.k];
    let (sin, cos) = frame.params.deg.to_radians().sin_cos();
    let sign = r.outward_sign();
    let mut points = Vec::new();
    let mut normals = Vec::new();
    let mut ids = Vec::new();
    for j in r.owned_points(frame.k) {
        let u = (r.points.x[j] - cx) / frame.params.mm_per_px;
        let v = (r.points.y[j] - cy) / frame.params.mm_per_px;
        let n = r.normal(j as f32 * r.spacing).map(|v| v * sign);
        points.push([
            img.width as f32 / 2.0 + frame.params.dx + u * cos - v * sin,
            img.height as f32 / 2.0 + frame.params.dy + u * sin + v * cos,
        ]);
        normals.push([n[0] * cos - n[1] * sin, n[0] * sin + n[1] * cos]);
        ids.push(j);
    }
    let stations = dir.join("stations.json");
    crate::fsio::write_atomic(
        &stations,
        &json!({ "points":points, "normals":normals, "ids":ids, "workspaceParams":frame.params })
            .to_string(),
    )?;
    let calib = if frozen.source == "Sim" {
        let sim = vision::assets_for(app, &r)?;
        sim.calib.clone()
    } else {
        vision::station_calib_path(app, &r.camera)?
    };
    if !calib.exists() {
        return Err("工位尚未标定，请先完成飞拍工位标定".into());
    }
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/");
    Ok((
        json!({ "template":norm(&template), "anchor":[x,y], "stations":norm(&stations), "calib":norm(&calib) }),
        measurement_graph(&frame.params)?,
    ))
}

fn measure_image(
    app: &AppHandle,
    w: &Workspace,
    frame: &Teaching,
    image: &FrameImage,
) -> Result<(Value, f64, f64, bool, String), String> {
    let host = app.state::<WorkspaceHost>();
    let image_id = &frame.image.as_ref().ok_or("没有冻结图像")?.id;
    let dir = host
        .dir(&w.doc.id)
        .join("trials")
        .join(format!("{image_id}-{}", fingerprint(&frame.params)));
    let (params, graph) = assets(app, w, frame, &dir)?;
    let settings = app.state::<CycleHost>().settings();
    let engine = app
        .state::<VisionHost>()
        .engine(settings.lyflow_core.as_deref())
        .ok_or("lyFlow 核心库未加载，请在系统设置里配置算法库")?;
    let result = engine.run(
        &graph,
        &format!("workspace-{}-{}", frame.k, ly_plc::now_ms()),
        &dir.to_string_lossy(),
        image,
        &params,
    )?;
    if result.status() == "failed" {
        return Err(result.failure());
    }
    let pose: vision::Pose =
        serde_json::from_value(result.record("pose").ok_or("算法没有输出定位结果")?.clone())
            .map_err(|e| e.to_string())?;
    let m = result
        .record("measure")
        .ok_or("算法没有输出测量结果")?
        .clone();
    let statuses = m["status"].as_array().ok_or("测量结果没有状态数组")?;
    if m["unit"].as_str() != Some("mm") {
        return Err("算法输出单位不是 mm，请检查工位标定".into());
    }
    let recipe = w.doc.build()?;
    let ids = m["ids"].as_array().ok_or("测量结果缺少点编号")?;
    let expected = recipe.owned_points(frame.k).count();
    let mut seen = std::collections::HashSet::new();
    let mut valid = 0;
    for (i, id) in ids.iter().enumerate() {
        let j = id
            .as_u64()
            .map(|j| j as usize)
            .filter(|&j| j < recipe.point_count() && recipe.points.k[j] as usize == frame.k)
            .ok_or("算法输出了不属于本帧的测量点")?;
        if !seen.insert(j) {
            return Err("算法输出了重复测量点".into());
        }
        if statuses.get(i).and_then(|s| s.as_str()).is_some_and(|s| {
            s == "no_bead"
                || (s == "ok" && m["innerCenter"].get(i).and_then(|v| v.as_f64()).is_some())
        }) {
            valid += 1;
        }
    }
    let coverage = if expected == 0 {
        0.0
    } else {
        valid as f64 / expected as f64
    };
    let passed = pose.ok && pose.score >= frame.params.min_score as f64 && coverage >= 0.995;
    let reason = if !pose.ok || pose.score < frame.params.min_score as f64 {
        "模板定位或匹配分数未通过".into()
    } else if coverage < 0.995 {
        "部分测量点未量成，请检查对齐、标定及搜索区域".into()
    } else {
        "定位和测量通过".into()
    };
    Ok((m, pose.score, coverage, passed, reason))
}

#[tauri::command]
pub async fn workspace_trial(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
    image_id: String,
    params: FrameParams,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut w = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.clone()
        };
        let r = w.doc.build()?;
        let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
        let frozen = frame
            .image
            .as_ref()
            .filter(|i| i.id == image_id)
            .ok_or("冻结图像已更新，请重新试测")?;
        let (camera_tag, calib_tag) = tags(&app, &r)?;
        if frozen.camera_tag != camera_tag
            || frozen.calib_tag != calib_tag
            || frozen.geometry_tag != r.geometry_hash()
        {
            return Err("相机参数、工位标定或胶路已变更，请重新冻结图像".into());
        }
        params.validate(frozen.size)?;
        frame.params = params;
        let image =
            crate::replay::load(&image_file(&app.state::<WorkspaceHost>(), &id, &image_id)?)?;
        let started = Instant::now();
        let result = measure_image(&app, &w, &w.frames[k], &image);
        let (measurement, score, coverage, passed, reason) =
            result.unwrap_or_else(|e| (Value::Null, 0.0, 0.0, false, e));
        let trial = Trial {
            image_id,
            params_tag: fingerprint(&w.frames[k].params),
            geometry_tag: r.geometry_hash(),
            passed,
            score,
            coverage,
            elapsed_ms: started.elapsed().as_millis(),
            reason,
            measurement,
        };
        let host = app.state::<WorkspaceHost>();
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(&id).ok_or("候选配置不存在")?;
        current.expect(revision)?;
        if current.frames[k].image.as_ref().map(|i| &i.id) != Some(&trial.image_id) {
            return Err("试测期间图像已更新，结果已丢弃".into());
        }
        current.frames[k].params = w.frames[k].params.clone();
        current.frames[k].trial = Some(trial);
        current.frames[k].saved = false;
        current.changed();
        host.save(current)?;
        view(&app, current.clone())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn workspace_save_params(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
    params: FrameParams,
) -> Result<WorkspaceView, String> {
    if ![
        params.dx,
        params.dy,
        params.deg,
        params.mm_per_px,
        params.search_mm,
        params.min_contrast,
        params.min_score,
    ]
    .iter()
    .all(|v| v.is_finite())
        || params.mm_per_px <= 0.0
        || params.search_mm <= 0.0
        || !(0.0..=255.0).contains(&params.min_contrast)
        || !(0.0..=1.0).contains(&params.min_score)
    {
        return Err("帧参数无效".into());
    }
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    if frame.params != params {
        frame.params = params;
        frame.trial = None;
        frame.saved = false;
        w.changed();
    }
    host.save(w)?;
    view(&app, w.clone())
}

#[tauri::command]
pub fn workspace_save_teach(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
    image_id: String,
    params: FrameParams,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let r = w.doc.build()?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    if frame.params != params || frame.image.as_ref().map(|i| &i.id) != Some(&image_id) {
        return Err("图像或参数已变更，请重新试测".into());
    }
    frame.checked(&r.geometry_hash())?;
    let (camera_tag, calib_tag) = tags(&app, &r)?;
    let frozen = frame.image.as_ref().unwrap();
    if frozen.camera_tag != camera_tag || frozen.calib_tag != calib_tag {
        return Err("相机或标定已变更，请重新取样".into());
    }
    frame.saved = true;
    w.changed();
    host.save(w)?;
    view(&app, w.clone())
}

#[tauri::command]
pub fn workspace_restore_teach(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    let mut old = *frame.backup.take().ok_or("本帧没有原示教备份")?;
    old.trial = None;
    old.saved = false;
    *frame = old;
    w.changed();
    host.save(w)?;
    view(&app, w.clone())
}

#[tauri::command]
pub fn workspace_save_overview(
    app: AppHandle,
    id: String,
    revision: u64,
    mut overview: Overview,
) -> Result<WorkspaceView, String> {
    if overview.background.as_ref().is_some_and(|s| {
        s.len() > 2_000_000
            || ![
                "data:image/png;base64,",
                "data:image/jpeg;base64,",
                "data:image/webp;base64,",
            ]
            .iter()
            .any(|p| s.starts_with(p))
    }) {
        return Err("总览背景只支持 1 MB 以内的 PNG、JPEG、WebP 图像".into());
    }
    if overview
        .positions
        .iter()
        .any(|p| p.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)))
    {
        return Err("总览显示位置必须在画布范围内".into());
    }
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    if overview.positions.len() != w.frames.len() {
        return Err("总览布置与拍照点数量不同".into());
    }
    overview.saved = true;
    w.overview = overview;
    w.changed();
    host.save(w)?;
    view(&app, w.clone())
}

fn validation(app: &AppHandle, w: &Workspace) -> Result<Validation, String> {
    let r = w.doc.build()?;
    let cycle = app.state::<CycleHost>();
    let usability = cycle::usable_cams(app, &r, cycle::real_parts(app))
        .and_then(|cams| cycle.camera.check_ready_at(&cams));
    let camera_ok = usability.is_ok();
    let mut checks = vec![Check {
        name: "设备与采集".into(),
        passed: camera_ok,
        detail: usability
            .err()
            .unwrap_or_else(|| "配方引用的相机已就绪".into()),
    }];
    let covered = coverage(&r, &w.frames);
    checks.push(Check {
        name: "物理覆盖".into(),
        passed: covered >= 99.995,
        detail: format!("含搜索余量的测量点覆盖 {:.2}%", covered),
    });
    let teaching = if r.mode == InspectMode::FlyShot {
        let (ct, cal) = tags(app, &r)?;
        w.frames.len() == r.shot_count()
            && w.frames.iter().all(|f| {
                f.saved
                    && f.checked(&r.geometry_hash()).is_ok()
                    && f.image
                        .as_ref()
                        .is_some_and(|i| i.camera_tag == ct && i.calib_tag == cal)
            })
    } else {
        r.cameras().iter().all(|id| {
            cycle
                .camera
                .configs()
                .iter()
                .find(|c| &c.id == id)
                .is_some_and(|c| c.follow.is_some())
        })
    };
    checks.push(Check {
        name: "示教与标定".into(),
        passed: teaching,
        detail: if teaching {
            "全部相机/拍照点的当前配置已保存".into()
        } else {
            "存在未示教、试测未通过或已失效的配置".into()
        },
    });
    checks.push(Check {
        name: "总览布置".into(),
        passed: r.mode == InspectMode::Follow || w.overview.saved,
        detail: if w.overview.saved {
            "总览显示布置已保存".into()
        } else {
            "请保存工件总览布置".into()
        },
    });
    let store = app.state::<Store>();
    let mut results = Vec::new();
    for sample in &w.samples {
        let result = (|| -> Result<SampleResult, String> {
            if let Some(sample_id) = &sample.sample_id {
                let bank = w
                    .sample_bank
                    .iter()
                    .find(|b| &b.id == sample_id)
                    .ok_or("样本组不存在")?;
                if bank.geometry_tag != r.geometry_hash() {
                    return Err("样本组来自另一版胶路，请重新导入".into());
                }
                let paths: Vec<_> = (0..r.shot_count())
                    .map(|k| {
                        app.state::<WorkspaceHost>()
                            .dir(&r.id)
                            .join("samples")
                            .join(sample_id)
                            .join(format!("k{k}.pgm"))
                    })
                    .collect();
                let (table, _) = measure_paths(app, w, &paths, 0)?;
                let j = judge::judge(&r, &table);
                return Ok(SampleResult {
                    history_id: None,
                    sample_id: Some(bank.id.clone()),
                    name: bank.name.clone(),
                    sn: 0,
                    expected: sample.expected,
                    actual: Some(j.verdict),
                    passed: j.verdict == sample.expected,
                    reason: j.reason,
                });
            }
            let history_id = sample.history_id.ok_or("样本来源未指定")?;
            let detail = store.detail(history_id)?;
            let original = detail
                .summary
                .recipe_hash
                .as_deref()
                .map(|h| store.recipe_snapshot(h))
                .transpose()?
                .flatten()
                .ok_or("原始配方快照缺失")?;
            if original.geometry_hash() != r.geometry_hash() || original.mode != r.mode {
                return Err("样本胶路与候选不一致，不能用旧测量数据验证".into());
            }
            let points = detail.points.ok_or("样本没有测量数据，不能用于规则验证")?;
            let table: Vec<_> = points
                .st
                .iter()
                .enumerate()
                .map(|(j, s)| match s {
                    0 => PointState::Measured {
                        d: points.d[j],
                        w: points.w.get(j).and_then(|w| *w).unwrap_or(f32::NAN),
                    },
                    1 => PointState::Gap,
                    2 => PointState::Invalid,
                    _ => PointState::Pending,
                })
                .collect();
            if table.len() != r.point_count() {
                return Err("样本的测量点数量不一致".into());
            }
            let j = judge::judge(&r, &table);
            Ok(SampleResult {
                history_id: sample.history_id,
                sample_id: None,
                name: format!("SN {}", detail.summary.sn),
                sn: detail.summary.sn,
                expected: sample.expected,
                actual: Some(j.verdict),
                passed: j.verdict == sample.expected,
                reason: j.reason,
            })
        })();
        results.push(result.unwrap_or_else(|e| {
            SampleResult {
                history_id: sample.history_id,
                sample_id: sample.sample_id.clone(),
                name: sample
                    .sample_id
                    .clone()
                    .unwrap_or_else(|| format!("记录 #{}", sample.history_id.unwrap_or(0))),
                sn: 0,
                expected: sample.expected,
                actual: None,
                passed: false,
                reason: e,
            }
        }));
    }
    let have_ok = w
        .samples
        .iter()
        .any(|s| matches!(s.expected, Verdict::Ok | Verdict::OkWithExcursion));
    let have_ng = w.samples.iter().any(|s| {
        !matches!(
            s.expected,
            Verdict::Ok | Verdict::OkWithExcursion | Verdict::ErrInspect
        )
    });
    checks.push(Check { name:"代表性样本".into(), passed:have_ok && have_ng && !results.is_empty() && results.iter().all(|r| r.passed),
        detail:"规则验证需至少一件合格样本、一件缺陷样本，全部结论与人工标注一致；图像能力由逐帧试测校验".into() });
    let base = cycle.recipe(&r.id).map(|r| r.hash.clone());
    checks.push(Check {
        name: "生产版本".into(),
        passed: base == w.base_hash,
        detail: if base == w.base_hash {
            "候选基于当前生产版本".into()
        } else {
            "生产配方已被其他操作更新，请重新建立候选".into()
        },
    });
    Ok(Validation {
        revision: w.revision,
        passed: checks.iter().all(|c| c.passed),
        checked_at: ly_plc::now_ms(),
        checks,
        samples: results,
        environment_tag: environment(app, &r)?,
    })
}

#[tauri::command]
pub async fn workspace_validate(
    app: AppHandle,
    id: String,
    revision: u64,
    samples: Vec<Sample>,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let mut w = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.clone()
        };
        let mut unique = std::collections::HashSet::new();
        if samples.len() > 100
            || samples
                .iter()
                .any(|s| !unique.insert((s.history_id, s.sample_id.clone())))
        {
            return Err("最多验证 100 件，不能重复添加相同样本".into());
        }
        w.samples = samples;
        let checked = validation(&app, &w)?;
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(&id).ok_or("候选配置不存在")?;
        current.expect(revision)?;
        current.samples = w.samples;
        current.validation = Some(checked);
        host.save(current)?;
        view(&app, current.clone())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn workspace_publish(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    if w.pending.is_some() {
        return Err("已有待生效版本，请等待工件结束".into());
    }
    let validated = w
        .validation
        .as_ref()
        .filter(|v| v.revision == revision && v.passed)
        .ok_or("请先通过当前候选的完整验证")?;
    let recipe = w.doc.build()?;
    let environment_tag = environment(&app, &recipe)?;
    let current_base = app.state::<CycleHost>().recipe(&id).map(|r| r.hash.clone());
    if current_base != w.base_hash || environment_tag != validated.environment_tag {
        return Err("验证后相机、标定或生产版本已变化，请重新验证".into());
    }
    let cams = cycle::usable_cams(&app, &recipe, cycle::real_parts(&app))?;
    app.state::<CycleHost>().camera.check_ready_at(&cams)?;
    w.doc.teaching_hash = Some(fingerprint(&json!([
        w.frames
            .iter()
            .map(|f| (&f.image, &f.params))
            .collect::<Vec<_>>(),
        w.overview,
        environment_tag
    ])));
    w.pending = Some(Box::new(Release {
        doc: w.doc.clone(),
        base_hash: w.base_hash.clone(),
        revision,
        frames: w.frames.clone(),
        overview: w.overview.clone(),
        validation: validated.clone(),
    }));
    w.publish_error = None;
    host.save(w)?;
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w.clone())
}

fn commit(app: &AppHandle, host: &WorkspaceHost, release: &Release) -> Result<Arc<Recipe>, String> {
    let cycle = app.state::<CycleHost>();
    let current = cycle.recipe(&release.doc.id);
    let recipe = release.doc.build()?;
    if let Some(saved) = current
        .as_ref()
        .filter(|r| r.hash == recipe.hash && Some(&r.hash) != release.base_hash.as_ref())
    {
        return Ok(saved.clone());
    }
    if current.as_ref().map(|r| r.hash.clone()) != release.base_hash {
        return Err("待生效版本与当前生产配方冲突".into());
    }
    if environment(app, &recipe)? != release.validation.environment_tag {
        return Err("等待期间设备或标定已变化，请重新验证".into());
    }
    let dir = vision::taught_dir(app, &recipe.id)?;
    let archive = host.dir(&recipe.id).join("published");
    std::fs::create_dir_all(&archive).map_err(|e| e.to_string())?;
    crate::fsio::write_atomic(
        &archive.join(format!("{}.json", recipe.hash)),
        &serde_json::to_string(release).map_err(|e| e.to_string())?,
    )?;
    let manifest = dir.join(vision::ASSETS_FILE);
    let previous = std::fs::read(&manifest).ok();
    if recipe.mode == InspectMode::FlyShot {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let snapshot = Workspace {
            doc: release.doc.clone(),
            base_hash: release.base_hash.clone(),
            revision: release.revision,
            frames: release.frames.clone(),
            overview: release.overview.clone(),
            samples: Vec::new(),
            sample_bank: Vec::new(),
            validation: None,
            pending: None,
            publish_error: None,
            updated_at: ly_plc::now_ms(),
        };
        let mut shots = Vec::new();
        let mut calibration = vision::station_calib_path(app, &recipe.camera)?;
        for frame in &release.frames {
            frame.checked(&recipe.geometry_hash())?;
            let frozen = frame.image.as_ref().unwrap();
            let work = host
                .dir(&recipe.id)
                .join("release")
                .join(format!("{}-k{}", release.revision, frame.k));
            let (params, _) = assets(app, &snapshot, frame, &work)?;
            calibration = PathBuf::from(params["calib"].as_str().ok_or("标定路径缺失")?);
            let template = dir.join(format!(
                "{}-{}-k{}.template.pgm",
                frozen.id, release.revision, frame.k
            ));
            let stations = dir.join(format!(
                "{}-{}-k{}.stations.json",
                frozen.id, release.revision, frame.k
            ));
            std::fs::copy(
                params["template"].as_str().ok_or("模板路径缺失")?,
                &template,
            )
            .map_err(|e| e.to_string())?;
            std::fs::copy(
                params["stations"].as_str().ok_or("测量点路径缺失")?,
                &stations,
            )
            .map_err(|e| e.to_string())?;
            shots.push(ShotAssets {
                template,
                stations,
                anchor: [frame.params.rect[0] as f64, frame.params.rect[1] as f64],
            });
        }
        let source = cycle
            .camera
            .configs()
            .into_iter()
            .find(|c| c.id == recipe.camera)
            .ok_or("相机不存在")?
            .source;
        VisionAssets {
            recipe_id: recipe.id.clone(),
            recipe_hash: recipe.geometry_hash(),
            camera: recipe.camera.clone(),
            sim_mm_per_px: (source == CameraSource::Sim).then_some(crate::simimage::SIM_MM_PER_PX),
            calib: calibration,
            shots,
        }
        .save(&manifest)?;
    }
    match cycle.recipes.save(
        release.doc.clone(),
        release.base_hash.as_ref().map(|_| release.doc.id.as_str()),
    ) {
        Ok(saved) => {
            app.state::<VisionHost>().forget(&saved.id);
            Ok(saved)
        }
        Err(e) => {
            if let Some(bytes) = previous {
                let _ = std::fs::write(&manifest, bytes);
            } else if recipe.mode == InspectMode::FlyShot {
                let _ = std::fs::remove_file(&manifest);
            }
            Err(e)
        }
    }
}

pub fn apply_pending(app: &AppHandle) -> bool {
    let Some(host) = app.try_state::<WorkspaceHost>() else {
        return false;
    };
    let mut items = host.items.lock().unwrap();
    let ids: Vec<_> = items
        .iter()
        .filter(|(_, w)| w.pending.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    let mut changed = false;
    for id in ids {
        let w = items.get_mut(&id).unwrap();
        let release = *w.pending.take().unwrap();
        match commit(app, &host, &release) {
            Ok(saved) => {
                w.base_hash = Some(saved.hash.clone());
                if w.revision == release.revision {
                    w.doc = app.state::<CycleHost>().recipes.doc(&id).unwrap();
                }
                w.doc.version = saved.version + 1;
                w.validation = None;
                w.publish_error = None;
                w.revision += 1;
                changed = true;
                cycle::log(
                    app,
                    "info",
                    "发布配方",
                    format!("{id} v{} 已生效", saved.version),
                );
            }
            Err(e) => {
                w.publish_error = Some(e.clone());
                cycle::log(app, "err", "发布配方", e);
            }
        }
        if let Err(error) = host.save(w) {
            w.publish_error = Some(format!("生产生效状态未能保存：{error}，请检查数据目录"));
            cycle::log(app, "err", "保存发布状态", error);
        }
        let _ = app.emit("workspace://changed", &id);
    }
    changed
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawFrame {
    pub k: usize,
    pub camera: String,
    pub file: String,
    pub ts: i64,
    pub available: bool,
    pub cam: Option<u8>,
    pub frame_counter: Option<u64>,
    pub trigger_counter: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordImages {
    pub history_id: i64,
    pub frames: Vec<RawFrame>,
    pub complete: bool,
    pub message: String,
}

fn recorded(app: &AppHandle, history_id: i64) -> Result<(PathBuf, Vec<RawFrame>), String> {
    let detail = app.state::<Store>().detail(history_id)?;
    let root = app.state::<CycleHost>().recorder.root().to_path_buf();
    let mut choices = Vec::new();
    for day in std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.ends_with("_pending"))
    {
        for dir in std::fs::read_dir(day)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
        {
            let Some(meta) = crate::fsio::read_text(&dir.join("part.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            else {
                continue;
            };
            let started = meta["startedTs"].as_i64().unwrap_or(0);
            if meta["sn"].as_u64() != Some(detail.summary.sn as u64)
                || meta["recipe"]["hash"].as_str() != detail.summary.recipe_hash.as_deref()
                || started > detail.summary.ts
                || detail.summary.ts - started > 300_000
            {
                continue;
            }
            choices.push((detail.summary.ts - started, dir, meta));
        }
    }
    choices.sort_by_key(|c| c.0);
    let (_, dir, meta) = choices
        .into_iter()
        .next()
        .ok_or("原图未保留或已清理；仍可使用完整测量数据进行规则重判")?;
    let mut frames = Vec::new();
    let raw = meta["frames"].as_array().ok_or("原图清单损坏")?;
    let camera = meta["recipe"]["camera"].as_str().unwrap_or("");
    let first = raw
        .iter()
        .find(|f| f["camera"].as_str() == Some(camera))
        .and_then(|f| f["frameCounter"].as_u64());
    for (i, f) in raw.iter().enumerate() {
        let Some(file) = f["file"].as_str() else {
            continue;
        };
        let p = Path::new(file);
        if p.components().count() != 1 || !p.file_name().is_some_and(|n| n == file) {
            continue;
        }
        let k = if meta["recipe"]["mode"].as_str() == Some("flyShot") {
            if f["camera"].as_str() != Some(camera) {
                continue;
            }
            f["frameCounter"]
                .as_u64()
                .zip(first)
                .map_or(i, |(fc, first)| fc.saturating_sub(first) as usize)
        } else {
            i
        };
        frames.push(RawFrame {
            k,
            camera: f["camera"].as_str().unwrap_or("").into(),
            file: file.into(),
            ts: f["ts"]
                .as_i64()
                .unwrap_or_else(|| meta["startedTs"].as_i64().unwrap_or(0)),
            available: dir.join(file).is_file(),
            cam: f["cam"].as_u64().and_then(|v| u8::try_from(v).ok()),
            frame_counter: f["frameCounter"].as_u64(),
            trigger_counter: f["triggerCounter"].as_u64(),
        });
    }
    Ok((dir, frames))
}

fn match_follow_frames<'a>(
    measured: &[crate::cycle::FrameView],
    raw: &'a [RawFrame],
) -> Result<Vec<&'a RawFrame>, String> {
    if measured.is_empty() || raw.is_empty() {
        return Err("随动原图复测需要已记录沿程位置的完整帧组".into());
    }
    let mut originals = std::collections::HashMap::new();
    for frame in raw {
        let counter = frame.frame_counter.ok_or("原图缺少绝对帧计数，不能匹配历史测量")?;
        if frame.camera.is_empty() || frame.cam.is_none() || !frame.available {
            return Err("随动原图缺帧或相机元数据不完整".into());
        }
        if originals.insert((frame.camera.as_str(), counter), frame).is_some() {
            return Err("随动原图清单包含重复相机帧计数".into());
        }
    }
    let mut used = std::collections::HashSet::new();
    measured.iter().map(|frame| {
        if frame.s.is_none_or(|s| !s.is_finite()) || frame.camera.is_empty() {
            return Err("历史帧缺少有限的沿程位置或相机编号".into());
        }
        let counter = frame.frame_counter.ok_or("历史帧缺少绝对帧计数")?;
        let key = (frame.camera.as_str(), counter);
        if !used.insert(key) {
            return Err("历史测量包含重复相机帧计数".into());
        }
        let original = originals.get(&key).ok_or("历史测量对应的原图缺失，不能复测整件")?;
        if original.cam != Some(frame.cam) || frame.trigger_counter.is_some_and(|n| original.trigger_counter != Some(n)) {
            return Err("历史测量与原图的相机或触发计数不匹配".into());
        }
        Ok(*original)
    }).collect()
}

fn measure_follow_record(
    recipe: &Recipe,
    calibs: &[(String, u8, crate::follow::FollowCalib)],
    measured: &[crate::cycle::FrameView],
    raw: &[RawFrame],
    sn: u32,
    mut load: impl FnMut(&RawFrame) -> Result<FrameImage, String>,
) -> Result<(Vec<PointState>, Vec<Value>), String> {
    let matched = match_follow_frames(measured, raw)?;
    let mut image_recipe = recipe.clone();
    let spec = image_recipe.follow.as_mut().ok_or("候选没有随动参数")?;
    if raw.iter().any(|f| !spec.cameras.contains(&f.camera)) {
        return Err("原图相机不属于当前候选随动相机组".into());
    }
    // 历史 s 已由现场时序/同步得到，本次仅重测该位置对应的图像。
    spec.auto_sync = false;
    let image_recipe = Arc::new(image_recipe);
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut measurements = Vec::new();
    for (k, (history, original)) in measured.iter().zip(matched).enumerate() {
        let (_, cam, calib) = calibs.iter().find(|(id, _, _)| id == &history.camera)
            .ok_or_else(|| format!("相机 {} 缺少当前工位随动标定", history.camera))?;
        calib.validate()?;
        let image = load(original)?;
        if [image.width, image.height] != calib.image_size {
            return Err(format!("相机 {} 原图尺寸与当前工位标定不一致", history.camera));
        }
        let s = history.s.ok_or("历史帧缺少沿程位置")?;
        let points: Vec<u32> = crate::follow::visible(&image_recipe, image_recipe.follow.as_ref().unwrap(), calib, s)
            .into_iter().filter(|&j| table[j] == PointState::Pending).map(|j| j as u32).collect();
        if points.is_empty() { continue; }
        let job = crate::measure::Job {
            run_id: 0,
            sn, k, cam: *cam, recipe: image_recipe.clone(), scenario: crate::sim::Scenario::Normal,
            image: None, kind: crate::measure::JobKind::Follow { s, points, calib: calib.clone(), start_probe: false },
        };
        let start = Instant::now();
        let mut result = crate::measure::measure_native(&job, &image)?;
        if let Some(error) = result.error.as_ref() { return Err(format!("相机 {}：{error}", history.camera)); }
        result.ms = start.elapsed().as_millis() as u32;
        for (i, &j) in result.idx.iter().enumerate() { table[j as usize] = result.point_state(i); }
        measurements.push(serde_json::to_value(&result).map_err(|e| e.to_string())?);
    }
    if measurements.is_empty() { return Err("当前候选与标定在历史沿程位置没有可测点".into()); }
    Ok((table, measurements))
}

#[tauri::command]
pub fn workspace_record_images(app: AppHandle, history_id: i64) -> Result<RecordImages, String> {
    let store = app.state::<Store>();
    let detail = store.detail(history_id)?;
    let mode = detail.summary.recipe_hash.as_deref()
        .map(|hash| store.recipe_snapshot(hash)).transpose()?.flatten().map(|recipe| recipe.mode);
    match recorded(&app, history_id) {
        Ok((_, frames)) => {
            let complete = if mode == Some(InspectMode::Follow) || detail.frames.iter().any(|f| f.s.is_some()) {
                match_follow_frames(&detail.frames, &frames).is_ok()
            } else {
                detail.summary.frames_expected > 0 && (0..detail.summary.frames_expected)
                    .all(|k| frames.iter().any(|f| f.k == k && f.available))
            };
            let count = frames.iter().filter(|f| f.available).count();
            Ok(RecordImages {
                history_id,
                frames,
                complete,
                message: format!("保留 {count} 张原图"),
            })
        }
        Err(e) => Ok(RecordImages {
            history_id,
            frames: Vec::new(),
            complete: false,
            message: e,
        }),
    }
}

#[tauri::command]
pub fn workspace_record_image(
    app: AppHandle,
    history_id: i64,
    k: usize,
) -> Result<tauri::ipc::Response, String> {
    let (dir, frames) = recorded(&app, history_id)?;
    let raw = frames
        .iter()
        .find(|f| f.k == k && f.available)
        .ok_or("所选帧原图未保留")?;
    Ok(preview_response(&crate::replay::load(
        &dir.join(&raw.file),
    )?))
}

#[tauri::command]
pub fn workspace_history_capture(
    app: AppHandle,
    id: String,
    revision: u64,
    history_id: i64,
    k: usize,
) -> Result<WorkspaceView, String> {
    let detail = app.state::<Store>().detail(history_id)?;
    let original = detail
        .summary
        .recipe_hash
        .as_deref()
        .map(|h| app.state::<Store>().recipe_snapshot(h))
        .transpose()?
        .flatten()
        .ok_or("历史配方快照缺失")?;
    let doc = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        let w = items.get(&id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.doc.build()?
    };
    if original.mode != InspectMode::FlyShot
        || doc.geometry_hash() != original.geometry_hash()
        || doc.camera != original.camera
    {
        return Err("历史帧的胶路、相机或拍照点与当前候选不同，不能直接用于示教".into());
    }
    let (dir, frames) = recorded(&app, history_id)?;
    let raw = frames
        .iter()
        .find(|f| f.k == k && f.available)
        .ok_or("所选帧原图已清理")?;
    keep_image(
        &app,
        &id,
        revision,
        k,
        crate::replay::load(&dir.join(&raw.file))?,
        format!("历史 SN {}", detail.summary.sn),
        Some(history_id),
    )
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub id: String,
    pub history_id: i64,
    pub source: String,
    pub candidate_id: String,
    pub candidate_revision: u64,
    pub candidate_recipe: Recipe,
    pub original_verdict: Verdict,
    pub judgement: judge::Judgement,
    pub measurements: Vec<Value>,
    pub created_at: i64,
}

#[tauri::command]
pub async fn workspace_compare(
    app: AppHandle,
    id: String,
    revision: u64,
    history_id: i64,
    raw: bool,
) -> Result<Comparison, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let w = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.clone()
        };
        let recipe = w.doc.build()?;
        let store = app.state::<Store>();
        let detail = store.detail(history_id)?;
        let original = detail
            .summary
            .recipe_hash
            .as_deref()
            .map(|h| store.recipe_snapshot(h))
            .transpose()?
            .flatten()
            .ok_or("历史配方快照缺失")?;
        if original.geometry_hash() != recipe.geometry_hash() || original.mode != recipe.mode {
            return Err("原始记录与候选的几何不同；请重新采集代表性样本".into());
        }
        let mut measurements = Vec::new();
        let table = if raw {
            let (dir, frames) = recorded(&app, history_id)?;
            if recipe.mode == InspectMode::Follow {
                let calibs: Vec<_> = app.state::<CycleHost>().camera.slots().iter().enumerate().filter_map(|(cam, slot)| {
                    let config = slot.config();
                    Some((config.id, u8::try_from(cam).ok()?, config.follow?))
                }).collect();
                let (table, measured) = measure_follow_record(&recipe, &calibs, &detail.frames, &frames, detail.summary.sn,
                    |frame| crate::replay::load(&dir.join(&frame.file)))?;
                measurements = measured;
                table
            } else {
            let paths = (0..recipe.shot_count())
                .map(|k| {
                    frames
                        .iter()
                        .find(|f| f.k == k && f.available)
                        .map(|f| dir.join(&f.file))
                        .ok_or_else(|| "原图不完整，无法完成整件复测".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (table, measured) = measure_paths(&app, &w, &paths, detail.summary.sn)?;
            measurements = measured;
            table
            }
        } else {
            let points = detail
                .points
                .as_ref()
                .ok_or("历史记录没有完整测量数据，不能重判")?;
            if points.st.len() != recipe.point_count() || points.st.iter().any(|s| *s >= 2) {
                return Err("历史测量未完成；缺失的测量不能靠修改规则补齐".into());
            }
            points
                .st
                .iter()
                .enumerate()
                .map(|(j, s)| {
                    if *s == 0 {
                        PointState::Measured {
                            d: points.d[j],
                            w: points.w.get(j).and_then(|w| *w).unwrap_or(f32::NAN),
                        }
                    } else {
                        PointState::Gap
                    }
                })
                .collect()
        };
        let comparison = Comparison {
            id: format!("{history_id}-{}", ly_plc::now_ms()),
            history_id,
            source: if raw { "raw".into() } else { "rules".into() },
            candidate_id: id.clone(),
            candidate_revision: revision,
            candidate_recipe: recipe.clone(),
            original_verdict: detail.summary.verdict,
            judgement: judge::judge(&recipe, &table),
            measurements,
            created_at: ly_plc::now_ms(),
        };
        let host = app.state::<WorkspaceHost>();
        let dir = host.dir(&id).join("comparisons");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        crate::fsio::write_atomic(
            &dir.join(format!("{}.json", comparison.id)),
            &serde_json::to_string(&comparison).map_err(|e| e.to_string())?,
        )?;
        Ok(comparison)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn workspace_runtime_overview(
    host: State<'_, WorkspaceHost>,
    id: String,
    hash: String,
) -> Result<Option<Overview>, String> {
    safe_id(&id)?;
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("配方哈希无效".into());
    }
    let file = host.dir(&id).join("published").join(format!("{hash}.json"));
    Ok(crate::fsio::read_text(&file)
        .ok()
        .and_then(|s| serde_json::from_str::<Release>(&s).ok())
        .map(|r| r.overview))
}

#[tauri::command]
pub fn workspace_comparisons(
    host: State<'_, WorkspaceHost>,
    id: String,
    history_id: i64,
) -> Result<Vec<Comparison>, String> {
    safe_id(&id)?;
    let dir = host.dir(&id).join("comparisons");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(text) = crate::fsio::read_text(&entry.path()) {
                if let Ok(saved) = serde_json::from_str::<Comparison>(&text) {
                    if saved.history_id == history_id {
                        out.push(saved);
                    }
                }
            }
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.created_at));
    out.truncate(100);
    Ok(out)
}

pub fn clear_live(app: &AppHandle) {
    if let Some(host) = app.try_state::<WorkspaceHost>() {
        *host.live.lock().unwrap() = LiveFrames::default();
    }
}

pub fn retain_live(
    app: &AppHandle,
    sn: u32,
    hash: &str,
    k: usize,
    image: &Option<Arc<FrameImage>>,
) {
    if let (Some(host), Some(image)) = (app.try_state::<WorkspaceHost>(), image) {
        host.live.lock().unwrap().insert(sn, hash, k, image.clone());
    }
}

#[tauri::command]
pub fn workspace_live_image(
    host: State<'_, WorkspaceHost>,
    sn: u32,
    hash: String,
    k: usize,
) -> Result<tauri::ipc::Response, String> {
    let image = {
        let live = host.live.lock().unwrap();
        if live
            .key
            .as_ref()
            .is_none_or(|key| key.0 != sn || key.1 != hash)
        {
            return Err("本件尚未收到整帧图像".into());
        }
        live.frames
            .iter()
            .find(|(index, _)| *index == k)
            .map(|(_, image)| image.clone())
            .ok_or("所选帧原图未到达或已超出预览缓存；请开启图像测量或帧录制")?
    };
    Ok(preview_response(&image))
}

pub fn station_image_ref(
    app: &AppHandle,
    cam: u8,
    image_id: &str,
) -> Result<Arc<FrameImage>, String> {
    let cycle = app.state::<CycleHost>();
    if cycle.busy() {
        return Err("工件正在检测，结束后再操作标定样本".into());
    }
    let config = cycle
        .camera
        .slot(cam as usize)
        .ok_or("相机不存在")?
        .config();
    let host = app.state::<WorkspaceHost>();
    let samples = host.station.lock().unwrap();
    let (meta, image) = samples
        .get(&cam)
        .filter(|(meta, _)| meta.id == image_id)
        .ok_or("冻结样本已更新，请重新选择图像")?;
    if meta.camera_tag != fingerprint(&config) {
        return Err("相机参数变化，请重新冻结标定样本".into());
    }
    Ok(image.clone())
}

#[tauri::command]
pub fn workspace_station_image(
    app: AppHandle,
    cam: u8,
    image_id: String,
) -> Result<tauri::ipc::Response, String> {
    let image = station_image_ref(&app, cam, &image_id)?;
    Ok(preview_response(&image))
}

#[tauri::command]
pub async fn workspace_station_capture(app: AppHandle, cam: u8) -> Result<FrozenImage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() {
            return Err("工件正在检测，结束后再取标定样本".into());
        }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        let image = if config.source == CameraSource::Sim {
            slot.last_full()
                .ok_or("模拟相机尚未出图，请先运行一个模拟工件")?
        } else if config.source == CameraSource::Replay
            || (config.acquisition == crate::camera::Acquisition::Triggered
                && config.trigger_source == "Software")
        {
            let before = slot.last_full();
            crate::camera::camera_soft_trigger(app.state::<CycleHost>(), cam as usize)?;
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(image) = slot
                    .last_full()
                    .filter(|image| before.as_ref().is_none_or(|old| !Arc::ptr_eq(old, image)))
                {
                    break image;
                }
                if Instant::now() >= until {
                    return Err("2 秒内未收到新的完整图像，请检查触发与回放目录".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        } else {
            slot.grab_full(Duration::from_secs(2))
                .ok_or("2 秒内未收到完整图像，请让相机在标定板位置出图")?
        };
        if cycle.busy() || fingerprint(&slot.config()) != fingerprint(&config) {
            return Err("取样期间设备或节拍状态变化，样本已丢弃".into());
        }
        let host = app.state::<WorkspaceHost>();
        let seq = host
            .capture_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let meta = FrozenImage {
            id: format!("station-{cam}-{}-{seq}", ly_plc::now_ms()),
            source: format!("{:?}", config.source),
            captured_at: ly_plc::now_ms(),
            size: [image.width, image.height],
            camera: config.id,
            camera_tag: fingerprint(&slot.config()),
            calib_tag: String::new(),
            geometry_tag: String::new(),
            exposure_us: Some(config.exposure_us),
            gain_db: Some(config.gain_db),
            history_id: None,
        };
        host.station
            .lock()
            .unwrap()
            .insert(cam, (meta.clone(), image));
        Ok(meta)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn workspace_station_import(app: AppHandle, cam: u8, bytes: Vec<u8>) -> Result<FrozenImage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() { return Err("工件正在检测，结束后再导入标定原图".into()); }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        let image = Arc::new(decode_imported_image(bytes)?);
        if cycle.busy() || fingerprint(&slot.config()) != fingerprint(&config) {
            return Err("导入期间设备或节拍状态变化，原图已丢弃".into());
        }
        let host = app.state::<WorkspaceHost>();
        let seq = host.capture_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let meta = FrozenImage {
            id: format!("station-import-{cam}-{}-{seq}", ly_plc::now_ms()),
            source: "import".into(), captured_at: ly_plc::now_ms(), size: [image.width, image.height],
            camera: config.id.clone(), camera_tag: fingerprint(&config), calib_tag: String::new(), geometry_tag: String::new(),
            exposure_us: None, gain_db: None, history_id: None,
        };
        host.station.lock().unwrap().insert(cam, (meta.clone(), image));
        Ok(meta)
    }).await.map_err(|e| e.to_string())?
}

fn measure_paths(
    app: &AppHandle,
    w: &Workspace,
    paths: &[PathBuf],
    sn: u32,
) -> Result<(Vec<PointState>, Vec<Value>), String> {
    let recipe = w.doc.build()?;
    if recipe.mode != InspectMode::FlyShot {
        return Err("随动原图应使用回放相机跑完整节拍".into());
    }
    if paths.len() != recipe.shot_count() {
        return Err("样本组的原图数量与拍照点不一致".into());
    }
    let (ct, cal) = tags(app, &recipe)?;
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut measurements = Vec::new();
    for (k, path) in paths.iter().enumerate() {
        let frame = w
            .frames
            .get(k)
            .filter(|f| f.saved)
            .ok_or("候选存在尚未保存的示教帧")?;
        frame.checked(&recipe.geometry_hash())?;
        let frozen = frame.image.as_ref().unwrap();
        if frozen.camera_tag != ct || frozen.calib_tag != cal {
            return Err("相机或标定变化，需要重新示教".into());
        }
        let image = crate::replay::load(path)?;
        if [image.width, image.height] != frozen.size {
            return Err(format!("k{} 样本尺寸与示教图像不同", k + 1));
        }
        let (m, score, _, passed, reason) = measure_image(app, w, frame, &image)?;
        if !passed {
            return Err(format!("k{}：{reason}", k + 1));
        }
        if m["unit"].as_str() != Some("mm") {
            return Err("算法输出单位不是 mm，请检查工位标定".into());
        }
        let ids = m["ids"].as_array().ok_or("测量结果缺少点编号")?;
        let statuses = m["status"].as_array().ok_or("测量结果缺少状态")?;
        let mut idx = Vec::new();
        let mut d = Vec::new();
        let mut width = Vec::new();
        let mut st = Vec::new();
        for (i, v) in ids.iter().enumerate() {
            let Some(j) = v
                .as_u64()
                .map(|j| j as usize)
                .filter(|&j| j < table.len() && recipe.points.k[j] as usize == k)
            else {
                continue;
            };
            let reading = m["innerCenter"]
                .get(i)
                .and_then(|v| v.as_f64())
                .map(|v| v as f32);
            let bead_width = m["width"].get(i).and_then(|v| v.as_f64()).map(|v| v as f32);
            table[j] = match (statuses.get(i).and_then(|v| v.as_str()), reading) {
                (Some("ok"), Some(d)) => PointState::Measured {
                    d,
                    w: bead_width.unwrap_or(f32::NAN),
                },
                (Some("no_bead"), _) => PointState::Gap,
                _ => PointState::Invalid,
            };
            idx.push(j);
            d.push(reading.unwrap_or(0.0));
            width.push(bead_width);
            st.push(match table[j] {
                PointState::Measured { .. } => 0,
                PointState::Gap => 1,
                _ => 2,
            });
        }
        measurements.push(json!({ "sn":sn, "k":k, "cam":0, "s":null, "located":score >= frame.params.min_score as f64, "score":score,
            "ms":0, "error":null, "idx":idx, "d":d, "w":width, "st":st, "px":[] }));
    }
    Ok((table, measurements))
}

#[derive(Deserialize)]
pub struct SampleImageInput {
    pub k: usize,
    pub bytes: Vec<u8>,
}

struct SampleImportFiles {
    path: PathBuf,
    keep: bool,
}

impl SampleImportFiles {
    fn new(root: &Path, sample_id: &str) -> Result<Self, String> {
        if sample_id.is_empty() || !sample_id.bytes().all(|c| c.is_ascii_digit() || c == b'-') {
            return Err("样本目录编号无效".into());
        }
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let path = root.join(format!(".import-{sample_id}"));
        // create_dir 不复用已有目录；仅此调用创建成功的目录归本次导入所有。
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self { path, keep: false })
    }

    fn write_images(&self, workspace: &Workspace, images: Vec<SampleImageInput>) -> Result<(), String> {
        for input in images {
            let image = decode_imported_image(input.bytes).map_err(|e| format!("k{} 图像无法读取：{e}", input.k + 1))?;
            let frame = workspace.frames.get(input.k).ok_or("原图拍照点不属于当前候选")?;
            if frame.image.as_ref().is_some_and(|reference| [image.width, image.height] != reference.size) {
                return Err(format!("k{} 原图尺寸与示教不同", input.k + 1));
            }
            crate::replay::save_pgm(&self.path.join(format!("k{}.pgm", input.k)), &image)?;
            // 单张解码、落盘、释放；不将整组高分辨率图像同时保留在内存。
        }
        Ok(())
    }

    fn publish(&mut self, sample_id: &str) -> Result<(), String> {
        if self.path.file_name().and_then(|v| v.to_str()) != Some(format!(".import-{sample_id}").as_str()) {
            return Err("样本目录编号不匹配".into());
        }
        let destination = self.path.parent().ok_or("样本目录无效")?.join(sample_id);
        if destination.exists() { return Err("样本目录已存在，请重试导入".into()); }
        std::fs::rename(&self.path, &destination).map_err(|e| e.to_string())?;
        self.path = destination;
        Ok(())
    }
}

impl Drop for SampleImportFiles {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_candidate_cannot_alias_windows_directory_or_duplicate_product_selection() {
        let mut doc = crate::recipe::samples().remove(0);
        doc.id = "Case-Test".into();
        doc.product_code = 93;
        assert!(check_new_identity(&doc, [("case-test", 94)].into_iter()).is_err());
        assert!(check_new_identity(&doc, [("another", 93)].into_iter()).is_err());
        assert!(check_new_identity(&doc, [("another", 94)].into_iter()).is_ok());
        doc.product_code = 0;
        assert!(check_new_identity(&doc, std::iter::empty()).is_err());
    }
    use super::*;

    fn follow_history(camera: &str, cam: u8, counter: u64, s: f32) -> crate::cycle::FrameView {
        crate::cycle::FrameView {
            status: crate::cycle::FrameStatus::Done, cam, camera: camera.into(), s: Some(s), arrived_ms: Some(1),
            frame_counter: Some(counter), trigger_counter: Some(counter + 10), counter_jump: false,
            score: Some(1.0), points: 5, gap_points: 0, ms: Some(1),
        }
    }

    fn follow_raw(camera: &str, cam: u8, counter: u64, k: usize) -> RawFrame {
        RawFrame { k, camera: camera.into(), file: format!("{camera}_{counter}.pgm"), ts: 1, available: true,
            cam: Some(cam), frame_counter: Some(counter), trigger_counter: Some(counter + 10) }
    }

    #[test]
    fn follow_retest_matches_camera_and_absolute_counter_instead_of_list_position() {
        let history = [follow_history("cam1", 0, 1024, 30.0), follow_history("cam2", 1, 2077, 36.0)];
        let raw = [follow_raw("cam2", 1, 2077, 0), follow_raw("cam1", 0, 1023, 1), follow_raw("cam1", 0, 1024, 2)];
        let matched = match_follow_frames(&history, &raw).unwrap();
        assert_eq!(matched.iter().map(|f| f.k).collect::<Vec<_>>(), [2, 0]);
    }

    #[test]
    fn follow_retest_rejects_missing_duplicate_or_incompatible_frame_metadata() {
        let history = [follow_history("cam1", 0, 100, 30.0)];
        let raw = [follow_raw("cam1", 0, 100, 0)];
        assert!(match_follow_frames(&[], &raw).is_err());
        assert!(match_follow_frames(&history, &[]).is_err());
        for change in 0..7 {
            let mut bad = raw.clone();
            match change {
                0 => bad[0].available = false,
                1 => bad[0].frame_counter = Some(101),
                2 => bad[0].camera = "other-camera".into(),
                3 => bad[0].cam = Some(1),
                4 => bad[0].trigger_counter = Some(99),
                5 => bad[0].frame_counter = None,
                _ => bad[0].cam = None,
            }
            assert!(match_follow_frames(&history, &bad).is_err(), "bad metadata {change}");
        }
        assert!(match_follow_frames(&history, &[raw[0].clone(), raw[0].clone()]).is_err());
        assert!(match_follow_frames(&[history[0].clone(), history[0].clone()], &raw).is_err());
        for s in [None, Some(f32::NAN), Some(f32::INFINITY)] {
            let mut bad = history.clone(); bad[0].s = s;
            assert!(match_follow_frames(&bad, &raw).is_err());
        }
    }

    #[test]
    fn follow_retest_measures_real_pixels_and_rejects_wrong_camera_or_dimensions() {
        let recipe = crate::recipe::samples().into_iter().find(|r| r.mode == InspectMode::Follow).unwrap().build().unwrap();
        let camera = recipe.cameras()[0].clone();
        let history = [follow_history(&camera, 0, 100, 50.0)];
        let raw = [follow_raw(&camera, 0, 100, 0)];
        let calib = crate::follow::FollowCalib { nozzle: [270.0, 120.0], angle_deg: 0.0, mirror: false,
            mm_per_px: 0.1, mask_px: 0.0, image_size: [320, 240] };
        let calibs = [(camera.clone(), 2, calib)];
        let blank = |_: &RawFrame| Ok(FrameImage::new(320, 240, vec![200; 320 * 240]));
        let (gap, gap_measurements) = measure_follow_record(&recipe, &calibs, &history, &raw, 42, blank).unwrap();
        assert!(gap.iter().any(|p| *p == PointState::Gap));
        assert_eq!(gap_measurements[0]["cam"], 2);
        assert_eq!(gap_measurements[0]["s"], 50.0);
        assert!(gap_measurements[0]["startSync"].is_null());
        let stripe = |_: &RawFrame| {
            let mut pixels = vec![200; 320 * 240];
            for y in 110..130 { pixels[y * 320..(y + 1) * 320].fill(20); }
            Ok(FrameImage::new(320, 240, pixels))
        };
        let (bead, _) = measure_follow_record(&recipe, &calibs, &history, &raw, 42, stripe).unwrap();
        assert!(bead.iter().any(|p| matches!(p, PointState::Measured { d, w } if d.abs() < 0.2 && *w > 1.8 && *w < 2.2)));
        assert!(measure_follow_record(&recipe, &calibs, &history, &raw, 42,
            |_| Ok(FrameImage::new(64, 64, vec![200; 4096]))).is_err());
        assert!(measure_follow_record(&recipe, &[], &history, &raw, 42, blank).is_err());
        let mut other_recipe = recipe.clone();other_recipe.follow.as_mut().unwrap().cameras = vec!["other".into()];
        assert!(measure_follow_record(&other_recipe, &calibs, &history, &raw, 42, blank).is_err());
    }

    struct ImportTestDir(PathBuf);
    impl ImportTestDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("tujiao-sample-import-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path).unwrap(); Self(path)
        }
    }
    impl Drop for ImportTestDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    fn import_workspace() -> Workspace {
        let mut workspace = Workspace::new(crate::recipe::samples().remove(0), None);
        for frame in &mut workspace.frames { frame.image = None; }
        workspace
    }
    fn tiny_image(k: usize) -> SampleImageInput {
        let mut bytes = b"P5\n3 2\n255\n".to_vec();bytes.extend([0, 32, 64, 128, 200, 255]);
        SampleImageInput { k, bytes }
    }

    #[test]
    fn sample_import_removes_earlier_images_when_a_later_image_is_corrupt() {
        let root = ImportTestDir::new();
        let preserved = root.0.join("existing");std::fs::create_dir(&preserved).unwrap();
        std::fs::write(preserved.join("keep.txt"), "prior sample").unwrap();
        let path;
        {
            let files = SampleImportFiles::new(&root.0, "1-7-0").unwrap();path = files.path.clone();
            let result = files.write_images(&import_workspace(), vec![tiny_image(0), SampleImageInput { k: 1, bytes: b"broken image".to_vec() }]);
            assert!(result.unwrap_err().contains("k2"));
            assert!(path.join("k0.pgm").is_file());
        }
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(preserved.join("keep.txt")).unwrap(), "prior sample");
    }

    #[test]
    fn sample_import_cleans_unregistered_published_files_and_keeps_successful_real_pixels() {
        let root = ImportTestDir::new();
        {
            let mut files = SampleImportFiles::new(&root.0, "1-7-1").unwrap();
            files.write_images(&import_workspace(), vec![tiny_image(0)]).unwrap();
            files.publish("1-7-1").unwrap();
            assert!(files.path.join("k0.pgm").exists());
            // 注册前修订或持久化失败，拥有的目录仍随事务清理。
        }
        assert!(!root.0.join("1-7-1").exists());
        {
            let mut files = SampleImportFiles::new(&root.0, "1-7-2").unwrap();
            files.write_images(&import_workspace(), vec![tiny_image(0)]).unwrap();
            files.publish("1-7-2").unwrap();files.keep = true;
        }
        let image = crate::replay::load(&root.0.join("1-7-2/k0.pgm")).unwrap();
        assert_eq!(image.pixels, [0, 32, 64, 128, 200, 255]);
    }

    #[test]
    fn sample_import_never_claims_existing_or_traversing_directories() {
        let root = ImportTestDir::new();
        let existing = root.0.join(".import-1-7-3");std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("keep.txt"), "prior import").unwrap();
        assert!(SampleImportFiles::new(&root.0, "1-7-3").is_err());
        assert!(SampleImportFiles::new(&root.0, "../escape").is_err());
        assert_eq!(std::fs::read_to_string(existing.join("keep.txt")).unwrap(), "prior import");
    }

    fn taught() -> Teaching {
        let params = FrameParams {
            rect: [10, 10, 30, 30],
            ..FrameParams::default()
        };
        let image = FrozenImage {
            id: "image-a".into(),
            source: "test".into(),
            captured_at: 1,
            size: [100, 100],
            camera: "cam".into(),
            camera_tag: "camera".into(),
            calib_tag: "calib".into(),
            geometry_tag: "geometry".into(),
            exposure_us: None,
            gain_db: None,
            history_id: None,
        };
        let trial = Trial {
            image_id: image.id.clone(),
            params_tag: fingerprint(&params),
            geometry_tag: "geometry".into(),
            passed: true,
            score: 0.9,
            coverage: 1.0,
            elapsed_ms: 1,
            reason: "pass".into(),
            measurement: Value::Null,
        };
        Teaching {
            k: 0,
            image: Some(image),
            params,
            trial: Some(trial),
            saved: true,
            backup: None,
        }
    }

    #[test]
    fn teach_rejects_replaced_image_parameters_geometry_and_failed_trial() {
        let original = taught();
        assert!(original.checked("geometry").is_ok());
        let mut frame = original.clone();
        frame.image.as_mut().unwrap().id = "image-b".into();
        assert!(frame.checked("geometry").is_err());
        let mut frame = original.clone();
        frame.params.search_mm += 0.5;
        assert!(frame.checked("geometry").is_err());
        let mut frame = original.clone();
        frame.trial.as_mut().unwrap().passed = false;
        assert!(frame.checked("geometry").is_err());
        assert!(original.checked("other-geometry").is_err());
    }

    #[test]
    fn teaching_and_display_changes_version_hash_but_keep_geometry_compatible() {
        let doc = crate::recipe::samples().remove(0);
        let legacy = serde_json::to_value(&doc).unwrap();
        assert!(legacy.get("teachingHash").is_none());
        let old = doc.build().unwrap();
        let mut candidate = doc.clone();
        candidate.version += 1;
        assert_eq!(candidate.build().unwrap().hash, old.hash);
        candidate.teaching_hash = Some("saved-image-and-overview".into());
        let new = candidate.build().unwrap();
        assert_ne!(new.hash, old.hash);
        assert_eq!(new.geometry_hash(), old.geometry_hash());
        let mut w = Workspace::new(doc, Some(old.hash.clone()));
        w.pending = Some(Box::new(Release {
            doc: w.doc.clone(),
            base_hash: w.base_hash.clone(),
            revision: w.revision,
            frames: vec![taught()],
            overview: Overview::default(),
            validation: Validation {
                revision: w.revision,
                passed: true,
                checked_at: 0,
                checks: vec![],
                samples: vec![],
                environment_tag: "test".into(),
            },
        }));
        w.doc.name = "changed after enqueue".into();
        w.changed();
        let restored: Workspace =
            serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        let pending = restored.pending.unwrap();
        assert_ne!(restored.doc.name, pending.doc.name);
        assert_eq!(pending.revision, 1);
        assert_eq!(pending.frames[0].image.as_ref().unwrap().id, "image-a");
        assert_eq!(restored.base_hash, Some(old.hash));
    }

    #[test]
    fn preview_cache_isolated_by_part_and_preserves_exact_frame() {
        let mut cache = LiveFrames::default();
        let first = Arc::new(FrameImage::new(1, 1, vec![7]));
        cache.insert(12, "old", 0, first.clone());
        cache.insert(12, "old", 1, Arc::new(FrameImage::new(1, 1, vec![9])));
        assert!(Arc::ptr_eq(&cache.frames[0].1, &first));
        cache.insert(13, "new", 2, Arc::new(FrameImage::new(1, 1, vec![11])));
        assert_eq!(cache.frames.len(), 1);
        assert_eq!(cache.bytes, 1);
        assert_eq!(cache.key, Some((13, "new".into())));
    }

    #[test]
    fn roi_bounds_and_owned_point_search_margin_are_enforced() {
        let mut params = FrameParams {
            rect: [90, 90, 20, 20],
            ..FrameParams::default()
        };
        assert!(params.validate([100, 100]).is_err());
        params.rect = [u32::MAX, 0, 20, 20];
        assert!(params.validate([100, 100]).is_err());
        let recipe = crate::recipe::samples().remove(0).build().unwrap();
        let mut frames: Vec<_> = (0..recipe.shot_count()).map(Teaching::empty).collect();
        let initial = coverage(&recipe, &frames);
        frames[0].params.search_mm = recipe.fov[0].max(recipe.fov[1]);
        assert!(coverage(&recipe, &frames) < initial);
    }

    #[test]
    fn offline_image_import_decodes_real_grayscale_pixels() {
        let mut bytes = b"P5\n3 2\n255\n".to_vec();
        bytes.extend([0, 32, 64, 128, 200, 255]);
        let image = decode_imported_image(bytes).unwrap();
        assert_eq!([image.width, image.height], [3, 2]);
        assert_eq!(image.pixels, [0, 32, 64, 128, 200, 255]);
    }

    #[test]
    fn offline_image_import_rejects_empty_oversized_corrupt_and_huge_images() {
        assert!(decode_imported_image(vec![]).unwrap_err().contains("不得为空"));
        assert!(decode_imported_image(vec![0; 15_000_001]).unwrap_err().contains("15 MB"));
        assert!(decode_imported_image(b"not an image".to_vec()).is_err());
        assert!(decode_imported_image(b"P5\n999999 999999\n255\n".to_vec()).is_err());
    }
}

#[tauri::command]
pub async fn workspace_import_sample(
    app: AppHandle,
    id: String,
    revision: u64,
    name: String,
    expected: Verdict,
    images: Vec<SampleImageInput>,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let w = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.clone()
        };
        let r = w.doc.build()?;
        if r.mode != InspectMode::FlyShot {
            return Err("随动样本使用历史完整测量数据验证".into());
        }
        let mut unique = std::collections::HashSet::new();
        if images.len() != r.shot_count()
            || images
                .iter()
                .any(|i| i.k >= r.shot_count() || !unique.insert(i.k) || i.bytes.len() > 15_000_000)
            || images.iter().map(|i| i.bytes.len()).sum::<usize>() > 80_000_000
        {
            return Err("每个拍照点需一张原图；单图不超过 15 MB，整组不超过 80 MB".into());
        }
        if w.sample_bank.len() >= 100 {
            return Err("每个候选最多保留 100 个验证样本组".into());
        }
        if name.trim().is_empty() { return Err("请填写代表性样本名称".into()); }
        let seq = host.capture_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sample_id = format!("{}-{revision}-{seq}", ly_plc::now_ms());
        let mut files = SampleImportFiles::new(&host.dir(&id).join("samples"), &sample_id)?;
        files.write_images(&w, images)?;
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(&id).ok_or("候选配置不存在")?;
        current.expect(revision)?;
        let mut next = current.clone();
        next.sample_bank.push(BankSample {
            id: sample_id.clone(),
            name: name.trim().chars().take(80).collect(),
            geometry_tag: r.geometry_hash(),
            expected,
            created_at: ly_plc::now_ms(),
        });
        next.changed();
        files.publish(&sample_id)?;
        host.save(&next)?;
        *current = next;
        files.keep = true;
        view(&app, current.clone())
    })
    .await
    .map_err(|e| e.to_string())?
}

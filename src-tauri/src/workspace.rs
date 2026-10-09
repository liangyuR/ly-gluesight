use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::camera::CameraSource;
use crate::cycle::{self, CycleHost};
use crate::frame::FrameImage;
use crate::judge::{self, PointState, Verdict};
use crate::recipe::{DetectParams, Recipe, RecipeDoc};
use crate::store::Store;
use crate::vision;

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

/// 一个拍照点的示教：在冻结原图上点出的胶路中线、像素当量与可选的检测参数，保存进候选配方的这个拍照点。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShotTeach {
    pub path: Vec<[f32; 2]>,
    pub mm_per_px: f32,
    #[serde(default)]
    pub detect: Option<DetectParams>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenImage {
    pub id: String,
    pub view: u8,
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
    pub views: Vec<FrozenImage>,
    pub image: Option<FrozenImage>,
    pub trial: Option<Trial>,
    pub saved: bool,
    pub backup: Option<Box<Teaching>>,
}

impl Teaching {
    fn empty(k: usize) -> Self {
        Self {
            k,
            views: Vec::new(),
            image: None,
            trial: None,
            saved: false,
            backup: None,
        }
    }

    /// 原图要对上拍照点（image_tag），试测要对上当前的中线与检测参数（teach_tag），并且通过。
    fn checked(&self, image_tag: &str, teach_tag: &str) -> Result<(), String> {
        let image = self.image.as_ref().ok_or("本帧尚未冻结原图")?;
        let trial = self.trial.as_ref().ok_or("本帧尚未试测")?;
        if image.geometry_tag != image_tag
            || !trial.passed
            || trial.image_id != image.id
            || trial.geometry_tag != image_tag
            || trial.params_tag != teach_tag
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
        let count = doc.shots.len();
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
        let uses = |doc: &RecipeDoc| doc.shots.iter().any(|s| s.camera == camera);
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

/// 要检的拍照点里已示教中线的比例（%）。
fn coverage(recipe: &Recipe) -> f32 {
    let measured = recipe.shots.iter().filter(|s| s.measured()).count();
    if measured == 0 {
        return 100.0;
    }
    100.0 * recipe.shots.iter().filter(|s| s.measured() && s.taught()).count() as f32 / measured as f32
}

/// 拍照点 k 的两个指纹：原图要对上编号、相机与标定引用；试测要对上中线、像素当量、检测参数与站距。
fn shot_tags(r: &Recipe, k: usize) -> Result<(String, String), String> {
    let shot = r.shots.get(k).ok_or("拍照点不存在")?;
    Ok((
        fingerprint(&json!([shot.id, shot.camera, shot.view, shot.calib_ref()])),
        fingerprint(&json!([shot.view, shot.path, shot.mm_per_px, r.shot_detect(k), r.spacing])),
    ))
}

/// 样本组的原图要对上每个拍照点的编号、相机与标定引用（改中线不影响样本）。
fn images_tag(r: &Recipe) -> String {
    fingerprint(&(0..r.shot_count()).map(|k| shot_tags(r, k).map(|t| t.0).unwrap_or_default()).collect::<Vec<_>>())
}

fn view(app: &AppHandle, w: Workspace) -> Result<WorkspaceView, String> {
    let layout = w.doc.build()?;
    let production_version = app
        .state::<CycleHost>()
        .recipe(&w.doc.id)
        .map(|r| r.version);
    let coverage = coverage(&layout);
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
        values.push(json!(config));
    }
    for k in 0..recipe.shot_count() {
        values.push(json!(crate::fsio::read_text(&vision::shot_calib_path(app, recipe, k)?).ok()));
    }
    let settings = cycle.settings();
    values.push(json!([settings.vision, settings.lyflow_core]));
    Ok(fingerprint(&values))
}

/// 拍照点 k 的相机参数与标定指纹：冻结图像时记下，之后变了就要重新取样。
fn tags(app: &AppHandle, recipe: &Recipe, k: usize) -> Result<(String, String), String> {
    let cycle = app.state::<CycleHost>();
    let shot = recipe.shots.get(k).ok_or("拍照点不存在")?;
    let cam = cycle.camera.require(&shot.camera)?;
    let config = cycle
        .camera
        .slot(cam as usize)
        .ok_or("相机不存在")?
        .config();
    let calib = vision::shot_calib_path(app, recipe, k)?;
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
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    update_doc(&mut w, doc)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

fn update_doc(w: &mut Workspace, mut doc: RecipeDoc) -> Result<(), String> {
    for shot in &mut doc.shots {
        if w.doc.shots.iter().find(|old| old.id == shot.id).is_some_and(|old| old.view != shot.view) {
            shot.path.clear();
            shot.mm_per_px = None;
        }
    }
    let recipe = doc.build()?;
    if doc == w.doc { return Ok(()); }
    let before = w.doc.build()?;
    let reordered = w.doc.shots.iter().map(|s| &s.id).ne(doc.shots.iter().map(|s| &s.id));
    let frames = recipe.shots.iter().enumerate().map(|(k, shot)| {
        let Some(old_k) = w.doc.shots.iter().position(|s| s.id == shot.id) else { return Teaching::empty(k); };
        let mut frame = w.frames.get(old_k).cloned().unwrap_or_else(|| Teaching::empty(k));
        frame.k = k;
        let current = shot_tags(&recipe, k).unwrap();
        if shot_tags(&before, old_k).ok().as_ref() != Some(&current) {
            frame.trial = None;
            frame.saved = false;
        }
        if frame.image.as_ref().is_some_and(|i| i.geometry_tag != current.0) {
            frame.image = frame.views.iter().find(|i| i.view == shot.view && i.geometry_tag == current.0).cloned();
            if frame.image.is_none() {
                frame.views.clear();
                frame.backup = None;
            }
        }
        frame
    }).collect();
    w.frames = frames;
    w.doc = doc;
    if reordered { w.overview.saved = false; }
    w.changed();
    Ok(())
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
    let selected = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.doc.shots.get(k).ok_or("拍照点不存在")?.view
    };
    keep_views(app, id, revision, k, vec![(selected, Arc::new(image))], source, history_id)
}

fn keep_views(
    app: &AppHandle,
    id: &str,
    revision: u64,
    k: usize,
    images: Vec<(u8, Arc<FrameImage>)>,
    source: String,
    history_id: Option<i64>,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    let r = w.doc.build()?;
    let shot = r.shots.get(k).ok_or("拍照点不存在")?;
    let mut seen = std::collections::HashSet::new();
    for (v, image) in &images {
        if !(1..=3).contains(v) || !seen.insert(*v) {
            return Err("冻结图像的视角编号无效或重复".into());
        }
        if image.width == 0 || image.height == 0 || u64::from(image.width) * u64::from(image.height) != image.pixels.len() as u64 {
            return Err(format!("视角 {v} 的图像数据不完整"));
        }
    }
    if !seen.contains(&shot.view) {
        return Err(format!("本次取图缺少拍照点 {} 选择的视角 {}", shot.id, shot.view));
    }
    let (camera_tag, calib_tag) = tags(app, &r, k)?;
    let camera = shot.camera.clone();
    let config = app
        .state::<CycleHost>()
        .camera
        .configs()
        .into_iter()
        .find(|c| c.id == camera)
        .ok_or("相机不存在")?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    let imported = source == "import";
    if (history_id.is_some() || imported) && frame.backup.is_none() && frame.image.is_some() {
        let mut backup = frame.clone();
        backup.backup = None;
        frame.backup = Some(Box::new(backup));
    }
    let captured_at = ly_plc::now_ms();
    let seq = host.capture_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut frozen = Vec::new();
    for (v, image) in images {
        let mut image_recipe = r.clone();
        image_recipe.shots[k].view = v;
        let image_id = format!("{captured_at}-{revision}-{k}-{seq}-v{v}");
        let path = image_file(&host, id, &image_id)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        crate::replay::save_pgm(&path, &image)?;
        frozen.push(FrozenImage {
            id: image_id,
            view: v,
            source: source.clone(),
            captured_at,
            size: [image.width, image.height],
            camera: camera.clone(),
            camera_tag: camera_tag.clone(),
            calib_tag: calib_tag.clone(),
            geometry_tag: shot_tags(&image_recipe, k)?.0,
            exposure_us: (history_id.is_none() && !imported).then_some(config.exposure_us),
            gain_db: (history_id.is_none() && !imported).then_some(config.gain_db),
            history_id,
        });
    }
    frame.image = frozen.iter().find(|i| i.view == shot.view).cloned();
    frame.views = frozen;
    frame.trial = None;
    frame.saved = false;
    // 还没有像素当量时按这台相机的标定（模拟相机按模拟画面）给一个，示教时可改
    if w.doc.shots[k].mm_per_px.is_none() {
        w.doc.shots[k].mm_per_px = if config.source == CameraSource::Sim && !imported {
            Some(crate::recipe::SIM_MM_PER_PX)
        } else if shot.view == 1 || shot.calib.is_some() {
            vision::calib_info(&vision::shot_calib_path(app, &r, k)?).and_then(|c| c.mm_per_px).map(|m| m as f32)
        } else {
            None
        };
    }
    w.changed();
    host.save(&w)?;
    items.insert(id.into(), w.clone());
    view(app, w)
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
            if k >= w.frames.len() {
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
        if k >= r.shot_count() {
            return Err("请选择飞拍配方中的拍照点".into());
        }
        let cam = cycle.camera.require(&r.shots[k].camera)?;
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        if r.shots[k].view > config.view_count {
            return Err(format!("拍照点 {} 选择视角 {}，设备只配置了 {} 个视角", r.shots[k].id, r.shots[k].view, config.view_count));
        }
        let render = (config.source == CameraSource::Sim).then(|| crate::camera::SimRender {
            recipe: Arc::new(r.clone()), k,
            scenario: crate::sim::Scenario::Normal,
            pose: crate::simimage::PoseError::default(), seed: 0,
        });
        let images = cycle.camera.capture_views(cam, render)?;
        if cycle.busy() || slot.config() != config {
            return Err("取样期间设备或节拍状态变化，这次取样已丢弃".into());
        }
        keep_views(
            &app,
            &id,
            revision,
            k,
            images.into_iter().enumerate().map(|(i, image)| (i as u8 + 1, image)).collect(),
            format!("{:?}", config.source),
            None,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

fn select_view(w: &mut Workspace, k: usize, selected: u8) -> Result<(), String> {
    if !(1..=3).contains(&selected) {
        return Err("视角必须在 1–3 之间".into());
    }
    let mut doc = w.doc.clone();
    let shot = doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    if shot.view == selected {
        return Ok(());
    }
    shot.view = selected;
    shot.path.clear();
    shot.mm_per_px = None;
    let recipe = doc.build()?;
    let (image_tag, _) = shot_tags(&recipe, k)?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    let image = frame.views.iter().find(|i| i.view == selected).ok_or("本次冻结图像没有这个视角，请重新取样")?;
    if image.geometry_tag != image_tag {
        return Err("拍照点或相机已变更，请重新冻结全部视角".into());
    }
    frame.image = Some(image.clone());
    frame.trial = None;
    frame.saved = false;
    w.doc = doc;
    w.changed();
    Ok(())
}

#[tauri::command]
pub fn workspace_select_view(app: AppHandle, id: String, revision: u64, k: usize, view: u8) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    select_view(&mut w, k, view)?;
    let recipe = w.doc.build()?;
    let (camera_tag, calib_tag) = tags(&app, &recipe, k)?;
    let image = w.frames[k].image.as_ref().ok_or("尚未冻结图像")?;
    if image.camera_tag != camera_tag || image.calib_tag != calib_tag {
        return Err("相机参数或标定已变更，请重新冻结全部视角".into());
    }
    host.save(&w)?;
    items.insert(id, w.clone());
    self::view(&app, w)
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

/// 示教帧试测：沿拍照点 k 的示教中线量胶。返回逐站结果、得分、量成比例、是否通过与原因。
fn measure_image(_app: &AppHandle, r: &Recipe, k: usize, _image: &FrameImage) -> Result<(Value, f64, f64, bool, String), String> {
    let shot = r.shots.get(k).ok_or("拍照点不存在")?;
    if !shot.measured() {
        return Err("这个拍照点设为不检，不需要试测".into());
    }
    if !shot.taught() {
        return Err("还没有胶路中线：先在原图上点出中线并保存".into());
    }
    Err(vision::TAUGHT_PATH_PENDING.into())
}

#[tauri::command]
pub async fn workspace_trial(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
    image_id: String,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let r = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            let frozen = w
                .frames
                .get(k)
                .ok_or("拍照点不存在")?
                .image
                .as_ref()
                .filter(|i| i.id == image_id)
                .ok_or("冻结图像已更新，请重新试测")?;
            let r = w.doc.build()?;
            let (camera_tag, calib_tag) = tags(&app, &r, k)?;
            if frozen.camera_tag != camera_tag || frozen.calib_tag != calib_tag || frozen.geometry_tag != shot_tags(&r, k)?.0 {
                return Err("相机参数、工位标定或拍照点已变更，请重新冻结图像".into());
            }
            r
        };
        let (image_tag, teach_tag) = shot_tags(&r, k)?;
        let image = crate::replay::load(&image_file(&app.state::<WorkspaceHost>(), &id, &image_id)?)?;
        let started = Instant::now();
        let (measurement, score, coverage, passed, reason) =
            measure_image(&app, &r, k, &image).unwrap_or_else(|e| (Value::Null, 0.0, 0.0, false, e));
        let trial = Trial {
            image_id,
            params_tag: teach_tag,
            geometry_tag: image_tag,
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
        current.frames[k].trial = Some(trial);
        current.frames[k].saved = false;
        current.changed();
        host.save(current)?;
        view(&app, current.clone())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 保存一个拍照点的示教（中线、像素当量、检测参数）进候选配方；这一帧的试测随之作废。
#[tauri::command]
pub fn workspace_save_params(
    app: AppHandle,
    id: String,
    revision: u64,
    k: usize,
    params: ShotTeach,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let mut doc = w.doc.clone();
    let shot = doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    shot.path = params.path;
    shot.mm_per_px = Some(params.mm_per_px);
    shot.detect = params.detect;
    doc.build()?;
    if doc != w.doc {
        w.doc = doc;
        let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
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
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let r = w.doc.build()?;
    let (image_tag, teach_tag) = shot_tags(&r, k)?;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    if frame.image.as_ref().map(|i| &i.id) != Some(&image_id) {
        return Err("图像已变更，请重新试测".into());
    }
    frame.checked(&image_tag, &teach_tag)?;
    let (camera_tag, calib_tag) = tags(&app, &r, k)?;
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
    if frame.backup.as_ref().and_then(|b| b.image.as_ref()).is_some_and(|i| i.view != w.doc.shots[k].view) {
        return Err("原始示教属于其他视角，请先选择对应视角".into());
    }
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
    let ready = r.ready();
    checks.push(Check {
        name: "胶路示教".into(),
        passed: ready.is_ok(),
        detail: ready.err().unwrap_or_else(|| format!("要检的拍照点都已示教中线（{:.0}%）", coverage(&r))),
    });
    let teaching = w.frames.len() == r.shot_count()
        && w.frames.iter().filter(|f| r.shots[f.k].measured()).all(|f| {
            let current = tags(app, &r, f.k);
            let shot = shot_tags(&r, f.k);
            f.saved
                && shot.as_ref().is_ok_and(|(image, teach)| f.checked(image, teach).is_ok())
                && f.image
                    .as_ref()
                    .zip(current.as_ref().ok())
                    .is_some_and(|(i, (ct, cal))| &i.camera_tag == ct && &i.calib_tag == cal)
        });
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
        passed: w.overview.saved,
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
                if bank.geometry_tag != images_tag(&r) {
                    return Err("样本组来自另一版拍照点或相机，请重新导入".into());
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
            if original.geometry_hash() != r.geometry_hash() {
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
            .map(|f| (&f.image, &f.trial))
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
    let archive = host.dir(&recipe.id).join("published");
    std::fs::create_dir_all(&archive).map_err(|e| e.to_string())?;
    crate::fsio::write_atomic(
        &archive.join(format!("{}.json", recipe.hash)),
        &serde_json::to_string(release).map_err(|e| e.to_string())?,
    )?;
    cycle.recipes.save(release.doc.clone(), release.base_hash.as_ref().map(|_| release.doc.id.as_str()))
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
    pub view: u8,
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
    let frames = recorded_frames(&dir, &meta)?;
    Ok((dir, frames))
}

fn recorded_frames(dir: &Path, meta: &Value) -> Result<Vec<RawFrame>, String> {
    let raw = meta["frames"].as_array().ok_or("原图清单损坏")?;
    let shots = meta["recipe"]["shots"].as_array().ok_or("录制配方缺少拍照点")?;
    let mut frames = Vec::new();
    let mut selected = std::collections::HashSet::new();
    for f in raw {
        let Some(k) = f["k"].as_u64().and_then(|k| usize::try_from(k).ok()) else { continue; };
        let Some(shot) = shots.get(k) else { continue; };
        let Some(view) = f["view"].as_u64().and_then(|v| u8::try_from(v).ok()).filter(|v| (1..=3).contains(v)) else { continue; };
        if shot["view"].as_u64() != Some(view as u64) || f["camera"].as_str() != shot["camera"].as_str() { continue; }
        if !selected.insert(k) { return Err(format!("拍照点 {k} 的视角 {view} 原图重复，无法确定对应图像")); }
        let Some(file) = f["file"].as_str() else { continue; };
        let path = Path::new(file);
        if path.components().count() != 1 || !path.file_name().is_some_and(|n| n == file) { continue; }
        frames.push(RawFrame {
            k, view,
            camera: f["camera"].as_str().unwrap_or("").into(),
            file: file.into(),
            ts: f["ts"].as_i64().unwrap_or_else(|| meta["startedTs"].as_i64().unwrap_or(0)),
            available: dir.join(file).is_file(),
            cam: f["cam"].as_u64().and_then(|v| u8::try_from(v).ok()),
            frame_counter: f["frameCounter"].as_u64(),
            trigger_counter: f["triggerCounter"].as_u64(),
        });
    }
    frames.sort_by_key(|f| f.k);
    Ok(frames)
}

#[tauri::command]
pub fn workspace_record_images(app: AppHandle, history_id: i64) -> Result<RecordImages, String> {
    let store = app.state::<Store>();
    let detail = store.detail(history_id)?;
    match recorded(&app, history_id) {
        Ok((_, frames)) => {
            let complete = detail.summary.frames_expected > 0 && (0..detail.summary.frames_expected)
                .all(|k| frames.iter().any(|f| f.k == k && f.available));
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
    if doc.geometry_hash() != original.geometry_hash() {
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
        if original.geometry_hash() != recipe.geometry_hash() {
            return Err("原始记录与候选的几何不同；请重新采集代表性样本".into());
        }
        let mut measurements = Vec::new();
        let table = if raw {
            let (dir, frames) = recorded(&app, history_id)?;
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
        let image = cycle.camera.capture_view(cam, 1, None)?;
        if cycle.busy() || fingerprint(&slot.config()) != fingerprint(&config) {
            return Err("取样期间设备或节拍状态变化，样本已丢弃".into());
        }
        let host = app.state::<WorkspaceHost>();
        let seq = host
            .capture_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let meta = FrozenImage {
            view: 1,
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
            view: 1,
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
    if paths.len() != recipe.shot_count() {
        return Err("样本组的原图数量与拍照点不一致".into());
    }
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut measurements = Vec::new();
    for (k, path) in paths.iter().enumerate() {
        if !recipe.shots[k].measured() {
            continue;
        }
        let frame = w
            .frames
            .get(k)
            .filter(|f| f.saved)
            .ok_or("候选存在尚未保存的示教帧")?;
        let (image_tag, teach_tag) = shot_tags(&recipe, k)?;
        frame.checked(&image_tag, &teach_tag)?;
        let frozen = frame.image.as_ref().unwrap();
        let (ct, cal) = tags(app, &recipe, k)?;
        if frozen.camera_tag != ct || frozen.calib_tag != cal {
            return Err("相机或标定变化，需要重新示教".into());
        }
        let image = crate::replay::load(path)?;
        if [image.width, image.height] != frozen.size {
            return Err(format!("k{} 样本尺寸与示教图像不同", k + 1));
        }
        let (m, score, _, passed, reason) = measure_image(app, &recipe, k, &image)?;
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
        measurements.push(json!({ "sn":sn, "k":k, "cam":0, "s":null, "located":passed, "score":score,
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
        let image = FrozenImage {
            id: "image-a".into(),
            view: 1,
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
            params_tag: "teach".into(),
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
            views: vec![image.clone()],
            image: Some(image),
            trial: Some(trial),
            saved: true,
            backup: None,
        }
    }

    fn tricam_workspace() -> Workspace {
        let mut w = Workspace::new(crate::recipe::samples().remove(1), None);
        let r = w.doc.build().unwrap();
        for k in 0..w.frames.len() {
            let mut frame = taught();
            frame.k = k;
            frame.views = (1..=3).map(|view| {
                let mut recipe = r.clone();
                recipe.shots[k].view = view;
                FrozenImage {
                    id: format!("group-{k}-v{view}"), view,
                    camera: r.shots[k].camera.clone(),
                    geometry_tag: shot_tags(&recipe, k).unwrap().0,
                    ..frame.image.as_ref().unwrap().clone()
                }
            }).collect();
            frame.image = Some(frame.views[0].clone());
            let (image_tag, teach_tag) = shot_tags(&r, k).unwrap();
            frame.trial.as_mut().unwrap().image_id = frame.image.as_ref().unwrap().id.clone();
            frame.trial.as_mut().unwrap().geometry_tag = image_tag;
            frame.trial.as_mut().unwrap().params_tag = teach_tag;
            w.frames[k] = frame;
        }
        w
    }

    #[test]
    fn changing_view_invalidates_only_that_shot_and_never_restores_old_trial() {
        let mut w = tricam_workspace();
        let before = w.doc.build().unwrap();
        let other = fingerprint(&w.frames[1]);
        assert!(w.frames[0].checked(&shot_tags(&before, 0).unwrap().0, &shot_tags(&before, 0).unwrap().1).is_ok());
        select_view(&mut w, 0, 2).unwrap();
        assert_eq!(w.doc.shots[0].view, 2);
        assert!(w.doc.shots[0].path.is_empty());
        assert!(w.doc.shots[0].mm_per_px.is_none());
        assert_eq!(w.frames[0].image.as_ref().unwrap().view, 2);
        assert_eq!(w.frames[0].views.len(), 3);
        assert!(w.frames[0].views.iter().all(|i| i.captured_at == 1));
        assert!(!w.frames[0].saved);
        assert!(w.frames[0].trial.is_none());
        assert_eq!(fingerprint(&w.frames[1]), other);
        let after = w.doc.build().unwrap();
        assert_ne!(images_tag(&before), images_tag(&after));
        assert_ne!(shot_tags(&before, 0).unwrap(), shot_tags(&after, 0).unwrap());
        assert_eq!(shot_tags(&before, 1).unwrap(), shot_tags(&after, 1).unwrap());
        select_view(&mut w, 0, 1).unwrap();
        assert_eq!(w.frames[0].image.as_ref().unwrap().id, "group-0-v1");
        assert!(w.frames[0].trial.is_none());
        assert!(w.doc.shots[0].path.is_empty());
    }

    #[test]
    fn missing_invalid_or_stale_view_does_not_mutate_workspace() {
        let mut w = tricam_workspace();
        for selected in [0, 4] {
            let before = fingerprint(&w);
            assert!(select_view(&mut w, 0, selected).is_err());
            assert_eq!(fingerprint(&w), before);
        }
        w.frames[0].views.retain(|i| i.view != 2);
        let before = fingerprint(&w);
        assert!(select_view(&mut w, 0, 2).is_err());
        assert_eq!(fingerprint(&w), before);
        w.doc.shots[0].camera = "cam2".into();
        let before = fingerprint(&w);
        assert!(select_view(&mut w, 0, 3).is_err());
        assert_eq!(fingerprint(&w), before);
    }

    #[test]
    fn editing_recipe_view_cannot_reuse_old_line_or_other_view_pixels() {
        let mut w = tricam_workspace();
        let other = fingerprint(&w.frames[1]);
        let mut doc = w.doc.clone();
        doc.shots[0].view = 3;
        update_doc(&mut w, doc).unwrap();
        assert!(w.doc.shots[0].path.is_empty());
        assert!(w.doc.shots[0].mm_per_px.is_none());
        assert_eq!(w.frames[0].image.as_ref().unwrap().view, 3);
        assert!(w.frames[0].trial.is_none());
        assert_eq!(fingerprint(&w.frames[1]), other);
        let mut doc = w.doc.clone();
        doc.shots[0].camera = "cam2".into();
        update_doc(&mut w, doc).unwrap();
        assert!(w.frames[0].image.is_none());
        assert!(w.frames[0].views.is_empty());
        assert!(select_view(&mut w, 0, 2).is_err());
    }

    #[test]
    fn recorded_triplets_keep_four_shots_and_select_exact_recipe_view() {
        let root = ImportTestDir::new();
        let mut doc = crate::recipe::samples().remove(1);
        for (shot, view) in doc.shots.iter_mut().zip([1, 2, 3, 1]) { shot.view = view; }
        let mut entries = Vec::new();
        for k in 0..4 {
            for view in 1..=3 {
                let file = format!("cam1_{:06}_v{view}.pgm", k + 1);
                crate::replay::save_pgm(&root.0.join(&file), &FrameImage::new(1, 1, vec![(k * 10 + view) as u8])).unwrap();
                entries.push(json!({"k": k, "view": view, "camera": "cam1", "file": file, "frameCounter": k + 1, "triggerCounter": k + 1, "ts": k * 100}));
            }
        }
        entries.reverse();
        let mut meta = json!({"recipe": doc, "frames": entries, "startedTs": 0});
        let frames = recorded_frames(&root.0, &meta).unwrap();
        assert_eq!(frames.iter().map(|f| (f.k, f.view)).collect::<Vec<_>>(), [(0, 1), (1, 2), (2, 3), (3, 1)]);
        for frame in &frames {
            assert!(frame.available);
            assert_eq!(crate::replay::load(&root.0.join(&frame.file)).unwrap().pixels[0], (frame.k * 10 + frame.view as usize) as u8);
        }
        std::fs::remove_file(root.0.join(&frames[1].file)).unwrap();
        let missing = recorded_frames(&root.0, &meta).unwrap();
        assert!(!missing[1].available);
        assert_eq!(missing[1].view, 2);
        let duplicate = meta["frames"].as_array().unwrap().iter().find(|v| v["k"] == 2 && v["view"] == 3).unwrap().clone();
        meta["frames"].as_array_mut().unwrap().push(duplicate);
        assert!(recorded_frames(&root.0, &meta).unwrap_err().contains("重复"));
    }

    #[test]
    fn teach_rejects_replaced_image_parameters_geometry_and_failed_trial() {
        let original = taught();
        assert!(original.checked("geometry", "teach").is_ok());
        let mut frame = original.clone();
        frame.image.as_mut().unwrap().id = "image-b".into();
        assert!(frame.checked("geometry", "teach").is_err());
        // 中线或检测参数改了：试测作废
        assert!(original.checked("geometry", "other-teach").is_err());
        let mut frame = original.clone();
        frame.trial.as_mut().unwrap().passed = false;
        assert!(frame.checked("geometry", "teach").is_err());
        assert!(original.checked("other-geometry", "teach").is_err());
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
    fn shot_tags_separate_image_from_teaching() {
        let base = crate::recipe::samples().remove(1).build().unwrap();
        let tag = |d: &crate::recipe::RecipeDoc, k| shot_tags(&d.build().unwrap(), k).unwrap();
        let doc = crate::recipe::samples().remove(1);
        let (image, teach) = shot_tags(&base, 1).unwrap();
        let mut moved = doc.clone();
        moved.shots[1].path[1][0] += 3.0;
        assert_eq!(tag(&moved, 1).0, image, "改中线不让原图作废");
        assert_ne!(tag(&moved, 1).1, teach, "改中线让试测作废");
        assert_eq!(tag(&moved, 0), shot_tags(&base, 0).unwrap(), "别的拍照点不受影响");
        let mut camera = doc.clone();
        camera.shots[1].camera = "cam2".into();
        assert_ne!(tag(&camera, 1).0, image);
        assert_eq!(images_tag(&moved.build().unwrap()), images_tag(&base));
        assert_eq!(coverage(&base), 100.0);
        let mut untaught = doc;
        untaught.shots[2].path.clear();
        untaught.shots[2].mm_per_px = None;
        assert_eq!(coverage(&untaught.build().unwrap()), 75.0);
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
            geometry_tag: images_tag(&r),
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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::camera::CameraSource;
use crate::cycle::{self, CycleHost};
use crate::frame::FrameImage;
use crate::judge::{self, Verdict};
use crate::recipe::{CameraImaging, DetectParams, Recipe, RecipeDoc, RecipeStage};
use crate::release::ValidationSite;
use crate::store::{self, Store};
use crate::vision;

mod burst;
mod burst_history;
mod burst_release;
#[cfg(test)]
mod burst_release_tests;
#[cfg(test)]
mod burst_tests;
mod capture_review;
pub(crate) mod centerline;
mod collect_release;
#[cfg(test)]
mod collect_release_tests;
mod comparisons;
mod history_samples;
pub mod progress;
mod recorded;
mod reproduce;
mod transfer;
pub(crate) use transfer::{copy_hashed, reset_for_project_import};
mod workflow;
pub use burst::*;
pub use burst_history::*;
pub use capture_review::*;
pub use collect_release::*;
pub use transfer::*;
pub use workflow::*;

fn state_value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn engine_state(engine: &vision::Engine) -> Value {
    // 换了同版本号的另一份 DLL（大小或修改时间变了）也要重新试测
    json!({"path": engine.path, "version": engine.version, "bytes": engine.bytes, "modifiedMs": engine.modified_ms})
}

fn calibration_state(path: &Path) -> Result<Value, String> {
    let value = match crate::fsio::read_text(path) {
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|e| format!("标定文件格式无效：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(format!("读取标定文件失败：{e}")),
    };
    Ok(json!({"path": path, "value": value}))
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenImage {
    pub id: String,
    pub view: u8,
    pub source: String,
    pub captured_at: i64,
    pub size: [u32; 2],
    pub camera: String,
    pub camera_tag: Value,
    pub calib_tag: Value,
    pub geometry_tag: Value,
    pub exposure_us: Option<f32>,
    pub gain_db: Option<f32>,
    pub history_id: Option<i64>,
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
    pub geometry_tag: Value,
    pub expected: Verdict,
    pub created_at: i64,
    #[serde(default)]
    pub capture_id: Option<String>,
    #[serde(default)]
    pub camera_state: Value,
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
    pub sn: String,
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
    pub environment_tag: Value,
}

/// 旧拍照点模型写进候选文件的键（`frames`、`overview`、`lastPosition`）：读入时丢弃，不再写出。
#[derive(Clone, Debug, Default)]
pub struct Discarded;

impl<'de> Deserialize<'de> for Discarded {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Discarded)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub doc: RecipeDoc,
    pub bundle_id: String,
    pub base_revision: Option<String>,
    pub revision: u64,
    pub validation: Validation,
    pub validation_site: ValidationSite,
    #[serde(default, rename = "frames", skip_serializing)]
    pub _frames: Discarded,
    #[serde(default, rename = "overview", skip_serializing)]
    pub _overview: Discarded,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    pub doc: RecipeDoc,
    #[serde(default)]
    pub instance_id: u64,
    pub base_revision: Option<String>,
    pub revision: u64,
    pub samples: Vec<Sample>,
    pub sample_bank: Vec<BankSample>,
    pub validation: Option<Validation>,
    pub pending: Option<Box<Release>>,
    pub publish: PublishState,
    pub updated_at: i64,
    #[serde(default)]
    pub capture_id: Option<String>,
    /// 还没有检测相机使用的相机在采集页调好的曝光与增益：配方只收检测相机，用上时并入配方
    #[serde(default)]
    pub unbound_imaging: BTreeMap<String, CameraImaging>,
    /// 段配方：每台计划相机的走廊示教帧
    #[serde(default)]
    pub camera_teaching: Vec<burst::CameraTeaching>,
    /// 段配方：多轮标定加入的轮次
    #[serde(default)]
    pub calibration: burst::Calibration,
    #[serde(default)]
    pub capture_review: capture_review::CaptureReview,
    #[serde(default, rename = "frames", skip_serializing)]
    pub _frames: Discarded,
    #[serde(default, rename = "overview", skip_serializing)]
    pub _overview: Discarded,
    #[serde(default, rename = "lastPosition", skip_serializing)]
    pub _last_position: Discarded,
}

/// 按拍照点用到的相机整理配方的成像参数：不再用到的挪到 unbound，新用到的优先取 unbound 里调好的，没有就用默认值。
fn sync_doc_imaging(doc: &mut RecipeDoc, unbound: &mut BTreeMap<String, CameraImaging>) {
    let before = doc.cameras.keys().cloned().collect::<Vec<_>>();
    let dropped = doc.sync_imaging();
    for (camera, imaging) in doc.cameras.iter_mut() {
        if let Some(staged) = unbound.remove(camera) {
            if !before.contains(camera) { *imaging = staged; }
        }
    }
    unbound.extend(dropped);
}

/// 改一台相机的成像参数：拍照点用到的写进候选配方，还没用到的先记在 unbound。不动已采集的图像与示教。
fn set_imaging(w: &mut Workspace, camera: &str, imaging: CameraImaging) -> Result<(), String> {
    if !crate::recipe::valid_camera_id(camera) { return Err("相机编号无效".into()); }
    imaging.validate(camera)?;
    let target = if w.doc.cameras.contains_key(camera) { &mut w.doc.cameras } else { &mut w.unbound_imaging };
    if target.get(camera) == Some(&imaging) { return Ok(()); }
    target.insert(camera.to_string(), imaging);
    w.doc.build()?;
    w.changed();
    Ok(())
}

impl Workspace {
    /// 这台相机当前该用的成像参数：配方里的，或采集页先调好的，都没有时用默认值。
    pub fn imaging(&self, camera: &str) -> CameraImaging {
        self.doc.cameras.get(camera).or_else(|| self.unbound_imaging.get(camera)).copied().unwrap_or_default()
    }

    fn new(mut doc: RecipeDoc, base_revision: Option<String>) -> Self {
        if base_revision.is_some() {
            doc.version = doc.version.saturating_add(1);
        }
        Self {
            doc,
            instance_id: 0,
            base_revision,
            revision: 1,
            samples: Vec::new(),
            sample_bank: Vec::new(),
            validation: None,
            pending: None,
            publish: PublishState::Draft,
            updated_at: ly_plc::now_ms(),
            capture_id: None,
            unbound_imaging: BTreeMap::new(),
            camera_teaching: Vec::new(),
            calibration: burst::Calibration::default(),
            capture_review: capture_review::CaptureReview::default(),
            _frames: Discarded,
            _overview: Discarded,
            _last_position: Discarded,
        }
    }

    fn ensure_instance(&mut self, store: &Store) -> Result<(), String> {
        self.instance_id = store.workspace_instance(&self.doc.id, self.instance_id)?;
        Ok(())
    }

    fn debug_revision(&self) -> Result<String, String> {
        if self.instance_id == 0 { return Err("候选尚未分配持久实例 ID".into()); }
        Ok(format!("candidate-{}-i{}-r{}", self.doc.id, self.instance_id, self.revision))
    }

    fn expect(&self, revision: u64) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("候选版本正在等待生效，完成或处理发布失败后再编辑".into());
        }
        if self.revision != revision {
            return Err("候选配置已更新，请重新加载后操作".into());
        }
        Ok(())
    }

    fn changed(&mut self) {
        capture_review::refresh(self);
        self.revision += 1;
        self.validation = None;
        self.updated_at = ly_plc::now_ms();
    }

    fn retry_publish(&mut self, revision: u64) -> Result<(), String> {
        if self.revision != revision || self.pending.is_none() || !matches!(self.publish, PublishState::Failed { .. } | PublishState::Uncertain { .. }) {
            return Err("没有可以重试的发布版本".into());
        }
        self.publish = PublishState::Waiting;
        Ok(())
    }

    /// 生效失败时放弃冻结的待生效版本，候选回到草稿；生产版本不受影响。状态不确定时须先核实，不能放弃。
    fn abandon_publish(&mut self, revision: u64) -> Result<(), String> {
        if self.revision != revision || self.pending.is_none() || !matches!(self.publish, PublishState::Failed { .. }) {
            return Err("只有生效失败的待生效版本可以放弃；生效状态不确定时请先核实发布状态".into());
        }
        self.pending = None;
        self.publish = PublishState::Draft;
        self.updated_at = ly_plc::now_ms();
        Ok(())
    }

    fn activation_uncertain(&self) -> bool {
        matches!(self.publish, PublishState::Uncertain { .. })
    }

    fn retain_failed_activation(&mut self, before: Self, error: &str) {
        *self = before;
        self.publish = PublishState::Uncertain { message: format!("生产生效状态未能保存：{error}，请检查数据目录") };
    }
}

/// 候选的发布与生效状态。Waiting / Failed / Uncertain 时 pending 保存冻结的待生效版本。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PublishState {
    #[default]
    Draft,
    Waiting,
    Failed { message: String },
    Uncertain { message: String },
    /// 已生效；revision 是生效后候选的修订号，候选再改动后回到草稿。
    Active { version: u32, revision: u64, warning: Option<String>, validation_site: ValidationSite },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    pub workspace: Workspace,
    pub layout: Recipe,
    pub production_version: Option<u32>,
    /// 生产配方是采集版还是正式版；未发布为空
    pub production_stage: Option<RecipeStage>,
    /// 生产配方的 PLC 计划版本
    pub production_plan_version: Option<u32>,
    /// 候选的计划身份与生产配方不同（或还没发布过）
    pub plan_changed: bool,
    /// 段配方的计划部分（决策记录 5.1）
    pub plan: burst::WorkspacePlanView,
}

pub struct WorkspaceHost {
    root: PathBuf,
    items: Mutex<HashMap<String, Workspace>>,
    pub notes: Vec<String>,
    /// 段配方在途件的实时原图，按 (计划相机下标, seq)
    live_burst: Mutex<LiveFrames<(usize, u64)>>,
    station: Mutex<HashMap<u8, (FrozenImage, Arc<FrameImage>)>>,
    capture_seq: std::sync::atomic::AtomicU64,
    comparisons: comparisons::Comparisons,
}

/// 在途件的实时原图：按工件与配方修订成组，换件即清空；总量超过上限时丢最早的。
/// 键为 (计划相机下标, seq)。
#[derive(Default)]
struct LiveFrames<K> {
    key: Option<(String, String)>,
    frames: VecDeque<(K, Arc<FrameImage>)>,
    bytes: usize,
}

impl<K: PartialEq + Default> LiveFrames<K> {
    fn insert(&mut self, cycle_id: &str, revision_id: &str, k: K, image: Arc<FrameImage>) {
        if self
            .key
            .as_ref()
            .is_none_or(|key| key.0 != cycle_id || key.1 != revision_id)
        {
            *self = Self {
                key: Some((cycle_id.into(), revision_id.into())),
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
    pub(crate) fn capture_for_candidate<T>(&self, id: &str, start: impl FnOnce(&Workspace) -> Result<T, String>) -> Result<T, String> {
        let items = self.items.lock().unwrap();
        let workspace = items.get(id).ok_or("请先创建或打开配方草稿")?;
        workspace.expect(workspace.revision)?;
        start(workspace)
    }

    /// 候选与已提交的发布快照都保留相机引用，删除相机前核对。
    pub fn camera_users(&self, camera: &str) -> Vec<String> {
        let uses = |doc: &RecipeDoc| doc.plan_cameras().iter().any(|c| c == camera);
        self.items.lock().unwrap().values()
            .filter(|w| uses(&w.doc) || w.pending.as_ref().is_some_and(|p| uses(&p.doc)))
            .map(|w| w.doc.id.clone()).collect()
    }

    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let root = crate::storage::project(app).join("workspaces");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut items = HashMap::new();
        let mut notes = Vec::new();
        for entry in std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let file = entry.path().join("workspace.json");
            if !file.is_file() { continue; }
            let loaded = crate::fsio::read_text(&file).map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str::<Workspace>(&text).map_err(|e| e.to_string()))
                .and_then(|mut w| {
                    safe_id(&w.doc.id)?;
                    if w.doc.id != entry.file_name().to_string_lossy() { return Err("候选编号与目录不一致".into()); }
                    w.doc.build()?;
                    let legacy = w.instance_id == 0;
                    w.ensure_instance(&app.state::<Store>())?;
                    if legacy {
                        crate::fsio::write_atomic(&file, &serde_json::to_string_pretty(&w).map_err(|e| e.to_string())?)?;
                    }
                    Ok(w)
                });
            match loaded {
                Ok(w) => {
                    items.insert(w.doc.id.clone(), w);
                }
                Err(error) => notes.push(format!(
                    "{} 未加载：{error}。{}",
                    file.display(),
                    unloaded_hint(&entry.path())
                )),
            }
        }
        Ok(Self {
            root,
            items: Mutex::new(items),
            notes,
            live_burst: Mutex::new(LiveFrames::default()),
            station: Mutex::new(HashMap::new()),
            capture_seq: std::sync::atomic::AtomicU64::new(0),
            comparisons: comparisons::Comparisons::default(),
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
        )?;
        Ok(())
    }
}

fn view(app: &AppHandle, w: Workspace) -> Result<WorkspaceView, String> {
    let layout = w.doc.build()?;
    let production_recipe = app.state::<CycleHost>().recipe(&w.doc.id);
    let production_version = production_recipe.as_ref().map(|r| r.version);
    let production_stage = production_recipe.as_ref().map(|r| r.stage);
    let production = app.state::<CycleHost>().recipes.doc(&w.doc.id);
    let plan_changed = production
        .as_ref()
        .is_none_or(|p| p.plan_version == 0 || !p.same_plan(&w.doc));
    let plan = burst::plan_view(
        app,
        &w,
        &layout,
        production_recipe.as_deref(),
        production.as_ref(),
        production_version,
    );
    Ok(WorkspaceView {
        workspace: w,
        layout,
        production_version,
        production_stage,
        production_plan_version: production.map(|p| p.plan_version),
        plan_changed,
        plan,
    })
}

fn environment(app: &AppHandle, recipe: &Recipe) -> Result<Value, String> {
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
    // 各检测相机的工位标定（缺省为设备标定 device/{device}）
    for inspect in &recipe.inspect {
        values.push(calibration_state(&vision::camera_calib_path(app, inspect)?)?);
    }
    values.push(json!([crate::site::image_measurement(app)]));
    values.push(json!(app.state::<vision::VisionHost>().engine().map(|engine| engine_state(&engine))));
    Ok(state_value(&values))
}

/// 相机标签：设备参数；样本集相机另含样本集编号，换同名相机的另一套样本集也算设备变化。
fn camera_state(config: &crate::camera::CameraConfig, sample_set: Option<&str>) -> Value {
    let mut value = state_value(config);
    if config.source == CameraSource::SampleSet {
        if let Some(object) = value.as_object_mut() {
            object.insert("sampleSet".into(), json!(sample_set));
        }
    }
    value
}

#[tauri::command]
pub fn workspace_list(host: State<'_, WorkspaceHost>) -> Vec<Workspace> {
    let mut out: Vec<_> = host.items.lock().unwrap().values().cloned().collect();
    out.sort_by(|a, b| a.doc.id.cmp(&b.doc.id));
    out
}

fn unloaded_hint(dir: &Path) -> String {
    format!("旧格式不兼容或文件损坏，文件已保留。请先移走候选目录 {}，再从当前生产版本重新建立候选；该目录含已发布归档，移走后需重新验证和发布才能生产", dir.display())
}

fn ensure_workspace_can_be_created(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法确认旧候选是否存在，拒绝覆盖：{error}")),
        Ok(_) => Err(format!("磁盘已有未加载的候选：{}", unloaded_hint(path.parent().unwrap_or(path)))),
    }
}

#[tauri::command]
pub fn workspace_get(app: AppHandle, id: String) -> Result<WorkspaceView, String> {
    safe_id(&id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    if !items.contains_key(&id) {
        ensure_workspace_can_be_created(&host.dir(&id).join("workspace.json"))?;
        let cycle = app.state::<CycleHost>();
        let mut doc = cycle.recipes.doc(&id).ok_or("配方不存在")?;
        let base = cycle.recipe(&id).map(|r| r.revision_id.clone());
        doc.version = cycle.recipes.next_version(&id)?;
        if base.is_some() { doc.version -= 1; }
        let mut w = Workspace::new(doc, base);
        w.ensure_instance(&app.state::<Store>())?;
        host.save(&w)?;
        items.insert(id.clone(), w);
    }
    view(&app, items[&id].clone())
}

#[tauri::command]
pub fn workspace_create(app: AppHandle, mut doc: RecipeDoc) -> Result<WorkspaceView, String> {
    safe_id(&doc.id)?;
    new_candidate_doc(&mut doc);
    doc.build()?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let production = app.state::<CycleHost>().recipes.list();
    check_new_identity(&doc, items.values().map(|w| (w.doc.id.as_str(), w.doc.product_code))
        .chain(production.iter().map(|r| (r.id.as_str(), r.product_code))))?;
    // Windows 路径不区分大小写，未登记的旧目录也不能被新候选覆盖。
    if host.dir(&doc.id).exists() { return Err("配方目录已经存在，请换一个编号".into()); }
    doc.version = app.state::<CycleHost>().recipes.next_version(&doc.id)?.max(doc.version.max(1));
    let mut w = Workspace::new(doc, None);
    w.ensure_instance(&app.state::<Store>())?;
    host.save(&w)?;
    items.insert(w.doc.id.clone(), w.clone());
    view(&app, w)
}

/// 新建候选的配方起点：不带相机参数，段配方的其余准备见 `burst::prepare_new_doc`。
fn new_candidate_doc(doc: &mut RecipeDoc) {
    doc.cameras.clear();
    burst::prepare_new_doc(doc);
}

fn check_new_identity<'a>(doc: &RecipeDoc, existing: impl Iterator<Item = (&'a str, u16)>) -> Result<(), String> {
    if !(1..=i16::MAX as u16).contains(&doc.product_code) { return Err("产品代码需在 1–32767 之间".into()); }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagingSaved {
    pub view: WorkspaceView,
    /// 已写进候选，但没能下发到相机的原因，按相机编号
    pub apply_errors: BTreeMap<String, String>,
}

/// 采集页调曝光与增益：几台相机一起写进候选，再在设备空闲时分别下发到各台相机，方便边看预览边调。
#[tauri::command]
pub async fn workspace_set_camera_imaging(app: AppHandle, id: String, revision: u64, imaging: BTreeMap<String, CameraImaging>) -> Result<ImagingSaved, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if imaging.is_empty() { return Err("没有要应用的相机参数".into()); }
        let saved = {
            let host = app.state::<WorkspaceHost>();
            let mut items = host.items.lock().unwrap();
            let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
            w.expect(revision)?;
            for (camera, imaging) in &imaging { set_imaging(&mut w, camera, *imaging)?; }
            host.save(&w)?;
            items.insert(id, w.clone());
            w
        };
        let apply_errors = imaging.iter().filter_map(|(camera, imaging)| apply_live_imaging(&app, camera, imaging).err().map(|e| (camera.clone(), e))).collect();
        Ok(ImagingSaved { view: view(&app, saved)?, apply_errors })
    }).await.map_err(|e| e.to_string())?
}

fn apply_live_imaging(app: &AppHandle, camera: &str, imaging: &CameraImaging) -> Result<(), String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在布防或处理其他配置操作，稍后再点应用下发到相机")?;
    crate::camera::check_idle(&cycle)?;
    let cam = cycle.camera.require(camera)?;
    cycle.camera.slot(cam as usize).ok_or("相机不存在")?.set_imaging(imaging)
}

fn update_doc(w: &mut Workspace, mut doc: RecipeDoc) -> Result<(), String> {
    // N_exp、标定统计、整件标定与 x_ref 只能由多轮标定写入，段序列或类型变化时清空
    burst::guard_calibration(&w.doc, &mut doc)?;
    let mut unbound = w.unbound_imaging.clone();
    sync_doc_imaging(&mut doc, &mut unbound);
    doc.build()?;
    if doc == w.doc { return Ok(()); }
    w.unbound_imaging = unbound;
    w.doc = doc;
    // 不再是计划相机的示教帧不保留
    let cameras = w.doc.plan_cameras();
    w.camera_teaching.retain(|t| cameras.contains(&t.camera));
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
    let image = crate::image_file::load(&image_file(&host, &id, &image_id)?)?;
    Ok(preview_response(&image))
}

impl SampleResult {
    /// 按候选判定结果对照人工期望：期望合格时 OK 与 OK_WITH_EXCURSION 都算符合，其余结论须完全一致。
    pub fn judged(sample: &Sample, name: String, sn: String, judgement: judge::Judgement) -> Self {
        let qualified = |v| matches!(v, Verdict::Ok | Verdict::OkWithExcursion);
        let actual = judgement.verdict;
        let passed = if qualified(sample.expected) { qualified(actual) } else { sample.expected == actual };
        Self { history_id: sample.history_id, sample_id: sample.sample_id.clone(), name, sn, expected: sample.expected,
            actual: Some(actual), passed, reason: judgement.reason }
    }
}

fn validation(app: &AppHandle, w: &Workspace) -> Result<Validation, String> {
    burst_release::validation(app, w)
}

#[tauri::command]
pub async fn workspace_validate(
    app: AppHandle,
    id: String,
    revision: u64,
    samples: Vec<Sample>,
    source_confirmed: Option<bool>,
) -> Result<WorkspaceView, String> {
    if source_confirmed != Some(true) { return Err("请确认验证样本与示教的轨迹、触发顺序及拍摄条件一致".into()); }
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
    let current_base = app.state::<CycleHost>().recipe(&id).map(|r| r.revision_id.clone());
    if current_base != w.base_revision || environment_tag != validated.environment_tag {
        return Err("验证后相机、标定或生产版本已变化，请重新验证".into());
    }
    let cams = cycle::usable_cams(&app, &recipe)?;
    app.state::<CycleHost>().camera.check_ready_at(&cams)?;
    let mut release_doc = w.doc.clone();
    release_doc.stage = RecipeStage::Formal;
    release_doc.teaching_id = Some(vision::unique_run_id(&format!("teaching-{}-v{}", release_doc.id, release_doc.version)));
    release_doc.plan_version = app.state::<CycleHost>().recipes.plan_version_for(&release_doc)?;
    let bundle = burst_release::freeze_bundle(&app, &host, &release_doc, &w.camera_teaching)?;
    let mut next = w.clone();
    next.doc = release_doc.clone();
    next.pending = Some(Box::new(Release {
        doc: release_doc,
        bundle_id: bundle.id,
        base_revision: w.base_revision.clone(),
        revision,
        validation: validated.clone(),
        validation_site: bundle.manifest.validation_site,
        _frames: Discarded,
        _overview: Discarded,
    }));
    next.publish = PublishState::Waiting;
    host.save(&next)?;
    *w = next;
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w.clone())
}

fn commit(app: &AppHandle, host: &WorkspaceHost, release: &Release) -> Result<Arc<Recipe>, String> {
    let cycle = app.state::<CycleHost>();
    let current = cycle.recipe(&release.doc.id);
    let recipe = release.doc.build()?;
    if !recipe.is_collect() {
        let bundle = load_bundle(app, &recipe.id, &release.bundle_id)?;
        if bundle.recipe != release.doc {
            return Err("待生效配方与不可变发布包不一致".into());
        }
    }
    if current.as_ref().is_some_and(|saved| saved.revision_id == recipe.revision_id) {
        return cycle.recipes.save_published(release.doc.clone(), release.base_revision.as_deref());
    }
    if current.as_ref().map(|r| r.revision_id.clone()) != release.base_revision {
        return Err("待生效版本与当前生产配方冲突".into());
    }
    let archive = host.dir(&recipe.id).join("published");
    std::fs::create_dir_all(&archive).map_err(|e| e.to_string())?;
    crate::fsio::write_atomic(
        &archive.join(format!("{}.json", recipe.revision_id)),
        &serde_json::to_string(release).map_err(|e| e.to_string())?,
    )?;
    cycle.recipes.save_published(release.doc.clone(), release.base_revision.as_deref())
}

pub fn releases_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::storage::project(app).join("vision").join("releases"))
}

fn read_release(path: &Path) -> Result<Release, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    { use std::os::windows::fs::MetadataExt;
      if metadata.file_attributes() & 0x400 != 0 { return Err("发布归档不能是 reparse point".into()); } }
    if !metadata.is_file() || metadata.file_type().is_symlink() { return Err("发布归档不是普通文件".into()); }
    serde_json::from_str(&crate::fsio::read_text(path).map_err(|e| e.to_string())?).map_err(|e| format!("发布归档格式不兼容：{e}"))
}

fn archived_release(dir: &Path, id: &str, version: u32) -> Result<Release, String> {
    safe_id(id)?;
    comparisons::checked_directory(dir, false)?;
    let path = dir.join(format!("{id}-v{version}.json"));
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("所选生产版本没有不可变发布包，请在工作台验证并发布".into()),
        Err(e) => return Err(e.to_string()),
    }
    let release = read_release(&path)?;
    if release.doc.id != id || release.doc.version != version { return Err("发布归档的配方或版本不一致".into()); }
    Ok(release)
}

fn load_bundle(app: &AppHandle, recipe_id: &str, bundle_id: &str) -> Result<crate::release::ReleaseBundle, String> {
    crate::release::load(&releases_root(app)?, recipe_id, bundle_id)
}

pub fn published_bundle(app: &AppHandle, recipe: &Recipe) -> Result<crate::release::ReleaseBundle, String> {
    let dir = app.state::<WorkspaceHost>().dir(&recipe.id).join("published");
    let published = archived_release(&dir, &recipe.id, recipe.version)?;
    let bundle = load_bundle(app, &recipe.id, &published.bundle_id)?;
    if bundle.recipe != published.doc || bundle.manifest.recipe_revision != recipe.revision_id {
        return Err("生产快照、配方与发布包身份不一致".into());
    }
    Ok(bundle)
}

/// 只核对生产版本对应发布包的清单与身份，不读取资源内容。
pub fn published_manifest(app: &AppHandle, recipe: &Recipe) -> Result<crate::release::ReleaseManifest, String> {
    let dir = app.state::<WorkspaceHost>().dir(&recipe.id).join("published");
    let published = archived_release(&dir, &recipe.id, recipe.version)?;
    let manifest = crate::release::peek(&releases_root(app)?, &recipe.id, &published.bundle_id)?;
    if manifest.recipe_revision != recipe.revision_id || manifest.recipe_version != recipe.version {
        return Err("生产快照、配方与发布包身份不一致".into());
    }
    Ok(manifest)
}

pub struct DraftSummary {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub product_code: u16,
}

pub fn draft_summaries(app: &AppHandle) -> Vec<DraftSummary> {
    let Some(host) = app.try_state::<WorkspaceHost>() else { return Vec::new() };
    let items = host.items.lock().unwrap();
    items
        .values()
        .map(|w| DraftSummary {
            id: w.doc.id.clone(),
            name: w.doc.name.clone(),
            version: w.doc.version,
            product_code: w.doc.product_code,
        })
        .collect()
}

pub fn apply_pending(app: &AppHandle) -> bool {
    let Some(host) = app.try_state::<WorkspaceHost>() else {
        return false;
    };
    let mut items = host.items.lock().unwrap();
    let ids: Vec<_> = items
        .iter()
        .filter(|(_, w)| w.pending.is_some() && w.publish == PublishState::Waiting)
        .map(|(id, _)| id.clone())
        .collect();
    let mut changed = false;
    for id in ids {
        let w = items.get_mut(&id).unwrap();
        let before = w.clone();
        let release = *w.pending.take().unwrap();
        match commit(app, &host, &release) {
            Ok(saved) => {
                w.base_revision = Some(saved.revision_id.clone());
                if w.revision == release.revision {
                    w.doc = app.state::<CycleHost>().recipes.doc(&id).unwrap();
                }
                let next_version = app.state::<CycleHost>().recipes.next_version(&saved.id);
                w.doc.version = next_version.as_ref().copied().unwrap_or(saved.version);
                w.validation = None;
                w.revision += 1;
                w.publish = PublishState::Active { version: saved.version, revision: w.revision, warning: next_version.err(), validation_site: release.validation_site };
                changed = true;
                let message = if saved.is_collect() {
                    format!(
                        "{id} v{} 采集版已生效；PLC 型号 {}，仅维护模式可用（只采图不判定）",
                        saved.version, saved.product_code
                    )
                } else {
                    let total = saved
                        .expected_total()
                        .map_or("未标定".to_string(), |n| n.to_string());
                    format!(
                        "{id} v{} 已生效；PLC 型号 {}，ΣN_exp = {total}",
                        saved.version, saved.product_code
                    )
                };
                cycle::log(app, "info", "发布配方", message);
            }
            Err(e) => {
                w.pending = Some(Box::new(release));
                w.publish = PublishState::Failed { message: e.clone() };
                cycle::log(app, "err", "发布配方", e);
            }
        }
        if let Err(error) = host.save(w) {
            w.retain_failed_activation(before, &error);
            cycle::log(app, "err", "保存发布状态", error);
        }
        let _ = app.emit("workspace://changed", &id);
    }
    changed
}

pub fn activation_uncertain(app: &AppHandle) -> bool {
    app.try_state::<WorkspaceHost>().is_some_and(|host| host.items.lock().unwrap().values()
        .any(Workspace::activation_uncertain))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub id: String,
    pub history_id: i64,
    pub source: String,
    pub cycle_id: String,
    pub bundle_id: Option<String>,
    pub candidate_id: String,
    pub candidate_revision: u64,
    pub candidate_recipe: Recipe,
    pub original_verdict: Verdict,
    pub judgement: judge::Judgement,
    pub measurements: Vec<Value>,
    pub created_at: i64,
    /// 段配方记录的对照（D3）：重判所用的段、计划相机、ΣN_exp 与区域判定；旧拍照点记录为空（之前保存的对照没有这一项）。
    /// 有值时 judgement 的 verdict / plcCode / faultCode / reason 与它相同，segments、gaps 为空
    #[serde(default)]
    pub burst: Option<burst_history::ComparisonBurst>,
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
            .recipe_revision
            .as_deref()
            .map(|h| store.recipe_snapshot(h))
            .transpose()?
            .flatten()
            .ok_or("历史配方快照缺失")?;
        let (judgement, burst) =
            burst_history::compare(&app, &w, &recipe, &detail, &original, raw)?;
        let comparison = Comparison {
            id: comparison_id(&app, history_id),
            history_id,
            source: if raw { "raw".into() } else { "rules".into() },
            cycle_id: detail
                .summary
                .cycle_id
                .clone()
                .ok_or("历史记录没有工件身份")?,
            bundle_id: detail.summary.bundle_id.clone(),
            candidate_id: id.clone(),
            candidate_revision: revision,
            candidate_recipe: recipe,
            original_verdict: detail.summary.verdict,
            judgement,
            measurements: Vec::new(),
            created_at: ly_plc::now_ms(),
            burst: Some(burst),
        };
        save_comparison(&app, &comparison)?;
        Ok(comparison)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn comparison_id(app: &AppHandle, history_id: i64) -> String {
    let seq = app.state::<WorkspaceHost>().capture_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{history_id}-{}-{seq}", ly_plc::now_ms())
}

fn save_comparison(app: &AppHandle, comparison: &Comparison) -> Result<(), String> {
    let host = app.state::<WorkspaceHost>();
    host.comparisons.save(&host.root, &app.state::<Store>(), comparison)
}

pub(crate) fn purge_history(app: &AppHandle, before: i64) -> Result<(usize, Vec<String>), String> {
    let host = app.state::<WorkspaceHost>();
    host.comparisons.purge(&host.root, &app.state::<Store>(), before)
}

#[tauri::command]
pub async fn workspace_compare_original(app: AppHandle, history_id: i64) -> Result<Comparison, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let detail = store.detail(history_id)?;
        if !detail.recording.available { return Err("原图未完整保留，不能重现原发布测量；请查看录制状态".into()); }
        let cycle_id = detail.summary.cycle_id.clone().ok_or("历史记录缺少 cycleId")?;
        let bundle_id = detail.summary.bundle_id.as_deref().ok_or("本件没有不可变发布包，不能按原版本重现")?;
        let recipe_revision = detail.summary.recipe_revision.as_deref().ok_or("历史记录缺少配方快照身份")?;
        let original = store.recipe_snapshot(recipe_revision)?.ok_or("原始配方快照缺失；不能使用当前配方替代")?;
        let bundle = load_bundle(&app, &original.id, bundle_id)?;
        let engine = app.state::<vision::VisionHost>().engine().ok_or("原版本兼容引擎未加载")?;
        let prepared = crate::production::Prepared::load(bundle, engine, &original)?;
        prepared.verify()?;
        let (judgement, burst) =
            burst_history::reproduce_original(&app, &detail, &prepared, &cycle_id)?;
        let comparison = Comparison {
            id: comparison_id(&app, history_id),
            history_id,
            source: "original".into(),
            cycle_id,
            bundle_id: Some(bundle_id.into()),
            candidate_id: original.id.clone(),
            candidate_revision: 0,
            candidate_recipe: original,
            original_verdict: detail.summary.verdict,
            judgement,
            measurements: Vec::new(),
            created_at: ly_plc::now_ms(),
            burst: Some(burst),
        };
        save_comparison(&app, &comparison)?;
        Ok(comparison)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn workspace_comparisons(
    host: State<'_, WorkspaceHost>,
    store: State<'_, Store>,
    id: String,
    history_id: i64,
) -> Result<Vec<Comparison>, String> {
    safe_id(&id)?;
    host.comparisons.list(&host.root, &store, &id, history_id)
}

pub fn clear_live(app: &AppHandle) {
    if let Some(host) = app.try_state::<WorkspaceHost>() {
        *host.live_burst.lock().unwrap() = LiveFrames::default();
    }
}

/// 段配方在途件一帧的原图（计划相机 camera 的第 seq 帧），供 `workspace_live_frame_image` 读。
pub fn retain_live_frame(app: &AppHandle, cycle_id: &str, revision_id: &str, camera: usize, seq: u64, image: &Option<Arc<FrameImage>>) {
    if let (Some(host), Some(image)) = (app.try_state::<WorkspaceHost>(), image) {
        host.live_burst.lock().unwrap().insert(cycle_id, revision_id, (camera, seq), image.clone());
    }
}

/// 段配方在途件某台相机第 seq 帧的实时原图（决策记录 5.4，camera 为计划相机下标）；未到、缺帧或已超出预览缓存时报错。
#[tauri::command]
pub fn workspace_live_frame_image(
    host: State<'_, WorkspaceHost>,
    cycle_id: String,
    revision_id: String,
    camera: usize,
    seq: u64,
) -> Result<tauri::ipc::Response, String> {
    let image = {
        let live = host.live_burst.lock().unwrap();
        if live.key.as_ref().is_none_or(|key| key.0 != cycle_id || key.1 != revision_id) {
            return Err("本件尚未收到整帧图像".into());
        }
        live.frames
            .iter()
            .find(|(key, _)| *key == (camera, seq))
            .map(|(_, image)| image.clone())
            .ok_or("这一帧原图未到达（缺帧）或已超出预览缓存；请开启图像测量或帧录制")?
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
    if meta.camera_tag != state_value(&config) {
        return Err("相机参数变化，请重新冻结标定样本".into());
    }
    Ok(image.clone())
}

pub fn station_image_view(app: &AppHandle, cam: u8, image_id: &str) -> Result<u8, String> {
    let host = app.state::<WorkspaceHost>();
    let samples = host.station.lock().unwrap();
    samples.get(&cam).filter(|(meta, _)| meta.id == image_id).map(|(meta, _)| meta.view).ok_or_else(|| "标定样本已更新".into())
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
pub async fn workspace_station_capture(app: AppHandle, cam: u8, view: Option<u8>) -> Result<FrozenImage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() {
            return Err("工件正在检测，结束后再取标定样本".into());
        }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        if !(1..=config.view_count).contains(&view.unwrap_or(1)) { return Err("图编号超出设备范围".into()); }
        let selected = view.unwrap_or(1);
        let image = cycle.camera.capture_view(cam, selected, None)?;
        if cycle.busy() || state_value(&slot.config()) != state_value(&config) {
            return Err("取样期间设备或节拍状态变化，样本已丢弃".into());
        }
        let host = app.state::<WorkspaceHost>();
        let seq = host
            .capture_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let meta = FrozenImage {
            view: view.unwrap_or(1),
            id: format!("station-{cam}-{}-{seq}", ly_plc::now_ms()),
            source: format!("{:?}", config.source),
            captured_at: ly_plc::now_ms(),
            size: [image.width, image.height],
            camera: config.id,
            camera_tag: state_value(&slot.config()),
            calib_tag: Value::Null,
            geometry_tag: Value::Null,
            exposure_us: None,
            gain_db: None,
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
pub async fn workspace_station_import(app: AppHandle, cam: u8, bytes: Vec<u8>, view: Option<u8>) -> Result<FrozenImage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() { return Err("工件正在检测，结束后再导入标定原图".into()); }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        if !(1..=config.view_count).contains(&view.unwrap_or(1)) { return Err("图编号超出设备范围".into()); }
        let image = Arc::new(decode_imported_image(bytes)?);
        if cycle.busy() || state_value(&slot.config()) != state_value(&config) {
            return Err("导入期间设备或节拍状态变化，原图已丢弃".into());
        }
        let host = app.state::<WorkspaceHost>();
        let seq = host.capture_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let meta = FrozenImage {
            view: view.unwrap_or(1),
            id: format!("station-import-{cam}-{}-{seq}", ly_plc::now_ms()),
            source: "import".into(), captured_at: ly_plc::now_ms(), size: [image.width, image.height],
            camera: config.id.clone(), camera_tag: state_value(&config), calib_tag: Value::Null, geometry_tag: Value::Null,
            exposure_us: None, gain_db: None, history_id: None,
        };
        host.station.lock().unwrap().insert(cam, (meta.clone(), image));
        Ok(meta)
    }).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct ImportTestDir(pub(super) PathBuf);
    impl ImportTestDir {
        pub(super) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("tujiao-sample-import-{}-{sequence}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).unwrap(); Self(path)
        }
    }
    impl Drop for ImportTestDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

}

/// 候选试运行冻结的配方：过 `Recipe::ready_burst`（三台相机走廊与当量、整件 ΣN_exp）。修订号为候选调试修订，计划版本至少为 1。
fn debug_recipe(w: &Workspace) -> Result<Recipe, String> {
    let mut recipe = w.doc.build()?;
    recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
    recipe.revision_id = w.debug_revision()?;
    if recipe.plan_version == 0 { recipe.plan_version = 1; }
    Ok(recipe)
}

#[tauri::command]
pub async fn workspace_debug_run(app: AppHandle, id: String, revision: u64, part_id: Option<String>) -> Result<(), String> {
    if !crate::offline::enabled(&app) || crate::offline::sample_set(&app).is_none() { return Err("候选试运行仅支持离线样本集模式".into()); }
    let set = crate::offline::sample_set(&app).ok_or("离线样本集缺失")?;
    let current = burst_release::current_camera_tags(&app);
    let (recipe, sizes) = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        let w = items.get(&id).ok_or("候选配方不存在")?;
        if w.revision != revision { return Err("候选已修改，请刷新后重试".into()); }
        if w.pending.is_some() { return Err("候选正在等待发布，请先完成或取消发布".into()); }
        let recipe = debug_recipe(w)?;
        // 每台计划相机按示教帧尺寸建走廊测量图，结果只量不判（D3 的候选调试记录按样本集读原图）
        let sizes = burst_release::debug_sizes(w, &current)?;
        (recipe, sizes)
    };
    let engine = app
        .state::<vision::VisionHost>()
        .engine()
        .ok_or("lyFlow 核心库未加载")?;
    let candidate = crate::candidate::Candidate::new_burst(recipe, revision, engine, &sizes)?;
    crate::offline::run_candidate(app, Arc::new(candidate), set, part_id).await
}

pub(crate) fn recheck_debug_candidate(app: &AppHandle, candidate: &crate::candidate::Candidate) -> Result<(), String> {
    let current = burst_release::current_camera_tags(app);
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    let w = items.get(&candidate.info.recipe_id).ok_or("候选已删除")?;
    if w.revision != candidate.info.revision || w.pending.is_some() || w.debug_revision()? != candidate.recipe.revision_id { return Err("候选已修改、重建或正在发布，请重新试运行".into()); }
    // 修订号不变即示教帧与走廊没变，只需再核对示教后设备标签没变
    burst_release::debug_sizes(w, &current)?;
    Ok(())
}

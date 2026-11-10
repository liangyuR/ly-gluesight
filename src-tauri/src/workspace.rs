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
use crate::recipe::{Recipe, RecipeDoc, RecipeStage};
use crate::release::ValidationSite;
use crate::store::Store;
use crate::vision;

pub(crate) mod agent;
mod burst;
mod burst_history;
mod burst_release;
#[cfg(test)]
mod burst_release_tests;
mod burst_skip;
#[cfg(test)]
mod burst_tests;
mod capture_review;
pub(crate) mod centerline;
mod collect_release;
mod comparisons;
mod history_import;
mod navigation;
pub(crate) mod package;
mod pass_align;
#[cfg(test)]
mod pass_align_tests;
mod pieces;
#[cfg(test)]
mod pieces_tests;
pub mod progress;
mod recorded;
mod reproduce;
#[cfg(test)]
mod segment_corridor_tests;
mod segment_remap;
mod snap;
#[cfg(test)]
mod teaching_transaction_tests;
mod workflow;
pub use burst::*;
pub use burst_history::*;
pub use burst_release::*;
pub use burst_skip::*;
pub use capture_review::*;
pub use collect_release::*;
pub use history_import::*;
pub use navigation::*;
pub use pass_align::*;
pub use pieces::*;
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
        Ok(text) => {
            serde_json::from_str::<Value>(&text).map_err(|e| format!("标定文件格式无效：{e}"))?
        }
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
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    /// 只提醒、不拦发布：没满足时照样能发布（如有段在示教帧上没测成，D-SB-59）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advisory: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    pub revision: u64,
    pub passed: bool,
    pub checked_at: i64,
    pub checks: Vec<Check>,
    pub environment_tag: Value,
}

/// 已停用的键：读入时丢弃，不再写出（`#[serde(default, rename = "…", skip_serializing)]`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    pub revision: u64,
    pub validation: Validation,
    pub validation_site: ValidationSite,
    #[serde(
        default,
        deserialize_with = "navigation::deserialize_archive_status",
        skip_serializing_if = "Option::is_none"
    )]
    pub navigation: Option<NavigationArchiveStatus>,
    #[serde(default, rename = "frames", skip_serializing)]
    pub _frames: Discarded,
    #[serde(default, rename = "overview", skip_serializing)]
    pub _overview: Discarded,
    #[serde(default, rename = "baseRevision", skip_serializing)]
    pub _base_revision: Discarded,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    pub doc: RecipeDoc,
    #[serde(default)]
    pub instance_id: u64,
    pub revision: u64,
    pub pending: Option<Box<Release>>,
    pub publish: PublishState,
    pub updated_at: i64,
    #[serde(default)]
    pub capture_id: Option<String>,
    /// 段配方：每台计划相机的走廊示教帧
    #[serde(default)]
    pub camera_teaching: Vec<burst::CameraTeaching>,
    /// 段配方：示教件的测量
    #[serde(default)]
    pub calibration: burst::Calibration,
    #[serde(default)]
    pub capture_review: capture_review::CaptureReview,
    /// 候选的建法（RB-7）：`pieces::FLOW` 为示教件流程，缺省为旧流程候选（RB-5 停用）
    #[serde(default)]
    pub flow: u32,
    /// 示教件（RB-1）：分段边界与全部走廊示教帧的唯一来源
    #[serde(default)]
    pub teaching_piece: Option<pieces::TeachingPiece>,
    #[serde(default)]
    pub navigation_id: Option<String>,
    #[serde(default)]
    pub navigation_structure_revision: u64,
    /// 换示教件的逐笔对照（RB-11）：发布时清空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass_alignment: Option<pass_align::PassAlignment>,
    #[serde(default, rename = "frames", skip_serializing)]
    pub _frames: Discarded,
    #[serde(default, rename = "overview", skip_serializing)]
    pub _overview: Discarded,
    #[serde(default, rename = "lastPosition", skip_serializing)]
    pub _last_position: Discarded,
    #[serde(default, rename = "unboundImaging", skip_serializing)]
    pub _unbound_imaging: Discarded,
    #[serde(default, rename = "baseRevision", skip_serializing)]
    pub _base_revision: Discarded,
    #[serde(default, rename = "samples", skip_serializing)]
    pub _samples: Discarded,
    #[serde(default, rename = "sampleBank", skip_serializing)]
    pub _sample_bank: Discarded,
    #[serde(default, rename = "validation", skip_serializing)]
    pub _validation: Discarded,
}

impl Workspace {
    fn new(mut doc: RecipeDoc) -> Self {
        doc.teaching_piece = None;
        Self {
            doc,
            instance_id: 0,
            revision: 1,
            pending: None,
            publish: PublishState::Draft,
            updated_at: ly_plc::now_ms(),
            capture_id: None,
            camera_teaching: Vec::new(),
            calibration: burst::Calibration::default(),
            capture_review: capture_review::CaptureReview::default(),
            flow: pieces::FLOW,
            teaching_piece: None,
            navigation_id: None,
            navigation_structure_revision: 0,
            pass_alignment: None,
            _frames: Discarded,
            _overview: Discarded,
            _last_position: Discarded,
            _unbound_imaging: Discarded,
            _base_revision: Discarded,
            _samples: Discarded,
            _sample_bank: Discarded,
            _validation: Discarded,
        }
    }

    fn ensure_instance(&mut self, store: &Store) -> Result<(), String> {
        self.instance_id = store.workspace_instance(&self.doc.id, self.instance_id)?;
        Ok(())
    }

    /// 旧流程候选（RB-5）：只能在配方库删除。
    pub fn legacy(&self) -> bool {
        self.flow != pieces::FLOW
    }

    fn expect(&self, revision: u64) -> Result<(), String> {
        if self.legacy() {
            return Err(pieces::LEGACY.into());
        }
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
        burst::calibrate_from_teaching(self);
        self.revision += 1;
        self.updated_at = ly_plc::now_ms();
    }

    fn retry_publish(&mut self, revision: u64) -> Result<(), String> {
        if self.revision != revision
            || self.pending.is_none()
            || !matches!(
                self.publish,
                PublishState::Failed { .. } | PublishState::Uncertain { .. }
            )
        {
            return Err("没有可以重试的发布版本".into());
        }
        self.publish = PublishState::Waiting;
        Ok(())
    }

    /// 生效失败时放弃冻结的待生效版本，候选回到草稿；生产版本不受影响。状态不确定时须先核实，不能放弃。
    fn abandon_publish(&mut self, revision: u64) -> Result<(), String> {
        if self.revision != revision
            || self.pending.is_none()
            || !matches!(self.publish, PublishState::Failed { .. })
        {
            return Err(
                "只有生效失败的待生效版本可以放弃；生效状态不确定时请先核实发布状态".into(),
            );
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
        self.publish = PublishState::Uncertain {
            message: format!("生产生效状态未能保存：{error}，请检查数据目录"),
        };
    }
}

/// 候选的发布与生效状态。Waiting / Failed / Uncertain 时 pending 保存冻结的待生效版本。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PublishState {
    #[default]
    Draft,
    Waiting,
    Failed {
        message: String,
    },
    Uncertain {
        message: String,
    },
    /// 已生效；revision 是生效后候选的修订号，候选再改动后回到草稿。
    Active {
        version: u32,
        revision: u64,
        warning: Option<String>,
        validation_site: ValidationSite,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    pub workspace: Workspace,
    pub layout: Recipe,
    pub production_version: Option<u32>,
    /// 生产配方是采集版还是正式版；未发布为空
    pub production_stage: Option<RecipeStage>,
    /// 段配方的计划部分（决策记录 5.1）
    pub plan: burst::WorkspacePlanView,
    pub navigation: NavigationView,
}

pub struct WorkspaceHost {
    root: PathBuf,
    items: Mutex<HashMap<String, Workspace>>,
    pub notes: Vec<String>,
    /// 段配方在途件的实时原图，按 (计划相机下标, seq)
    live_burst: Mutex<LiveFrames>,
    /// 段配方在途件各帧测到的叠图，按 (计划相机下标, seq)
    live_views: Mutex<LiveViews>,
    station: Mutex<HashMap<u8, (FrozenImage, Arc<FrameImage>)>>,
    capture_seq: std::sync::atomic::AtomicU64,
    comparisons: comparisons::Comparisons,
    /// 历史叠图最近用的发布包及其项目（不在生产预热里时另加载，只留一个，不占生产预热）
    history_bundle: Mutex<Option<(String, Arc<crate::production::Prepared>)>>,
}

/// 在途件的原图：按工件与配方修订成组，换件即清空。内存里只留最近的帧（总量超过上限时丢最早的），
/// 更早的帧按本件的录制原图读回。彩色帧留的是原始马赛克，读图时才去马赛克。键为 (计划相机下标, seq)。
#[derive(Default)]
struct LiveFrames {
    key: Option<(String, String)>,
    frames: VecDeque<((usize, u64), crate::frame::FrameView)>,
    bytes: usize,
    recorded: HashMap<(usize, u64), crate::recorder::PendingFrame>,
}

/// 在途件各帧测到的叠图：按工件与配方修订成组，换件即清空；同一帧重测（收尾补测）时以后测的为准。
#[derive(Default)]
struct LiveViews {
    key: Option<(String, String)>,
    views: HashMap<(usize, u64), CalibrationFrameView>,
}

impl LiveFrames {
    fn insert(
        &mut self,
        cycle_id: &str,
        revision_id: &str,
        k: (usize, u64),
        image: Option<crate::frame::FrameView>,
        recorded: Option<crate::recorder::PendingFrame>,
    ) {
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
        if let Some(recorded) = recorded {
            self.recorded.entry(k).or_insert(recorded);
        }
        let Some(image) = image else {
            return;
        };
        if self.frames.iter().any(|(index, _)| *index == k) {
            return;
        }
        const LIMIT: usize = 128 * 1024 * 1024;
        let bytes = image.held_bytes();
        if bytes > LIMIT {
            return;
        }
        while self.bytes + bytes > LIMIT {
            let Some((_, old)) = self.frames.pop_front() else {
                break;
            };
            self.bytes -= old.held_bytes();
        }
        self.bytes += bytes;
        self.frames.push_back((k, image));
    }
}

impl WorkspaceHost {
    pub(crate) fn comparison_snapshot(
        &self,
        id: &str,
        revision: u64,
        instance_id: Option<u64>,
    ) -> Result<Workspace, String> {
        let items = self.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        if instance_id.is_some_and(|instance| instance != w.instance_id) {
            return Err("候选已删除或重建，请重新读取后复测".into());
        }
        w.expect(revision)?;
        Ok(w.clone())
    }

    /// 候选与已提交的发布快照都保留相机引用，删除相机前核对。
    pub fn camera_users(&self, camera: &str) -> Vec<String> {
        let uses = |doc: &RecipeDoc| doc.plan_cameras().iter().any(|c| c == camera);
        self.items
            .lock()
            .unwrap()
            .values()
            .filter(|w| uses(&w.doc) || w.pending.as_ref().is_some_and(|p| uses(&p.doc)))
            .map(|w| w.doc.id.clone())
            .collect()
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
            if !file.is_file() {
                continue;
            }
            let loaded = crate::fsio::read_text(&file)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    serde_json::from_str::<Workspace>(&text).map_err(|e| e.to_string())
                })
                .and_then(|mut w| {
                    safe_id(&w.doc.id)?;
                    if w.doc.id != entry.file_name().to_string_lossy() {
                        return Err("候选编号与目录不一致".into());
                    }
                    w.doc.build()?;
                    let legacy = w.instance_id == 0;
                    let dropped = burst::drop_camera_teaching(&mut w);
                    w.ensure_instance(&app.state::<Store>())?;
                    if legacy || dropped {
                        crate::fsio::write_atomic(
                            &file,
                            &serde_json::to_string_pretty(&w).map_err(|e| e.to_string())?,
                        )?;
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
            live_views: Mutex::new(LiveViews::default()),
            station: Mutex::new(HashMap::new()),
            capture_seq: std::sync::atomic::AtomicU64::new(0),
            comparisons: comparisons::Comparisons::default(),
            history_bundle: Mutex::new(None),
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

fn view(app: &AppHandle, mut w: Workspace) -> Result<WorkspaceView, String> {
    // 旧候选的示教件还没记笔结构：显示时按轮次补上，分段页才能按笔分段（RB-12）；读不到就按没有笔
    let _ = pieces::fill_piece_passes(app.state::<CycleHost>().camera.recipe_capture(), &mut w);
    let layout = w.doc.build()?;
    let production_recipe = app.state::<CycleHost>().recipe(&w.doc.id);
    let production_version = production_recipe.as_ref().map(|r| r.version);
    let production_stage = production_recipe.as_ref().map(|r| r.stage);
    let plan = burst::plan_view(
        app,
        &w,
        &layout,
        production_recipe.as_deref(),
        production_version,
    );
    Ok(WorkspaceView {
        navigation: navigation::navigation_view(
            &crate::storage::project(app),
            &w,
            production_recipe.as_ref().map(|r| r.revision_id.as_str()),
        ),
        workspace: w,
        layout,
        production_version,
        production_stage,
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
    // 各检测相机的设备标定 device/{device}
    for inspect in &recipe.inspect {
        values.push(calibration_state(&vision::camera_calib_path(
            app, inspect,
        )?)?);
    }
    values.push(json!([true]));
    values.push(json!(app
        .state::<vision::VisionHost>()
        .engine()
        .map(|engine| engine_state(&engine))));
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
        Ok(_) => Err(format!(
            "磁盘已有未加载的候选：{}",
            unloaded_hint(path.parent().unwrap_or(path))
        )),
    }
}

/// 读盘与校验导航图放在阻塞线程，不占窗口主线程。
#[tauri::command]
pub async fn workspace_get(app: AppHandle, id: String) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || get(app, id))
        .await
        .map_err(|e| e.to_string())?
}

fn get(app: AppHandle, id: String) -> Result<WorkspaceView, String> {
    safe_id(&id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    if !items.contains_key(&id) {
        ensure_workspace_can_be_created(&host.dir(&id).join("workspace.json"))?;
        let cycle = app.state::<CycleHost>();
        let mut doc = cycle.recipes.doc(&id).ok_or("配方不存在")?;
        let production = cycle.recipe(&id).map(|r| r.revision_id.clone());
        doc.version = cycle.recipes.next_version(&id)?;
        let mut w = Workspace::new(doc);
        w.ensure_instance(&app.state::<Store>())?;
        if let Some(message) = navigation::inherit_navigation(
            &crate::storage::project(&app),
            &mut w,
            production.as_deref(),
        ) {
            cycle::log(&app, "warn", "导航资料", message);
        }
        host.save(&w)?;
        items.insert(id.clone(), w);
    }
    if items[&id].legacy() {
        return Err(pieces::LEGACY.into());
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
    check_new_identity(
        &doc,
        items
            .values()
            .map(|w| (w.doc.id.as_str(), w.doc.product_code))
            .chain(production.iter().map(|r| (r.id.as_str(), r.product_code))),
    )?;
    // Windows 路径不区分大小写，未登记的旧目录也不能被新候选覆盖。
    if host.dir(&doc.id).exists() {
        return Err("配方目录已经存在，请换一个编号".into());
    }
    doc.version = app
        .state::<CycleHost>()
        .recipes
        .next_version(&doc.id)?
        .max(doc.version.max(1));
    let mut w = Workspace::new(doc);
    w.ensure_instance(&app.state::<Store>())?;
    host.save(&w)?;
    items.insert(w.doc.id.clone(), w.clone());
    view(&app, w)
}

/// 新建候选的配方起点，见 `burst::prepare_new_doc`。
fn new_candidate_doc(doc: &mut RecipeDoc) {
    burst::prepare_new_doc(doc);
}

fn check_product_code(doc: &RecipeDoc) -> Result<(), String> {
    if !(1..=i16::MAX as u16).contains(&doc.product_code) {
        return Err("产品代码需在 1–32767 之间".into());
    }
    Ok(())
}

fn check_new_identity<'a>(
    doc: &RecipeDoc,
    existing: impl Iterator<Item = (&'a str, u16)>,
) -> Result<(), String> {
    check_product_code(doc)?;
    check_identity(&doc.id, doc.product_code, existing)
}

fn check_identity<'a>(
    recipe_id: &str,
    product_code: u16,
    existing: impl Iterator<Item = (&'a str, u16)>,
) -> Result<(), String> {
    for (id, code) in existing {
        if id.eq_ignore_ascii_case(recipe_id) {
            return Err("配方编号已经存在（不区分大小写）".into());
        }
        if code == product_code {
            return Err(format!("产品代码 {} 已被配方 {} 使用", code, id));
        }
    }
    Ok(())
}

/// 项目整包导入时整理候选：回到草稿，不带待生效版本。
pub(crate) fn reset_for_project_import(path: &Path) -> Result<(), String> {
    let text = crate::fsio::read_text(path).map_err(|e| e.to_string())?;
    let mut w: Workspace = serde_json::from_str(&text)
        .map_err(|e| format!("候选 {} 无法读取：{e}", path.display()))?;
    w.pending = None;
    w.publish = PublishState::Draft;
    w.changed();
    crate::fsio::write_atomic(
        path,
        &serde_json::to_string_pretty(&w).map_err(|e| e.to_string())?,
    )
}

#[tauri::command]
pub fn workspace_delete(app: AppHandle, id: String) -> Result<(), String> {
    safe_id(&id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    if items.get(&id).is_some_and(|w| w.pending.is_some()) {
        return Err("已有待生效版本，不能删除".into());
    }
    let rounds = || {
        app.state::<CycleHost>()
            .camera
            .recipe_capture()
            .delete_recipe_rounds(&id)
    };
    if items.get(&id).is_some_and(Workspace::legacy) {
        // 旧流程候选只删候选本身与它导入的轮次（RB-5）：生产配方与发布归档照常运行，之后可从生产配方新建新流程候选
        rounds()?;
        pieces::remove_legacy_dir(&host.dir(&id))?;
        items.remove(&id);
        return Ok(());
    }
    // 生产配方的删除检查（在用、在途事务、配置门）通过后才删轮次，被拒时示教与标定原图不受影响
    if app.state::<CycleHost>().recipe(&id).is_some() {
        crate::recipe_api::recipe_delete(app.clone(), app.state::<CycleHost>(), id.clone())?;
    }
    rounds()?;
    let dir = host.dir(&id);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    navigation::remove_navigation_archives(&crate::storage::project(&app), &id)?;
    items.remove(&id);
    Ok(())
}

/// 自动保存高频调用：写盘与生成视图放在阻塞线程，不占窗口主线程。
#[tauri::command]
pub async fn workspace_save_doc(
    app: AppHandle,
    id: String,
    revision: u64,
    doc: RecipeDoc,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || save_doc(app, id, revision, doc))
        .await
        .map_err(|e| e.to_string())?
}

fn save_doc(
    app: AppHandle,
    id: String,
    revision: u64,
    doc: RecipeDoc,
) -> Result<WorkspaceView, String> {
    let w = app.state::<WorkspaceHost>().save_doc(&id, revision, doc)?;
    view(&app, w)
}

impl WorkspaceHost {
    /// 整份保存候选配方（`workspace_save_doc`）。
    pub(crate) fn save_doc(
        &self,
        id: &str,
        revision: u64,
        mut doc: RecipeDoc,
    ) -> Result<Workspace, String> {
        if doc.id != id {
            return Err("编辑候选配置时不能改变配方编号；请在配方库复制".into());
        }
        let mut items = self.items.lock().unwrap();
        let mut w = items.get(id).ok_or("候选配置不存在")?.clone();
        w.expect(revision)?;
        burst::keep_color_measure(&w, &mut doc);
        update_doc(&mut w, doc)?;
        self.save(&w)?;
        items.insert(id.to_string(), w.clone());
        Ok(w)
    }
}

fn update_doc(w: &mut Workspace, doc: RecipeDoc) -> Result<(), String> {
    replace_doc(w, doc, None)
}

/// 保存候选配置。`remapped` 为分段保存时已按示教件帧范围迁好的段示教帧（走廊也已在 `doc` 里迁到新段号，见 `segment_remap`）：
/// 这时不按段的类型序列整体清空走廊。
fn replace_doc(
    w: &mut Workspace,
    mut doc: RecipeDoc,
    remapped: Option<Vec<burst::CameraTeaching>>,
) -> Result<(), String> {
    check_product_code(&doc)?;
    // 示教件来源只在发布时写入，候选里恒为空
    doc.teaching_piece = None;
    inherit_segment_ids(&w.doc.segments, &mut doc.segments);
    if doc.segments != w.doc.segments {
        pass_align::require_confirmed(w)?;
    }
    // N_exp、标定统计、整件标定与 x_ref 只能由示教件得出，段序列或类型变化时清空
    burst::guard_calibration(&w.doc, &mut doc);
    let teaching = remapped.as_deref().unwrap_or(&w.camera_teaching);
    let segments_changed =
        burst::guard_segment_corridors(&w.doc, &mut doc, teaching, remapped.is_some())?;
    doc.build()?;
    if doc == w.doc && remapped.is_none() {
        return Ok(());
    }
    if navigation::structure_changed(&w.doc, &doc) {
        w.navigation_structure_revision += 1;
    }
    w.doc = doc;
    if let Some(teaching) = remapped {
        w.camera_teaching = teaching;
    }
    // 不再是计划相机、段结构变了或这台相机不再检测所在段的示教帧不保留
    let cameras = w.doc.plan_cameras();
    let doc = &w.doc;
    w.camera_teaching.retain(|t| {
        cameras.contains(&t.camera) && burst::teaching_applies(doc, t, segments_changed)
    });
    w.changed();
    Ok(())
}

/// 候选已用段 ID（按笔换过示教件，RB-11）时，回传的段没带段 ID（前端新建的段）：段数与类型序列没变就按位置沿用原来的
/// 段 ID，其余补新 ID。没用过段 ID 的候选不加。
fn inherit_segment_ids(
    before: &[crate::plan::PathSegment],
    segments: &mut [crate::plan::PathSegment],
) {
    if before.iter().all(|s| s.id.is_none()) {
        return;
    }
    if before.len() == segments.len()
        && before
            .iter()
            .zip(segments.iter())
            .all(|(a, b)| a.kind == b.kind)
    {
        for (a, b) in before.iter().zip(segments.iter_mut()) {
            if b.id.is_none() {
                b.id = a.id.clone();
            }
        }
    }
    crate::plan::ensure_segment_ids(segments);
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

/// 导入的标定原图：PNM 按 `image_file::decode` 的规则读（带 Bayer 注释的读成 RGB）；其他格式彩色相机保留颜色读成 RGB，
/// 灰度相机读成灰度。
fn decode_imported_image(bytes: Vec<u8>, color: bool) -> Result<FrameImage, String> {
    if bytes.is_empty() || bytes.len() > 15_000_000 {
        return Err("原图不得为空，单图不得超过 15 MB".into());
    }
    let too_many = |w: u32, h: u32| u64::from(w) * u64::from(h) > 40_000_000;
    if let Some(info) = crate::image_file::pnm_info(&bytes) {
        if too_many(info.width, info.height) {
            return Err("单图像素数量超过 4000 万".into());
        }
        return crate::image_file::decode(&bytes).map_err(|e| format!("原图无法读取：{e}"));
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("原图格式无法识别：{e}"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128_000_000);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| format!("原图无法读取：{e}"))?;
    if too_many(image.width(), image.height()) {
        return Err("单图像素数量超过 4000 万".into());
    }
    let (width, height) = (image.width(), image.height());
    Ok(if color {
        FrameImage::rgb(width, height, image.into_rgb8().into_raw())
    } else {
        FrameImage::new(width, height, image.into_luma8().into_raw())
    })
}

/// 读图命令的响应，字节格式见 `frame::preview_bytes`（20 字节头 + 灰度或 RGB）。
fn preview_response(image: &FrameImage) -> tauri::ipc::Response {
    tauri::ipc::Response::new(crate::frame::preview_bytes(image))
}

#[tauri::command]
pub async fn workspace_image(
    app: AppHandle,
    id: String,
    image_id: String,
) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        safe_id(&id)?;
        let image = crate::image_file::load(&image_file(&host, &id, &image_id)?)?;
        Ok(preview_response(&image))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(async)]
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
    if !w.doc.is_burst() {
        return Err("旧拍照点配方已停用，请创建段配方".into());
    }
    // D-SB-46：发布时当场核对发布前检查
    let validated = burst_release::publish_checks(&app, w)?;
    if let Some(failed) = burst_release::publish_blocker(&validated.checks) {
        return Err(format!("不能发布：{}：{}", failed.name, failed.detail));
    }
    let recipes = &app.state::<CycleHost>().recipes;
    let release_doc = formal_release_doc(w, recipes.release_version(&w.doc)?)?;
    let bundle = burst_release::freeze_bundle(&app, &host, &release_doc, &w.camera_teaching)?;
    let navigation = navigation::prepare_navigation_release(
        &crate::storage::project(&app),
        w,
        &release_doc,
        &bundle.id,
    );
    let mut next = w.clone();
    next.doc = release_doc.clone();
    next.doc.teaching_piece = None;
    next.pass_alignment = None;
    next.pending = Some(Box::new(Release {
        doc: release_doc,
        bundle_id: bundle.id,
        revision,
        validation: validated.clone(),
        validation_site: bundle.manifest.validation_site,
        navigation: Some(navigation),
        _frames: Discarded,
        _overview: Discarded,
        _base_revision: Discarded,
    }));
    next.publish = PublishState::Waiting;
    host.save(&next)?;
    *w = next;
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w.clone())
}

/// 正式版的配方文档：带示教件来源（RB-7），版本取调用方按配方库分配的 `version`。
fn formal_release_doc(w: &Workspace, version: u32) -> Result<RecipeDoc, String> {
    let mut doc = w.doc.clone();
    doc.version = version;
    doc.stage = RecipeStage::Formal;
    for segment in &mut doc.segments {
        segment.id = None;
    }
    doc.teaching_id = Some(vision::unique_run_id(&format!(
        "teaching-{}-v{}",
        doc.id, doc.version
    )));
    doc.teaching_piece = Some(
        w.teaching_piece
            .as_ref()
            .ok_or("还没有示教件，不能发布")?
            .recipe_ref(),
    );
    Ok(doc)
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
    if current
        .as_ref()
        .is_some_and(|saved| saved.revision_id == recipe.revision_id)
    {
        return cycle.recipes.save_published(release.doc.clone());
    }
    let archive = host.dir(&recipe.id).join("published");
    std::fs::create_dir_all(&archive).map_err(|e| e.to_string())?;
    crate::fsio::write_atomic(
        &archive.join(format!("{}.json", recipe.revision_id)),
        &serde_json::to_string(release).map_err(|e| e.to_string())?,
    )?;
    cycle.recipes.save_published(release.doc.clone())
}

pub fn releases_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::storage::project(app).join("vision").join("releases"))
}

fn read_release(path: &Path) -> Result<Release, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("发布归档不能是 reparse point".into());
        }
    }
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("发布归档不是普通文件".into());
    }
    serde_json::from_str(&crate::fsio::read_text(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("发布归档格式不兼容：{e}"))
}

fn archived_release(dir: &Path, id: &str, version: u32) -> Result<Release, String> {
    safe_id(id)?;
    comparisons::checked_directory(dir, false)?;
    let path = dir.join(format!("{id}-v{version}.json"));
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("所选生产版本没有不可变发布包，请在工作台验证并发布".into())
        }
        Err(e) => return Err(e.to_string()),
    }
    let release = read_release(&path)?;
    if release.doc.id != id || release.doc.version != version {
        return Err("发布归档的配方或版本不一致".into());
    }
    Ok(release)
}

fn load_bundle(
    app: &AppHandle,
    recipe_id: &str,
    bundle_id: &str,
) -> Result<crate::release::ReleaseBundle, String> {
    crate::release::load(&releases_root(app)?, recipe_id, bundle_id)
}

/// 历史记录的发布包：在记录所属项目的 `vision/releases/` 里找；项目已删除时报原因。
fn record_bundle(
    app: &AppHandle,
    project: &str,
    recipe_id: &str,
    bundle_id: &str,
) -> Result<crate::release::ReleaseBundle, String> {
    let root = crate::project::records::releases_root(&crate::storage::root(app), project)?;
    crate::release::load(&root, recipe_id, bundle_id)
}

pub fn published_bundle(
    app: &AppHandle,
    recipe: &Recipe,
) -> Result<crate::release::ReleaseBundle, String> {
    let dir = app
        .state::<WorkspaceHost>()
        .dir(&recipe.id)
        .join("published");
    let published = archived_release(&dir, &recipe.id, recipe.version)?;
    let bundle = load_bundle(app, &recipe.id, &published.bundle_id)?;
    if bundle.recipe != published.doc || bundle.manifest.recipe_revision != recipe.revision_id {
        return Err("生产快照、配方与发布包身份不一致".into());
    }
    Ok(bundle)
}

/// 只核对生产版本对应发布包的清单与身份，不读取资源内容。
pub fn published_manifest(
    app: &AppHandle,
    recipe: &Recipe,
) -> Result<crate::release::ReleaseManifest, String> {
    let dir = app
        .state::<WorkspaceHost>()
        .dir(&recipe.id)
        .join("published");
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
    let Some(host) = app.try_state::<WorkspaceHost>() else {
        return Vec::new();
    };
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
                let navigation_warning = navigation::activate_navigation_release(
                    &crate::storage::project(app),
                    &release,
                )
                .err();
                if w.revision == release.revision {
                    w.doc = app.state::<CycleHost>().recipes.doc(&id).unwrap();
                    w.doc.teaching_piece = None;
                    // 生产配方不带段 ID：按位置接回候选原来的段 ID，逐笔对照的沿用状态不丢（采集版生效后还要继续示教）
                    inherit_segment_ids(&before.doc.segments, &mut w.doc.segments);
                }
                let next_version = app.state::<CycleHost>().recipes.next_version(&saved.id);
                w.doc.version = next_version.as_ref().copied().unwrap_or(saved.version);
                w.revision += 1;
                w.publish = PublishState::Active {
                    version: saved.version,
                    revision: w.revision,
                    warning: match (next_version.err(), navigation_warning) {
                        (Some(a), Some(b)) => Some(format!("{a}；{b}")),
                        (a, b) => a.or(b),
                    },
                    validation_site: release.validation_site,
                };
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
    app.try_state::<WorkspaceHost>().is_some_and(|host| {
        host.items
            .lock()
            .unwrap()
            .values()
            .any(Workspace::activation_uncertain)
    })
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
    task_id: Option<String>,
) -> Result<Comparison, String> {
    compare_at(app, id, revision, None, history_id, raw, task_id).await
}

pub(crate) async fn workspace_compare_at(
    app: AppHandle,
    id: String,
    revision: u64,
    instance_id: u64,
    history_id: i64,
    raw: bool,
) -> Result<Comparison, String> {
    compare_at(app, id, revision, Some(instance_id), history_id, raw, None).await
}

async fn compare_at(
    app: AppHandle,
    id: String,
    revision: u64,
    instance_id: Option<u64>,
    history_id: i64,
    raw: bool,
    task_id: Option<String>,
) -> Result<Comparison, String> {
    let operation = crate::operation::Operation::start(&app, task_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let w = app
                .state::<WorkspaceHost>()
                .comparison_snapshot(&id, revision, instance_id)?;
            let recipe = w.doc.build()?;
            let store = app.state::<Store>();
            let detail = store.detail(history_id)?;
            // 任意候选都能复测任意记录（不看型号、配方编号与所属项目）；按规则重判仍需记录的配方快照
            let original = store.record_recipe(&detail.summary)?;
            let (judgement, burst) = burst_history::compare(
                &app,
                &w,
                &recipe,
                &detail,
                original.as_ref(),
                raw,
                operation,
            )?;
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
            operation.saving()?;
            save_comparison(&app, &comparison)?;
            Ok(comparison)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn comparison_id(app: &AppHandle, history_id: i64) -> String {
    let seq = app
        .state::<WorkspaceHost>()
        .capture_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{history_id}-{}-{seq}", ly_plc::now_ms())
}

fn save_comparison(app: &AppHandle, comparison: &Comparison) -> Result<(), String> {
    let host = app.state::<WorkspaceHost>();
    host.comparisons
        .save(&host.root, &app.state::<Store>(), comparison)
}

/// 按天清理：被示教件、标定件引用的历史件（记录与录制）保留，引用核对不了时本次不清理。调用方持
/// `recipe_capture::REFERENCES`（先于追溯 spool 锁取得）。
pub(crate) fn purge_history(app: &AppHandle, before: i64) -> Result<(usize, Vec<String>), String> {
    let host = app.state::<WorkspaceHost>();
    let data = crate::storage::root(app);
    let keep = crate::project::records::referenced_cycles(&data)
        .map_err(|e| format!("无法核对示教件引用的历史件，本次不清理：{e}"))?;
    let (count, mut warnings, orphaned) =
        host.comparisons
            .purge(&host.root, &app.state::<Store>(), before, &keep)?;
    for (project, source, directory) in orphaned {
        if let Err(error) = crate::project::records::remove_recording(
            &data,
            &project,
            source,
            Path::new(&directory),
        ) {
            warnings.push(format!(
                "项目 {project} 的过期原图 {directory} 未删除：{error}"
            ));
        }
    }
    Ok((count, warnings))
}

/// 删除历史记录；示教件、标定件引用的记录拒绝删除。调用方持 `recipe_capture::REFERENCES`（先于追溯 spool 锁取得）。
pub(crate) fn delete_history(app: &AppHandle, ids: &[i64]) -> Result<Vec<String>, String> {
    let host = app.state::<WorkspaceHost>();
    let store = app.state::<Store>();
    let data = crate::storage::root(app);
    let referenced = crate::project::records::referenced_cycles(&data)?;
    for &id in ids {
        if store
            .comparison_cycle(id)?
            .is_some_and(|cycle| referenced.contains(&cycle))
        {
            return Err(format!(
                "记录 #{id} 是候选的示教件或标定件，不能删除；先换示教件或删除引用它的候选"
            ));
        }
    }
    host.comparisons.delete_history(&data, &store, ids)
}

#[tauri::command]
pub async fn workspace_compare_original(
    app: AppHandle,
    history_id: i64,
    task_id: Option<String>,
) -> Result<Comparison, String> {
    let operation = crate::operation::Operation::start(&app, task_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let store = app.state::<Store>();
            let detail = store.detail(history_id)?;
            if !detail.recording.available {
                return Err("原图未完整保留，不能重现原发布测量；请查看录制状态".into());
            }
            let cycle_id = detail
                .summary
                .cycle_id
                .clone()
                .ok_or("历史记录缺少 cycleId")?;
            let bundle_id = detail
                .summary
                .bundle_id
                .as_deref()
                .ok_or("本件没有不可变发布包，不能按原版本重现")?;
            if detail.summary.recipe_revision.is_none() {
                return Err("历史记录缺少配方快照身份".into());
            }
            let original = store
                .record_recipe(&detail.summary)?
                .ok_or("原始配方快照缺失；不能使用当前配方替代")?;
            let bundle = record_bundle(&app, &detail.summary.project_id, &original.id, bundle_id)?;
            let engine = app
                .state::<vision::VisionHost>()
                .engine()
                .ok_or("原版本兼容引擎未加载")?;
            let prepared = crate::production::Prepared::load(bundle, engine, &original)?;
            prepared.verify()?;
            let (judgement, burst) =
                burst_history::reproduce_original(&app, &detail, &prepared, &cycle_id, operation)?;
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
            operation.saving()?;
            save_comparison(&app, &comparison)?;
            Ok(comparison)
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 一条历史记录能做哪几种复测（历史详情的按钮与原因）：用 `candidate` 指定的本机候选（不给时用记录所属配方的候选），
/// 任意候选都能选，不看型号、配方编号与所属项目；不新建候选、不看工作台当前打开的配方。`choices` 列出可选的候选。
#[tauri::command]
pub async fn workspace_history_compare_options(
    app: AppHandle,
    history_id: i64,
    candidate: Option<String>,
) -> Result<burst_history::CompareOptions, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let detail = store.detail(history_id)?;
        let original = store.record_recipe(&detail.summary)?;
        let (chosen, choices) = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let mut choices: Vec<&Workspace> = items.values().filter(|w| !w.legacy()).collect();
            choices.sort_by(|a, b| a.doc.id.cmp(&b.doc.id));
            let wanted = candidate.as_deref().or(detail.summary.recipe_id.as_deref());
            let chosen = wanted
                .and_then(|id| items.get(id))
                .filter(|w| !w.legacy())
                .cloned();
            (
                chosen,
                choices
                    .into_iter()
                    .map(burst_history::CompareCandidate::of)
                    .collect(),
            )
        };
        let releases = crate::project::records::releases_root(
            &crate::storage::root(&app),
            &detail.summary.project_id,
        )
        .err();
        let mut options =
            burst_history::compare_options(&detail, original.as_ref(), chosen.as_ref(), releases);
        options.choices = choices;
        Ok(options)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn workspace_comparisons(
    host: State<'_, WorkspaceHost>,
    store: State<'_, Store>,
    id: String,
    history_id: i64,
) -> Result<Vec<Comparison>, String> {
    // 历史详情列出这件记录所有候选的对照（任意配方都能验证任意记录）；id 只核对格式
    safe_id(&id)?;
    host.comparisons.list(&host.root, &store, None, history_id)
}

#[tauri::command]
pub async fn workspace_comparison_summaries(
    app: AppHandle,
    history_id: i64,
) -> Result<Vec<comparisons::ComparisonSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        host.comparisons
            .summaries(&host.root, &app.state::<Store>(), None, history_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_comparison_get(
    app: AppHandle,
    history_id: i64,
    comparison_id: String,
) -> Result<Comparison, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        host.comparisons.get(
            &host.root,
            &app.state::<Store>(),
            history_id,
            &comparison_id,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn clear_live(app: &AppHandle) {
    if let Some(host) = app.try_state::<WorkspaceHost>() {
        *host.live_burst.lock().unwrap() = LiveFrames::default();
        *host.live_views.lock().unwrap() = LiveViews::default();
    }
}

/// 一帧测量轨迹转成叠图（第 `segment` 段走廊、搜索带与胶边）：在线与历史按原发布包重算共用，保证两边画出的线一致。
fn trace_view(
    recipe: &Recipe,
    segment: u32,
    trace: crate::measure::FrameTrace,
    metrics: &crate::burst_judge::FrameMetrics,
) -> CalibrationFrameView {
    let (corridor, search_half_px) = burst::corridor_overlay(recipe, metrics.camera, segment);
    CalibrationFrameView {
        segment: Some(segment),
        corridor: if trace.line.is_empty() {
            corridor
        } else {
            trace.line
        },
        search_half_px,
        edges: trace.edges,
        part_edges: trace.part_edges,
        metrics: metrics.clone(),
    }
}

/// 记录的发布包：生产预热里有就复用，否则加载并留作历史叠图的唯一缓存。
fn history_prepared(
    app: &AppHandle,
    project: &str,
    original: &Recipe,
    bundle_id: &str,
) -> Result<Arc<crate::production::Prepared>, String> {
    let current = project == crate::project::current_id(app);
    if let Some(prepared) = app
        .state::<crate::production::ProductionHost>()
        .ready_bundle(bundle_id)
        .filter(|_| current)
    {
        return Ok(prepared);
    }
    let host = app.state::<WorkspaceHost>();
    let mut cached = host.history_bundle.lock().unwrap();
    if let Some(prepared) = cached
        .as_ref()
        .filter(|(owner, p)| owner == project && p.bundle.id == bundle_id)
    {
        return Ok(prepared.1.clone());
    }
    let bundle = record_bundle(app, project, &original.id, bundle_id)?;
    let engine = app
        .state::<vision::VisionHost>()
        .engine()
        .ok_or("图像核心库未加载")?;
    let prepared = Arc::new(crate::production::Prepared::load(bundle, engine, original)?);
    prepared.verify()?;
    *cached = Some((project.to_string(), prepared.clone()));
    Ok(prepared)
}

/// 历史件一帧按原发布包重算的叠图（历史详情看原始记录时显示，只读、不落盘）：用记录的发布包、这一帧当时所用走廊段，
/// 从录制原图走与在线检测同一测量入口（`Prepared::measure_frame`），给出走廊、搜索带与胶边。缺帧、本段不检测、
/// 没记走廊的帧只给记录的指标、不画线。指标一律取记录里存的。
#[tauri::command]
pub async fn workspace_history_frame_view(
    app: AppHandle,
    history_id: i64,
    camera: usize,
    seq: u64,
) -> Result<CalibrationFrameView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let detail = store.detail_frame(history_id, camera, seq)?;
        let burst = detail
            .burst
            .as_ref()
            .ok_or("这条历史是旧拍照点记录，没有按帧的叠图")?;
        let frame = burst
            .frames
            .iter()
            .find(|f| f.camera == camera && f.seq == seq)
            .ok_or("没有这一帧的记录")?;
        let metrics = frame
            .metrics
            .clone()
            .ok_or("这台相机本帧组缺帧，没有原图")?;
        let Some(segment) = metrics.corridor.filter(|_| {
            !matches!(
                metrics.state,
                crate::burst_judge::FrameState::Missing
                    | crate::burst_judge::FrameState::NotInspected
            )
        }) else {
            return Ok(CalibrationFrameView {
                segment: None,
                corridor: Vec::new(),
                search_half_px: None,
                edges: Vec::new(),
                part_edges: Vec::new(),
                metrics,
            });
        };
        if frame.raw_file.is_none() {
            return Err("这一帧原图未保存或已按保留策略清理，画不了辅助线".into());
        }
        let bundle_id = detail
            .summary
            .bundle_id
            .as_deref()
            .ok_or("本件没有发布包（维护采集件或未匹配到可用配方），画不了辅助线")?;
        if detail.summary.recipe_revision.is_none() {
            return Err("历史记录缺少配方快照身份".into());
        }
        let original = store
            .record_recipe(&detail.summary)?
            .ok_or("原始配方快照缺失")?;
        let prepared = history_prepared(&app, &detail.summary.project_id, &original, bundle_id)?;
        let image = burst_history::frame_image(&app, &detail, camera, seq)?;
        let measured = prepared.measure_frame(
            camera,
            segment,
            seq,
            &image,
            &format!("history-view-{history_id}-c{camera}-{seq}"),
        )?;
        let trace = crate::measure::FrameTrace {
            edges: measured.edges,
            part_edges: measured.part_edges,
            line: measured.line,
        };
        Ok(trace_view(&prepared.recipe, segment, trace, &metrics))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 段配方在途件一帧测到的叠图（第 `segment` 段走廊、搜索带与胶边），供 `workspace_live_frame_view` 读。
pub fn retain_live_view(
    app: &AppHandle,
    cycle_id: &str,
    recipe: &Recipe,
    segment: u32,
    trace: crate::measure::FrameTrace,
    metrics: &crate::burst_judge::FrameMetrics,
) {
    let Some(host) = app.try_state::<WorkspaceHost>() else {
        return;
    };
    let view = trace_view(recipe, segment, trace, metrics);
    let mut live = host.live_views.lock().unwrap();
    let key = (cycle_id.to_owned(), recipe.revision_id.clone());
    if live.key.as_ref() != Some(&key) {
        *live = LiveViews {
            key: Some(key),
            ..LiveViews::default()
        };
    }
    live.views.insert((metrics.camera, metrics.seq), view);
}

/// 段配方在途件某台相机第 seq 帧测到的叠图（camera 为计划相机下标）；还没测完、没测成或不是本件时为 None。
#[tauri::command]
pub fn workspace_live_frame_view(
    host: State<'_, WorkspaceHost>,
    cycle_id: String,
    revision_id: String,
    camera: usize,
    seq: u64,
) -> Option<CalibrationFrameView> {
    let live = host.live_views.lock().unwrap();
    live.key
        .as_ref()
        .filter(|key| key.0 == cycle_id && key.1 == revision_id)
        .and_then(|_| live.views.get(&(camera, seq)).cloned())
}

/// 段配方在途件一帧的原图（计划相机 camera 的第 seq 帧）与它排进写盘的录制原图，供 `workspace_live_frame_image` 读。
pub fn retain_live_frame(
    app: &AppHandle,
    cycle_id: &str,
    revision_id: &str,
    camera: usize,
    seq: u64,
    image: &Option<crate::frame::FrameView>,
    recorded: Option<crate::recorder::PendingFrame>,
) {
    if let Some(host) = app.try_state::<WorkspaceHost>() {
        host.live_burst.lock().unwrap().insert(
            cycle_id,
            revision_id,
            (camera, seq),
            image.clone(),
            recorded,
        );
    }
}

/// 段配方在途件某台相机第 seq 帧的原图（决策记录 5.4，camera 为计划相机下标）：先取内存里的最近帧，不在时读本件的
/// 录制原图（收尾前在录制目录里，收尾后按本件的历史记录）；没到、缺帧或没有录制原图时报错。
#[tauri::command]
pub async fn workspace_live_frame_image(
    app: AppHandle,
    cycle_id: String,
    revision_id: String,
    camera: usize,
    seq: u64,
) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let image = live_frame_image(
            &host.live_burst,
            &cycle_id,
            &revision_id,
            (camera, seq),
            || recorded_part_frame(&app, &cycle_id, &revision_id, camera, seq),
        )?;
        Ok(preview_response(&image))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// `workspace_live_frame_image` 的读图：内存里的最近帧 → 收尾前录制目录里已写盘的这一帧 → `recorded_part`（按历史记录读）。
fn live_frame_image(
    live: &Mutex<LiveFrames>,
    cycle_id: &str,
    revision_id: &str,
    k: (usize, u64),
    recorded_part: impl FnOnce() -> Result<FrameImage, String>,
) -> Result<Arc<FrameImage>, String> {
    let (held, recorded) = {
        let live = live.lock().unwrap();
        if live
            .key
            .as_ref()
            .is_none_or(|key| key.0 != cycle_id || key.1 != revision_id)
        {
            return Err("本件尚未收到整帧图像".into());
        }
        (
            live.frames
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, view)| view.clone()),
            live.recorded.get(&k).cloned(),
        )
    };
    if let Some(view) = held {
        return view.image();
    }
    let image = match recorded.map(|frame| frame.state()) {
        Some(crate::recorder::RecordedFrame::Ready(path)) if path.is_file() => {
            crate::image_file::load(&path).map_err(|e| format!("读回录制原图失败：{e}"))?
        }
        Some(crate::recorder::RecordedFrame::Writing) => {
            return Err("这一帧原图还在写盘，稍后再看".into())
        }
        _ => recorded_part()?,
    };
    Ok(Arc::new(image))
}

/// 已收尾的本件按历史记录读录制原图。
fn recorded_part_frame(
    app: &AppHandle,
    cycle_id: &str,
    revision_id: &str,
    camera: usize,
    seq: u64,
) -> Result<FrameImage, String> {
    let store = app.state::<Store>();
    let id = store
        .id_by_cycle(cycle_id)?
        .ok_or("这一帧原图已不在内存里，本件也没有录到它（录制关闭、录制盘都不足或录制队满）")?;
    let detail = store.detail_frame(id, camera, seq)?;
    if detail.summary.recipe_revision.as_deref() != Some(revision_id) {
        return Err("本件的历史记录与所看的配方修订不一致".into());
    }
    burst_history::frame_image(app, &detail, camera, seq)
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
    samples
        .get(&cam)
        .filter(|(meta, _)| meta.id == image_id)
        .map(|(meta, _)| meta.view)
        .ok_or_else(|| "标定样本已更新".into())
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
pub async fn workspace_station_capture(
    app: AppHandle,
    cam: u8,
    view: Option<u8>,
) -> Result<FrozenImage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() {
            return Err("工件正在检测，结束后再取标定样本".into());
        }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        if !(1..=config.view_count).contains(&view.unwrap_or(1)) {
            return Err("图编号超出设备范围".into());
        }
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
pub async fn workspace_station_import(
    app: AppHandle,
    cam: u8,
    bytes: Vec<u8>,
    view: Option<u8>,
) -> Result<FrozenImage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() {
            return Err("工件正在检测，结束后再导入标定原图".into());
        }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        if !(1..=config.view_count).contains(&view.unwrap_or(1)) {
            return Err("图编号超出设备范围".into());
        }
        let image = Arc::new(decode_imported_image(bytes, config.image_mode.color())?);
        if cycle.busy() || state_value(&slot.config()) != state_value(&config) {
            return Err("导入期间设备或节拍状态变化，原图已丢弃".into());
        }
        let host = app.state::<WorkspaceHost>();
        let seq = host
            .capture_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let meta = FrozenImage {
            view: view.unwrap_or(1),
            id: format!("station-import-{cam}-{}-{seq}", ly_plc::now_ms()),
            source: "import".into(),
            captured_at: ly_plc::now_ms(),
            size: [image.width, image.height],
            camera: config.id.clone(),
            camera_tag: state_value(&config),
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

/// 标定页进入时载入当前标定所用的原图作冻结样本（编号沿用标定记的样本编号，重新标定复用同一原图）；没有可载入的原图为 None。
#[tauri::command]
pub async fn workspace_station_restore(
    app: AppHandle,
    cam: u8,
    view: Option<u8>,
) -> Result<Option<FrozenImage>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        if cycle.busy() {
            return Err("工件正在检测，结束后再读取标定原图".into());
        }
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let config = slot.config();
        let selected = view.unwrap_or(1);
        if !(1..=config.view_count).contains(&selected) {
            return Err("图编号超出设备范围".into());
        }
        let keys = vision::camera_calib_keys(&app, cam, Some(selected))?;
        let calib = vision::calib_key_path(&app, &keys[0])?;
        let Some((id, image, ts)) = vision::calib_sample(&crate::storage::project(&app), &calib)?
        else {
            return Ok(None);
        };
        if cycle.busy() || state_value(&slot.config()) != state_value(&config) {
            return Err("读取期间设备或节拍状态变化，原图已丢弃".into());
        }
        let meta = FrozenImage {
            view: selected,
            id,
            source: "calib".into(),
            captured_at: ts.unwrap_or(0),
            size: [image.width, image.height],
            camera: config.id.clone(),
            camera_tag: state_value(&config),
            calib_tag: Value::Null,
            geometry_tag: Value::Null,
            exposure_us: None,
            gain_db: None,
            history_id: None,
        };
        app.state::<WorkspaceHost>()
            .station
            .lock()
            .unwrap()
            .insert(cam, (meta.clone(), Arc::new(image)));
        Ok(Some(meta))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct ImportTestDir(pub(super) PathBuf);
    impl ImportTestDir {
        pub(super) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "tujiao-sample-import-{}-{sequence}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for ImportTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 第 1 帧带着录制文件 `file` 进内存预览，再来两帧大图把它挤出去。
    fn evicted_first_frame(file: PathBuf) -> Mutex<LiveFrames> {
        let big = || Some(Arc::new(FrameImage::new(8192, 6144, vec![0; 8192 * 6144])).into());
        let mut live = LiveFrames::default();
        live.insert(
            "c1",
            "r1",
            (0, 1),
            big(),
            Some(crate::recorder::PendingFrame::written(file)),
        );
        for seq in 2..=3 {
            live.insert("c1", "r1", (0, seq), big(), None);
        }
        Mutex::new(live)
    }

    /// 防的回归：一件帧多时，内存预览挤掉的早期帧在线检测页看不了（只能看最后二十来帧）。
    #[test]
    fn frame_pushed_out_of_memory_preview_reads_its_pending_recording() {
        let dir = ImportTestDir::new();
        let file = dir.0.join("cam1_s000001.pgm");
        let recorded = FrameImage::new(4, 2, (0..8).collect());
        crate::image_file::save_pgm(&file, &recorded).unwrap();
        let live = evicted_first_frame(file);
        let image = live_frame_image(&live, "c1", "r1", (0, 1), || {
            Err("收尾前不该按历史记录读".into())
        })
        .unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(&image.pixels[..], &recorded.pixels[..]);
    }

    /// 防的回归：收尾后录制目录移走，挤出内存的帧按旧路径读不到就报错，不再按历史记录读。
    #[test]
    fn frame_pushed_out_after_its_recording_moved_reads_the_part_history() {
        let dir = ImportTestDir::new();
        let live = evicted_first_frame(dir.0.join("moved").join("cam1_s000001.pgm"));
        let image = live_frame_image(&live, "c1", "r1", (0, 1), || {
            Ok(FrameImage::new(2, 2, vec![7; 4]))
        })
        .unwrap();
        assert_eq!(&image.pixels[..], &[7; 4]);
    }
}

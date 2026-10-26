use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::camera::CameraSource;
use crate::cycle::{self, CycleHost};
use crate::frame::FrameImage;
use crate::judge::{self, PointState, Verdict};
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
mod fixture;
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
pub struct Trial {
    pub image_id: String,
    pub engine_tag: Value,
    pub params_tag: Value,
    pub geometry_tag: Value,
    pub passed: bool,
    pub score: f64,
    pub coverage: f64,
    pub elapsed_ms: u128,
    pub reason: String,
    pub measurement: Value,
    #[serde(default)]
    pub verdict: Option<Verdict>,
    /// 特征定位视图的示教图检查报告（G4）；G4 之前的试测没有它，按需要重新试测处理
    #[serde(default)]
    pub fixture: Option<vision::fixture_quality::FixtureTeachReport>,
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
    #[serde(default)]
    pub view_states: Vec<workflow::ViewTeaching>,
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
            view_states: Vec::new(),
        }
    }

    /// 原图要对上拍照点（image_tag），试测要对上当前的中线与检测参数（teach_tag），并且通过。
    /// 特征定位视图（`fixture`）另要求试测带着通过的示教图检查报告：G4 之前 `passed = true` 的试测没有报告，不能绕过定位验收。
    fn checked(&self, image_tag: &Value, teach_tag: &Value, fixture: bool) -> Result<(), String> {
        let image = self.image.as_ref().ok_or("本帧尚未冻结原图")?;
        let trial = self.trial.as_ref().ok_or("本帧尚未试测")?;
        if &image.geometry_tag != image_tag
            || !trial.passed
            || trial.image_id != image.id
            || &trial.geometry_tag != image_tag
            || &trial.params_tag != teach_tag
        {
            return Err("图像或参数已变更，需要重新试测".into());
        }
        if fixture && trial.fixture.as_ref().is_none_or(|report| !report.passed) {
            return Err("特征定位的示教图检查没有通过，或试测早于这项检查，请重新试测".into());
        }
        Ok(())
    }

    fn checked_engine(&self, engine_tag: &Value) -> Result<(), String> {
        if self.trial.as_ref().is_none_or(|trial| &trial.engine_tag != engine_tag) {
            return Err("图像引擎版本已变化，需要重新试测".into());
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
    /// 每个特征定位视图在独立正常样本上的统计（G4）；G4 之前的验证没有它，发布门按没通过处理
    #[serde(default)]
    pub fixture: Vec<vision::fixture_quality::FixtureViewReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub doc: RecipeDoc,
    pub bundle_id: String,
    pub base_revision: Option<String>,
    pub revision: u64,
    pub frames: Vec<Teaching>,
    pub overview: Overview,
    pub validation: Validation,
    pub validation_site: ValidationSite,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    pub doc: RecipeDoc,
    #[serde(default)]
    pub instance_id: u64,
    pub base_revision: Option<String>,
    pub revision: u64,
    pub frames: Vec<Teaching>,
    pub overview: Overview,
    pub samples: Vec<Sample>,
    pub sample_bank: Vec<BankSample>,
    pub validation: Option<Validation>,
    pub pending: Option<Box<Release>>,
    pub publish: PublishState,
    pub updated_at: i64,
    #[serde(default)]
    pub last_position: Option<workflow::Position>,
    #[serde(default)]
    pub capture_id: Option<String>,
    /// 还没有拍照点使用的相机在采集页调好的曝光与增益：配方只收拍照点用到的相机，用上时并入配方
    #[serde(default)]
    pub unbound_imaging: BTreeMap<String, CameraImaging>,
    /// 段配方：每台计划相机的走廊示教帧（旧拍照点配方为空，示教在 frames）
    #[serde(default)]
    pub camera_teaching: Vec<burst::CameraTeaching>,
    /// 段配方：多轮标定加入的轮次（旧拍照点配方为空）
    #[serde(default)]
    pub calibration: burst::Calibration,
    #[serde(default)]
    pub capture_review: capture_review::CaptureReview,
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
        let count = doc.shots.len();
        Self {
            doc,
            instance_id: 0,
            base_revision,
            revision: 1,
            frames: (0..count).map(Teaching::empty).collect(),
            overview: Overview::default(),
            samples: Vec::new(),
            sample_bank: Vec::new(),
            validation: None,
            pending: None,
            publish: PublishState::Draft,
            updated_at: ly_plc::now_ms(),
            last_position: None,
            capture_id: None,
            unbound_imaging: BTreeMap::new(),
            camera_teaching: Vec::new(),
            calibration: burst::Calibration::default(),
            capture_review: capture_review::CaptureReview::default(),
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

    fn require_single_frame_source(&self) -> Result<(), String> {
        if self.capture_id.is_some() {
            return Err("整圈示教不能替换其他轮次的单帧，请重新采集完整一圈".into());
        }
        Ok(())
    }

    fn retry_publish(&mut self, revision: u64) -> Result<(), String> {
        if self.revision != revision || self.pending.is_none() || !matches!(self.publish, PublishState::Failed { .. } | PublishState::Uncertain { .. }) {
            return Err("没有可以重试的发布版本".into());
        }
        self.pending.as_ref().unwrap().check_group_selection()?;
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
    /// 候选的拍照计划与生产配方不同（或还没发布过）：生效后 PLC 侧要同步新的计划版本
    pub plan_changed: bool,
    /// 生产配方的 PLC 对接参数；未发布或编不出计划时为空
    pub production_plc: Option<crate::plc_plan::PlcParams>,
    /// 候选发布后的 PLC 对接参数；编不出时原因在 candidate_plc_error
    pub candidate_plc: Option<crate::plc_plan::PlcParams>,
    pub candidate_plc_error: Option<String>,
    pub coverage: Option<f32>,
    pub progress: progress::Progress,
    /// 段配方的计划部分（决策记录 5.1）；旧拍照点配方为空。段配方时上面的 productionPlc / candidatePlc / candidatePlcError /
    /// coverage 为空、progress 没有步骤，进度在 plan.progress
    pub plan: Option<burst::WorkspacePlanView>,
}

pub struct WorkspaceHost {
    root: PathBuf,
    items: Mutex<HashMap<String, Workspace>>,
    pub notes: Vec<String>,
    live: Mutex<LiveFrames<usize>>,
    /// 段配方在途件的实时原图，按 (计划相机下标, seq)
    live_burst: Mutex<LiveFrames<(usize, u64)>>,
    station: Mutex<HashMap<u8, (FrozenImage, Arc<FrameImage>)>>,
    capture_seq: std::sync::atomic::AtomicU64,
    comparisons: comparisons::Comparisons,
}

/// 在途件的实时原图：按工件与配方修订成组，换件即清空；总量超过上限时丢最早的。
/// 旧拍照点配方按拍照点 k，段配方按 (计划相机下标, seq)。
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
        let uses = |doc: &RecipeDoc| doc.shots.iter().any(|s| s.camera == camera);
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
                Ok(w) => { fixture::prune_fixture_dirs(&entry.path(), &w); items.insert(w.doc.id.clone(), w); }
                Err(error) => notes.push(format!("{} 未加载：{error}。{}", file.display(), unloaded_hint(&entry.path()))),
            }
        }
        Ok(Self {
            root,
            items: Mutex::new(items),
            notes,
            live: Mutex::new(LiveFrames::default()),
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
        fixture::prune_fixture_dirs(&dir, w);
        Ok(())
    }
}

/// 要检的拍照点里已示教中线的比例（%）；没有要检的拍照点时为空。
fn coverage(recipe: &Recipe) -> Option<f32> {
    let measured = recipe.shots.iter().filter(|s| s.measured()).count();
    (measured > 0).then(|| 100.0 * recipe.shots.iter().filter(|s| s.measured() && s.taught()).count() as f32 / measured as f32)
}

/// 拍照点 k 的两组实际字段：原图要对上编号、相机与标定引用；试测要对上中线、像素当量、检测参数、站距与特征定位。
/// r 是按视图投影过的配方，fixture 就是这个视图的。
fn shot_tags(r: &Recipe, k: usize) -> Result<(Value, Value), String> {
    let shot = r.shots.get(k).ok_or("拍照点不存在")?;
    Ok((
        state_value(&json!([shot.id, shot.pose_id, shot.camera, shot.view, shot.calib_ref()])),
        state_value(&json!([shot.view, shot.path, shot.mm_per_px, r.shot_detect(k), shot.limits.as_ref().unwrap_or(&r.limits), r.spacing, r.filter_window, shot.fixture])),
    ))
}

/// 样本组的原图要对上每个拍照点的编号、相机与标定引用（改中线不影响样本）。
fn images_tag(r: &Recipe) -> Value {
    json!(r.shots.iter().map(|s| (&s.id, &s.pose_id, &s.camera)).collect::<Vec<_>>())
}

fn view(app: &AppHandle, w: Workspace) -> Result<WorkspaceView, String> {
    let layout = w.doc.build()?;
    let production_recipe = app.state::<CycleHost>().recipe(&w.doc.id);
    let production_version = production_recipe.as_ref().map(|r| r.version);
    let production_stage = production_recipe.as_ref().map(|r| r.stage);
    if w.doc.is_burst() {
        // 段配方：旧拍照点的 PLC 参数、示教覆盖率与进度为空，计划导出与进度在 plan（D-SB-25、决策记录 5.1）
        let production = app.state::<CycleHost>().recipes.doc(&w.doc.id);
        let plan_changed = production.as_ref().is_none_or(|p| p.plan_version == 0 || !p.same_plan(&w.doc));
        let plan = burst::plan_view(app, &w, &layout, production_recipe.as_deref(), production.as_ref(), production_version);
        return Ok(WorkspaceView {
            workspace: w,
            layout,
            production_version,
            production_stage,
            production_plan_version: production.map(|p| p.plan_version),
            plan_changed,
            production_plc: None,
            candidate_plc: None,
            candidate_plc_error: None,
            coverage: None,
            progress: progress::Progress {
                steps: Vec::new(),
                shots: Vec::new(),
                next: None,
            },
            plan: Some(plan),
        });
    }
    let production_plc = production_recipe.and_then(|r| crate::plc_plan::PlcParams::from_recipe(&r).ok());
    let (candidate_plc, candidate_plc_error) = match crate::plc_plan::PlcParams::from_recipe(&layout) {
        Ok(params) => (Some(params), None),
        Err(reason) => (None, Some(reason)),
    };
    let production = app.state::<CycleHost>().recipes.doc(&w.doc.id);
    let plan_changed = production.as_ref().is_none_or(|p| p.plan_version == 0 || !p.same_plan(&w.doc));
    let coverage = coverage(&layout);
    let progress = progress::compute(&w, production_version, &devices(app, &w, &layout));
    Ok(WorkspaceView {
        workspace: w,
        layout,
        production_version,
        production_stage,
        production_plan_version: production.map(|p| p.plan_version),
        plan_changed,
        production_plc,
        candidate_plc,
        candidate_plc_error,
        coverage,
        progress,
        plan: None,
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
    for k in 0..recipe.shot_count() {
        for selected in recipe.shots[k].enabled_views() {
            let projected = recipe.for_view(k, selected)?;
            values.push(calibration_state(&vision::shot_calib_path(app, &projected, k)?)?);
        }
    }
    // 段配方各检测相机的工位标定（缺省为设备标定 device/{device}；旧拍照点配方没有检测相机，这一项为空）
    for inspect in &recipe.inspect {
        values.push(calibration_state(&vision::camera_calib_path(app, inspect)?)?);
    }
    values.push(json!([crate::site::image_measurement(app)]));
    values.push(json!(app.state::<vision::VisionHost>().engine().map(|engine| engine_state(&engine))));
    Ok(state_value(&values))
}

/// 当前设备的相机与标定标签（按冻结图的拍照点与视角）以及当前引擎，只读，不改候选记录。
fn devices(app: &AppHandle, w: &Workspace, recipe: &Recipe) -> progress::Devices {
    let engine = app.state::<vision::VisionHost>().engine().map(|e| engine_state(&e));
    let mut views = HashMap::new();
    for (k, frame) in w.frames.iter().enumerate().take(recipe.shot_count()) {
        for image in &frame.views {
            if let Ok(current) = recipe.for_view(k, image.view).and_then(|r| tags(app, &r, k)) {
                views.insert((k, image.view), current);
            }
        }
    }
    progress::Devices { views, engine }
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

/// 拍照点 k 的相机参数与实际标定值：冻结图像时记下，之后变了就要重新取样。
fn tags(app: &AppHandle, recipe: &Recipe, k: usize) -> Result<(Value, Value), String> {
    let cycle = app.state::<CycleHost>();
    let shot = recipe.shots.get(k).ok_or("拍照点不存在")?;
    let cam = cycle.camera.require(&shot.camera)?;
    let config = cycle
        .camera
        .slot(cam as usize)
        .ok_or("相机不存在")?
        .config();
    let calib = vision::shot_calib_path(app, recipe, k)?;
    let sample_set = crate::offline::sample_set(app).map(|set| set.manifest().id.clone());
    Ok((
        camera_state(&config, sample_set.as_deref()),
        calibration_state(&calib)?,
    ))
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
    if !doc.is_burst() { return Err("旧拍照点配方已停用，请创建段配方".into()); }
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

/// 新建候选的配方起点：不带拍照点与相机参数（旧拍照点配方原行为）；段配方另见 `burst::prepare_new_doc`。
fn new_candidate_doc(doc: &mut RecipeDoc) {
    doc.shots.clear();
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
    // 段配方：N_exp、标定统计、整件标定与 x_ref 只能由多轮标定写入，段序列或类型变化时清空（旧拍照点配方不经过）
    burst::guard_calibration(&w.doc, &mut doc)?;
    for shot in &mut doc.shots {
        let Some(old_k) = w.doc.shots.iter().position(|old| old.id == shot.id) else { continue };
        let old = &w.doc.shots[old_k];
        if old.view != shot.view {
            if shot.views.is_empty() {
                shot.views = w.frames[old_k].views.iter().map(|image| crate::recipe::ShotViewSpec {
                    view: image.view, enabled: image.view == shot.view,
                    path: if image.view == old.view { old.path.clone() } else { Vec::new() },
                    mm_per_px: if image.view == old.view { old.mm_per_px } else { None },
                    detect: if image.view == old.view { old.detect.clone() } else { None },
                    limits: if image.view == old.view { old.limits.clone() } else { None },
                    calib: if image.view == old.view { old.calib.clone() } else { None },
                    fixture: if image.view == old.view { old.fixture.clone() } else { None },
                }).collect();
            }
            let projected = shot.for_view(shot.view)?;
            shot.path = projected.path; shot.mm_per_px = projected.mm_per_px; shot.detect = projected.detect; shot.limits = projected.limits; shot.calib = projected.calib;
            shot.fixture = projected.fixture;
        }
    }
    fixture::reject_unconfirmed_rotation(&w.doc, &doc)?;
    fixture::reject_end_param_edit(&w.doc, &doc)?;
    bind_fixture_sources(w, &mut doc, None)?;
    let mut unbound = w.unbound_imaging.clone();
    sync_doc_imaging(&mut doc, &mut unbound);
    let recipe = doc.build()?;
    if doc == w.doc { return Ok(()); }
    w.unbound_imaging = unbound;
    let before = w.doc.build()?;
    let frames = recipe.shots.iter().enumerate().map(|(k, shot)| {
        let Some(old_k) = w.doc.shots.iter().position(|s| s.id == shot.id) else { return Teaching::empty(k); };
        let mut frame = w.frames.get(old_k).cloned().unwrap_or_else(|| Teaching::empty(k));
        frame.k = k;
        for state in &mut frame.view_states {
            let old = before.for_view(old_k, state.view).and_then(|r| shot_tags(&r, old_k));
            let new = recipe.for_view(k, state.view).and_then(|r| shot_tags(&r, k));
            if old.ok() != new.ok() { state.trial = None; state.saved = false; }
        }
        let current = shot_tags(&recipe, k).unwrap();
        if shot_tags(&before, old_k).ok().as_ref() != Some(&current) { frame.trial = None; frame.saved = false; }
        if before.shots[old_k].camera != shot.camera || before.shots[old_k].pose_id != shot.pose_id {
            frame.views.clear(); frame.image = None; frame.view_states.clear(); frame.backup = None;
        }
        frame
    }).collect();
    w.frames = frames;
    w.doc = doc;
    // 不再是计划相机的示教帧不保留（旧拍照点配方没有）
    let cameras = w.doc.plan_cameras();
    w.camera_teaching.retain(|t| cameras.contains(&t.camera));
    for k in 0..w.frames.len() {
        if !w.doc.shots[k].views.is_empty() { let selected = w.doc.shots[k].view; workflow::load_active(w, k, selected)?; }
    }
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

/// 定位模板（glue.locate 的 base_dir）：`<workspaces>/<配方>/fixtures/<冻结图>-r<x>_<y>_<w>_<h>-p<外圈>/template.pgm`。
/// 按来源寻址，内容只由冻结图、框与外圈决定；冻结图不会改写。每次使用都按冻结图重新裁、与磁盘上的字节比对，
/// 缺失或不同（损坏、过期）才原子写，不复用坏文件。并发写的是同样的内容，所以不在 items 锁里做。
/// 删除工作区时一并删掉；不再对应任何定位框的旧目录在保存与启动时清理（`fixture::prune_fixture_dirs`）。
/// 返回模板目录（UTF-8 绝对路径，非 UTF-8 时报错，不做有损转换）。
/// 框的模板来源必须就是这张冻结图（见 `fixture_source`）：重新冻结后旧框不会悄悄裁到新图上。
fn ensure_template(host: &WorkspaceHost, recipe_id: &str, frozen: &FrozenImage, fixture: &crate::recipe::FixtureSpec, image: Option<&FrameImage>) -> Result<String, String> {
    check_source(frozen, fixture)?;
    let rect = fixture.rect;
    fixture.check_image_size(frozen.size)?;
    let source = image_file(host, recipe_id, &frozen.id)?;
    // 来源就是这张冻结图（check_source），目录名只由 (来源, 框, 外圈) 决定
    let dir = host.dir(recipe_id).join("fixtures").join(fixture::template_dir_name(fixture));
    let loaded;
    let image = match image {
        Some(image) => image,
        None => { loaded = crate::image_file::load(&source)?; &loaded }
    };
    if [image.width, image.height] != frozen.size { return Err("示教原图尺寸与冻结记录不一致".into()); }
    let bytes = vision::fixture::template_pgm(image, rect)?;
    let path = dir.join(vision::fixture::TEMPLATE_FILE);
    let current = || std::fs::read(&path).is_ok_and(|on_disk| on_disk == bytes);
    if !current() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建定位模板目录失败：{e}"))?;
        let tmp = dir.join(format!("{}.tmp-{}-{}", vision::fixture::TEMPLATE_FILE, std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(&tmp, &bytes).map_err(|e| format!("写定位模板失败：{e}"))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            // 另一处同时写了同样的内容，或文件正被 lyFlow 打开着、内容已经对：照常用
            if !current() { return Err(format!("写定位模板失败：{e}")); }
        }
    }
    dir.to_str().map(str::to_string).ok_or_else(|| format!("定位模板目录不是有效的 UTF-8 路径：{}", dir.display()))
}

/// 拍照点 k 各启用且用特征定位的视图的模板目录（按候选当前的冻结示教原图裁出）。
/// `loaded`：调用方已读入的冻结示教原图（按视图），免得再读一遍；样本图、历史原图不能放在这里。会读图、写文件，不在 items 锁里调用。
fn fixture_bases(host: &WorkspaceHost, w: &Workspace, recipe: &Recipe, k: usize, loaded: &[(u8, &FrameImage)]) -> Result<BTreeMap<u8, String>, String> {
    let shot = recipe.shots.get(k).ok_or("拍照点不存在")?;
    let frame = w.frames.get(k).ok_or("拍照点不存在")?;
    let mut bases = BTreeMap::new();
    for view in shot.enabled_views() {
        let Some(fixture) = shot.for_view(view)?.fixture else { continue };
        let frozen = fixture_source(frame, view, &fixture)?;
        let image = loaded.iter().find(|(v, _)| *v == view).map(|(_, image)| *image);
        bases.insert(view, ensure_template(host, &recipe.id, frozen, &fixture, image)?);
    }
    Ok(bases)
}

/// 视图 view 当前的冻结示教原图。
fn frozen_view(frame: &Teaching, view: u8) -> Option<&FrozenImage> {
    frame.views.iter().chain(frame.image.as_ref()).find(|i| i.view == view)
}

fn check_source(frozen: &FrozenImage, fixture: &crate::recipe::FixtureSpec) -> Result<(), String> {
    if fixture.source != frozen.id {
        return Err(format!("定位框是在示教图 {} 上保存的，当前示教图是 {}：请确认定位框后重新保存示教参数", fixture.source, frozen.id));
    }
    Ok(())
}

/// 用框的模板来源找冻结图：来源必须是视图 view 当前的冻结示教原图。重新冻结、整圈采集（含继承示教）、
/// 恢复示教都不改写来源，旧框在这里被拦下，要在示教页确认框后重新保存示教参数。
pub(super) fn fixture_source<'a>(frame: &'a Teaching, view: u8, fixture: &crate::recipe::FixtureSpec) -> Result<&'a FrozenImage, String> {
    let frozen = frozen_view(frame, view).ok_or("含特征定位的图需要先冻结示教原图")?;
    check_source(frozen, fixture)?;
    Ok(frozen)
}

/// 模板来源由后端掌管，前端传来的 source 一律不用：
/// - `confirm` 指定的（拍照点 k, 图）：显式保存示教参数，强制改绑到这幅图当前的冻结原图（没冻结就报错）；
/// - 其余 fixture：忽略来源后与旧候选同一拍照点、同一幅图的 fixture 相同，沿用旧来源；新增或改动的绑到这幅图当前的冻结原图。
///
/// 草稿自动保存、整份保存不会把没动过的旧框改绑到重新冻结的新图上，使用时的 `fixture_source` 检查才有意义。
/// 拍照点级与视图级的 fixture 都处理（拍照点级跟随所选视图）。
pub(super) fn bind_fixture_sources(w: &Workspace, doc: &mut RecipeDoc, confirm: Option<(usize, u8)>) -> Result<(), String> {
    for (k, shot) in doc.shots.iter_mut().enumerate() {
        let old_k = w.doc.shots.iter().position(|old| old.id == shot.id);
        let old = old_k.map(|k| &w.doc.shots[k]);
        let frame = old_k.and_then(|k| w.frames.get(k));
        let id = shot.id.clone();
        let mut slots: Vec<(u8, &mut Option<crate::recipe::FixtureSpec>)> = vec![(shot.view, &mut shot.fixture)];
        slots.extend(shot.views.iter_mut().map(|v| (v.view, &mut v.fixture)));
        for (view, slot) in slots {
            let Some(fixture) = slot.as_mut() else { continue };
            let same = |previous: Option<&crate::recipe::FixtureSpec>| previous
                .filter(|p| crate::recipe::FixtureSpec { source: p.source.clone(), ..fixture.clone() } == **p).map(|p| p.source.clone());
            let kept = old.filter(|_| confirm != Some((k, view))).and_then(|old| {
                same(old.for_view(view).ok().and_then(|s| s.fixture).as_ref()).or_else(|| same(old.fixture.as_ref().filter(|_| old.view == view)))
            });
            fixture.source = match kept {
                Some(source) => source,
                None => frame.and_then(|f| frozen_view(f, view)).map(|i| i.id.clone())
                    .ok_or_else(|| format!("拍照点 {id} 图 {view}：含特征定位的图需要先冻结示教原图"))?,
            };
        }
    }
    Ok(())
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
    w.require_single_frame_source()?;
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
    let (camera_tag, _calib_tag) = tags(app, &r, k)?;
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
        image_recipe.shots[k].calib = r.shots[k].views.iter().find(|entry| entry.view == v).and_then(|entry| entry.calib.clone());
        let (_, view_calib_tag) = tags(app, &image_recipe, k)?;
        let image_id = format!("{captured_at}-{revision}-{k}-{seq}-v{v}");
        let path = image_file(&host, id, &image_id)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        crate::image_file::save_pgm_new(&path, &image)?;
        frozen.push(FrozenImage {
            id: image_id,
            view: v,
            source: source.clone(),
            captured_at,
            size: [image.width, image.height],
            camera: camera.clone(),
            camera_tag: camera_tag.clone(),
            calib_tag: view_calib_tag,
            geometry_tag: shot_tags(&image_recipe, k)?.0,
            exposure_us: None,
            gain_db: None,
            history_id,
        });
    }
    frame.image = frozen.iter().find(|i| i.view == shot.view).cloned();
    frame.views = frozen;
    frame.view_states.clear();
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
    workflow::initialize_views(&mut w, k);
    workflow::store_active(&mut w, k);
    w.capture_id = None;
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
    if !w.doc.is_burst() { return Err("旧拍照点配方已停用，请创建段配方".into()); }
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
    if let Some((_, message)) = w.group_selection_errors().into_iter().next() { return Err(message); }
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
        frames: w.frames.clone(),
        overview: w.overview.clone(),
        validation: validated.clone(),
        validation_site: bundle.manifest.validation_site,
    }));
    next.publish = PublishState::Waiting;
    host.save(&next)?;
    *w = next;
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w.clone())
}

fn commit(app: &AppHandle, host: &WorkspaceHost, release: &Release) -> Result<Arc<Recipe>, String> {
    release.check_group_selection()?;
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
    pub shot_count: usize,
}

pub fn draft_summaries(app: &AppHandle) -> Vec<DraftSummary> {
    let Some(host) = app.try_state::<WorkspaceHost>() else { return Vec::new() };
    let items = host.items.lock().unwrap();
    items.values().map(|w| DraftSummary { id: w.doc.id.clone(), name: w.doc.name.clone(), version: w.doc.version,
        product_code: w.doc.product_code, shot_count: w.doc.shots.len() }).collect()
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
        let previous_plan = app.state::<CycleHost>().recipe(&id).map(|r| r.plan_version);
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
                } else if saved.is_burst() {
                    // 段配方：shotCount / deviceShots = ΣN_exp（决策补充第二节）；重新标定使 ΣN_exp 变了就换计划版本（默认做法 6）
                    let total = saved.expected_total().map_or("未标定".to_string(), |n| n.to_string());
                    match previous_plan {
                        Some(previous) if previous != saved.plan_version => format!(
                            "{id} v{} 已生效；计划已变，PLC 计划版本 {previous} → {}，PLC 侧同步 planVersion 与 shotCount / deviceShots（ΣN_exp = {total}）后才能布防",
                            saved.version, saved.plan_version),
                        None => format!("{id} v{} 已生效；PLC 计划版本 {}，shotCount / deviceShots = ΣN_exp = {total}", saved.version, saved.plan_version),
                        _ => format!("{id} v{} 已生效（计划未变，ΣN_exp = {total}，PLC 不用改）", saved.version),
                    }
                } else {
                    match previous_plan {
                        Some(previous) if previous != saved.plan_version => format!(
                            "{id} v{} 已生效；拍照计划已变，PLC 计划版本 {previous} → {}，PLC 侧同步 planVersion 与拍照点数后才能布防",
                            saved.version, saved.plan_version),
                        None => format!("{id} v{} 已生效；PLC 计划版本 {}", saved.version, saved.plan_version),
                        _ => format!("{id} v{} 已生效（拍照计划未变，PLC 不用改）", saved.version),
                    }
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawFrame {
    pub error: Option<String>,
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
    pub image_source: &'static str,
    pub frames: Vec<RawFrame>,
    pub complete: bool,
    pub message: String,
}

fn recorded(app: &AppHandle, history_id: i64) -> Result<(PathBuf, Vec<RawFrame>), String> {
    let detail = app.state::<Store>().detail(history_id)?;
    let cycle_id = detail.summary.cycle_id.as_deref().ok_or("历史记录没有 cycleId，不能确定对应原图")?;
    let root = app.state::<CycleHost>().recorders.root(detail.summary.source).to_path_buf();
    let frames = recorded::frames(&root, cycle_id, &detail.shots, detail.summary.ts)?;
    Ok((root, frames))
}

fn history_image(app: &AppHandle, detail: &store::PartDetail, k: usize, view: u8) -> Result<FrameImage, String> {
    if detail.summary.source == store::RecordSource::OfflineCandidate {
        return history_samples(app, detail)?.load(k, view);
    }
    let cycle_id = detail.summary.cycle_id.as_deref().ok_or("历史记录没有 cycleId")?;
    let shot = detail.shots.iter().find(|shot| shot.k == k).ok_or("历史拍照点不存在")?;
    let raw = shot.raw_files.iter().find(|raw| raw.view == view)
        .ok_or("所选拍照点视角的原图未保留")?;
    recorded::load_verified(app.state::<CycleHost>().recorders.root(detail.summary.source), cycle_id, shot, raw)
}

fn history_samples(app: &AppHandle, detail: &store::PartDetail) -> Result<history_samples::HistorySamples, String> {
    let revision = detail.summary.recipe_revision.as_deref().ok_or("历史配方快照身份缺失")?;
    let recipe = app.state::<Store>().recipe_snapshot(revision)?.ok_or("历史配方快照缺失，不能确定离线样本原图")?;
    history_samples::HistorySamples::open(&crate::storage::root(app), detail, &recipe)
}

#[tauri::command]
pub fn workspace_record_images(app: AppHandle, history_id: i64) -> Result<RecordImages, String> {
    let store = app.state::<Store>();
    let detail = store.detail(history_id)?;
    if detail.summary.source == store::RecordSource::OfflineCandidate {
        let (frames, message) = match history_samples(&app, &detail) {
            Ok(samples) => {
                let frames = samples.frames();
                let count = frames.iter().filter(|frame| frame.available).count();
                let errors: Vec<_> = frames.iter().filter_map(|frame| frame.error.as_deref()).collect();
                let message = format!("离线样本原图 {count} 幅{}", if errors.is_empty() { String::new() } else { format!("；{}", errors.join("；")) });
                (frames, message)
            }
            Err(error) => (Vec::new(), error),
        };
        let complete = !detail.shots.is_empty() && detail.shots.iter().all(|shot| frames.iter()
            .any(|frame| frame.k == shot.k && frame.view == shot.view && frame.available));
        return Ok(RecordImages { history_id, image_source: "offlineSample", frames, complete, message });
    }
    match recorded(&app, history_id) {
        Ok((_, frames)) => {
            let complete = detail.recording.available && !detail.shots.is_empty()
                && detail.shots.iter().all(|shot| frames.iter().any(|f| f.k == shot.k && f.view == shot.view && f.available));
            let count = frames.iter().filter(|f| f.available).count();
            Ok(RecordImages {
                history_id,
                image_source: "recording",
                frames,
                complete,
                message: format!("保留 {count} 幅原始图像{}", if detail.recording.errors.is_empty() { String::new() } else { format!("；{}", detail.recording.errors.join("；")) }),
            })
        }
        Err(e) => Ok(RecordImages {
            history_id,
            image_source: "recording",
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
    view: Option<u8>,
) -> Result<tauri::ipc::Response, String> {
    let detail = app.state::<Store>().detail(history_id)?;
    let selected = view.or_else(|| detail.shots.iter().find(|shot| shot.k == k).map(|shot| shot.view)).ok_or("拍照点不存在")?;
    Ok(preview_response(&history_image(&app, &detail, k, selected)?))
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
        .recipe_revision
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
    if !same_frame_sources(&doc, &original) {
        return Err("历史帧的 Pose、相机或视角与当前候选不同，不能直接用于示教".into());
    }
    let selected_view = doc.shots.get(k).ok_or("拍照点不存在")?.view;
    keep_image(
        &app,
        &id,
        revision,
        k,
        history_image(&app, &detail, k, selected_view)?,
        format!("{} SN {}", if detail.summary.source == store::RecordSource::OfflineCandidate { "离线样本原图" } else { "历史" }, detail.summary.sn),
        Some(history_id),
    )
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
        if detail.burst.is_some() || original.is_burst() || recipe.is_burst() {
            // 段配方记录按 (计划相机下标, seq) 从原图复测或按已存每帧指标重判（D3）；以下是旧拍照点记录，行为不变
            let (judgement, burst) = burst_history::compare(&app, &w, &recipe, &detail, &original, raw)?;
            let comparison = Comparison {
                id: comparison_id(&app, history_id), history_id, source: if raw { "raw".into() } else { "rules".into() },
                cycle_id: detail.summary.cycle_id.clone().ok_or("历史记录没有工件身份")?, bundle_id: detail.summary.bundle_id.clone(),
                candidate_id: id.clone(), candidate_revision: revision, candidate_recipe: recipe, original_verdict: detail.summary.verdict,
                judgement, measurements: Vec::new(), created_at: ly_plc::now_ms(), burst: Some(burst),
            };
            save_comparison(&app, &comparison)?;
            return Ok(comparison);
        }
        if (raw && !same_frame_sources(&original, &recipe)) || (!raw && !crate::store::same_measurement_layout(&original, &recipe)) {
            return Err(if raw { "历史原图的 Pose、相机或视角与候选不同；请重新采集样本" }
                else { "保存的测点布局与候选不同，不能复用旧测量值；请从原图复测" }.into());
        }
        let mut measurements = Vec::new();
        let table = if raw {
            if detail.summary.source == store::RecordSource::OfflineCandidate {
                let samples = history_samples(&app, &detail)?;
                let (table, measured) = measure_images_from(&app, &w, recipe.shot_count(), &detail.summary.sn,
                    MeasurementSource::HistorySample, |k, v| samples.load(k, v))?;
                measurements = measured;
                table
            } else {
                if !detail.recording.available { return Err("原图未完整保留，无法完成整件复测".into()); }
                let (table, measured) = measure_images(&app, &w, recipe.shot_count(), &detail.summary.sn,
                    |k, v| history_image(&app, &detail, k, v))?;
                measurements = measured;
                table
            }
        } else {
            let points = detail
                .points
                .as_ref()
                .ok_or("历史记录没有完整测量数据，不能重判")?;
            let table = points.table();
            if table.len() != recipe.point_count() || table.iter().any(|p| matches!(p, PointState::Invalid | PointState::Pending)) {
                return Err("历史测量未完成；缺失的测量不能靠修改规则补齐".into());
            }
            table.to_vec()
        };
        let comparison = Comparison {
            id: comparison_id(&app, history_id),
            history_id,
            source: if raw { "raw".into() } else { "rules".into() },
            cycle_id: detail.summary.cycle_id.clone().ok_or("历史记录没有工件身份")?,
            bundle_id: detail.summary.bundle_id.clone(),
            candidate_id: id.clone(),
            candidate_revision: revision,
            candidate_recipe: recipe.clone(),
            original_verdict: detail.summary.verdict,
            judgement: judge::judge(&recipe, &table),
            measurements,
            created_at: ly_plc::now_ms(),
            burst: None,
        };
        save_comparison(&app, &comparison)?;
        Ok(comparison)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn same_frame_sources(original: &Recipe, candidate: &Recipe) -> bool {
    let sources = |recipe: &Recipe| json!([recipe.id, recipe.shots.iter().map(|shot|
        json!([shot.id, shot.pose_id, shot.camera])).collect::<Vec<_>>()]);
    sources(original) == sources(candidate)
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
        if original.is_burst() {
            // 段配方：按 (计划相机下标, seq) 读录制原图、用原发布包的走廊逐帧重现（D3）；以下是旧拍照点记录，行为不变
            let (judgement, burst) = burst_history::reproduce_original(&app, &detail, &prepared, &cycle_id)?;
            let comparison = Comparison { id: comparison_id(&app, history_id), history_id, source: "original".into(),
                cycle_id, bundle_id: Some(bundle_id.into()), candidate_id: original.id.clone(),
                candidate_revision: 0, candidate_recipe: original, original_verdict: detail.summary.verdict,
                judgement, measurements: Vec::new(), created_at: ly_plc::now_ms(), burst: Some(burst) };
            save_comparison(&app, &comparison)?;
            return Ok(comparison);
        }
        let (judgement, measurements) = reproduce::measure(&prepared, original.shot_count(), &detail.summary.sn, &cycle_id,
            |k, selected| history_image(&app, &detail, k, selected))?;
        let comparison = Comparison { id: comparison_id(&app, history_id), history_id, source: "original".into(),
            cycle_id, bundle_id: Some(bundle_id.into()), candidate_id: original.id.clone(),
            candidate_revision: 0, candidate_recipe: original, original_verdict: detail.summary.verdict,
            judgement, measurements, created_at: ly_plc::now_ms(), burst: None };
        save_comparison(&app, &comparison)?;
        Ok(comparison)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn workspace_runtime_overview(
    app: AppHandle,
    host: State<'_, WorkspaceHost>,
    id: String,
    revision_id: String,
) -> Result<Option<Overview>, String> {
    safe_id(&id)?;
    if let Some(candidate) = crate::offline::last_candidate(&app).filter(|c| c.recipe.id == id && c.recipe.revision_id == revision_id) { return Ok(Some(candidate.overview.clone())); }
    if revision_id.is_empty() || !revision_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err("配方版本编号无效".into());
    }
    let version = revision_id.strip_prefix(&format!("{id}-v")).and_then(|v| v.parse::<u32>().ok()).ok_or("配方版本编号与配方不一致")?;
    archived_release(&host.dir(&id).join("published"), &id, version).map(|release| Some(release.overview))

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
        *host.live.lock().unwrap() = LiveFrames::default();
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

pub fn retain_live(
    app: &AppHandle,
    cycle_id: String,
    revision_id: &str,
    k: usize,
    image: &Option<Arc<FrameImage>>,
) {
    if let (Some(host), Some(image)) = (app.try_state::<WorkspaceHost>(), image) {
        host.live.lock().unwrap().insert(&cycle_id, revision_id, k, image.clone());
    }
}

#[tauri::command]
pub fn workspace_live_image(
    host: State<'_, WorkspaceHost>,
    cycle_id: String,
    revision_id: String,
    k: usize,
) -> Result<tauri::ipc::Response, String> {
    let image = {
        let live = host.live.lock().unwrap();
        if live
            .key
            .as_ref()
            .is_none_or(|key| key.0 != cycle_id || key.1 != revision_id)
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

fn measure_images(app: &AppHandle, w: &Workspace, count: usize, sn: &str,
    load: impl FnMut(usize, u8) -> Result<FrameImage, String>) -> Result<(Vec<PointState>, Vec<Value>), String> {
    measure_images_from(app, w, count, sn, MeasurementSource::CurrentDevices, load)
}

enum MeasurementSource { CurrentDevices, HistorySample }

fn measure_images_from(app: &AppHandle, w: &Workspace, count: usize, sn: &str, source: MeasurementSource,
    mut load: impl FnMut(usize, u8) -> Result<FrameImage, String>) -> Result<(Vec<PointState>, Vec<Value>), String> {
    let recipe = w.doc.build()?;
    if count != recipe.shot_count() { return Err("样本组的原图数量与拍照点不一致".into()); }
    let engine = app.state::<vision::VisionHost>().engine().ok_or("lyFlow 核心库未加载")?;
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut measurements = Vec::new();
    let run = vision::unique_run_id(&format!("candidate-{}", recipe.revision_id));
    for k in 0..count {
        let shot = &recipe.shots[k];
        if !shot.measured() { let _ = load(k, shot.view)?; continue; }
        // 模板一律按候选当前的冻结示教原图裁（复测历史原图也一样），样本图只用来测
        let bases = fixture_bases(&app.state::<WorkspaceHost>(), w, &recipe, k, &[])?;
        let mut images = Vec::new();
        for selected in workflow::selected_views(w, k) {
            match source {
                MeasurementSource::CurrentDevices => workflow::check_view(app, w, k, selected, false)?,
                MeasurementSource::HistorySample => workflow::check_history_sample_view(app, w, k, selected)?,
            }
            let frozen = w.frames[k].views.iter().find(|i| i.view == selected).ok_or("示教原图缺失")?;
            let image = load(k, selected)?;
            if [image.width, image.height] != frozen.size { return Err(format!("拍照点 {} 图 {selected} 样本尺寸与示教不一致", shot.id)); }
            images.push((selected, Arc::new(image)));
        }
        let result = vision::measure_views(&engine, &recipe, k, &images, &format!("{run}-{k}"), &bases)?;
        let reference_failed = result.reference_failed();
        let reading = result.reading;
        for (i, &j) in reading.idx.iter().enumerate() {
            table[j as usize] = crate::measure::point_state(reading.st[i], reading.d[i], reading.w[i]);
        }
        let cam = match source {
            MeasurementSource::CurrentDevices => app.state::<CycleHost>().camera.require(&shot.camera)?,
            MeasurementSource::HistorySample => recipe.cameras().iter().position(|camera| *camera == shot.camera)
                .and_then(|index| u8::try_from(index).ok()).ok_or("历史样本的配方相机索引无效")?,
        };
        measurements.push(json!({"cycleId":run,"bundleId":null,"shotId":shot.id,"camera":shot.camera,
            "sn":sn,"k":k,"cam":cam,"referenceFailed":reference_failed,"score":reading.coverage,"ms":reading.ms,
            "error":null,"idx":reading.idx,"d":reading.d,"w":reading.w,"st":reading.st,"px":reading.px,"views":result.views}));
    }
    Ok((table, measurements))
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

/// 候选试运行冻结的配方，按配方类型分流（决策记录第九节第 14 条）：段配方过 `Recipe::ready_burst`（三台相机走廊与当量、
/// 整件 ΣN_exp）；旧拍照点配方过组选图与 `Recipe::ready`，行为不变。修订号为候选调试修订，计划版本至少为 1。
fn debug_recipe(w: &Workspace) -> Result<Recipe, String> {
    if let Some((_, error)) = w.group_selection_errors().into_iter().next() { return Err(error); }
    let mut recipe = w.doc.build()?;
    if recipe.is_burst() {
        recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
    } else {
        recipe.ready()?;
    }
    recipe.revision_id = w.debug_revision()?;
    if recipe.plan_version == 0 { recipe.plan_version = 1; }
    Ok(recipe)
}

/// 候选试运行的示教图尺寸：旧拍照点配方按 (拍照点, 图)，交给 `Candidate::new`；段配方按计划相机顺序，交给
/// `Candidate::new_burst`（载荷类型不同，两种尺寸不会交错传给另一种构造）。
enum DebugSizes {
    Legacy(HashMap<(usize, u8), [u32; 2]>),
    Burst(Vec<[u32; 2]>),
}

/// 候选试运行按配方类型分派示教图尺寸（`workspace_debug_run` 在锁内调用）：段配方按计划相机顺序取各相机示教帧尺寸
/// （`burst_release::debug_sizes`，含示教后设备标签核对）；旧拍照点配方按 (拍照点, 图) 取冻结示教原图尺寸，每个选中视图先过
/// `check_view`（旧配方的视图检查），行为不变。
fn debug_run_sizes(w: &Workspace, recipe: &Recipe, current: &BTreeMap<String, Value>,
    mut check_view: impl FnMut(usize, u8) -> Result<(), String>) -> Result<DebugSizes, String> {
    if recipe.is_burst() { return Ok(DebugSizes::Burst(burst_release::debug_sizes(w, current)?)); }
    let mut sizes = HashMap::new();
    for (k, shot) in recipe.shots.iter().enumerate() {
        if !shot.measured() { continue; }
        for selected in workflow::selected_views(w, k) {
            check_view(k, selected)?;
            let frozen = w.frames[k].views.iter().find(|i| i.view == selected).ok_or("示教原图缺失")?;
            sizes.insert((k, selected), frozen.size);
        }
    }
    Ok(DebugSizes::Legacy(sizes))
}

#[tauri::command]
pub async fn workspace_debug_run(app: AppHandle, id: String, revision: u64, part_id: Option<String>) -> Result<(), String> {
    if !crate::offline::enabled(&app) || crate::offline::sample_set(&app).is_none() { return Err("候选试运行仅支持离线样本集模式".into()); }
    let set = crate::offline::sample_set(&app).ok_or("离线样本集缺失")?;
    let current = burst_release::current_camera_tags(&app);
    let (recipe, sizes, w) = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        let w = items.get(&id).ok_or("候选配方不存在")?;
        if w.revision != revision { return Err("候选已修改，请刷新后重试".into()); }
        if w.pending.is_some() { return Err("候选正在等待发布，请先完成或取消发布".into()); }
        let recipe = debug_recipe(w)?;
        let sizes = debug_run_sizes(w, &recipe, &current, |k, selected| workflow::check_view(&app, w, k, selected, false))?;
        (recipe, sizes, w.clone())
    };
    let candidate = match sizes {
        // 段配方：每台计划相机按示教帧尺寸建走廊测量图，结果只量不判（D3 的候选调试记录按样本集读原图）
        DebugSizes::Burst(sizes) => {
            let engine = app.state::<vision::VisionHost>().engine().ok_or("lyFlow 核心库未加载")?;
            crate::candidate::Candidate::new_burst(recipe, revision, engine, &sizes, w.overview.clone())?
        }
        DebugSizes::Legacy(sizes) => {
            // 定位模板在锁外按冻结示教原图生成；目录按内容寻址，之后 recheck_debug_candidate 核对冻结图即可
            let mut bases = HashMap::new();
            for k in 0..recipe.shot_count() {
                if !recipe.shots[k].measured() { continue; }
                for (view, base) in fixture_bases(&app.state::<WorkspaceHost>(), &w, &recipe, k, &[])? { bases.insert((k, view), base); }
            }
            let engine = app.state::<vision::VisionHost>().engine().ok_or("lyFlow 核心库未加载")?;
            crate::candidate::Candidate::new(recipe, revision, engine, sizes, bases, w.overview.clone())?
        }
    };
    crate::offline::run_candidate(app, Arc::new(candidate), set, part_id).await
}

pub(crate) fn recheck_debug_candidate(app: &AppHandle, candidate: &crate::candidate::Candidate) -> Result<(), String> {
    let current = candidate.recipe.is_burst().then(|| burst_release::current_camera_tags(app));
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    let w = items.get(&candidate.info.recipe_id).ok_or("候选已删除")?;
    if w.revision != candidate.info.revision || w.pending.is_some() || w.debug_revision()? != candidate.recipe.revision_id { return Err("候选已修改、重建或正在发布，请重新试运行".into()); }
    // 段配方：修订号不变即示教帧与走廊没变，只需再核对示教后设备标签没变；没有拍照点，下面的循环不执行
    if let Some(current) = &current { burst_release::debug_sizes(w, current)?; }
    for (k, shot) in w.doc.shots.iter().enumerate() {
        if shot.skip { continue; }
        for view in workflow::selected_views(w, k) { workflow::check_view(app, w, k, view, false)?; }
    }
    Ok(())
}

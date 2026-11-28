mod admission;
mod burst;
mod daily_stats;
mod interrupted;
mod maintenance_progress;
pub mod readiness;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate};
use ly_plc::{now_ms, PlcEngine};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, UnboundedReceiver, UnboundedSender};

use crate::applog::events::{self, Event};
use crate::applog::throttle::{self, Tally, Throttle};
use crate::applog::{self, Fields};
use crate::audit::{Audit, RecordedPart};
use crate::burst_judge::{BurstJudgement, FrameState};
use crate::camera::{CameraRig, FRAME_QUEUE};
use crate::frame::Frame;
use crate::handshake::Request;
use crate::history;
use crate::judge::{fault, Judgement, Verdict};
use crate::measure::{self, FrameJob, FrameMeasured, FramePool};
use crate::plc::PlcHost;
use crate::plc_session::{
    PartSite, PartTiming, PlcSession, SessionEvent, SessionPhase, SessionView,
};
use crate::recipe::{Recipe, RecipeStore, TriggerMode};
use crate::recorder::{Recorder, Recorders, Recording};
use crate::recording_health::{AlarmChange, RecordingAlarm, RecordingHealth};
use crate::settings::{CycleSettings, ProductSource};
use crate::shot_router::{arm_frames, ArmCam, Floor, FrameMeta, Ledgers, Policy};
use crate::store::{
    HistoryScope, PartMode, PlcDelivery, PlcDeliveryState, RecordSource, Store, VerdictCounts,
};
use daily_stats::DailyStats;
use maintenance_progress::MaintenanceProgress;

pub enum Input {
    /// 段配方一台相机一帧的测量结果（FramePool 交来）
    FrameMeasured(FrameMeasured),
    Refresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Phase {
    Idle,
    Validate,
    Acquire,
    Drain,
    Judge,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FrameStatus {
    Waiting,
    Measuring,
    /// 测完了
    Done,
    /// 测量本身没做成（lyFlow 运行失败、缺示教资料等）
    Error,
    Missing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewResult {
    pub view: u8,
    pub verdict: Verdict,
    pub error: Option<String>,
    pub score: Option<f32>,
    pub ms: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameView {
    #[serde(default)]
    pub view_results: Vec<ViewResult>,
    pub status: FrameStatus,
    #[serde(default)]
    pub cam: u8,
    /// 相机编号（相机组序号会随增删相机变，编号不变）
    #[serde(default)]
    pub camera: String,
    #[serde(default)]
    pub shot_id: String,
    #[serde(default)]
    pub view: u8,
    #[serde(default)]
    pub session: Option<u64>,
    #[serde(default)]
    pub ordinal: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    pub arrived_ms: Option<u64>,
    pub frame_counter: Option<u64>,
    pub trigger_counter: Option<u64>,
    pub counter_jump: bool,
    pub score: Option<f32>,
    pub points: usize,
    pub gap_points: usize,
    pub ms: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartView {
    #[serde(skip)]
    settled_json: Option<OnceLock<Option<Box<RawValue>>>>,
    pub cycle_id: String,
    pub bundle_id: Option<String>,
    pub sn: String,
    pub recipe_id: String,
    /// 本件配方修订：配方中途改了，界面仍按这一版画
    pub recipe_revision: String,
    /// 计划帧数
    pub n: usize,
    pub received: usize,
    pub triggers: u64,
    pub queue: usize,
    pub measured_frames: usize,
    /// 段配方在途件（决策记录 5.2）
    pub burst: PartViewBurst,
}

/// 在线帧格状态：到达前 waiting，测量中 measuring，之后为帧的最终状态（`FrameState`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FrameCellStatus {
    Waiting,
    Measuring,
    Measured,
    Missing,
    Error,
    Unmeasured,
    OutOfCorridor,
    NotInspected,
}

impl From<FrameState> for FrameCellStatus {
    fn from(state: FrameState) -> Self {
        match state {
            FrameState::Measured => Self::Measured,
            FrameState::Missing => Self::Missing,
            FrameState::Error => Self::Error,
            FrameState::Unmeasured => Self::Unmeasured,
            FrameState::OutOfCorridor => Self::OutOfCorridor,
            FrameState::NotInspected => Self::NotInspected,
        }
    }
}

/// 一台相机一帧的在线状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCellView {
    pub status: FrameCellStatus,
    /// 这一帧单独判的结论；没测成或未测完为 None
    pub verdict: Option<Verdict>,
    /// 测过、已判定，但没有单帧结论（整件故障判 ERR、没有区域）：与还在测量中区分
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unjudged: bool,
    pub frame_counter: Option<u64>,
    /// 测量所用走廊的段；收尾补测换了走廊时随之变，检测视图据此重读叠图
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum FrameGroupCells {
    Inline([FrameCellView; crate::plan::PLAN_CAMERAS]),
    Dynamic(Vec<FrameCellView>),
}

impl FrameGroupCells {
    fn from_cells(mut cells: impl ExactSizeIterator<Item = FrameCellView>) -> Self {
        if cells.len() == crate::plan::PLAN_CAMERAS {
            Self::Inline(std::array::from_fn(|_| cells.next().expect("帧格数已核对")))
        } else {
            Self::Dynamic(cells.collect())
        }
    }
}

impl std::ops::Deref for FrameGroupCells {
    type Target = [FrameCellView];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Inline(cells) => cells,
            Self::Dynamic(cells) => cells,
        }
    }
}

/// 一个帧组：cells 按计划相机顺序。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameGroupView {
    pub seq: u64,
    pub incomplete: bool,
    pub cells: FrameGroupCells,
}

/// 段配方在途件（IPC 契约见 docs/decisions/segment-burst.md 5.2）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartViewBurst {
    pub cameras: Vec<String>,
    pub expected_total: Option<u32>,
    /// partEnd 时 PLC 报的 deviceTriggers，只作参考；partEnd 前为 None
    pub device_triggers: Option<u32>,
    /// 已见帧组数 = 各相机最大 seq 的最大值
    pub group_count: u64,
    /// 各相机已接受的帧数
    pub camera_frames: Vec<u32>,
    pub measured_frames: usize,
    pub pending_measure: usize,
    pub groups: Vec<FrameGroupView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultView {
    pub cycle_id: Option<String>,
    pub sn: String,
    pub recipe_id: Option<String>,
    pub ts: i64,
    pub drain_ms: Option<u64>,
    pub plc_write: Option<PlcWriteTrace>,
    #[serde(flatten)]
    pub judgement: Judgement,
    /// 段配方的判定；有值时 judgement 的 verdict / plc_code / fault_code / reason 与它相同。没有配方（拒收）时为 None
    pub burst: Option<BurstJudgement>,
    pub mode: crate::store::PartMode,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlcWriteState {
    Written,
    Failed,
    NotWritten,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlcWriteTrace {
    #[serde(flatten)]
    timing: PartTiming,
    state: PlcWriteState,
    updated_at: i64,
    message: Option<String>,
}

/// 当日计数，跨天清零。
#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub total: u64,
    pub ok: u64,
    pub ng: u64,
    pub err: u64,
}

impl From<VerdictCounts> for Stats {
    fn from(c: VerdictCounts) -> Self {
        Self {
            total: c.ok + c.excursion + c.ng + c.err,
            ok: c.ok + c.excursion,
            ng: c.ng,
            err: c.err,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub stats_scope: HistoryScope,
    pub measurement_workers: measure::WorkerHealthSnapshot,
    pub phase: Phase,
    pub plc_locked: bool,
    pub plc_handshake: Option<SessionView>,
    pub since: i64,
    pub fault: Option<String>,
    pub product_source: ProductSource,
    pub active_recipe_id: Option<String>,
    pub trigger_mode: Option<TriggerMode>,
    #[serde(serialize_with = "serialize_snapshot_part")]
    pub part: Option<Arc<PartView>>,
    pub result: Option<ResultView>,
    pub stats: Stats,
    pub stray_frames: u64,
    pub alarms: Vec<String>,
    pub recording_alarm: Option<RecordingAlarm>,
    pub maintenance: Option<crate::settings::Maintenance>,
    /// 维护采集进度，按配方
    pub collection: Vec<crate::store::CollectionProgress>,
    /// 维护试判统计，按配方（最新正式版本）
    pub trial: Vec<crate::store::TrialProgress>,
}

fn serialize_snapshot_part<S: serde::Serializer>(
    part: &Option<Arc<PartView>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let raw = part.as_deref().and_then(|view| {
        view.settled_json
            .as_ref()?
            .get_or_init(|| serde_json::value::to_raw_value(view).ok())
            .as_deref()
    });
    match raw {
        Some(raw) => serializer.serialize_some(raw),
        None => part.serialize(serializer),
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    production_busy: bool,
    plc_locked: bool,
    plc_handshake: Option<SessionView>,
    maintenance: Option<crate::settings::Maintenance>,
}

impl Snapshot {
    fn production_busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Fault)
    }

    fn control_state(&self) -> ControlState {
        ControlState {
            production_busy: self.production_busy(),
            plc_locked: self.plc_locked,
            plc_handshake: self.plc_handshake.clone(),
            maintenance: self.maintenance.clone(),
        }
    }

    fn control_state_changed(&self, previous: &Self) -> bool {
        self.production_busy() != previous.production_busy()
            || self.plc_locked != previous.plc_locked
            || self.plc_handshake != previous.plc_handshake
            || self.maintenance != previous.maintenance
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeSummary {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub revision_id: String,
    pub product_code: u16,
    pub trigger_mode: TriggerMode,
    pub cameras: Vec<String>,
    pub length: f32,
    /// PLC 触发次数 ΣN_exp（未标定为空）
    pub trigger_count: Option<usize>,
    pub segment_count: usize,
    /// 整件预期帧组数 ΣN_exp；未标定为空
    pub expected_total: Option<u32>,
    /// 已标定（expected_total 非空）
    pub calibrated: bool,
    /// 示教件来源（RB-7）：取自生产配方文档；旧流程发布的配方为空
    pub teaching_piece: Option<crate::recipe::TeachingPieceRef>,
}

impl RecipeSummary {
    /// 配方库里一份生产配方的摘要，带配方文档里的示教件来源。
    pub fn listed(store: &RecipeStore, r: &Recipe) -> Self {
        Self {
            teaching_piece: store.doc(&r.id).and_then(|d| d.teaching_piece),
            ..Self::from(r)
        }
    }
}

impl From<&Recipe> for RecipeSummary {
    fn from(r: &Recipe) -> Self {
        let expected_total = r.expected_total();
        Self {
            id: r.id.clone(),
            name: r.name.clone(),
            version: r.version,
            revision_id: r.revision_id.clone(),
            product_code: r.product_code,
            trigger_mode: r.trigger_mode,
            cameras: r.cameras(),
            // 各段段长之和，段长未知按 0
            length: r
                .path_segments
                .iter()
                .map(|s| s.length_mm.unwrap_or(0.0) as f32)
                .sum(),
            trigger_count: expected_total.map(|n| n as usize),
            segment_count: r.path_segments.len(),
            expected_total,
            calibrated: expected_total.is_some(),
            teaching_piece: None,
        }
    }
}

struct Shared {
    snapshot: Option<Arc<Snapshot>>,
    /// 当前（或刚结束的）一件用的配方快照
    part_recipe: Option<Arc<Recipe>>,
    settings: CycleSettings,
}

pub struct CycleHost {
    pub tx: UnboundedSender<Input>,
    /// PLC 链路门：S7 握手、PLC 配置/连接/手动写点互斥。节拍每个周期都持有。
    /// 两把门都要时先取 link_gate 再取 config_gate；只持 config_gate 时不能再取 link_gate。
    pub link_gate: tokio::sync::Mutex<()>,
    /// 接件与配置门：接件登记、切换生产配方与相机重配置、配方/设置/标定修改互斥。节拍只在接件到登记在途、切换待生效配方时持有。
    pub config_gate: tokio::sync::Mutex<()>,
    /// 工件配方包导入与项目切换互斥，先于 link_gate 或 config_gate 取得。
    pub(crate) recipe_import_gate: Arc<tokio::sync::Mutex<()>>,
    pub camera: CameraRig,
    pub recipes: RecipeStore,
    pub recorders: Recorders,
    pub cycle_ids: crate::cycle_ids::CycleIds,
    pub audit: Audit,
    pub disk: crate::disk_guard::DiskGuard,
    pub recording_health: Arc<RecordingHealth>,
    settings_path: PathBuf,
    /// 本机历史保留天数与空闲压缩开关（`config/history.json`）
    history_path: PathBuf,
    shared: Mutex<Shared>,
    rx: Mutex<Option<(UnboundedReceiver<Input>, Receiver<Frame>)>>,
    /// 有在途工件。
    busy: AtomicBool,
    /// 节拍不在空闲或故障（开工那一刻就置上，比发布的快照早）
    in_flight: AtomicBool,
    /// 检测侧不允许切换项目的原因：有在途工件
    switch_block: Mutex<Option<String>>,
    settings_note: Option<String>,
    interrupted_recordings: usize,
}

impl CycleHost {
    /// 设置页改了录制盘：新件按新列表选盘，滚动清理与孤立录制清理也按新列表。
    pub fn set_record_roots(&self, roots: Vec<PathBuf>) {
        *self.disk.roots().write().unwrap() = roots;
        self.disk.invalidate();
    }

    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let settings_path = crate::storage::project_config(app).join("cycle.json");
        let data = crate::storage::project(app);
        let record_roots: crate::disk_guard::RecordRoots = Arc::new(std::sync::RwLock::new(
            crate::project::records::prepared_roots(&crate::log_storage::recording_projects(
                &data,
            )?),
        ));
        let (tx, rx) = unbounded_channel();
        let (frame_tx, frame_rx) = channel(FRAME_QUEUE);
        let (mut settings, project_note) = CycleSettings::load(&settings_path);
        let history_path = crate::storage::config(app).join("history.json");
        let (history, history_note) =
            crate::settings::load_history(&history_path, settings.history_days);
        settings.history_days = history.history_days;
        settings.idle_compress = history.idle_compress;
        crate::raw_compress::set_enabled(history.idle_compress);
        let settings_note = match (project_note, history_note) {
            (Some(a), Some(b)) => Some(format!("{a}；{b}")),
            (a, b) => a.or(b),
        };
        let audit = Audit::new(
            app,
            crate::project::records::legacy_spool(&crate::storage::root(app)),
        )?;
        let interrupted_recordings = app.state::<Store>().finish_interrupted_recordings()?;
        let camera = CameraRig::new(app, frame_tx)?;
        let recording_health = Arc::new(RecordingHealth::default());
        let recorder = |roots: crate::disk_guard::RecordRoots, scope: HistoryScope| {
            let (guarded_roots, guard_app, protection_audit, record_audit) =
                (roots.clone(), app.clone(), audit.clone(), audit.clone());
            let health = recording_health.clone();
            let protection = Arc::new(move || {
                let store = guard_app
                    .try_state::<Store>()
                    .ok_or("检测记录数据库尚未初始化")?;
                let roots = guarded_roots.read().unwrap().clone();
                let mut paths = store.pending_recording_directories(&roots, scope)?;
                // 离线的盘不清理，也不用核对它上面待入库的录制
                for root in roots.iter().filter(|root| root.is_dir()) {
                    paths.extend(protection_audit.protected_recording_directories(root)?);
                }
                // 示教件、标定件引用的录制不滚动清理
                let referenced =
                    crate::project::records::referenced_cycles(&crate::storage::root(&guard_app))?;
                paths.extend(store.recording_directories(&referenced)?);
                Ok(paths)
            });
            let warning = Arc::new(move |errors: &[String]| {
                log("warn", "录制清理失败", errors.join("；"));
            });
            let release_app = app.clone();
            let release = Arc::new(move |directories: &[PathBuf]| {
                release_app
                    .try_state::<Store>()
                    .ok_or("检测记录数据库尚未初始化")?
                    .release_recordings(directories)
            });
            Recorder::guarded(
                roots,
                Some(Arc::new(
                    move |outcome: crate::recorder::RecordingOutcome| {
                        if let Some(change) = health.observe(scope, &outcome, now_ms()) {
                            log_recording_alarm(change);
                        }
                        record_audit.recording(outcome);
                    },
                )),
                crate::recorder::Cleanup {
                    protection,
                    warning,
                    release,
                },
            )
        };
        let recorders = Recorders::new(recorder(record_roots.clone(), HistoryScope::Real));
        Ok(Self {
            camera,
            tx,
            link_gate: tokio::sync::Mutex::new(()),
            config_gate: tokio::sync::Mutex::new(()),
            recipe_import_gate: Arc::default(),
            recipes: RecipeStore::open_with_floors(
                data.join("recipes"),
                &app.state::<crate::store::Store>().recipe_version_floors()?,
            )?,
            recorders,
            cycle_ids: crate::cycle_ids::CycleIds::default(),
            audit,
            disk: crate::disk_guard::DiskGuard::new(data.clone(), record_roots),
            recording_health,
            shared: Mutex::new(Shared {
                snapshot: None,
                part_recipe: None,
                settings,
            }),
            settings_path,
            history_path,
            rx: Mutex::new(Some((rx, frame_rx))),
            busy: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            switch_block: Mutex::new(None),
            settings_note,
            interrupted_recordings,
        })
    }

    pub fn start(app: &AppHandle) {
        let host = app.state::<CycleHost>();
        let Some((mut rx, mut frames)) = host.rx.lock().unwrap().take() else {
            return;
        };
        let frame_tx = host.tx.clone();
        let frame_pool = measure::spawn_frame_worker(Arc::new(move |measured| {
            let _ = frame_tx.send(Input::FrameMeasured(measured));
        }));
        let mut machine = Machine::new(app.clone(), frame_pool);
        for e in host.recipes.errors() {
            log("err", "配方", e);
        }
        for e in host.camera.notes.iter().chain(&host.settings_note) {
            log("err", "配置", e.clone());
        }
        for e in &app.state::<crate::workspace::WorkspaceHost>().notes {
            log("err", "候选加载", e.clone());
        }
        let store = app.state::<Store>();
        if let Some(path) = store.backup_path() {
            log(
                "warn",
                "记录库重建",
                format!("旧版数据库已备份至 {}，当前使用新版空库", path.display()),
            );
        }
        if host.interrupted_recordings > 0 {
            log(
                "err",
                "录制恢复",
                format!(
                    "上次服务退出时有 {} 件原图尚未确认完整，已标记录制失败；原检测结论保留",
                    host.interrupted_recordings
                ),
            );
        }
        tauri::async_runtime::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            let mut drain_wake = None;
            machine.publish();
            loop {
                tokio::select! {
                    input = rx.recv() => match input {
                        Some(input) => machine.on_input(input).await,
                        None => break,
                    },
                    Some(frame) = frames.recv() => machine.on_frame(frame),
                    _ = async {
                        if let Some(deadline) = drain_wake {
                            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        drain_wake = None;
                        machine.on_tick().await;
                    },
                    _ = tick.tick() => {
                        machine.on_tick().await;
                        drain_wake = match (machine.phase, &machine.part) {
                            (Phase::Drain, Some(part)) => part.next_drain_wake(Instant::now()),
                            _ => None,
                        };
                    },
                }
                // 快照最多 20 次/秒
                if machine.dirty && machine.published.elapsed() >= Duration::from_millis(50) {
                    machine.publish();
                }
            }
        });
    }

    pub fn phase(&self) -> Phase {
        self.shared
            .lock()
            .unwrap()
            .snapshot
            .as_ref()
            .map_or(Phase::Idle, |s| s.phase)
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub fn switch_block(&self) -> Option<String> {
        self.switch_block.lock().unwrap().clone()
    }

    fn store_busy(&self, phase: Phase, s7: &PlcSession) {
        let in_flight = !matches!(phase, Phase::Idle | Phase::Fault);
        if in_flight || s7.pending() {
            crate::raw_compress::activity().touch();
        }
        self.in_flight.store(in_flight, Ordering::SeqCst);
        self.busy.store(in_flight || s7.pending(), Ordering::SeqCst);
        *self.switch_block.lock().unwrap() =
            in_flight.then(|| "检测仍有在途工件，工件结束后再切换项目".into());
    }

    /// 检测中的配方：人工选型号时选中的，或在途工件用的。
    pub fn recipe_in_use(&self, id: &str) -> bool {
        let settings = self.settings();
        let selected = settings.product_source == ProductSource::Manual
            && settings.manual_recipe_id.as_deref() == Some(id);
        self.busy()
            && (selected
                || self
                    .shared
                    .lock()
                    .unwrap()
                    .part_recipe
                    .as_ref()
                    .is_some_and(|r| r.id == id))
    }

    pub fn recipe(&self, id: &str) -> Option<Arc<Recipe>> {
        self.recipes.get(id)
    }

    pub fn settings(&self) -> CycleSettings {
        self.shared.lock().unwrap().settings.clone()
    }

    pub fn save_settings(&self, settings: CycleSettings) -> Result<(), String> {
        let gate = self
            .config_gate
            .try_lock()
            .map_err(|_| "正在接件或处理其他配置操作，请稍后重试")?;
        self.save_settings_locked(settings, &gate)
    }

    pub(crate) fn save_settings_locked(
        &self,
        settings: CycleSettings,
        _gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("在途工件结束后才能修改检测设置".into());
        }
        settings.validate()?;
        let current = self.settings();
        if (settings.history_days, settings.idle_compress)
            != (current.history_days, current.idle_compress)
        {
            crate::settings::save_history(
                &self.history_path,
                &crate::settings::HistorySettings {
                    history_days: settings.history_days,
                    idle_compress: settings.idle_compress,
                },
            )?;
        }
        settings.save(&self.settings_path)?;
        crate::raw_compress::set_enabled(settings.idle_compress);
        self.recorders.apply_retention(&settings);
        self.shared.lock().unwrap().settings = settings;
        let _ = self.tx.send(Input::Refresh);
        Ok(())
    }
}

/// 删除超过保留天数的检测记录（本机全部项目）；非当前项目的记录连同原图一起删，当前项目的原图由录制滚动清理。
pub fn purge_history(app: &AppHandle) {
    let _busy = crate::raw_compress::activity().busy();
    let days = host(app).settings().history_days.max(1) as i64;
    // 引用锁先于追溯状态锁：录制滚动清理持引用锁时会查待入库目录
    let references = crate::recipe_capture::lock_references();
    let purged = host(app)
        .audit
        .with_ready(|| crate::workspace::purge_history(app, now_ms() - days * 86_400_000));
    drop(references);
    match purged {
        Ok((n, warnings)) => {
            if n > 0 {
                log(
                    "info",
                    "记录清理",
                    format!("删除 {days} 天前的 {n} 条检测记录"),
                );
            }
            for warning in warnings {
                log("warn", "复测记录清理", warning);
            }
        }
        Err(e) => log("err", "记录清理", e),
    }
}

/// 启动时补回维护试判件丢失的区域判定（MM-10），幂等：已补回的记录不再列出。
pub fn repair_history(app: &AppHandle) {
    let store = app.state::<crate::store::Store>();
    match host(app)
        .audit
        .with_ready(|| crate::workspace::repair_shadow_regions(&store))
    {
        Ok((n, skipped)) => {
            if n > 0 {
                log(
                    "info",
                    "记录修复",
                    format!("补回 {n} 条维护试判记录的区域判定"),
                );
            }
            for reason in skipped {
                log("warn", "记录修复", reason);
            }
        }
        Err(e) => log("err", "记录修复", e),
    }
}

/// 启动时握手日志里有已绑定身份的在途件，而历史与追溯暂存里都没有它：进程在检测中途退出，补一条 ERR 记录。
fn record_interrupted_part(app: &AppHandle, s7: &PlcSession, stats: &mut DailyStats, now: i64) {
    let (Some(cycle_id), Some(request)) = (s7.cycle_id(), s7.request()) else {
        return;
    };
    let Some(site) = s7.site() else {
        return log(
            "err",
            "中断工件补记录",
            format!("cycleId={cycle_id}：握手日志缺少开工时的场景与录制方式，未补记录"),
        );
    };
    let host = host(app);
    let known = host
        .audit
        .evidenced_cycle_ids()
        .map(|ids| ids.contains(cycle_id))
        .and_then(|pending| {
            let store = app.state::<Store>();
            Ok(pending
                || store.id_by_cycle(cycle_id)?.is_some()
                || store.history_cycle_deleted(cycle_id)?)
        });
    match known {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            return log(
                "err",
                "中断工件补记录",
                format!("cycleId={cycle_id}：核对历史失败，未补记录：{error}"),
            )
        }
    }
    let started_at = s7.started_at();
    let interrupted = interrupted::Interrupted {
        cycle_id,
        sn: &request.sn,
        request_seq: request.request_seq,
        started_at,
        site,
        maintenance: s7.part_maintenance(),
    };
    let part = interrupted::part(
        &interrupted,
        s7.recipe_id().and_then(|id| host.recipe(id)),
        app.package_info().version.to_string(),
        now,
    );
    let label = site.source.label();
    stats.record(part.mode, part.ts, part.judgement.verdict);
    let outcome = interrupted::recording(cycle_id, site.record);
    host.audit.insert(part);
    host.audit.recording(outcome);
    log_with(
        "warn",
        "中断工件补记录",
        format!(
            "SN={} 件序号 {}：进程在检测中途中断，已按 ERR 补入历史（{label}）",
            request.sn, request.request_seq
        ),
        Fields {
            cycle_id: Some(cycle_id.into()),
            sn: Some(request.sn.clone()),
            request_seq: Some(request.request_seq),
            ..Fields::default()
        },
    );
}

fn plc(app: &AppHandle) -> &PlcEngine {
    app.state::<PlcHost>().inner().engine()
}

fn host(app: &AppHandle) -> &CycleHost {
    app.state::<CycleHost>().inner()
}

fn log_recording_alarm(change: AlarmChange) {
    let (tone, ev, msg, cycle_id) = match change {
        AlarmChange::Raised(alarm) => {
            ("err", "原图录制失败", alarm.message(), Some(alarm.cycle_id))
        }
        AlarmChange::Cleared => ("ok", "原图录制恢复", "录制失败告警已清除".to_string(), None),
    };
    let fields = Fields {
        cycle_id,
        ..Fields::default()
    };
    log_with(tone, ev, msg, fields);
}

pub fn log(level: &str, ev: impl AsRef<str>, msg: impl AsRef<str>) {
    applog::write(level, ev.as_ref(), msg.as_ref());
}

fn log_with(level: &str, ev: impl AsRef<str>, msg: impl AsRef<str>, fields: Fields) {
    applog::emit(
        applog::Level::from_tone(level),
        ev.as_ref(),
        msg.as_ref(),
        fields,
    );
}

fn trace(ev: impl AsRef<str>, msg: impl AsRef<str>, fields: Fields) {
    applog::emit(applog::Level::Debug, ev.as_ref(), msg.as_ref(), fields);
}

pub fn log_event(event: &Event, msg: impl AsRef<str>, fields: Fields) {
    applog::emit_event(event, msg.as_ref(), fields);
}

struct Part {
    mix_captures: Vec<Option<crate::frame::MixCapture>>,
    mode: crate::store::PartMode,
    cycle_id: String,
    bundle_id: Option<String>,
    production: Option<Arc<crate::production::Prepared>>,
    sn: String,
    recipe: Arc<Recipe>,
    /// 帧组与每帧测量（`cycle::burst`）
    burst: Box<burst::BurstPart>,
    /// partEnd 时 PLC 报的设备触发数（协议 6 只作参考，可与计划数不同）；partEnd 前为空。
    /// 协议 8 不报触发数，恒为空，不参与下一件基线（D-SB-7）
    device_triggers: Option<u16>,
    routing_closed: bool,
    proc_timeout: Duration,
    drain_timeout: Duration,
    /// Line0 门控（D-SB-54、D-SB-55）：门控开始边沿给帧编号定门控号，关闭边沿判定收尾；模拟测试里为空
    gate: Option<Arc<crate::gate::Gate>>,
    /// 看到门控关闭后最多再等的余量；为空时不按门控提前，固定等满 drain_timeout
    gate_guard: Option<Duration>,
    admitted_at: Instant,
    end_at: Option<Instant>,
    fault: Option<(u16, String)>,
    /// 本件用到的相机（相机组序号）与对应的相机编号
    cams: Vec<u8>,
    camera_ids: Vec<String>,
    /// 开工时相机组的版本：检测中增删了相机，cams 里的序号就不对了
    rig_gen: u64,
    source: RecordSource,
    recording: Option<Recording>,
}

impl Part {
    /// 计划帧数 ΣN_exp（未标定为 0）。
    fn n(&self) -> usize {
        self.recipe.expected_total().map_or(0, |n| n as usize)
    }

    /// PLC 报的 deviceTriggers（partEnd 前为 0）。
    fn triggers(&self) -> u64 {
        self.device_triggers.map_or(0, u64::from)
    }

    /// 用本件已经观察到的 SDK 帧号更新下一件基线。
    fn close_routing(&mut self, ledgers: &mut Ledgers) {
        if self.routing_closed {
            return;
        }
        self.burst.close(ledgers);
        self.routing_closed = true;
    }

    fn camera_id(&self, cam: u8) -> String {
        self.cams
            .iter()
            .position(|&c| c == cam)
            .and_then(|i| self.camera_ids.get(i).cloned())
            .unwrap_or_else(|| format!("#{}", cam + 1))
    }

    fn complete(&self) -> bool {
        self.burst_complete(Instant::now())
    }

    fn view(&self) -> PartView {
        let burst = self.burst_view();
        PartView {
            settled_json: None,
            cycle_id: self.cycle_id.clone(),
            bundle_id: self.bundle_id.clone(),
            sn: self.sn.clone(),
            recipe_id: self.recipe.id.clone(),
            recipe_revision: self.recipe.revision_id.clone(),
            n: self.n(),
            received: burst.camera_frames.iter().map(|&n| n as usize).sum(),
            triggers: self.triggers(),
            queue: burst.pending_measure,
            measured_frames: burst.measured_frames,
            burst,
        }
    }
}

fn millis(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// 接件里阻塞的文件检查（发布包核验）的时限：存储卡住时本件按不能检测处理，节拍照常收帧、
/// 处理工件结束与复位；远小于到达到拍照信号的间隔（协议 8 第十节，≥ 2 s）。
const INTAKE_CHECK_TIMEOUT: Duration = Duration::from_secs(1);

/// 在阻塞线程上做接件检查：超时返回 `Err(None)`，检查线程留在后台跑完；线程异常返回 `Err(Some(原因))`。
async fn bounded_check<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Option<String>> {
    match tokio::time::timeout(
        INTAKE_CHECK_TIMEOUT,
        tauri::async_runtime::spawn_blocking(work),
    )
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Some(error.to_string())),
        Err(_) => Err(None),
    }
}

fn missing_recipe(id: &str) -> String {
    format!("选中的配方 {id} 不存在或没能加载（见配方页）")
}

/// 配方用的相机此刻在相机组里的序号。相机不在相机组里或视角不够时返回原因。
/// 计划相机必须是单视角独立设备（D-SB-11）。
pub fn usable_cams(app: &AppHandle, recipe: &Recipe) -> Result<Vec<u8>, String> {
    let host = host(app);
    let cams = host.camera.resolve(&recipe.cameras())?;
    crate::vision::recipe_source(app, recipe)?;
    crate::camera::check_burst_cameras(&host.camera.configs(), &recipe.plan_cameras())?;
    Ok(cams)
}

/// 配方能不能开工：三台相机都示教了走廊且整件已标定（D-SB-20）。
pub fn ready_to_run(recipe: &Recipe) -> Result<(), String> {
    recipe.ready_burst().map_err(|reasons| reasons.join("；"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// 维护模式下的采集版：只录图，只核对相机，不要求走廊、标定、发布包与拍照计划
    Collect,
    Production,
}

/// 配方按哪种要求开工。正常模式不收采集版；正式版不论模式都要过 [`ready_to_run`]。
pub fn run_kind(recipe: &Recipe, maintenance: bool) -> Result<RunKind, String> {
    if recipe.is_collect() && maintenance {
        return Ok(RunKind::Collect);
    }
    crate::production::stage_admits(recipe)?;
    ready_to_run(recipe)?;
    Ok(RunKind::Production)
}

/// 没能建立检测的件：照常采图，工件结束后回 ERR（维护模式回 0）。
struct Uninspected {
    sn: String,
    recipe_id: Option<String>,
    judgement: Judgement,
    admitted_at: Instant,
}

/// 段配方一帧的测量任务：用第 `segment` 段走廊量计划相机 `camera` 的第 `seq` 帧（到达送测与收尾补测共用）。
fn frame_job(
    part: &Part,
    camera: usize,
    camera_id: String,
    seq: u64,
    segment: u32,
    image: Option<crate::frame::FrameView>,
) -> FrameJob {
    FrameJob {
        capture: part.mix_captures.get(camera).cloned().flatten(),
        cycle_id: part.cycle_id.clone(),
        bundle_id: part.bundle_id.clone(),
        production: part.production.clone(),
        camera_idx: camera,
        camera: camera_id,
        seq,
        segment,
        image,
        image_file: None,
        recipe: part.recipe.clone(),
        submitted_at: Instant::now(),
        timeout: part.proc_timeout,
    }
}

/// 维护模式不判定的件：记 ERR 以免混入合格统计，回写 PLC 的结果码 0。
fn uninspected_judgement(fault_code: u16, reason: String) -> Judgement {
    Judgement {
        plc_code: 0,
        ..Judgement::error(fault_code, reason)
    }
}

/// part.json 的附加字段：件的模式、各计划相机的设备配置与实际下发的曝光增益、PLC 请求。历史件导入为采集轮次时据此核对相机。
fn annotate_recording(
    rec: &mut crate::recorder::Recording,
    host: &CycleHost,
    mode: crate::store::PartMode,
    recipe: &Recipe,
    cams: &[u8],
    identities: &[ArmCam],
    request: &Request,
) {
    let devices: Vec<serde_json::Value> = cams
        .iter()
        .zip(recipe.plan_cameras())
        .map(|(&cam, camera)| {
            let slot = host.camera.slot(cam as usize);
            let session = identities
                .iter()
                .find(|identity| identity.cam == cam)
                .map(|identity| identity.session);
            let mut entry = json!({
                "cameraId": camera,
                "cameraConfig": slot.as_ref().map(|slot| slot.config()),
                "imaging": slot.as_ref().map(|slot| slot.recorded_imaging(session)),
                "deviceSession": session,
            });
            if let Some(capture) = slot.and_then(|slot| slot.synthetic_capture(session)) {
                entry["syntheticCapture"] =
                    json!({"serial": capture.serial, "whiteBalance": capture.white_balance});
            }
            entry
        })
        .collect();
    rec.annotate("partMode", json!(mode));
    rec.annotate("devices", json!(devices));
    rec.annotate(
        "plc",
        json!({"requestSeq": request.request_seq, "productCode": request.product_code}),
    );
}

/// 配方能开工时用到的相机。采集版只在维护模式下开工，只核对相机，不要求走廊、标定与发布包。
pub fn runnable_cams(app: &AppHandle, recipe: &Recipe) -> Result<Vec<u8>, String> {
    if run_kind(recipe, host(app).settings().maintenance.is_some())? == RunKind::Production {
        crate::production::ready(app, recipe)?;
    }
    usable_cams(app, recipe)
}

/// 空闲时对相机的要求：配方、设置变了（Refresh）或相机组增删过才重算。
struct Required {
    rig_gen: u64,
    /// 必须就绪、否则停在故障的相机：人工选型号时是选中配方的（配方开不了工时是原因）；PLC 下发型号时没有
    cams: Result<Vec<u8>, String>,
    /// PLC 下发型号时各配方的相机：没就绪只报警，那个型号开工时判 ERR，别的型号照常；一个也开不了工才停在故障
    watch: Vec<(String, Vec<u8>)>,
    /// 配置上就开不了工的配方
    warnings: Vec<String>,
}

/// 同一报警文案在这段时间内只写一次日志，其余计入 alarm.raised.summary。
const ALARM_LOG_WINDOW: Duration = Duration::from_secs(600);

struct Machine {
    app: AppHandle,
    /// 测量池：每台相机每一帧独立测（D-SB-12）
    frame_pool: FramePool,
    ledgers: Ledgers,
    phase: Phase,
    since: i64,
    part: Option<Part>,
    /// 维护模式下没能建立检测的件（找不到配方、设备或发布包有问题）：照常采图，工件结束后回结果码 0
    uninspected: Option<Uninspected>,
    active_recipe: Option<Arc<Recipe>>,
    fault: Option<String>,
    /// 当前故障的来源、进入时刻，以及同一来源同一原因被再次上报的次数（首次、最近一次时刻）
    fault_source: Option<&'static str>,
    fault_at: Option<i64>,
    fault_repeat: Option<FaultRepeat>,
    result: Option<ResultView>,
    stats: DailyStats,
    maintenance_progress: MaintenanceProgress,
    stats_checked: Instant,
    history_purge_day: NaiveDate,
    stray: u64,
    stray_times: VecDeque<Instant>,
    alarms: Vec<String>,
    required: Option<Required>,
    config_warnings: Vec<String>,
    camera_warnings: Vec<String>,
    /// camera_warnings 对应的配方
    camera_down: Vec<String>,
    /// 上次生成 camera_warnings 的时刻：同一批配方开不了工时，原因也会变
    camera_checked: Instant,
    dirty: bool,
    published: Instant,
    dropped_seen: u64,
    budget_seen: u64,
    /// 逐帧、逐拍重复的告警聚合（游离帧、迟到帧、丢帧等），节拍每拍写出到期摘要
    noise: Throttle,
    /// 报警写日志的去重：alarms 每件开工、复位都会清空，同一报警不再每件重写
    alarm_log: Throttle,
    s7: PlcSession,
    current_cycle_id: Option<String>,
    recording_alarm: Option<RecordingAlarm>,
}

impl Machine {
    fn new(app: AppHandle, frame_pool: FramePool) -> Self {
        let started_at = Local::now();
        let day = started_at.date_naive();
        let mut stats = DailyStats::new(day, today_stats(&app, day));
        let mut s7 = PlcSession::open(crate::storage::state(&app).join("plc-handshake.json"));
        if let Some(notice) = s7.archived() {
            log("warn", "S7 握手日志归档", notice.to_owned());
        }
        record_interrupted_part(&app, &s7, &mut stats, started_at.timestamp_millis());
        s7.forget_previous();
        host(&app).store_busy(Phase::Fault, &s7);
        Self {
            app,
            frame_pool,
            ledgers: Ledgers::default(),
            phase: Phase::Fault,
            since: now_ms(),
            part: None,
            uninspected: None,
            active_recipe: None,
            fault: Some("PLC 未连接".into()),
            fault_source: None,
            fault_at: None,
            fault_repeat: None,
            result: None,
            stats,
            maintenance_progress: MaintenanceProgress::default(),
            stats_checked: Instant::now(),
            history_purge_day: day,
            stray: 0,
            stray_times: VecDeque::new(),
            alarms: Vec::new(),
            required: None,
            config_warnings: Vec::new(),
            camera_warnings: Vec::new(),
            camera_down: Vec::new(),
            camera_checked: Instant::now(),
            dirty: true,
            published: Instant::now(),
            dropped_seen: 0,
            budget_seen: 0,
            noise: Throttle::new(throttle::WINDOW),
            alarm_log: Throttle::new(ALARM_LOG_WINDOW),
            s7,
            current_cycle_id: None,
            recording_alarm: None,
        }
    }

    /// 原图录制失败不停产，告警独立于每件清空的 alarms，跟着当前场景的录制结果走。
    fn sync_recording_alarm(&mut self) {
        let alarm = host(&self.app).recording_health.alarm(HistoryScope::Real);
        if alarm != self.recording_alarm {
            self.recording_alarm = alarm;
            self.dirty = true;
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        host(&self.app).store_busy(phase, &self.s7);
        self.since = now_ms();
        self.dirty = true;
    }

    fn alarm(&mut self, msg: String) {
        if !self.alarms.contains(&msg) {
            let fields = Fields::default().with_data(json!({"alarm": msg}));
            if self.alarm_log.observe(
                &events::ALARM_RAISED_SUMMARY,
                &msg,
                now_ms(),
                Tally::None,
                fields.clone(),
            ) {
                log_event(&events::ALARM_RAISED, msg.clone(), fields);
            }
            self.alarms.push(msg);
            self.dirty = true;
        }
    }

    /// 聚合一条高频告警：窗口内首次照写原事件，之后只计数，摘要由节拍写出。
    fn noisy(
        &mut self,
        event: &Event,
        summary: &'static Event,
        key: &str,
        tally: Tally,
        msg: String,
        fields: Fields,
    ) {
        if self
            .noise
            .observe(summary, key, now_ms(), tally, fields.clone())
        {
            log_event(event, msg, fields);
        }
    }

    /// 帧归属失败：判本件 ERR 的记 frame.route_fault，其余记 frame.unrouted；按相机 + 原因聚合。
    fn unrouted(
        &mut self,
        camera: String,
        cycle_id: String,
        fault: bool,
        data: serde_json::Value,
        msg: String,
        frame_counter: u64,
    ) {
        let key = format!("{camera}/{}", data["reason"].as_str().unwrap_or_default());
        let fields = Fields {
            cycle_id: Some(cycle_id),
            camera: Some(camera),
            ..Fields::default()
        }
        .with_data(data);
        let (event, summary) = if fault {
            (
                &events::FRAME_ROUTE_FAULT,
                &events::FRAME_ROUTE_FAULT_SUMMARY,
            )
        } else {
            (&events::FRAME_UNROUTED, &events::FRAME_UNROUTED_SUMMARY)
        };
        self.noisy(
            event,
            summary,
            &key,
            Tally::Range("frameCounter", frame_counter as i64),
            msg,
            fields,
        );
    }

    fn flush_noise(&mut self) {
        let now = now_ms();
        for summary in self
            .noise
            .flush(now)
            .into_iter()
            .chain(self.alarm_log.flush(now))
        {
            log_event(summary.event, summary.msg, summary.fields);
        }
    }

    /// 磁盘空间不足时告警（所有录制盘都低于余量时新件不录原图，见 start_part）；空间恢复后撤告警。
    fn sync_disk_alarm(&mut self) {
        const PREFIX: &str = crate::disk_guard::ALARM_PREFIX;
        let shown = self.alarms.iter().any(|m| m.starts_with(PREFIX));
        match host(&self.app).disk.status().alarm {
            Some(reason) => match self.alarms.iter_mut().find(|m| m.starts_with(PREFIX)) {
                Some(m) if *m == reason => {}
                Some(m) => {
                    *m = reason;
                    self.dirty = true;
                }
                None => self.alarm(reason),
            },
            None => {
                if shown {
                    self.alarms.retain(|m| !m.starts_with(PREFIX));
                    self.dirty = true;
                    log("ok", "磁盘空间", "剩余空间已恢复，磁盘告警解除");
                }
            }
        }
    }

    /// 当前配方（人工选择或上一件 PLC 下发的）。
    fn current_recipe(&self) -> Option<Arc<Recipe>> {
        let host = host(&self.app);
        let settings = host.settings();
        match settings.product_source {
            ProductSource::Manual => settings
                .manual_recipe_id
                .as_deref()
                .and_then(|id| host.recipe(id)),
            ProductSource::Plc => self.active_recipe.clone(),
        }
    }

    fn compute_required(&self) -> Required {
        let host = host(&self.app);
        let rig_gen = host.camera.generation();
        let settings = host.settings();
        match settings.product_source {
            ProductSource::Manual => {
                let cams = match (self.current_recipe(), settings.manual_recipe_id.as_deref()) {
                    (Some(r), _) => runnable_cams(&self.app, &r),
                    (None, Some(id)) => Err(missing_recipe(id)),
                    (None, None) => Ok(Vec::new()),
                };
                Required {
                    rig_gen,
                    cams,
                    watch: Vec::new(),
                    warnings: Vec::new(),
                }
            }
            ProductSource::Plc => {
                let (mut watch, mut warnings) = (Vec::new(), Vec::new());
                for r in host.recipes.list() {
                    match runnable_cams(&self.app, &r) {
                        Ok(c) => watch.push((r.id.clone(), c)),
                        Err(e) => warnings.push(format!("配方 {} 开不了工：{e}", r.id)),
                    }
                }
                Required {
                    rig_gen,
                    cams: Ok(Vec::new()),
                    watch,
                    warnings,
                }
            }
        }
    }

    /// 与配方无关的服务就绪：生产生效版本已确认、工件身份可分配。
    fn service_ready(&self) -> Result<(), String> {
        if crate::workspace::activation_uncertain(&self.app) {
            return Err("生产生效版本尚未确认，请先核实发布状态".into());
        }
        host(&self.app).cycle_ids.ready(&self.app)
    }

    fn check_idle_devices(&mut self) -> Result<(), String> {
        self.service_ready()?;
        let host = host(&self.app);
        if self
            .required
            .as_ref()
            .is_none_or(|r| r.rig_gen != host.camera.generation())
        {
            let req = self.compute_required();
            for w in req
                .warnings
                .iter()
                .filter(|w| !self.config_warnings.contains(w))
            {
                log("err", "配方", w.clone());
            }
            if req.warnings != self.config_warnings {
                self.config_warnings = req.warnings.clone();
                self.dirty = true;
            }
            self.required = Some(req);
        }
        let req = self.required.as_ref().unwrap();
        let down: Vec<String> = req
            .watch
            .iter()
            .filter(|(_, cams)| !host.camera.all_ready(cams))
            .map(|(id, _)| id.clone())
            .collect();
        if down != self.camera_down
            || (!down.is_empty() && self.camera_checked.elapsed() > Duration::from_secs(1))
        {
            self.camera_checked = Instant::now();
            let warnings: Vec<String> = req
                .watch
                .iter()
                .filter(|(id, _)| down.contains(id))
                .filter_map(|(id, cams)| {
                    host.camera
                        .check_ready_at(cams)
                        .err()
                        .map(|e| format!("配方 {id} 暂时开不了工：{e}"))
                })
                .collect();
            if warnings != self.camera_warnings {
                for w in warnings
                    .iter()
                    .filter(|w| !self.camera_warnings.contains(w))
                {
                    log("warn", "相机", w.clone());
                }
                self.camera_warnings = warnings;
                self.dirty = true;
            }
            self.camera_down = down;
        }
        // 维护模式下配方问题不拦来件：照常采图放行（MM-3），只有在用配方的相机全掉线才撤就绪
        let maintenance = host.settings().maintenance.is_some();
        let none_usable = !maintenance && !req.warnings.is_empty() && req.watch.is_empty();
        if none_usable || (!req.watch.is_empty() && self.camera_down.len() == req.watch.len()) {
            let first = self
                .camera_warnings
                .first()
                .or(req.warnings.first())
                .cloned()
                .unwrap_or_default();
            return Err(format!("没有一个配方开得了工（{first}）"));
        }
        match &req.cams {
            Ok(cams) => host.camera.check_ready_at(cams),
            Err(_) if maintenance => Ok(()),
            Err(e) => Err(e.clone()),
        }
    }

    fn publish(&mut self) {
        self.dirty = false;
        self.published = Instant::now();
        let host = host(&self.app);
        let settings = host.settings();
        let progress = self
            .maintenance_progress
            .snapshot(settings.maintenance.as_ref());
        let active = self.current_recipe();
        let snapshot = Arc::new(Snapshot {
            stats_scope: HistoryScope::Real,
            measurement_workers: self.frame_pool.health().snapshot(),
            phase: self.phase,
            plc_locked: host.busy(),
            plc_handshake: Some(self.s7.view()),
            since: self.since,
            fault: self.fault.clone(),
            product_source: settings.product_source,
            active_recipe_id: active.as_ref().map(|r| r.id.clone()),
            trigger_mode: active.as_ref().map(|r| r.trigger_mode),
            part: self
                .part
                .as_ref()
                .map(|part| part.published_view(self.phase)),
            result: self.result.clone(),
            stats: self.stats.snapshot(),
            stray_frames: self.stray,
            alarms: self
                .alarms
                .iter()
                .chain(&self.config_warnings)
                .chain(&self.camera_warnings)
                .cloned()
                .chain(self.recording_alarm.as_ref().map(RecordingAlarm::message))
                .collect(),
            recording_alarm: self.recording_alarm.clone(),
            maintenance: settings.maintenance,
            collection: progress.collection,
            trial: progress.trial,
        });
        let previous = host
            .shared
            .lock()
            .unwrap()
            .snapshot
            .replace(Arc::clone(&snapshot));
        let control_changed = previous
            .as_deref()
            .is_none_or(|previous| snapshot.control_state_changed(previous));
        drop(previous);
        if control_changed {
            let _ = self
                .app
                .emit("cycle://control-state", snapshot.control_state());
        }
        let _ = self.app.emit("cycle://snapshot", snapshot.as_ref());
    }

    async fn on_input(&mut self, input: Input) {
        self.dirty = true;
        match input {
            Input::FrameMeasured(m) => self.on_frame_measured(m),
            Input::Refresh => self.required = None,
        }
    }

    async fn start_part(&mut self, request: Request) {
        let started = Instant::now();
        let settings = host(&self.app).settings();
        if crate::workspace::apply_pending(&self.app) {
            self.required = None;
        }
        self.set_phase(Phase::Validate);
        self.part = None;
        self.uninspected = None;
        self.current_cycle_id = None;
        self.result = None;
        crate::workspace::clear_live(&self.app);
        self.alarms.clear();
        for summary in self.noise.end_all(&events::FRAME_STRAY_SUMMARY) {
            log_event(summary.event, summary.msg, summary.fields);
        }
        let app = self.app.clone();
        let (sn, code) = (request.sn.clone(), request.product_code as u32);
        let host = host(&app);
        let taken = host.cycle_ids.take(&app);
        let context = Fields {
            cycle_id: taken.as_ref().ok().cloned(),
            sn: Some(sn.clone()),
            request_seq: Some(request.request_seq),
            ..Fields::default()
        };
        applog::begin_cycle(context.clone());
        log_with(
            "info",
            "工件到达",
            format!("SN={sn} 型号={code}"),
            Fields::stage("partStart").with_data(json!({"productCode": code})),
        );
        let cycle_id = match taken {
            Ok(id) => id,
            Err(reason) => return self.refuse(sn, None, fault::PROCESS_TIMEOUT, reason).await,
        };
        self.current_cycle_id = Some(cycle_id.clone());
        let site = PartSite {
            source: RecordSource::Real,
            record: settings.record,
        };
        if let Err(error) = self.s7.bind_cycle_id(&cycle_id, site) {
            return self
                .refuse(
                    sn,
                    None,
                    fault::PROCESS_TIMEOUT,
                    format!("在途件无法绑定工件身份：{error}"),
                )
                .await;
        }
        if crate::workspace::activation_uncertain(&app) {
            return self
                .refuse(
                    sn,
                    None,
                    fault::NO_RECIPE,
                    "生产生效版本尚未确认，本件未检；请在配方工作台核实发布状态".into(),
                )
                .await;
        }
        let maintenance = self.s7.part_maintenance();
        let recipe = match settings.product_source {
            ProductSource::Plc => host
                .recipes
                .list()
                .into_iter()
                .find(|r| r.product_code as u32 == code),
            ProductSource::Manual => settings
                .manual_recipe_id
                .as_deref()
                .and_then(|id| host.recipe(id)),
        };
        let Some(recipe) = recipe else {
            let reason = match (
                settings.product_source,
                settings.manual_recipe_id.as_deref(),
            ) {
                (ProductSource::Plc, _) => format!("产品代码 {code} 没有对应的配方"),
                (ProductSource::Manual, Some(id)) => missing_recipe(id),
                (ProductSource::Manual, None) => "未选择配方".to_string(),
            };
            return self.refuse(sn, None, fault::NO_RECIPE, reason).await;
        };
        if recipe.is_collect() && !maintenance {
            return self
                .refuse(
                    sn,
                    Some(recipe.id.clone()),
                    fault::NO_RECIPE,
                    format!("配方 {} 只有采集版，仅维护模式可用", recipe.id),
                )
                .await;
        }
        let collect = maintenance && recipe.is_collect();
        let mode = match (maintenance, collect) {
            (false, _) => crate::store::PartMode::Production,
            (true, true) => crate::store::PartMode::MaintenanceCollect,
            (true, false) => crate::store::PartMode::MaintenanceShadow,
        };
        self.active_recipe = Some(recipe.clone());
        applog::begin_cycle(Fields {
            recipe_id: Some(recipe.id.clone()),
            ..context
        });
        // 从这一刻起这个配方算在检测中（不能删、不能改编号）
        host.shared.lock().unwrap().part_recipe = Some(recipe.clone());
        let id = Some(recipe.id.clone());
        // 日志里的计划帧数 ΣN_exp
        let planned = recipe.expected_total().map_or(0, |n| n as usize);
        // 采集版只录图：不编拍照计划、不测量、没有发布包
        if !collect {
            let plan =
                match admission::admit(&request, &recipe, crate::plc::recipe_plan(host, &recipe)) {
                    Ok(plan) => plan,
                    Err((code, reason)) => return self.refuse(sn, id, code, reason).await,
                };
            if let Err(reason) = self.s7.validate_plan(&plan) {
                return self.refuse(sn, id, fault::PLAN_MISMATCH, reason).await;
            }
        }
        if !collect && !self.frame_pool.health().can_arm() {
            return self
                .refuse(
                    sn,
                    id,
                    fault::PROCESS_TIMEOUT,
                    "测量线程全部超时且尚未返回，本件不能检测".into(),
                )
                .await;
        }
        let production = if collect {
            None
        } else {
            match crate::production::ready(&app, &recipe) {
                Ok(prepared) => prepared,
                Err(reason) => return self.refuse(sn, id, fault::NO_RECIPE, reason).await,
            }
        };
        if let Some(prepared) = production.clone() {
            match bounded_check(move || prepared.verify()).await {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => {
                    return self
                        .refuse(
                            sn,
                            id,
                            fault::NO_RECIPE,
                            format!("发布包核验失败：{reason}"),
                        )
                        .await
                }
                Err(error) => {
                    let reason = match error {
                        Some(error) => format!("发布包核验线程异常：{error}"),
                        None => format!(
                            "发布包核验超过 {} ms 未完成",
                            INTAKE_CHECK_TIMEOUT.as_millis()
                        ),
                    };
                    return self.refuse(sn, id, fault::PROCESS_TIMEOUT, reason).await;
                }
            }
        }
        let rig_gen = host.camera.generation();
        let cams = match runnable_cams(&app, &recipe) {
            Ok(c) => c,
            Err(reason) => return self.refuse(sn, id, fault::NO_RECIPE, reason).await,
        };
        if let Err(reason) = host.camera.check_ready_at(&cams) {
            return self.refuse(sn, id, fault::DEVICE_LOST, reason).await;
        }
        let camera_ids = recipe.cameras();
        let mut identities: Vec<ArmCam> = Vec::new();
        for &cam in &cams {
            let Some(slot) = host.camera.slot(cam as usize) else {
                return self
                    .refuse(sn, id, fault::DEVICE_LOST, "相机组已变化".into())
                    .await;
            };
            let (identity, observed) = match slot.routing_state() {
                Ok(state) => state,
                Err(error) => return self.refuse(sn, id, fault::DEVICE_LOST, error).await,
            };
            if let Some(ledger) = observed {
                self.ledgers.merge(cam, &ledger);
            }
            identities.push(identity);
        }
        let burst = {
            // 按 SDK 帧计数定每台相机的基线（D-SB-10），会话或计数来源不可信时本件不能检测（98）
            let policy = Policy::burst_production(None);
            let baselines =
                match arm_frames(&recipe.plan_cameras(), &self.ledgers, &policy, &identities) {
                    Ok(baselines) => baselines,
                    Err(error) => {
                        return self.refuse(sn, id, error.fault(), error.to_string()).await
                    }
                };
            for camera in &baselines {
                if let Some(baseline) = camera.baseline {
                    self.ledgers.set_frame_floor(&Floor {
                        cam: camera.cam,
                        camera: camera.camera.clone(),
                        session: camera.session,
                        baseline,
                    });
                }
            }
            // 帧编号起点（D-SB-55）：接件时锁存各计划相机的时钟作下限，时间戳不大于它的帧属于上一件
            let first_gate = host.camera.gate().open_count();
            let starts: Vec<crate::frame_clock::ClockStart> = baselines
                .iter()
                .map(|armed| {
                    let slot = host.camera.slot(armed.cam as usize);
                    let floor = slot.as_ref().and_then(|s| s.clock_now());
                    if floor.is_none() {
                        log(
                            "warn",
                            "相机时钟",
                            format!(
                                "接件时读不出相机 {} 的时钟，本件这台相机不判上一件的迟到帧",
                                armed.camera
                            ),
                        );
                    }
                    crate::frame_clock::ClockStart {
                        period_ns: slot
                            .as_ref()
                            .map_or(crate::frame_clock::SYNTH_PERIOD_NS as f64, |s| {
                                s.frame_period_ns()
                            }),
                        floor: floor.unwrap_or(0),
                        anchored: identities.iter().any(|c| {
                            c.cam == armed.cam
                                && c.source == Some(crate::frame::CounterSource::Synthetic)
                        }),
                        delay_ns: slot.as_ref().map_or(0, |s| s.trigger_delay_ns()),
                        first_gate,
                    }
                })
                .collect();
            let mut burst = burst::BurstPart::arm(&recipe, baselines, policy.trusted, &starts);
            burst.collect = collect;
            Box::new(burst)
        };
        let bundle_id = production
            .as_ref()
            .map(|prepared| prepared.bundle.id.clone());
        host.audit.begin_recording(&cycle_id);
        let source = RecordSource::Real;
        // 维护件不论录制设置都录全部帧：采集件是示教与标定的样本，试判件要靠原图复核误判（MM-11）；
        // 发布包有段预热没测成时也录全部帧，修好配方后能从原图复测这些段（D-SB-59）
        let untested = production.as_ref().and_then(|p| p.untested_label());
        let unmeasurable = production.as_ref().filter(|p| !p.measurable()).map(|p| {
            (
                fault::NO_RECIPE,
                format!(
                    "发布包每一段在示教帧上都没测成，本件只录原图、不检测：{}",
                    p.untested_reasons().join("；")
                ),
            )
        });
        // 按录制盘顺序写到第一块剩余不少于余量的盘；所有盘都不足时不录原图，照常检测（收尾补测仍用临时录制，
        // 写到剩余最多的在线盘）
        let disk = host.disk.status();
        let record = if disk.target.is_none() {
            crate::settings::RecordMode::Off
        } else if maintenance || untested.is_some() {
            crate::settings::RecordMode::All
        } else {
            settings.record
        };
        // 收尾补测只从录制原图读（D-SB-60）：录制关闭时要测图的件也写临时录制，收尾删除
        let mut recording = if record == crate::settings::RecordMode::Off && !collect {
            host.recorders.scratch(
                source,
                disk.scratch.as_deref(),
                &sn,
                recipe.clone(),
                &cycle_id,
                bundle_id.as_deref(),
            )
        } else {
            host.recorders.begin(
                source,
                record,
                disk.target.as_deref(),
                &sn,
                recipe.clone(),
                &cycle_id,
                bundle_id.as_deref(),
            )
        };
        if let Some(rec) = recording.as_mut() {
            annotate_recording(rec, host, mode, &recipe, &cams, &identities, &request);
        }
        self.part = Some(Part {
            mix_captures: cams
                .iter()
                .map(|&cam| {
                    let slot = host.camera.slot(cam as usize)?;
                    let session = identities
                        .iter()
                        .find(|identity| identity.cam == cam)
                        .map(|identity| identity.session);
                    let config = slot.config();
                    match config.source {
                        crate::camera::CameraSource::Mvs => Some(crate::frame::MixCapture {
                            serial: config.serial,
                            white_balance: slot.recorded_imaging(session).white_balance?,
                            synthetic: false,
                        }),
                        crate::camera::CameraSource::Simulator => slot.synthetic_capture(session),
                        _ => None,
                    }
                })
                .collect(),
            mode,
            cycle_id,
            bundle_id,
            production,
            sn: sn.clone(),
            burst,
            device_triggers: None,
            routing_closed: false,
            proc_timeout: settings.timeouts.proc(),
            drain_timeout: settings.timeouts.drain(),
            gate: Some(host.camera.gate()),
            gate_guard: settings.timeouts.gate_guard(),
            admitted_at: Instant::now(),
            end_at: None,
            fault: unmeasurable,
            cams,
            camera_ids,
            rig_gen,
            source,
            recording,
            recipe: recipe.clone(),
        });
        self.s7.admit(Some(&recipe.id)).await;
        self.part.as_mut().unwrap().admitted_at = Instant::now();
        self.set_phase(Phase::Acquire);
        log_with(
            "info",
            "接件登记",
            format!(
                "{} · N={planned} · 登记耗时 {} ms{}",
                recipe.id,
                started.elapsed().as_millis(),
                untested
                    .as_ref()
                    .map_or_else(String::new, |label| format!(" · 预热没测成、不测：{label}"))
            ),
            Fields::stage("admitted")
                .with_data(json!({"intakeMs": millis(started), "shots": planned})),
        );
        log("info", "接件阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "admitted", "elapsedMs": millis(started)}).to_string());
    }

    /// 本件不能检测：PLC 不等 PC，工件照常运动采图；登记为未检件，PLC 报工件结束后回 ERR（维护模式回 0），原因记进本件记录。
    async fn refuse(&mut self, sn: String, recipe_id: Option<String>, code: u16, reason: String) {
        let maintenance = self.s7.part_maintenance();
        let reason = if maintenance {
            format!("维护模式：{reason}；本件未检")
        } else {
            reason
        };
        log(
            if maintenance { "warn" } else { "err" },
            "接件校验",
            format!("{reason}；工件结束后回结果"),
        );
        log(
            "warn",
            "接件阶段",
            json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "deferred"}).to_string(),
        );
        match self.part.as_mut() {
            Some(part) => {
                part.fault.get_or_insert((code, reason));
            }
            None => {
                let judgement = if maintenance {
                    uninspected_judgement(code, reason)
                } else {
                    Judgement::error(code, reason)
                };
                self.uninspected = Some(Uninspected {
                    sn,
                    recipe_id,
                    judgement,
                    admitted_at: Instant::now(),
                });
            }
        }
        let recipe_id = self
            .part
            .as_ref()
            .map(|part| part.recipe.id.as_str())
            .or_else(|| self.uninspected.as_ref()?.recipe_id.as_deref());
        self.s7.admit(recipe_id).await;
        if let Some(part) = self.part.as_mut() {
            part.admitted_at = Instant::now();
        }
        self.set_phase(Phase::Acquire);
    }

    fn on_frame(&mut self, f: Frame) {
        let meta = FrameMeta::from(&f);
        self.ledgers.observe(&meta);
        let accepting = matches!(self.phase, Phase::Acquire | Phase::Drain);
        let slot = host(&self.app).camera.slot(f.cam as usize);
        if f.manual {
            if accepting {
                if let Some(part) = self.part.as_mut() {
                    let current = slot.as_ref().is_some_and(|slot| {
                        slot.routing_state()
                            .is_ok_and(|state| state.0.session == f.session)
                    });
                    let burst = &mut part.burst;
                    // 接件后的手动帧丢弃并告警，不判 ERR（它占了这台相机一个帧号，此后的帧组可能错开一格）
                    if let Some(camera) = burst
                        .baseline(f.cam)
                        .filter(|base| current && f.frame_counter > *base)
                        .and_then(|_| part.cams.iter().position(|&c| c == f.cam))
                        .filter(|&camera| burst.first_warning(camera, burst::Warning::Manual))
                    {
                        let message = format!("生产接件后收到相机 {} 的手动触发帧（帧计数 {}），已丢弃；该相机此后的帧组可能错开一格", burst.camera_name(camera), f.frame_counter);
                        log_with(
                            "warn",
                            "手动帧",
                            message,
                            Fields {
                                cycle_id: Some(part.cycle_id.clone()),
                                ..Fields::default()
                            },
                        );
                        self.dirty = true;
                    }
                }
            }
            return;
        }
        // 没能建立检测的件照常出帧：只记入设备计数账，不算游离帧
        if accepting && self.uninspected.is_some() {
            return;
        }
        if !accepting || self.part.is_none() {
            self.stray += 1;
            self.dirty = true;
            let now = Instant::now();
            self.stray_times.push_back(now);
            self.stray_times
                .retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            let camera = slot.map_or_else(|| format!("#{}", f.cam + 1), |s| s.config().id);
            let msg = format!(
                "空闲时收到相机 {camera} 的帧（帧计数 {}），已记入设备计数账",
                f.frame_counter
            );
            let fields = Fields {
                camera: Some(camera.clone()),
                ..Fields::default()
            }
            .with_data(json!({"frameCounter": f.frame_counter, "phase": self.phase}));
            self.noisy(
                &events::FRAME_STRAY,
                &events::FRAME_STRAY_SUMMARY,
                &camera,
                Tally::Range("frameCounter", f.frame_counter as i64),
                msg,
                fields,
            );
            if self.stray_times.len() >= 3 {
                self.alarm("1 分钟内游离帧 ≥ 3，检查 Line0 接线与输入滤波".into());
            }
            return;
        }
        self.dirty = true;
        self.on_burst_frame(f);
    }

    /// 一帧（D-SB-55）：按相机时间戳编号后配组；接受的帧录制、留预览并送测（所有帧都测，不裁帧）。上一件的迟到帧只记游离帧；
    /// 重复帧号、帧号越界丢弃并告警；会话变化或计数来源不可信判 ERR 98（`receive_burst` 已记进故障）。
    fn on_burst_frame(&mut self, f: Frame) {
        let app = self.app.clone();
        let part = self.part.as_mut().unwrap();
        let Some(arrival) = part.receive_burst(&f) else {
            let (camera, cycle_id) = (part.camera_id(f.cam), part.cycle_id.clone());
            let msg = format!(
                "工件 {cycle_id} 已到收尾截止，丢弃相机 {camera} 的帧（帧计数 {}）",
                f.frame_counter
            );
            let fields = Fields {
                cycle_id: Some(cycle_id),
                camera: Some(camera.clone()),
                ..Fields::default()
            }
            .with_data(json!({"frameCounter": f.frame_counter}));
            self.noisy(
                &events::FRAME_AFTER_DEADLINE,
                &events::FRAME_AFTER_DEADLINE_SUMMARY,
                &camera,
                Tally::Range("frameCounter", f.frame_counter as i64),
                msg,
                fields,
            );
            return;
        };
        let cycle_id = part.cycle_id.clone();
        let burst = &mut part.burst;
        let fields = |camera: &str, seq: Option<u64>| Fields {
            cycle_id: Some(cycle_id.clone()),
            camera: Some(camera.to_owned()),
            seq,
            ..Fields::default()
        };
        match arrival {
            burst::Arrival::Accepted { camera, seq } => {
                let camera_id = burst.camera_name(camera).to_owned();
                let recorded = part.recording.as_mut().and_then(|rec| {
                    host(&app).recorders.burst_frame(rec, &f, camera, seq);
                    rec.pending_frame(camera, seq)
                });
                let image = f.view();
                crate::workspace::retain_live_frame(
                    &app,
                    &part.cycle_id,
                    &part.recipe.revision_id,
                    camera,
                    seq,
                    &image,
                    recorded,
                );
                trace("帧到达", format!("cycle={cycle_id} · {camera_id} 第 {seq} 帧 · 帧计数 {}", f.frame_counter),
                    fields(&camera_id, Some(seq)).with_stage("frame")
                        .with_data(json!({"frameCounter": f.frame_counter, "triggerCounter": f.trigger_counter, "session": f.session})));
                if let Some(segment) = burst
                    .arrival_corridor(camera, seq)
                    .filter(|_| part.mode != crate::store::PartMode::MaintenanceCollect)
                {
                    self.frame_pool.submit(frame_job(
                        part,
                        camera,
                        camera_id.clone(),
                        seq,
                        segment,
                        image,
                    ));
                }
            }
            burst::Arrival::Duplicate { camera, seq } => {
                if burst.first_warning(camera, burst::Warning::Duplicate) {
                    let camera_id = burst.camera_name(camera).to_owned();
                    log_with("warn", "重复帧号", format!("相机 {camera_id} 第 {seq} 帧重复到达，保留先到的那帧、丢弃后到的（本件同类情况不再逐条记录）"), fields(&camera_id, Some(seq)));
                }
            }
            burst::Arrival::OutOfRange { camera, seq } => {
                let camera_id = burst.camera_name(camera).to_owned();
                if burst.first_warning(camera, burst::Warning::OutOfRange) {
                    self.alarm(format!("相机 {camera_id} 帧号比本件基线大 {seq}，超出本件帧组上限（ΣN_exp + Σtol + 余量），按计数异常丢弃，不计入本件"));
                }
            }
            burst::Arrival::Stale { camera } => {
                let camera_id = burst.camera_name(camera).to_owned();
                self.stray += 1;
                let msg = format!("相机 {camera_id} 的帧（帧计数 {}）不属于本件：上一件的迟到帧或接件前那次打开的帧，记为游离帧", f.frame_counter);
                let fields =
                    fields(&camera_id, None).with_data(json!({"frameCounter": f.frame_counter}));
                self.noisy(
                    &events::FRAME_LATE,
                    &events::FRAME_LATE_SUMMARY,
                    &camera_id,
                    Tally::Range("frameCounter", f.frame_counter as i64),
                    msg,
                    fields,
                );
            }
            burst::Arrival::Late { camera } => {
                if burst.first_warning(camera, burst::Warning::Late) {
                    let camera_id = burst.camera_name(camera).to_owned();
                    log_with("warn", "迟到帧", format!("相机 {camera_id} 有帧属于已经结束的前一个门控、又超出它的帧组数，已丢弃（本件同类情况不再逐条记录）"), fields(&camera_id, None));
                }
            }
            burst::Arrival::ClockBack { camera } => {
                if burst.first_warning(camera, burst::Warning::ClockBack) {
                    let camera_id = burst.camera_name(camera).to_owned();
                    self.alarm(format!("相机 {camera_id} 的时间戳回退（断电重启或时钟被复位），本件这台相机之后的帧不收，缺的帧组标检测不完整"));
                }
            }
            burst::Arrival::NoClock { camera } => {
                if burst.first_warning(camera, burst::Warning::NoClock) {
                    let camera_id = burst.camera_name(camera).to_owned();
                    self.alarm(format!("相机 {camera_id} 的帧没有相机时间戳，编不了帧号，已丢弃；检查相机的时间戳频率"));
                }
            }
            burst::Arrival::Fault(reason) => {
                let data = json!({"reason": "fault", "frameCounter": f.frame_counter});
                let camera = part.camera_id(f.cam);
                self.unrouted(camera, cycle_id, true, data, reason, f.frame_counter);
            }
            burst::Arrival::Unknown => {
                let data = json!({"reason": "unknown_camera", "frameCounter": f.frame_counter});
                let camera = part.camera_id(f.cam);
                self.unrouted(
                    camera,
                    cycle_id,
                    false,
                    data,
                    format!("相机组序号 {} 不在本件计划相机里", f.cam),
                    f.frame_counter,
                );
            }
        }
    }

    /// 一帧的测量结果：写进本件；没测成（队满、超时、失败）只记检测不完整，不判 ERR（D-SB-13）。
    fn on_frame_measured(&mut self, mut m: FrameMeasured) {
        if !matches!(self.phase, Phase::Acquire | Phase::Drain) {
            return;
        }
        let Some(part) = self.part.as_mut() else {
            return;
        };
        let (ms, queue_ms, engine_ms) = (m.ms, m.queue_ms, m.engine_ms);
        let (segment, overlay) = (m.segment, m.trace.take());
        let Some(key) = part.apply_burst(m) else {
            return;
        };
        let metrics = part
            .burst
            .frame_metrics(key)
            .expect("已接收的帧缺少测量指标");
        if let Some(overlay) = overlay {
            crate::workspace::retain_live_view(
                &self.app,
                &part.cycle_id,
                &part.recipe,
                segment,
                overlay,
                metrics,
            );
        }
        self.dirty = true;
        let camera = part.burst.camera_name(metrics.camera).to_owned();
        let seq = metrics.seq;
        let fields = Fields { cycle_id: Some(part.cycle_id.clone()), camera: Some(camera.clone()), seq: Some(seq), ..Fields::default() }.with_stage("measured")
            .with_data(json!({"ms": ms, "queueMs": queue_ms, "engineMs": engine_ms, "state": metrics.state, "error": metrics.error}));
        let why = metrics.error.as_deref().unwrap_or_default();
        let untested = part
            .production
            .as_ref()
            .is_some_and(|p| p.untested(metrics.camera, segment).is_some());
        match metrics.state {
            FrameState::Measured => trace(
                "测量完成",
                format!(
                    "{camera} 第 {seq} 帧 · 最大胶宽 {} · {ms} ms",
                    metrics
                        .width_max
                        .map_or_else(|| "—".to_owned(), |w| format!("{w:.2} mm"))
                ),
                fields,
            ),
            FrameState::OutOfCorridor => trace(
                "走出走廊",
                format!("{camera} 第 {seq} 帧胶条走出走廊，记检测不完整"),
                fields,
            ),
            FrameState::NotInspected => trace(
                "本段不检测",
                format!("{camera} 第 {seq} 帧所在段不由这台相机检测，不测"),
                fields,
            ),
            FrameState::Unmeasured if untested => trace(
                "本段不测",
                format!("{camera} 第 {seq} 帧所在段预热没测成，不测，记检测不完整"),
                fields,
            ),
            FrameState::Unmeasured => log_with(
                "warn",
                "帧没测",
                format!("{camera} 第 {seq} 帧没测（{why}），记检测不完整"),
                fields,
            ),
            FrameState::Error | FrameState::Missing => log_with(
                "warn",
                "测量出错",
                format!("{camera} 第 {seq} 帧测量失败（{why}），记检测不完整"),
                fields,
            ),
        }
    }

    async fn on_tick(&mut self) {
        if self.stats_checked.elapsed() >= Duration::from_secs(1) {
            self.refresh_stats_day();
        }
        let app = self.app.clone();
        self.flush_noise();
        let _link = host(&app).link_gate.lock().await;
        let settings = host(&app).settings();
        let progress_app = app.clone();
        self.dirty |= self.maintenance_progress.poll(
            settings.maintenance.as_ref(),
            Instant::now(),
            move || {
                progress_app
                    .state::<Store>()
                    .maintenance_statistics(HistoryScope::Real)
                    .unwrap_or_default()
            },
        );
        self.s7.set_maintenance(settings.maintenance.is_some());
        if !host(&app).busy() && matches!(self.phase, Phase::Idle | Phase::Fault) {
            // 配置操作进行中就留到下一拍，不让空闲节拍排队
            if let Ok(_config) = host(&app).config_gate.try_lock() {
                if crate::workspace::apply_pending(&self.app) {
                    self.required = None;
                    self.dirty = true;
                }
            }
        }
        self.sync_recording_alarm();
        let cameras = match self.phase {
            // 在途的件只看它自己用的相机
            Phase::Acquire | Phase::Drain => match self.part.as_ref() {
                Some(p) if p.rig_gen != host(&self.app).camera.generation() => {
                    Err("检测中增删了相机".into())
                }
                Some(p) => self.check_part_cams(p),
                None => Ok(()),
            },
            _ => self.check_idle_devices(),
        };
        let dropped = host(&self.app).camera.dropped_total();
        if dropped > self.dropped_seen {
            let added = dropped - self.dropped_seen;
            self.dropped_seen = dropped;
            let msg =
                format!("帧通道满，新丢弃 {added} 帧（累计 {dropped} 帧）：检测节拍处理不过来");
            let fields = Fields::default().with_data(json!({"dropped": added, "total": dropped}));
            self.noisy(
                &events::FRAME_DROPPED,
                &events::FRAME_DROPPED_SUMMARY,
                "",
                Tally::Sum("dropped", added),
                msg,
                fields,
            );
        }
        // 图像内存预算满（D-SB-32）：帧没拷图、不交付，相机页也不刷新；报警并记到在途件上（D-SB-60）
        let budget = host(&self.app).camera.budget_dropped_total();
        if budget < self.budget_seen {
            self.budget_seen = budget;
        }
        if budget > self.budget_seen {
            let added = budget - self.budget_seen;
            self.budget_seen = budget;
            if matches!(self.phase, Phase::Acquire | Phase::Drain) {
                if let Some(part) = self.part.as_mut() {
                    part.burst.budget_dropped += added;
                }
            }
            let msg = format!(
                "图像内存预算（每台相机 {} MiB）已满，新丢弃 {added} 帧（累计 {budget} 帧）：这些帧没收进来，在途件记检测不完整",
                crate::camera::IMAGE_BUDGET_PER_CAMERA >> 20
            );
            let fields = Fields::default().with_data(json!({"dropped": added, "total": budget}));
            self.noisy(
                &events::FRAME_BUDGET_DROPPED,
                &events::FRAME_BUDGET_DROPPED_SUMMARY,
                "",
                Tally::Sum("dropped", added),
                msg,
                fields,
            );
        }
        {
            self.sync_disk_alarm();
            if let Err(reason) = cameras {
                self.latch_device_lost(reason);
            }
            let previous = self.s7.view();
            let event = self.s7.poll(plc(&app)).await;
            // 接件持有配置门直到下面 store_busy 登记在途，相机与配置操作要么在接件前完成，要么看到在途后拒绝
            let mut _config = None;
            match event {
                SessionEvent::Ready => {
                    self.ledgers.clear_floors();
                    self.clear_fault();
                    self.alarms.clear();
                    self.set_phase(Phase::Idle);
                    log_event(
                        &applog::events::S7_READY,
                        "PLC 已连接、协议版本一致、心跳在变化，通讯就绪",
                        Fields::default()
                            .with_data(json!({"connection": self.s7.view().connection})),
                    );
                    applog::end_cycle();
                }
                SessionEvent::Start(request) => {
                    _config = Some(host(&app).config_gate.lock().await);
                    self.start_part(request).await;
                }
                SessionEvent::End(ended_at) => {
                    if let Some(part) = self.part.as_mut() {
                        part.end_at = Some(ended_at);
                    }
                    self.set_phase(Phase::Drain);
                    log_with(
                        "info",
                        "工件结束",
                        "PLC 报工件结束，等待剩余帧",
                        Fields::stage("partEnd"),
                    );
                }
                SessionEvent::Lost(reason) => self.abandon_part("s7", reason).await,
                SessionEvent::None => {}
            }
            // 空闲提示（没接上的在检件等）跟着会话现状走，不进故障
            let notice = self.s7.notice().map(str::to_owned);
            let shown: Vec<String> = self
                .alarms
                .iter()
                .filter(|m| m.starts_with(crate::plc_session::NOTICE))
                .cloned()
                .collect();
            if shown.as_slice() != notice.as_slice() {
                self.alarms
                    .retain(|m| !m.starts_with(crate::plc_session::NOTICE));
                self.alarms.extend(notice);
                self.dirty = true;
            }
            if self.s7.phase() == SessionPhase::NotReady {
                let reason = self
                    .s7
                    .view()
                    .message
                    .unwrap_or_else(|| "S7 通讯未就绪".into());
                if self.phase != Phase::Fault || self.fault.as_ref() != Some(&reason) {
                    self.enter_fault("s7", reason).await;
                }
            }
            let current = self.s7.view();
            if (previous.phase, &previous.message) != (current.phase, &current.message) {
                self.dirty = true;
            }
            host(&app).store_busy(self.phase, &self.s7);
        }
        let timeouts = host(&self.app).settings().timeouts;
        if matches!(self.phase, Phase::Acquire | Phase::Drain) {
            if let Some(part) = self.part.as_mut() {
                let expired = part.expire_burst(Instant::now(), timeouts.proc());
                if !expired.is_empty() {
                    let frames: Vec<String> = expired
                        .iter()
                        .take(8)
                        .map(|(camera, seq)| {
                            format!("相机 {} 第 {seq} 帧", part.burst.camera_name(*camera))
                        })
                        .collect();
                    let more = if expired.len() > frames.len() {
                        format!(" 等 {} 帧", expired.len())
                    } else {
                        String::new()
                    };
                    log_with(
                        "warn",
                        "帧测量超时",
                        format!(
                            "{}{more} 测量超时，记为没测（检测不完整）",
                            frames.join("、")
                        ),
                        Fields {
                            cycle_id: Some(part.cycle_id.clone()),
                            ..Fields::default()
                        },
                    );
                    self.dirty = true;
                }
            }
        }
        if let Some(bare) = self.uninspected.as_ref() {
            match self.phase {
                Phase::Acquire if bare.admitted_at.elapsed() > timeouts.motion() => {
                    self.part_end_timeout(timeouts.motion_ms).await;
                }
                Phase::Drain => {
                    let bare = self.uninspected.take().unwrap();
                    self.report(bare.sn, bare.recipe_id, bare.judgement).await;
                }
                _ => {}
            }
            return;
        }
        match self.phase {
            Phase::Acquire => {
                if self
                    .part
                    .as_ref()
                    .is_some_and(|part| part.admitted_at.elapsed() > timeouts.motion())
                {
                    self.part_end_timeout(timeouts.motion_ms).await;
                }
            }
            Phase::Drain => {
                if let Some(part) = self.part.as_mut() {
                    for (camera, seq, segment, file) in part.start_post_measure(Instant::now()) {
                        let camera_id = part.burst.camera_name(camera).to_owned();
                        trace(
                            "收尾补测",
                            format!(
                                "cycle={} · {camera_id} 第 {seq} 帧用第 {segment} 段走廊补测",
                                part.cycle_id
                            ),
                            Fields {
                                cycle_id: Some(part.cycle_id.clone()),
                                camera: Some(camera_id.clone()),
                                seq: Some(seq),
                                ..Fields::default()
                            }
                            .with_stage("post"),
                        );
                        self.frame_pool.submit(FrameJob {
                            image_file: file,
                            ..frame_job(part, camera, camera_id, seq, segment, None)
                        });
                    }
                }
                if self.part.as_ref().is_some_and(Part::complete) {
                    self.judge_part().await;
                }
            }
            _ => {}
        }
    }

    /// 等不到工件结束：本件记 ERR 97、不回结果，放下它等下一件。
    async fn part_end_timeout(&mut self, motion_ms: u64) {
        let reason = format!(
            "接件后 {} s 内未收到工件结束（inspecting 未回 0）",
            motion_ms / 1000
        );
        log("err", "等工件结束超时", format!("T_motion {motion_ms} ms"));
        self.abandon_part("motion", reason).await;
    }

    /// 在途件等不到正常结束（中途换件、等工件结束超时）：按中断记录、不回结果；通讯仍然就绪，回空闲等下一件。
    async fn abandon_part(&mut self, source: &'static str, reason: String) {
        self.close_routing();
        self.interrupt_part(source, &reason).await;
        self.alarm(reason);
        applog::end_cycle();
        self.set_phase(Phase::Idle);
    }

    async fn judge_part(&mut self) {
        if let Some(part) = self.part.as_ref() {
            if let Err(error) = self.check_part_routing(part) {
                self.part.as_mut().unwrap().fault.get_or_insert(error);
            }
        }
        self.set_phase(Phase::Judge);
        let Some(part) = self.part.as_mut() else {
            return;
        };
        let mut warnings = Vec::new();
        let judgement = if part.mode == crate::store::PartMode::MaintenanceCollect {
            let groups = part.burst.group_count();
            let reason = match &part.fault {
                Some((_, why)) => format!("维护采集：只录图、不判定（{groups} 个帧组）；{why}"),
                None => format!("维护采集：只录图、不判定（{groups} 个帧组）"),
            };
            uninspected_judgement(0, reason)
        } else {
            let mut decided = part.burst_judgement();
            if let Some(label) = part
                .fault
                .is_none()
                .then(|| part.production.as_ref().and_then(|p| p.untested_label()))
                .flatten()
            {
                decided.reason = format!(
                    "{}；{label} 预热没测成、未测，结论只覆盖其余段",
                    decided.reason
                );
            }
            let burst = &mut part.burst;
            warnings = burst_warnings(&decided, |camera| burst.camera_name(camera).to_owned());
            let summary = burst::summary(&decided);
            burst.set_judgement(decided);
            summary
        };
        let (sn, id, cycle_id) = (
            part.sn.clone(),
            part.recipe.id.clone(),
            part.cycle_id.clone(),
        );
        for (ev, message) in warnings {
            log_with(
                "warn",
                ev,
                message,
                Fields {
                    cycle_id: Some(cycle_id.clone()),
                    ..Fields::default()
                },
            );
        }
        self.report(sn, Some(id), judgement).await;
    }

    async fn report(&mut self, sn: String, recipe_id: Option<String>, mut judgement: Judgement) {
        // 维护模式：PLC 按未检放行，回写与记录的结果码都是 0；试判结论留在 verdict 与 faultCode
        let maintenance = self.s7.part_maintenance();
        if maintenance {
            judgement.plc_code = 0;
        }
        let mode = self.part_mode(&sn);
        // 下一件基线只看见到的帧计数，直接收尾
        self.close_routing();
        let app = self.app.clone();
        let mut drain_ms = None;
        let recorded_at = now_ms();
        let pending = PlcDelivery {
            state: PlcDeliveryState::Pending,
            updated_at: recorded_at,
            message: None,
        };
        let record = self.recorded_part(
            &sn,
            recipe_id.as_deref(),
            &judgement,
            drain_ms,
            pending,
            mode,
        );
        let burst = record
            .as_ref()
            .ok()
            .and_then(|part| part.burst.as_ref())
            .map(|b| b.judgement.clone());
        // 记录写不写得进都照常回写结果：追溯尽力而为，不挡 PLC 交互
        let recorded = match record {
            Ok(part) => {
                let (recording, missing) = match self.no_part_recording(&sn, &judgement) {
                    Ok(recording) => (recording, Ok(())),
                    Err(error) => (None, Err(error)),
                };
                host(&app).audit.insert_with_recording(part, recording);
                missing
            }
            Err(error) => Err(error),
        };
        if let Err(error) = recorded {
            log(
                "err",
                "检测记录",
                format!("本件检测记录未生成，结果照常回写 PLC：{error}"),
            );
        }
        let (code, fault_code) = if maintenance {
            (0, 0)
        } else {
            (judgement.plc_code, judgement.fault_code)
        };
        let timing = self.s7.part_timing();
        let sent = self.s7.report(plc(&app), code, fault_code).await;
        drain_ms = sent
            .as_ref()
            .ok()
            .and_then(|_| self.part.as_ref().and_then(|part| part.end_at))
            .map(|end| end.elapsed().as_millis() as u64);
        let delivery = PlcDelivery {
            state: if sent.is_ok() {
                PlcDeliveryState::Submitted
            } else {
                PlcDeliveryState::Failed
            },
            updated_at: now_ms(),
            message: sent
                .as_ref()
                .err()
                .map(|error| format!("回写 PLC 失败：{error}")),
        };
        let plc_write = timing.map(|timing| PlcWriteTrace {
            timing,
            state: if sent.is_ok() {
                PlcWriteState::Written
            } else {
                PlcWriteState::Failed
            },
            updated_at: delivery.updated_at,
            message: delivery.message.clone(),
        });
        match self.current_cycle_id.as_deref() {
            Some(id) => host(&app).audit.submission(id, delivery, drain_ms),
            None => log("err", "PLC 交付", "缺少工件身份，交付状态未记录"),
        }
        let r = sent.map_err(|error| format!("回写 PLC 失败：{error}"));
        self.count(mode, recorded_at, judgement.verdict);
        let level = match judgement.verdict {
            Verdict::Ok | Verdict::OkWithExcursion => "ok",
            Verdict::ErrInspect => "err",
            _ => "ng",
        };
        let fault = if judgement.fault_code > 0 {
            format!(" faultCode={}", judgement.fault_code)
        } else {
            String::new()
        };
        let delivery = if r.is_ok() {
            "已回写 PLC"
        } else {
            "结果未回写 PLC"
        };
        log_with(
            level,
            "检测结果",
            format!(
                "resultCode={}{fault} snEcho={sn} · {delivery} · {}",
                judgement.plc_code, judgement.reason
            ),
            Fields::stage("result").with_data(
                json!({"verdict": judgement.verdict, "plcCode": judgement.plc_code,
                "faultCode": judgement.fault_code, "drainMs": drain_ms, "delivered": r.is_ok()}),
            ),
        );
        self.finish_recording(&sn, &judgement);
        self.result = Some(ResultView {
            cycle_id: self.current_cycle_id.clone(),
            sn,
            recipe_id,
            ts: now_ms(),
            drain_ms,
            plc_write,
            judgement,
            burst,
            mode,
        });
        if let Err(reason) = r {
            self.alarm(reason);
        }
        applog::end_cycle();
        self.set_phase(Phase::Idle);
    }

    fn close_routing(&mut self) {
        if let Some(part) = self.part.as_mut() {
            part.close_routing(&mut self.ledgers);
        }
    }

    /// 在途件的相机掉线只让本件记 ERR 98，不打断握手；相机恢复后下一件照常检测。
    fn latch_device_lost(&mut self, reason: String) {
        if !matches!(self.phase, Phase::Acquire | Phase::Drain) {
            return;
        }
        let Some(part) = self.part.as_mut() else {
            return;
        };
        if part.fault.is_some() {
            return;
        }
        log(
            "err",
            "相机",
            format!("在途件相机异常，本件记 ERR 98：{reason}"),
        );
        part.fault = Some((fault::DEVICE_LOST, reason));
        self.dirty = true;
    }

    fn check_part_cams(&self, part: &Part) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        rig.check_ready_at(&part.cams)?;
        let identities = part
            .cams
            .iter()
            .map(|&cam| {
                rig.slot(cam as usize)
                    .ok_or_else(|| "相机组已变化".to_string())?
                    .routing_state()
                    .map(|state| state.0)
            })
            .collect::<Result<Vec<_>, _>>()?;
        part.burst.check_sessions(&identities)
    }

    fn check_part_routing(&self, part: &Part) -> Result<(), (u16, String)> {
        self.check_part_cams(part)
            .map_err(|reason| (fault::DEVICE_LOST, reason))?;
        let rig = &host(&self.app).camera;
        for &cam in &part.cams {
            let slot = rig
                .slot(cam as usize)
                .ok_or_else(|| (fault::DEVICE_LOST, "相机组已变化".into()))?;
            let (_, observed) = slot
                .routing_state()
                .map_err(|reason| (fault::DEVICE_LOST, reason))?;
            if let Some(ledger) = observed {
                part.burst.check_observed(cam, &ledger)?;
            }
        }
        Ok(())
    }

    fn finish_recording(&mut self, sn: &str, judgement: &Judgement) {
        let Some(rec) = self
            .part
            .as_mut()
            .filter(|p| p.sn == sn)
            .and_then(|p| p.recording.take())
        else {
            return;
        };
        let host = host(&self.app);
        let settings = host.settings();
        host.recorders
            .finish(rec, judgement.verdict, &judgement.reason, &settings);
    }

    fn recorded_part(
        &self,
        sn: &str,
        recipe_id: Option<&str>,
        judgement: &Judgement,
        drain_ms: Option<u64>,
        delivery: PlcDelivery,
        mode: PartMode,
    ) -> Result<RecordedPart, String> {
        let Some(cycle_id) = self.current_cycle_id.clone() else {
            return Err("工件身份尚未持久分配，不能关联历史与原图".into());
        };
        let part = self.part.as_ref().filter(|p| p.sn == sn);
        let recipe = part
            .map(|p| p.recipe.clone())
            .or_else(|| recipe_id.and_then(|id| host(&self.app).recipe(id)));
        let burst = match part {
            Some(part) => Some(part.burst_record(judgement)),
            None => recipe
                .as_deref()
                .map(|recipe| burst::empty_record(recipe, judgement)),
        };
        // 预期帧数取 ΣN_exp（待标定为 0），收到的帧数取帧组数，触发数取 PLC 报的 deviceTriggers
        let frames_expected = burst
            .as_ref()
            .map_or(0, |b| b.expected_total.unwrap_or(0) as usize);
        let (received, triggers) = burst.as_ref().map_or((0, 0), |b| {
            (
                b.frames.iter().map(|f| f.seq).max().unwrap_or(0) as usize,
                b.device_triggers.map_or(0, u64::from),
            )
        });
        let source = part.map_or_else(|| RecordSource::Real, |p| p.source);
        Ok(RecordedPart {
            ts: delivery.updated_at,
            sn: sn.to_owned(),
            recipe,
            judgement: judgement.clone(),
            drain_ms,
            frames: Vec::new(),
            frames_expected,
            frames_received: received,
            triggers,
            table: None,
            software_version: self.app.package_info().version.to_string(),
            cycle_id: cycle_id.clone(),
            bundle_id: part.and_then(|p| p.bundle_id.clone()),
            delivery,
            shots: Vec::new(),
            source,
            burst,
            mode,
        })
    }

    fn record_without_part(&self, sn: &str, judgement: &Judgement) -> Result<(), String> {
        if let Some(outcome) = self.no_part_recording(sn, judgement)? {
            host(&self.app).audit.recording(outcome);
        }
        Ok(())
    }

    /// 没建立检测的件（接件被拒）没有录制，补一条录制结果；建立了检测的件返回 None。
    fn no_part_recording(
        &self,
        sn: &str,
        judgement: &Judgement,
    ) -> Result<Option<crate::recorder::RecordingOutcome>, String> {
        if self.part.as_ref().is_some_and(|part| part.sn == sn) {
            return Ok(None);
        }
        let cycle_id = self.current_cycle_id.clone().ok_or("工件身份缺失")?;
        let off = host(&self.app).settings().record == crate::settings::RecordMode::Off;
        Ok(Some(crate::recorder::RecordingOutcome {
            cycle_id,
            directory: None,
            files: Vec::new(),
            retention_errors: Vec::new(),
            available: false,
            state: if off {
                crate::recorder::RecordingState::Off
            } else {
                crate::recorder::RecordingState::Failed
            },
            errors: if off {
                Vec::new()
            } else {
                vec![format!("接件校验未通过，未开始录制：{}", judgement.reason)]
            },
        }))
    }

    /// 没建立检测的件（拒收或维护模式未检）按会话锁存的模式记。
    fn part_mode(&self, sn: &str) -> PartMode {
        self.part
            .as_ref()
            .filter(|part| part.sn == sn)
            .map(|part| part.mode)
            .unwrap_or(if self.s7.part_maintenance() {
                PartMode::MaintenanceCollect
            } else {
                PartMode::Production
            })
    }

    fn record(
        &self,
        sn: &str,
        recipe_id: Option<&str>,
        judgement: &Judgement,
        drain_ms: Option<u64>,
        delivery: PlcDelivery,
        mode: PartMode,
    ) -> Result<(), String> {
        host(&self.app)
            .audit
            .insert(self.recorded_part(sn, recipe_id, judgement, drain_ms, delivery, mode)?);
        self.record_without_part(sn, judgement)
    }

    fn refresh_stats_day(&mut self) -> NaiveDate {
        self.stats_checked = Instant::now();
        let today = Local::now().date_naive();
        if self.stats.advance(today) {
            self.dirty = true;
        }
        today
    }

    fn count(&mut self, mode: PartMode, recorded_at: i64, verdict: Verdict) {
        let today = self.refresh_stats_day();
        if self.stats.record(mode, recorded_at, verdict) && self.history_purge_day != today {
            self.history_purge_day = today;
            let app = self.app.clone();
            tauri::async_runtime::spawn_blocking(move || purge_history(&app));
        }
    }

    /// 同一来源同一原因的故障在汇总前被重复上报的次数，写一条 fault.repeat。
    fn flush_fault_repeat(&mut self) {
        let Some(FaultRepeat {
            source,
            reason,
            count,
            first,
            last,
        }) = self.fault_repeat.take()
        else {
            return;
        };
        let fields = Fields {
            source: Some(source.into()),
            ..Fields::default()
        }
        .with_data(json!({"count": count, "firstTs": first, "lastTs": last, "reason": reason}));
        log_event(
            &applog::events::FAULT_REPEAT,
            format!("同一故障又上报 {count} 次：{reason}"),
            fields,
        );
    }

    fn clear_fault(&mut self) {
        if let Some(reason) = self.fault.clone() {
            self.flush_fault_repeat();
            let duration = self.fault_at.map(|at| now_ms() - at);
            let fields = Fields {
                source: self.fault_source.map(str::to_owned),
                ..Fields::default()
            }
            .with_data(json!({"reason": reason, "durationMs": duration}));
            log_event(
                &applog::events::FAULT_CLEARED,
                format!("故障解除：{reason}"),
                fields,
            );
        }
        self.fault = None;
        self.fault_source = None;
        self.fault_at = None;
    }

    /// 在途件中断：记 ERR（等工件结束超时 97，其余 98），不回结果，会话放下这一件。
    async fn interrupt_part(&mut self, source: &'static str, reason: &str) {
        let in_flight = self.s7.request().is_some()
            && matches!(
                self.phase,
                Phase::Validate | Phase::Acquire | Phase::Drain | Phase::Judge
            );
        if in_flight {
            let sn = self
                .part
                .as_ref()
                .map(|p| p.sn.clone())
                .or_else(|| self.s7.request().map(|r| r.sn.clone()))
                .unwrap_or_default();
            let code = if source == "motion" {
                fault::MOTION_TIMEOUT
            } else {
                fault::DEVICE_LOST
            };
            let judgement = Judgement::error(code, format!("{reason}，结果未回写"));
            let delivery = PlcDelivery {
                state: PlcDeliveryState::Failed,
                updated_at: now_ms(),
                message: Some(reason.to_owned()),
            };
            let mode = self.part_mode(&sn);
            self.count(mode, delivery.updated_at, judgement.verdict);
            let plc_write = self.s7.part_timing().map(|timing| PlcWriteTrace {
                timing,
                state: PlcWriteState::NotWritten,
                updated_at: delivery.updated_at,
                message: delivery.message.clone(),
            });
            if let Err(error) = self.record(
                &sn,
                self.part.as_ref().map(|p| p.recipe.id.clone()).as_deref(),
                &judgement,
                None,
                delivery,
                mode,
            ) {
                log("err", "中断工件审计持久化失败", error);
            }
            self.finish_recording(&sn, &judgement);
            log_with(
                "err",
                "在途件中断",
                format!("SN {sn} 记 ERR {code}，结果未回写 PLC"),
                Fields::stage("result"),
            );
            let burst = self
                .part
                .as_ref()
                .map(|part| part.burst_record_judgement(&judgement));
            let mode = self.part_mode(&sn);
            self.result = Some(ResultView {
                cycle_id: self.current_cycle_id.clone(),
                sn,
                recipe_id: self.part.as_ref().map(|p| p.recipe.id.clone()),
                ts: now_ms(),
                drain_ms: None,
                plc_write,
                judgement,
                burst,
                mode,
            });
        }
        self.uninspected = None;
        self.s7.settle(plc(&self.app), reason).await;
    }

    /// 通讯未就绪。已因同一来源同一原因处于故障时只计数；在途件按中断记录。
    async fn enter_fault(&mut self, source: &'static str, reason: String) {
        if self.phase == Phase::Fault
            && self.fault.as_deref() == Some(reason.as_str())
            && self.fault_source == Some(source)
        {
            let now = now_ms();
            let repeat = self.fault_repeat.get_or_insert_with(|| FaultRepeat {
                source,
                reason,
                count: 0,
                first: now,
                last: now,
            });
            repeat.count += 1;
            repeat.last = now;
            return;
        }
        self.flush_fault_repeat();
        self.close_routing();
        self.interrupt_part(source, &reason).await;
        let fields = Fields {
            source: Some(source.into()),
            ..Fields::stage("fault")
        }
        .with_data(json!({"reason": reason, "s7Phase": format!("{:?}", self.s7.phase())}));
        log_event(&applog::events::FAULT_ENTER, reason.clone(), fields);
        self.fault = Some(reason);
        self.fault_source = Some(source);
        self.fault_at = Some(now_ms());
        self.set_phase(Phase::Fault);
    }
}

/// 同一来源同一原因的故障被再次上报：原因记在计数里，汇总时不受之后改写的故障原因影响
struct FaultRepeat {
    source: &'static str,
    reason: String,
    count: u64,
    first: i64,
    last: i64,
}

/// 段配方判定里只告警的项（丢帧、偏宽预警），写进运行日志。
fn burst_warnings(
    judgement: &BurstJudgement,
    camera: impl Fn(usize) -> String,
) -> Vec<(&'static str, String)> {
    use crate::frame_group::LossReason;
    let mut out: Vec<(&'static str, String)> = judgement
        .alarms
        .iter()
        .map(|alarm| {
            let name = camera(alarm.camera);
            let basis = match &alarm.reason {
                LossReason::Triggers { device_triggers } => {
                    format!("PLC 报 deviceTriggers {device_triggers}")
                }
                LossReason::Cameras { other, other_count } => {
                    format!("相机 {} 本件 {other_count} 帧", camera(*other))
                }
            };
            (
                "丢帧告警",
                format!(
                    "相机 {name} 本件 {} 帧，与{basis}相差超过 1：疑似丢帧（只告警，不改结论）",
                    alarm.count
                ),
            )
        })
        .collect();
    for (set, title, what) in [
        (judgement.flags.wide_warning, "胶宽偏宽", "超出胶宽上限"),
        (judgement.flags.overflow_warning, "溢胶预警", "超出溢胶上限"),
    ] {
        if set {
            out.push((
                title,
                format!("有帧{what}，本段设为偏宽只预警（只告警，PLC 照常收到合格）"),
            ));
        }
    }
    out
}

fn today_stats(app: &AppHandle, day: NaiveDate) -> Stats {
    app.state::<Store>()
        .counts_since(history::local_midnight_ms(day), HistoryScope::Real)
        .map(Stats::from)
        .unwrap_or_default()
}

#[tauri::command]
pub fn cycle_snapshot(cycle: State<'_, CycleHost>) -> Option<Arc<Snapshot>> {
    cycle.shared.lock().unwrap().snapshot.clone()
}

#[tauri::command]
pub fn cycle_control_state(cycle: State<'_, CycleHost>) -> Option<ControlState> {
    cycle_snapshot(cycle).map(|snapshot| snapshot.control_state())
}

#[tauri::command]
pub fn cycle_recipes(cycle: State<'_, CycleHost>) -> Vec<RecipeSummary> {
    cycle
        .recipes
        .list()
        .iter()
        .map(|r| RecipeSummary::listed(&cycle.recipes, r))
        .collect()
}

/// 配方的运行数据。给了 revision_id 时要的是那一版：工件用的配方刚改过，这一件仍按开工时的快照画。
#[tauri::command]
pub fn cycle_layout(
    cycle: State<'_, CycleHost>,
    recipe_id: String,
    revision_id: Option<String>,
) -> Result<Arc<Recipe>, String> {
    if let Some(r) = cycle
        .shared
        .lock()
        .unwrap()
        .part_recipe
        .clone()
        .filter(|r| r.id == recipe_id && revision_id.as_ref() == Some(&r.revision_id))
    {
        return Ok(r);
    }
    let r = cycle
        .recipe(&recipe_id)
        .ok_or_else(|| format!("配方不存在：{recipe_id}"))?;
    match revision_id {
        Some(h) if h != r.revision_id => Err(format!("配方 {recipe_id} 已经改过，找不到这一版")),
        _ => Ok(r),
    }
}

#[tauri::command]
pub fn cycle_get_settings(cycle: State<'_, CycleHost>) -> CycleSettings {
    cycle.settings()
}

#[tauri::command]
pub fn cycle_save_settings(
    app: AppHandle,
    cycle: State<'_, CycleHost>,
    mut settings: CycleSettings,
) -> Result<(), String> {
    settings.maintenance = cycle.settings().maintenance;
    if let Some(id) = &settings.manual_recipe_id {
        cycle.recipe(id).ok_or("配方不存在")?;
    }
    cycle.save_settings(settings)?;
    measure::apply_settings(&app);
    let _ = app.emit("cycle://settings-changed", ());
    crate::commands::log_config(&app);
    Ok(())
}

/// 开启或退出维护模式，从下一件起生效（在途件按接件时锁存的模式完成）。开启须填写原因。
#[tauri::command]
pub fn cycle_set_maintenance(
    app: AppHandle,
    cycle: State<'_, CycleHost>,
    enabled: bool,
    reason: String,
) -> Result<(), String> {
    let collect_only: Vec<String> = cycle
        .recipes
        .list()
        .into_iter()
        .filter(|recipe| recipe.is_collect())
        .map(|recipe| recipe.id.clone())
        .collect();
    let result = set_maintenance(&cycle, enabled, reason.trim());
    let action = if enabled { "enter" } else { "exit" };
    log_op(
        &events::OP_MAINTENANCE,
        json!({"action": action, "reason": reason.trim(), "collectOnly": collect_only}),
        &result,
    );
    if result.is_ok() {
        let _ = app.emit("cycle://settings-changed", ());
    }
    result
}

fn set_maintenance(cycle: &CycleHost, enabled: bool, reason: &str) -> Result<(), String> {
    if enabled && reason.is_empty() {
        return Err("开启维护模式须填写原因".into());
    }
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在接件或处理其他配置操作，请稍后重试")?;
    let mut settings = cycle.settings();
    settings.maintenance = enabled.then(|| crate::settings::Maintenance {
        since: now_ms(),
        reason: reason.to_owned(),
    });
    settings.save(&cycle.settings_path)?;
    cycle.shared.lock().unwrap().settings = settings;
    let _ = cycle.tx.send(Input::Refresh);
    Ok(())
}

/// 人工选择配方，只能在空闲或故障时切换，新配方从下一个工件起生效。
#[tauri::command]
pub fn cycle_select_recipe(cycle: State<'_, CycleHost>, recipe_id: String) -> Result<(), String> {
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在接件或处理其他配置操作，请稍后重试")?;
    cycle.recipe(&recipe_id).ok_or("配方不存在")?;
    let mut shared = cycle.shared.lock().unwrap();
    let phase = shared.snapshot.as_ref().map_or(Phase::Idle, |s| s.phase);
    if cycle.busy() || !matches!(phase, Phase::Idle | Phase::Fault) {
        return Err("检测进行中，工件结束后再切换".into());
    }
    let mut settings = shared.settings.clone();
    settings.manual_recipe_id = Some(recipe_id);
    settings.save(&cycle.settings_path)?;
    shared.settings = settings;
    let _ = cycle.tx.send(Input::Refresh);
    Ok(())
}

/// 记录操作员命令的参数摘要与结果。
pub fn log_op<T>(
    event: &applog::events::Event,
    mut data: serde_json::Value,
    result: &Result<T, String>,
) {
    data["ok"] = json!(result.is_ok());
    if let Err(error) = result {
        data["error"] = json!(error);
    }
    let msg = match result {
        Ok(_) => format!("{}：完成", event.ev),
        Err(error) => format!("{}：失败，{error}", event.ev),
    };
    log_event(
        event,
        msg,
        Fields {
            source: Some("operator".into()),
            ..Fields::default()
        }
        .with_data(data),
    );
}

/// 开工就绪报告：逐项列出 PLC 会话、相机、可开工配方、图像测量、审计录制与站点模式。
#[tauri::command]
pub async fn cycle_readiness(app: AppHandle) -> Result<readiness::Readiness, String> {
    tauri::async_runtime::spawn_blocking(move || readiness::gather(&app))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests;

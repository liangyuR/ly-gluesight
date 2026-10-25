mod admission;
mod burst;
mod capture;
mod interrupted;
pub mod readiness;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ly_plc::{now_ms, LinkState, PlcEngine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender};

use crate::applog::codes::{self, Event};
use crate::applog::throttle::{self, Tally, Throttle};
use crate::applog::{self, Fields};
use crate::arming::ArmBudget;
use crate::burst_judge::{BurstJudgement, FrameState};
use crate::camera::{CameraRig, FRAME_QUEUE};
use crate::frame::Frame;
use crate::history;
use crate::handshake::Request;
use crate::judge::{self, fault, Judgement, PointState, Verdict};
use crate::measure::{self, FrameJob, FrameMeasured, FramePool, Job, Measured, WorkerHealth};
use crate::plc::PlcHost;
use crate::plc_session::{Origin, PartSite, PendingKind, PlcSession, SessionEvent, SessionPhase, SessionView};
use crate::recipe::{Recipe, RecipeStore, TriggerMode};
use crate::recorder::{Recorder, Recorders, Recording};
use crate::recording_health::{AlarmChange, RecordingAlarm, RecordingHealth};
use crate::locate_health::{LocateAlarm, LocateChange, LocateHealth, Subject};
use crate::settings::{CycleSettings, ProductSource};
use crate::shot_router::{arm_frames, ArmCam, Floor, FrameMeta, Ledgers, Plan, Policy, Route, ShotRouter};
use crate::offline::Scenario;
use crate::audit::{Audit, RecordedPart};
use crate::store::{BurstRecord, HistoryScope, PartShot, PlcDelivery, PlcDeliveryState, RecordSource, Store, VerdictCounts};

pub(crate) fn s7_ack_delivery(had_ack: bool, session: &PlcSession) -> Option<PlcDelivery> {
    (!had_ack && session.acknowledged()).then(|| PlcDelivery {
        state: PlcDeliveryState::Acknowledged,
        updated_at: now_ms(),
        message: Some("PLC 结果序号已匹配确认".into()),
    })
}

pub enum Input {
    Measured(Measured),
    /// 段配方一台相机一帧的测量结果（FramePool 交来）
    FrameMeasured(FrameMeasured),
    Reset,
    Refresh,
    SiteChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Phase {
    Idle,
    Validate,
    Acquire,
    Drain,
    Judge,
    Report,
    Release,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FrameStatus {
    Waiting,
    Measuring,
    /// 测完了。没有位置基准的视图另记在 `FrameView::reference_failed`
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
    /// 用特征定位的视图的位姿（含没被采用的），追溯与叠图用；没用定位或测量出错时为空
    pub pose: Option<crate::vision::PoseRecord>,
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
    /// 没有位置基准的视图与原因（帧徽标）；这些视图的有胶站已是 ST_NO_REF，判定只看站状态
    #[serde(default)]
    pub reference_failed: Vec<measure::ReferenceFailure>,
}

impl FrameView {
    pub(crate) fn waiting() -> Self {
        Self {
            view_results: Vec::new(),
            status: FrameStatus::Waiting,
            cam: 0,
            camera: String::new(),
            shot_id: String::new(),
            view: 1,
            session: None,
            ordinal: None,
            error: None,
            arrived_ms: None,
            frame_counter: None,
            trigger_counter: None,
            counter_jump: false,
            score: None,
            points: 0,
            gap_points: 0,
            ms: None,
            reference_failed: Vec::new(),
        }
    }
}

fn recorded_shots(recipe: Option<&Recipe>, frames: &[FrameView], reason: &str) -> Vec<PartShot> {
    let Some(recipe) = recipe else { return Vec::new() };
    recipe.shots.iter().enumerate().map(|(k, shot)| {
        let frame = frames.get(k).cloned().unwrap_or_else(FrameView::waiting);
        let (status, error) = match frame.status {
            FrameStatus::Waiting => (FrameStatus::Missing, Some(format!("未收到计划帧：{reason}"))),
            FrameStatus::Measuring => (FrameStatus::Error, Some(format!("测量未完成：{reason}"))),
            status => (status, frame.error),
        };
        PartShot { k, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view,
            session: frame.session, ordinal: frame.ordinal, frame_counter: frame.frame_counter,
            trigger_counter: frame.trigger_counter, status, error, score: frame.score, ms: frame.ms,
            raw_files: Vec::new() }
    }).collect()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartView {
    pub candidate_debug: Option<crate::candidate::DebugInfo>,
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
    pub filled: usize,
    pub total: usize,
    /// 各拍照点的帧
    pub frames: Vec<FrameView>,
    pub measured_frames: usize,
    /// 段配方在途件（决策记录 5.2）；旧拍照点配方为 None
    pub burst: Option<PartViewBurst>,
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
}

impl From<FrameState> for FrameCellStatus {
    fn from(state: FrameState) -> Self {
        match state {
            FrameState::Measured => Self::Measured,
            FrameState::Missing => Self::Missing,
            FrameState::Error => Self::Error,
            FrameState::Unmeasured => Self::Unmeasured,
            FrameState::OutOfCorridor => Self::OutOfCorridor,
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
    pub frame_counter: Option<u64>,
}

/// 一个帧组：cells 按计划相机顺序。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameGroupView {
    pub seq: u64,
    pub incomplete: bool,
    pub cells: Vec<FrameCellView>,
}

/// 段配方在途件（IPC 契约见 docs/decisions/segment-burst.md 5.2）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartViewBurst {
    pub cameras: Vec<String>,
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
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
    pub candidate_debug: Option<crate::candidate::DebugInfo>,
    pub cycle_id: Option<String>,
    pub sn: String,
    pub recipe_id: Option<String>,
    pub ts: i64,
    pub drain_ms: Option<u64>,
    #[serde(flatten)]
    pub judgement: Judgement,
    /// 段配方的判定；有值时 judgement 的 verdict / plc_code / fault_code / reason 与它相同。旧拍照点配方为 None
    pub burst: Option<BurstJudgement>,
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
        Self { total: c.ok + c.excursion + c.ng + c.err, ok: c.ok + c.excursion, ng: c.ng, err: c.err }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub stats_scope: HistoryScope,
    pub candidate_debug: Option<crate::candidate::DebugInfo>,
    pub measurement_workers: measure::WorkerHealthSnapshot,
    pub phase: Phase,
    pub plc_locked: bool,
    pub plc_handshake: Option<SessionView>,
    pub since: i64,
    pub fault: Option<String>,
    pub product_source: ProductSource,
    pub active_recipe_id: Option<String>,
    pub trigger_mode: Option<TriggerMode>,
    pub part: Option<PartView>,
    pub result: Option<ResultView>,
    pub stats: Stats,
    pub stray_frames: u64,
    pub alarms: Vec<String>,
    pub recording_alarm: Option<RecordingAlarm>,
    /// 连续定位失败告警（当前分槽）；不随每件清空的 alarms 消失，定位成功后清除
    pub locate_alarms: Vec<LocateAlarm>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts: i64,
    pub level: &'static str,
    pub ev: String,
    pub msg: String,
    /// 这一行属于哪个工件：取日志记录关联后的 cycleId，上一件迟到的结果仍标上一件，界面据此不混入当前件
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<String>,
}

impl LogLine {
    pub(crate) fn from_record(record: applog::Record, tone: &'static str) -> Self {
        LogLine { ts: record.ts, level: tone, ev: record.ev, msg: record.msg, cycle_id: record.fields.cycle_id }
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
    pub shot_count: usize,
    pub trigger_mode: TriggerMode,
    pub cameras: Vec<String>,
    pub length: f32,
    /// PLC 计划版本
    pub plan_version: u32,
    /// PLC 触发次数；拍照点编不成触发分组时为空。段配方为 ΣN_exp（未标定为空）
    pub trigger_count: Option<usize>,
    /// 段数；旧拍照点配方为 0
    pub segment_count: usize,
    /// 整件预期帧组数 ΣN_exp；旧配方与未标定的段配方为空
    pub expected_total: Option<u32>,
    /// 整件帧数容差 Σtolerance；旧配方为 0
    pub tolerance_total: u32,
    /// 段配方已标定（expected_total 非空）
    pub calibrated: bool,
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
            shot_count: r.shot_count(),
            trigger_mode: r.trigger_mode,
            cameras: r.cameras(),
            // 段配方：各段段长之和，段长未知按 0
            length: if r.is_burst() { r.path_segments.iter().map(|s| s.length_mm.unwrap_or(0.0) as f32).sum() }
                else { r.segments.iter().map(|g| g.length(r.spacing)).sum() },
            plan_version: r.plan_version,
            trigger_count: if r.is_burst() { expected_total.map(|n| n as usize) }
                else { crate::plc_plan::trigger_groups(r).ok().map(|groups| groups.len()) },
            segment_count: r.path_segments.len(),
            expected_total,
            tolerance_total: r.tolerance_total(),
            calibrated: expected_total.is_some(),
        }
    }
}

struct Shared {
    snapshot: Option<Snapshot>,
    logs: VecDeque<LogLine>,
    measured: Vec<Measured>,
    /// 当前（或刚结束的）一件用的配方快照
    part_recipe: Option<Arc<Recipe>>,
    settings: CycleSettings,
}

pub struct CycleHost {
    pub tx: UnboundedSender<Input>,
    /// PLC 链路门：S7 握手与示教轮询、PLC 配置/连接/手动写点、示教 DB6 发布互斥。节拍每个周期都持有。
    /// 两把门都要时先取 link_gate 再取 config_gate；只持 config_gate 时不能再取 link_gate。
    pub link_gate: tokio::sync::Mutex<()>,
    /// 布防与配置门：布防、切换生产配方与相机重配置、配方/设置/标定修改互斥。节拍只在布防到登记在途、切换待生效配方时持有。
    pub config_gate: tokio::sync::Mutex<()>,
    pub teaching: tokio::sync::Mutex<crate::teaching::Teaching>,
    teaching_busy: AtomicBool,
    teaching_switch_block: AtomicBool,
    teaching_preempted: AtomicBool,
    pub camera: CameraRig,
    pub recipes: RecipeStore,
    pub recorders: Recorders,
    pub cycle_ids: crate::cycle_ids::CycleIds,
    pub audit: Audit,
    pub disk: crate::disk_guard::DiskGuard,
    pub recording_health: Arc<RecordingHealth>,
    settings_path: PathBuf,
    shared: Mutex<Shared>,
    rx: Mutex<Option<(UnboundedReceiver<Input>, Receiver<Frame>)>>,
    /// 有在途工件或未结 PLC 事务。
    busy: AtomicBool,
    /// 节拍不在空闲或故障（开工那一刻就置上，比发布的快照早）
    in_flight: AtomicBool,
    /// 检测侧不允许切换站点模式的原因：有在途工件，或有只能在真机 PLC 上复位的未结事务
    switch_block: Mutex<Option<String>>,
    settings_note: Option<String>,
    interrupted_recordings: usize,
}

impl CycleHost {
    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let settings_path = crate::storage::project_config(app).join("cycle.json");
        let data = crate::storage::project(app);
        let (tx, rx) = unbounded_channel();
        let (frame_tx, frame_rx) = channel(FRAME_QUEUE);
        let (settings, settings_note) = CycleSettings::load(&settings_path);
        let audit = Audit::new(app, data.join("audit-spool"))?;
        let interrupted_recordings = app.state::<Store>().finish_interrupted_recordings()?;
        let camera = CameraRig::new(app, frame_tx)?;
        let recording_health = Arc::new(RecordingHealth::default());
        let recorder = |root: PathBuf, scope: HistoryScope| {
            let (guarded_root, guard_app, protection_audit, warning_app, record_audit) = (root.clone(), app.clone(), audit.clone(), app.clone(), audit.clone());
            let (health, alarm_app) = (recording_health.clone(), app.clone());
            let protection = Arc::new(move || {
                let mut paths = guard_app.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.pending_recording_directories(&guarded_root, scope)?;
                paths.extend(protection_audit.protected_recording_directories(&guarded_root)?);
                Ok(paths)
            });
            let warning = Arc::new(move |errors: &[String]| {
                if warning_app.try_state::<CycleHost>().is_some() {
                    log(&warning_app, "warn", "录制清理失败", errors.join("；"));
                } else { crate::applog::write("warn", "录制清理失败", &errors.join("；")); }
            });
            let release_app = app.clone();
            let release = Arc::new(move |directories: &[PathBuf]| {
                release_app.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.release_recordings(directories)
            });
            Recorder::guarded(root, Some(Arc::new(move |outcome: crate::recorder::RecordingOutcome| {
                let cycle_id = outcome.cycle_id.clone();
                if let Some(change) = health.observe(scope, &outcome, now_ms()) { log_recording_alarm(&alarm_app, change); }
                if let Err(error) = record_audit.recording(outcome) {
                    applog::emit(applog::Level::Error, "录制审计持久化失败", &error, Fields { cycle_id: Some(cycle_id), ..Fields::default() });
                }
            })), crate::recorder::Cleanup { protection, warning, release })
        };
        let recorders = Recorders::new(
            recorder(data.join("records"), HistoryScope::Real),
            recorder(data.join("records-offline"), HistoryScope::Offline),
        );
        let teaching = crate::teaching::Teaching::open(
            crate::storage::state(app).join("teaching-handshake.json"),
        );
        let teaching_busy = teaching.busy();
        let teaching_switch_block = teaching.blocks_site_switch();
        Ok(Self {
            camera,
            tx,
            link_gate: tokio::sync::Mutex::new(()),
            config_gate: tokio::sync::Mutex::new(()),
            teaching: tokio::sync::Mutex::new(teaching),
            teaching_busy: AtomicBool::new(teaching_busy),
            teaching_switch_block: AtomicBool::new(teaching_switch_block),
            teaching_preempted: AtomicBool::new(false),
            recipes: RecipeStore::open_with_floors(
                data.join("recipes"),
                crate::storage::state(app).join("plan-version"),
                &app.state::<crate::store::Store>().recipe_version_floors()?,
            )?,
            recorders,
            cycle_ids: crate::cycle_ids::CycleIds::default(),
            audit,
            disk: crate::disk_guard::DiskGuard::new(data.clone()),
            recording_health,
            shared: Mutex::new(Shared {
                snapshot: None,
                logs: VecDeque::new(),
                measured: Vec::new(),
                part_recipe: None,
                settings,
            }),
            settings_path,
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
        let Some((mut rx, mut frames)) = host.rx.lock().unwrap().take() else { return };
        let (measure_tx, workers) = measure::spawn_worker(app.clone(), host.tx.clone());
        let frame_tx = host.tx.clone();
        let frame_pool = measure::spawn_frame_worker(Arc::new(move |measured| { let _ = frame_tx.send(Input::FrameMeasured(measured)); }));
        let mut machine = Machine::new(app.clone(), measure_tx, workers, frame_pool);
        for e in host.recipes.errors() {
            log(app, "err", "配方", e);
        }
        for e in host.camera.notes.iter().chain(&host.settings_note) {
            log(app, "err", "配置", e.clone());
        }
        for e in &app.state::<crate::workspace::WorkspaceHost>().notes {
            log(app, "err", "候选加载", e.clone());
        }
        let store = app.state::<Store>();
        if let Some(path) = store.backup_path() {
            log(app, "warn", "记录库重建", format!("旧版数据库已备份至 {}，当前使用新版空库", path.display()));
        }
        if host.interrupted_recordings > 0 {
            log(app, "err", "录制恢复", format!("上次服务退出时有 {} 件原图尚未确认完整，已标记录制失败；原检测结论保留", host.interrupted_recordings));
        }
        tauri::async_runtime::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            machine.publish();
            loop {
                tokio::select! {
                    input = rx.recv() => match input {
                        Some(input) => machine.on_input(input).await,
                        None => break,
                    },
                    Some(frame) = frames.recv() => machine.on_frame(frame),
                    _ = tick.tick() => machine.on_tick().await,
                }
                // 快照最多 20 次/秒
                if machine.dirty && machine.published.elapsed() >= Duration::from_millis(50) {
                    machine.publish();
                }
            }
        });
    }

    pub fn phase(&self) -> Phase {
        self.shared.lock().unwrap().snapshot.as_ref().map_or(Phase::Idle, |s| s.phase)
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst) || self.teaching_busy.load(Ordering::SeqCst)
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) || self.teaching_busy.load(Ordering::SeqCst)
    }

    pub fn switch_block(&self) -> Option<String> {
        if self.teaching_switch_block.load(Ordering::SeqCst) {
            Some("示教握手尚未结束，请等待 PLC 释放或显式复位".into())
        } else {
            self.switch_block.lock().unwrap().clone()
        }
    }

    pub fn store_teaching_busy(&self, busy: bool, blocks_switch: bool) {
        self.teaching_busy.store(busy, Ordering::SeqCst);
        self.teaching_switch_block
            .store(blocks_switch, Ordering::SeqCst);
    }

    fn store_busy(&self, phase: Phase, s7: &PlcSession) {
        let in_flight = !matches!(phase, Phase::Idle | Phase::Fault);
        self.in_flight.store(in_flight, Ordering::SeqCst);
        self.busy.store(in_flight || s7.pending(), Ordering::SeqCst);
        *self.switch_block.lock().unwrap() = if in_flight {
            Some("检测仍有在途工件，完成握手或复位故障后再切换站点模式".into())
        } else if s7.pending_kind() == PendingKind::Blocking {
            Some(format!("检测仍有未结 S7 事务（{}），需连接真机 PLC 复位后再切换站点模式", s7.pending_summary().unwrap_or_default()))
        } else { None };
    }

    /// 检测中的配方：人工选型号时选中的，或在途工件用的。
    pub fn recipe_in_use(&self, id: &str) -> bool {
        let settings = self.settings();
        let selected = settings.product_source == ProductSource::Manual && settings.manual_recipe_id.as_deref() == Some(id);
        self.busy() && (selected || self.shared.lock().unwrap().part_recipe.as_ref().is_some_and(|r| r.id == id))
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
            .map_err(|_| "正在布防或处理其他配置操作，请稍后重试")?;
        self.save_settings_locked(settings, &gate)
    }

    pub(crate) fn save_settings_locked(&self, settings: CycleSettings, _gate: &tokio::sync::MutexGuard<'_, ()>) -> Result<(), String> {
        if self.busy() { return Err("在途事务结束或故障复位后才能修改检测设置".into()); }
        settings.validate()?;
        settings.save(&self.settings_path)?;
        self.recorders.apply_retention(&settings);
        self.shared.lock().unwrap().settings = settings;
        let _ = self.tx.send(Input::Refresh);
        Ok(())
    }
}

/// 删除超过保留天数的检测记录。
pub fn purge_history(app: &AppHandle) {
    let days = host(app).settings().history_days.max(1) as i64;
    match host(app).audit.with_ready(|| crate::workspace::purge_history(app, now_ms() - days * 86_400_000)) {
        Ok((n, warnings)) => {
            if n > 0 { log(app, "info", "记录清理", format!("删除 {days} 天前的 {n} 条检测记录")); }
            for warning in warnings { log(app, "warn", "复测记录清理", warning); }
        }
        Err(e) => log(app, "err", "记录清理", e),
    }
    purge_handshake_audit(app);
}

/// 握手审计分段与检测记录同时清理：未结交付（PLC 尚未确认的件）读不到时整次跳过，不冒险删掉 ACK 恢复要读的记录。
fn purge_handshake_audit(app: &AppHandle) {
    let unresolved = match app.state::<Store>().unresolved_delivery_cycles() {
        Ok(unresolved) => unresolved,
        Err(error) => return log(app, "warn", "握手审计清理", format!("读取未结 PLC 交付失败，本次不清理：{error}")),
    };
    let active = crate::storage::state(app).join("plc-handshake.audit.jsonl");
    let pruned = crate::handshake_audit::prune(&active, &unresolved, crate::handshake_audit::LIMITS, now_ms());
    if pruned.removed > 0 || pruned.kept_unresolved > 0 {
        log(app, "info", "握手审计清理", format!("删除 {} 个过期分段；{} 个分段仍有未结交付，保留", pruned.removed, pruned.kept_unresolved));
    }
    for error in pruned.errors { log(app, "warn", "握手审计清理", error); }
}

/// 启动时握手日志里有已绑定工件、尚无结果的在途事务，而历史与追溯暂存里都没有它：进程在事务中退出，补一条 ERR 记录。
/// 只补记录，不动握手事务；事务仍由 PLC 复位处置。
pub(crate) fn record_interrupted_part(app: &AppHandle, s7: &PlcSession, stats: &mut Stats, scope: HistoryScope) {
    let (Some(cycle_id), Some(request)) = (s7.cycle_id(), s7.request()) else { return };
    if s7.result().is_some() { return; }
    let Some(site) = s7.site() else {
        return log(app, "err", "中断工件补记录", format!("cycleId={cycle_id}：握手日志缺少开工时的场景与录制方式，未补记录"));
    };
    let host = host(app);
    let known = host.audit.protected_cycle_ids().map(|ids| ids.contains(cycle_id))
        .and_then(|spooled| Ok(spooled || app.state::<Store>().detail_by_cycle(cycle_id)?.is_some()));
    match known {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => return log(app, "err", "中断工件补记录", format!("cycleId={cycle_id}：核对历史失败，未补记录：{error}")),
    }
    let started_at = s7.started_at();
    let records = applog::dir(app).map(|dir| applog::cycle_records(&dir, cycle_id, started_at.unwrap_or_else(now_ms), 200).records).unwrap_or_default();
    let interrupted = interrupted::Interrupted { cycle_id, sn: &request.sn, request_seq: request.request_seq, started_at, site };
    let part = interrupted::part(&interrupted, &records, |id| host.recipe(id), app.package_info().version.to_string(), now_ms());
    let label = site.source.label();
    let counted = interrupted::counts_in_stats(part.source, part.ts, scope, history::local_midnight_ms());
    let outcome = interrupted::recording(cycle_id, site.record);
    let inserted = host.audit.insert(part);
    if inserted.is_ok() && counted {
        stats.total += 1;
        stats.err += 1;
    }
    match inserted.and_then(|()| host.audit.recording(outcome)) {
        Ok(()) => log_with(app, "warn", "中断工件补记录", format!("SN={} 请求序号 {}：进程在事务中中断，已按 ERR 补入历史（{label}）；PLC 事务仍需复位处置", request.sn, request.request_seq),
            Fields { cycle_id: Some(cycle_id.into()), sn: Some(request.sn.clone()), request_seq: Some(request.request_seq), ..Fields::default() }),
        Err(error) => log(app, "err", "中断工件补记录", format!("cycleId={cycle_id}：{error}")),
    }
}

pub(crate) async fn production_start_pending(engine: &PlcEngine) -> Result<bool, String> {
    let config = engine.config();
    let point = config
        .points
        .iter()
        .find(|p| {
            p.tags
                .iter()
                .any(|t| t == crate::inspection::tag::PLC_STATE)
        })
        .ok_or("缺少 plcState 点位")?;
    match engine
        .read_now(&point.id)
        .await
        .map_err(|e| e.to_string())?
    {
        ly_plc::PlcValue::Int(value) => Ok(value == i64::from(crate::inspection::state::PLC_REQUEST)),
        _ => Err("plcState 点位类型错误".into()),
    }
}

fn plc(app: &AppHandle) -> &PlcEngine {
    app.state::<PlcHost>().inner().engine()
}

fn host(app: &AppHandle) -> &CycleHost {
    app.state::<CycleHost>().inner()
}

fn log_recording_alarm(app: &AppHandle, change: AlarmChange) {
    let (tone, ev, msg, cycle_id) = match change {
        AlarmChange::Raised(alarm) => ("err", "原图录制失败", alarm.message(), Some(alarm.cycle_id)),
        AlarmChange::Cleared => ("ok", "原图录制恢复", "录制失败告警已清除".to_string(), None),
    };
    let fields = Fields { cycle_id, ..Fields::default() };
    if app.try_state::<CycleHost>().is_some() {
        log_with(app, tone, ev, msg, fields);
    } else {
        applog::emit(applog::Level::from_tone(tone), ev, &msg, fields);
    }
}

/// 告警的出现、消除与作废都记在触发它的这一件下（cycle_id 是当前件），本件时间线里能看到。
fn log_locate_alarm(app: &AppHandle, change: LocateChange, cycle_id: &str) {
    let (tone, ev, msg, alarm) = match change {
        LocateChange::Raised(alarm) => ("err", "连续定位失败", alarm.message(), alarm),
        LocateChange::Cleared(alarm) => ("ok", "定位恢复", format!("配方 {} 拍照点 {} 图 {} 定位成功，连续定位失败告警已清除", alarm.recipe_id, alarm.shot_id, alarm.view), alarm),
        LocateChange::Superseded(alarm) => ("info", "定位告警撤销", format!("配方 {} 已换修订，旧修订 {} 拍照点 {} 图 {} 的连续定位失败告警作废，新模板重新计数",
            alarm.recipe_id, alarm.revision_id, alarm.shot_id, alarm.view), alarm),
    };
    let fields = Fields { cycle_id: Some(cycle_id.to_string()), recipe_id: Some(alarm.recipe_id.clone()), view: Some(alarm.view), ..Fields::shot(alarm.k, &alarm.shot_id, &alarm.camera) }
        .with_stage("locate-alarm")
        .with_data(json!({"recipeId": alarm.recipe_id, "revisionId": alarm.revision_id, "view": alarm.view, "consecutive": alarm.consecutive, "reason": alarm.reason}));
    log_with(app, tone, ev, msg, fields);
}

pub fn log(app: &AppHandle, level: &'static str, ev: impl Into<String>, msg: impl Into<String>) {
    log_with(app, level, ev, msg, Fields::default());
}

pub fn log_with(app: &AppHandle, level: &'static str, ev: impl Into<String>, msg: impl Into<String>, fields: Fields) {
    publish_log(app, level, applog::Level::from_tone(level), ev.into(), msg.into(), fields);
}

/// 逐帧、逐拍照点的明细：界面事件流照常显示，日志文件记为 debug。
pub fn trace(app: &AppHandle, ev: impl Into<String>, msg: impl Into<String>, fields: Fields) {
    publish_log(app, "info", applog::Level::Debug, ev.into(), msg.into(), fields);
}

/// 按事件编码记一条（applog::codes），同时进界面事件流。
pub fn log_event(
    app: &AppHandle,
    event: &applog::codes::Event,
    msg: impl Into<String>,
    fields: Fields,
) {
    let tone = match event.level {
        applog::Level::Error => "err",
        applog::Level::Warn => "warn",
        applog::Level::Info | applog::Level::Debug => "info",
    };
    publish_record(app, tone, applog::emit_event(event, &msg.into(), fields));
}

fn publish_log(app: &AppHandle, tone: &'static str, level: applog::Level, ev: String, msg: String, fields: Fields) {
    publish_record(app, tone, applog::emit(level, &ev, &msg, fields));
}

fn publish_record(app: &AppHandle, tone: &'static str, record: applog::Record) {
    let line = LogLine::from_record(record, tone);
    {
        let mut s = host(app).shared.lock().unwrap();
        s.logs.push_back(line.clone());
        if s.logs.len() > 300 {
            s.logs.pop_front();
        }
    }
    let _ = app.emit("cycle://log", line);
}

struct Part {
    cycle_id: String,
    bundle_id: Option<String>,
    production: Option<Arc<crate::production::Prepared>>,
    candidate: Option<Arc<crate::candidate::Candidate>>,
    sn: String,
    recipe: Arc<Recipe>,
    scenario: Scenario,
    frames: Vec<FrameView>,
    measuring_since: Vec<Option<Instant>>,
    table: Vec<PointState>,
    received: usize,
    extra: usize,
    queue: usize,
    /// 旧拍照点配方的帧路由；段配方不经过（空路由）
    router: ShotRouter,
    /// 段配方的帧组与每帧测量（`cycle::burst`）；旧拍照点配方为 None
    burst: Option<Box<burst::BurstPart>>,
    /// 旧拍照点配方：接在 PLC 触发线上的相机（S7 计划）；收尾时按 PLC 报的触发数给它们推下一件基线。段配方为 None
    issued: Option<Vec<String>>,
    /// partEnd 时 PLC 报的设备触发数（协议 6 只作参考，可与计划数不同）；partEnd 前为空。
    /// 段配方只用于丢帧告警和收尾等待，不参与下一件基线（D-SB-4、D-SB-7）；旧拍照点配方仍用它推下一件基线
    device_triggers: Option<u16>,
    routing_closed: bool,
    image_measurement: bool,
    proc_timeout: Duration,
    drain_timeout: Duration,
    armed_at: Instant,
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
    /// 计划帧数：旧拍照点配方为拍照点数，段配方为 ΣN_exp（未标定为 0）。
    fn n(&self) -> usize {
        if self.burst.is_some() { self.recipe.expected_total().map_or(0, |n| n as usize) } else { self.recipe.shot_count() }
    }

    /// 旧拍照点配方：路由见到的触发数；段配方：PLC 报的 deviceTriggers（partEnd 前为 0）。
    fn triggers(&self) -> u64 {
        if self.burst.is_some() { self.device_triggers.map_or(0, u64::from) } else { self.router.triggers() }
    }

    /// 用本件已经观察到的 SDK 帧号更新下一件基线。
    fn close_routing(&mut self, ledgers: &mut Ledgers, _plc_triggers: Option<u16>) {
        if self.routing_closed { return; }
        if let Some(burst) = &self.burst {
            burst.close(ledgers);
        } else {
            self.router.close(ledgers, None);
        }
        self.routing_closed = true;
    }

    fn camera_id(&self, cam: u8) -> String {
        self.cams.iter().position(|&c| c == cam).and_then(|i| self.camera_ids.get(i).cloned()).unwrap_or_else(|| format!("#{}", cam + 1))
    }

    fn complete(&self) -> bool {
        if self.burst.is_some() { return self.burst_complete(Instant::now()); }
        self.frames.iter().all(|f| !matches!(f.status, FrameStatus::Waiting | FrameStatus::Measuring))
    }

    fn receive_frame(&mut self, frame: &Frame) -> Option<Route> {
        let now = Instant::now();
        self.expire(now, self.proc_timeout, self.drain_timeout);
        if self.end_at.is_some_and(|end| now.saturating_duration_since(end) >= self.drain_timeout) { return None; }
        let meta = FrameMeta::from(frame);
        let route = self.router.route(&meta);
        if let Route::Bound { shot: k, ordinal } = route {
            let shot = &self.recipe.shots[k];
            let jump = ordinal > 1 && self.recipe.shots.iter().enumerate().filter(|(_, s)| s.camera == shot.camera)
                .nth(ordinal as usize - 2).is_some_and(|(previous, _)| self.router.bound(previous).is_none());
            self.frames[k] = FrameView {
                status: FrameStatus::Measuring, cam: frame.cam, camera: shot.camera.clone(), shot_id: shot.id.clone(), view: shot.view,
                session: Some(frame.session), ordinal: Some(ordinal), arrived_ms: Some(self.armed_at.elapsed().as_millis() as u64),
                frame_counter: Some(frame.frame_counter), trigger_counter: Some(frame.trigger_counter), counter_jump: jump,
                ..FrameView::waiting()
            };
            self.received += 1;
            self.queue += 1;
            self.measuring_since[k] = Some(Instant::now());
        } else if let Some(code) = route.fault() {
            self.fault.get_or_insert((code, self.router.describe(&meta, &route)));
            if matches!(route, Route::Duplicate { .. } | Route::Extra { .. }) { self.extra += 1; }
        }
        Some(route)
    }

    fn matches_result(&self, m: &Measured) -> bool {
        self.cycle_id == m.cycle_id && self.sn == m.sn && self.bundle_id == m.bundle_id
            && self.recipe.shots.get(m.k).is_some_and(|s| s.id == m.shot_id && s.camera == m.camera)
            && self.frames.get(m.k).is_some_and(|f| f.status == FrameStatus::Measuring && f.cam == m.cam)
    }

    fn apply_result(&mut self, mut m: Measured) -> Option<Measured> {
        self.expire(Instant::now(), self.proc_timeout, self.drain_timeout);
        if !self.matches_result(&m) { return None; }
        if !m.score.is_finite() {
            m.score = 0.0;
            m.error.get_or_insert("测量输出含无效分数".into());
        }
        let owned: Vec<usize> = self.recipe.owned_points(m.k).collect();
        if m.error.is_none() {
            let views = self.recipe.shots[m.k].enabled_views();
            let valid = m.score.is_finite() && (0.0..=1.0).contains(&m.score)
                && (m.px.is_empty() || (m.px.len() == owned.len() && m.px.iter().flatten().all(|v| v.is_finite())))
                && m.idx.len() == owned.len() && m.d.len() == owned.len() && m.w.len() == owned.len()
                && m.st.len() == owned.len() && m.idx.iter().zip(&owned).all(|(&actual, &expected)| actual as usize == expected)
                && (0..owned.len()).all(|i| measure::valid_station(m.st[i], m.d[i], m.w[i]))
                && m.reference_failed.iter().all(|f| views.contains(&f.view));
            if !valid { m.error = Some("测量输出与拍照点测点表不一致或含无效数值".into()); }
        }
        for (i, &j) in owned.iter().enumerate() {
            self.table[j] = if m.error.is_some() { PointState::Invalid } else { m.point_state(i) };
        }
        self.queue = self.queue.saturating_sub(1);
        self.measuring_since[m.k] = None;
        let view_results = self.recipe.shots[m.k].enabled_views().into_iter().map(|view| {
            let details = m.views.iter().find(|v| v.view == view);
            let verdict = self.recipe.for_view(m.k, view).map(|recipe| judge::judge(&recipe, &self.table).verdict).unwrap_or(Verdict::ErrInspect);
            ViewResult { view, verdict, error: details.and_then(|v| v.error.clone()).or_else(|| m.error.clone()),
                score: details.and_then(|v| v.reading.as_ref().map(|r| r.coverage)),
                ms: details.and_then(|v| v.reading.as_ref().map(|r| r.ms)),
                pose: m.error.is_none().then(|| m.poses.iter().find(|(v, _)| *v == view).map(|(_, pose)| pose.clone())).flatten() }
        }).collect();
        let frame = &mut self.frames[m.k];
        frame.view_results = view_results;
        frame.status = if m.error.is_some() { FrameStatus::Error } else { FrameStatus::Done };
        frame.error = m.error.clone();
        frame.score = m.error.is_none().then_some(m.score);
        frame.points = owned.len();
        frame.gap_points = if m.error.is_none() { m.st.iter().filter(|&&s| s == measure::ST_GAP).count() } else { 0 };
        frame.ms = Some(m.ms);
        if let Some(error) = &m.error {
            self.fault.get_or_insert((fault::PROCESS_TIMEOUT, format!("拍照点 {} 测量出错：{error}", m.shot_id)));
            m.idx.clear(); m.d.clear(); m.w.clear(); m.st.clear(); m.px.clear(); m.reference_failed.clear();
        }
        frame.reference_failed = m.reference_failed.clone();
        Some(m)
    }

    fn expire(&mut self, now: Instant, proc: Duration, drain: Duration) -> Vec<usize> {
        let deadline = self.end_at.is_some_and(|end| now.saturating_duration_since(end) >= drain);
        let mut expired = Vec::new();
        for k in 0..self.frames.len() {
            let frame = &mut self.frames[k];
            if frame.status == FrameStatus::Waiting && deadline {
                frame.status = FrameStatus::Missing;
                frame.error = Some(format!("拍照点 {} 截止时未收到帧", self.recipe.shots[k].id));
                expired.push(k);
            } else if frame.status == FrameStatus::Measuring
                && (deadline || self.measuring_since[k].is_some_and(|start| now.saturating_duration_since(start) >= proc)) {
                let reason = format!("拍照点 {} 测量超时", self.recipe.shots[k].id);
                frame.status = FrameStatus::Error;
                frame.error = Some(reason.clone());
                frame.ms = self.measuring_since[k].map(|start| now.saturating_duration_since(start).as_millis() as u32);
                self.measuring_since[k] = None;
                self.queue = self.queue.saturating_sub(1);
                self.fault.get_or_insert((fault::PROCESS_TIMEOUT, reason));
                expired.push(k);
            }
            if matches!(frame.status, FrameStatus::Error | FrameStatus::Missing) {
                for j in self.recipe.owned_points(k) { self.table[j] = PointState::Invalid; }
            }
        }
        expired
    }

    fn judgement(&self) -> Judgement {
        if let Some((code, reason)) = &self.fault {
            return Judgement::error(*code, reason.clone());
        }
        if let Some(reason) = self.router.missing_reason() {
            return Judgement::error(fault::MISSING_FRAME, reason);
        }
        if self.extra > 0 {
            return Judgement::error(fault::EXTRA_FRAME, format!("多收到 {} 帧", self.extra));
        }
        if !self.complete() {
            return Judgement::error(fault::PROCESS_TIMEOUT, "计划内拍照点尚未完成");
        }
        judge::judge(&self.recipe, &self.table)
    }

    fn view(&self) -> PartView {
        let burst = self.burst_view();
        PartView {
            candidate_debug: self.candidate.as_ref().map(|c| c.info.clone()),
            cycle_id: self.cycle_id.clone(),
            bundle_id: self.bundle_id.clone(),
            sn: self.sn.clone(),
            recipe_id: self.recipe.id.clone(),
            recipe_revision: self.recipe.revision_id.clone(),
            n: self.n(),
            received: burst.as_ref().map_or(self.received, |b| b.camera_frames.iter().map(|&n| n as usize).sum()),
            triggers: self.triggers(),
            queue: burst.as_ref().map_or(self.queue, |b| b.pending_measure),
            filled: self.table.iter().filter(|p| **p != PointState::Pending).count(),
            total: self.table.len(),
            frames: self.frames.clone(),
            measured_frames: burst.as_ref().map_or_else(|| self.frames.iter().filter(|f| matches!(f.status, FrameStatus::Done | FrameStatus::Error)).count(), |b| b.measured_frames),
            burst,
        }
    }
}

/// 布防阶段的追溯健康检查。探针自身失败时 `Audit::health` 已标记追溯故障；只是布防总预算用完时只拒绝本件，
/// 不能把一次偶发超时变成需要重启才能恢复的追溯故障。探针线程异常无法确认结果，按追溯故障处理。
pub(crate) async fn arm_health_check(budget: &mut ArmBudget, probe: impl FnOnce() -> Result<(), String> + Send + 'static,
    poison: impl FnOnce(String)) -> Result<(), String> {
    let mut panicked = None;
    let result = budget.run("audit-health", async {
        match tauri::async_runtime::spawn_blocking(probe).await {
            Ok(result) => result,
            Err(error) => {
                let error = format!("追溯健康检查线程异常：{error}");
                panicked = Some(error.clone());
                Err(error)
            }
        }
    }).await;
    if let Some(error) = panicked { poison(error); }
    result
}

fn missing_recipe(id: &str) -> String {
    format!("选中的配方 {id} 不存在或没能加载（见配方页）")
}

/// 离线产线没有实物，按离线计数策略布防。
pub fn real_parts(app: &AppHandle) -> bool {
    !crate::offline::enabled(app)
}

/// 配方用的相机此刻在相机组里的序号。相机不在相机组里或视角不够时返回原因。
/// 段配方的计划相机必须是单视角独立设备（D-SB-11）。
pub fn usable_cams(app: &AppHandle, recipe: &Recipe) -> Result<Vec<u8>, String> {
    let host = host(app);
    let cams = host.camera.resolve(&recipe.cameras())?;
    crate::vision::recipe_source(app, recipe)?;
    if !recipe.is_burst() { return Err("旧拍照点配方已停用，请新建段配方".into()); }
    crate::camera::check_burst_cameras(&host.camera.configs(), &recipe.plan_cameras())?;
    Ok(cams)
}

/// 配方能不能开工：旧拍照点配方要检的拍照点都示教过；段配方三台相机都示教了走廊且整件已标定（D-SB-20）。
pub fn ready_to_run(recipe: &Recipe) -> Result<(), String> {
    if !recipe.is_burst() { return Err("旧拍照点配方已停用，请新建段配方".into()); }
    recipe.ready_burst().map_err(|reasons| reasons.join("；"))
}

pub fn runnable_cams(app: &AppHandle, recipe: &Recipe) -> Result<Vec<u8>, String> {
    ready_to_run(recipe)?;
    crate::production::ready(app, recipe)?;
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

/// 帧归属失败的原因，用作聚合 key 与 data.reason。
fn route_kind(route: &Route) -> &'static str {
    match route {
        Route::Bound { .. } => "bound",
        Route::Stale { .. } => "stale",
        Route::Duplicate { .. } => "duplicate",
        Route::Extra { .. } => "extra",
        Route::SessionChanged { .. } => "session_changed",
        Route::UntrustedCounter(_) => "untrusted_counter",
        Route::UnknownCamera => "unknown_camera",
    }
}

struct Machine {
    app: AppHandle,
    measure_tx: Sender<Job>,
    workers: Arc<WorkerHealth>,
    /// 段配方的测量池：每台相机每一帧独立测（D-SB-12）
    frame_pool: FramePool,
    ledgers: Ledgers,
    phase: Phase,
    since: i64,
    part: Option<Part>,
    active_recipe: Option<Arc<Recipe>>,
    fault: Option<String>,
    /// 当前故障的来源、进入时刻，以及同一来源同一原因被再次上报的次数（首次、最近一次时刻）
    fault_source: Option<&'static str>,
    fault_at: Option<i64>,
    fault_repeat: Option<FaultRepeat>,
    done_at: Option<Instant>,
    ack_alarmed: bool,
    result: Option<ResultView>,
    stats: Stats,
    stats_day: i64,
    stats_scope: HistoryScope,
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
    /// 逐帧、逐拍重复的告警聚合（游离帧、迟到帧、丢帧等），节拍每拍写出到期摘要
    noise: Throttle,
    /// 报警写日志的去重：alarms 每件开工、复位都会清空，同一报警不再每件重写
    alarm_log: Throttle,
    s7: PlcSession,
    s7_reset_requested: bool,
    current_cycle_id: Option<String>,
    recording_alarm: Option<RecordingAlarm>,
    /// 连续定位失败计数：只在内存，重启清零；复位、站点切换不清除（见 locate_health.rs）
    locate_health: LocateHealth,
}

impl Machine {
    fn new(app: AppHandle, measure_tx: Sender<Job>, workers: Arc<WorkerHealth>, frame_pool: FramePool) -> Self {
        let mut stats = today_stats(&app, HistoryScope::Real);
        let s7 = PlcSession::open(crate::storage::state(&app).join("plc-handshake.json"));
        if let Some(notice) = s7.archived() { log(&app, "warn", "S7 握手日志归档", notice.to_owned()); }
        let recovery = match app.state::<Store>().unresolved_delivery_cycles() {
            Ok(unresolved) => s7.recover_acknowledgements(&unresolved),
            Err(error) => crate::plc_session::AckRecovery { receipts: Vec::new(), errors: vec![format!("读取未决 PLC 交付失败，跳过 ACK 恢复：{error}")] },
        };
        for error in recovery.errors { log(&app, "err", "PLC 确认恢复", error); }
        let mut recovered = 0;
        let mut failed = 0;
        for receipt in recovery.receipts {
            match app.state::<Store>().recover_acknowledgement(&receipt.cycle_id, &receipt.sn, receipt.request_seq, receipt.ts) {
                Ok(true) => recovered += 1,
                Ok(false) => {}
                Err(error) => {
                    if failed < 10 { log(&app, "err", "PLC 确认恢复", format!("cycleId={}：{error}", receipt.cycle_id)); }
                    failed += 1;
                }
            }
        }
        if recovered > 0 { log(&app, "ok", "PLC 确认恢复", format!("已从持久握手记录恢复 {recovered} 件 PLC 确认，原检测结论保留")); }
        if failed > 10 { log(&app, "err", "PLC 确认恢复", format!("另有 {} 条收据未能关联或保存，请核对握手审计与检测记录", failed - 10)); }
        record_interrupted_part(&app, &s7, &mut stats, HistoryScope::Real);
        match (s7.pending_kind(), s7.pending_summary()) {
            (PendingKind::Offline, Some(summary)) => log(&app, "warn", "S7 未结事务", format!("上次离线运行留下{summary}：进入离线由虚拟 PLC 复位清除，或连接真机 PLC 后复位")),
            (PendingKind::Blocking, Some(summary)) => log(&app, "warn", "S7 未结事务", format!("留有{summary}：连接真机 PLC 核对该件后复位；复位前不能切换站点模式")),
            _ => {}
        }
        host(&app).store_busy(Phase::Fault, &s7);
        Self {
            app,
            measure_tx,
            workers,
            frame_pool,
            ledgers: Ledgers::default(),
            phase: Phase::Fault,
            since: now_ms(),
            part: None,
            active_recipe: None,
            fault: Some("PLC 未连接".into()),
            fault_source: None,
            fault_at: None,
            fault_repeat: None,
            done_at: None,
            ack_alarmed: false,
            result: None,
            stats,
            stats_day: history::local_day(),
            stats_scope: HistoryScope::Real,
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
            noise: Throttle::new(throttle::WINDOW),
            alarm_log: Throttle::new(ALARM_LOG_WINDOW),
            s7,
            s7_reset_requested: false,
            current_cycle_id: None,
            recording_alarm: None,
            locate_health: LocateHealth::default(),
        }
    }

    /// 原图录制失败不停产，告警独立于每件清空的 alarms，跟着当前场景的录制结果走。
    fn sync_recording_alarm(&mut self) {
        let alarm = host(&self.app).recording_health.alarm(self.stats_scope);
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
                &codes::ALARM_RAISED_SUMMARY,
                &msg,
                now_ms(),
                Tally::None,
                fields.clone(),
            ) {
                log_event(&self.app, &codes::ALARM_RAISED, msg.clone(), fields);
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
            log_event(&self.app, event, msg, fields);
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
            (&codes::FRAME_ROUTE_FAULT, &codes::FRAME_ROUTE_FAULT_SUMMARY)
        } else {
            (&codes::FRAME_UNROUTED, &codes::FRAME_UNROUTED_SUMMARY)
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
            log_event(&self.app, summary.event, summary.msg, summary.fields);
        }
    }

    /// 资料盘空间不足时暂停接件：在途件照常完成释放，回到空闲后 pcState 撤到 0；空间恢复后自动就绪，无需复位。
    fn disk_space_ready(&mut self) -> bool {
        const PREFIX: &str = "资料盘剩余空间";
        let shown = self.alarms.iter().any(|m| m.starts_with(PREFIX));
        match host(&self.app).disk.check() {
            Err(reason) => {
                match self.alarms.iter_mut().find(|m| m.starts_with(PREFIX)) {
                    Some(m) if *m == reason => {}
                    Some(m) => { *m = reason; self.dirty = true; }
                    None => self.alarm(reason),
                }
                false
            }
            Ok(()) => {
                if shown {
                    self.alarms.retain(|m| !m.starts_with(PREFIX));
                    self.dirty = true;
                    log(&self.app, "ok", "资料盘空间", "剩余空间已恢复，恢复接件");
                }
                true
            }
        }
    }

    /// 当前配方（人工选择或上一件 PLC 下发的）。
    fn current_recipe(&self) -> Option<Arc<Recipe>> {
        if let Some(c) = crate::offline::candidate(&self.app) { return Some(c.recipe.clone()); }
        let host = host(&self.app);
        let settings = host.settings();
        match settings.product_source {
            ProductSource::Manual => settings.manual_recipe_id.as_deref().and_then(|id| host.recipe(id)),
            ProductSource::Plc => self.active_recipe.clone(),
        }
    }

    fn compute_required(&self) -> Required {
        if let Some(c) = crate::offline::candidate(&self.app) {
            return Required { rig_gen: host(&self.app).camera.generation(), cams: runnable_cams(&self.app, &c.recipe), watch: Vec::new(), warnings: Vec::new() };
        }
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
                Required { rig_gen, cams, watch: Vec::new(), warnings: Vec::new() }
            }
            ProductSource::Plc => {
                let (mut watch, mut warnings) = (Vec::new(), Vec::new());
                for r in host.recipes.list() {
                    match runnable_cams(&self.app, &r) {
                        Ok(c) => watch.push((r.id.clone(), c)),
                        Err(e) => warnings.push(format!("配方 {} 开不了工：{e}", r.id)),
                    }
                }
                Required { rig_gen, cams: Ok(Vec::new()), watch, warnings }
            }
        }
    }

    /// 与配方无关的服务就绪：生产生效版本已确认、工件身份可分配、测量线程可布防。
    fn service_ready(&self) -> Result<(), String> {
        if crate::workspace::activation_uncertain(&self.app) { return Err("生产生效版本尚未确认，请先核实发布状态".into()); }
        host(&self.app).cycle_ids.ready(&self.app)?;
        if !self.workers.can_arm() {
            return Err("测量工作线程全部超时且尚未返回，拒绝布防；需排查图像引擎或重启检测服务".into());
        }
        Ok(())
    }

    /// 人工选定的配方先把曝光与增益下发下去，布防时只需核对；失败只记日志，布防阶段再强制下发。
    fn preapply_imaging(&self, recipe: Arc<Recipe>, cams: Vec<u8>) {
        if host(&self.app).camera.recipe_capture().active() { return; }
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = app.state::<CycleHost>().camera.apply_imaging(&cams, &recipe) {
                log(&app, "warn", "相机", format!("配方 {} 的曝光与增益预先下发失败，开工布防时再下发：{error}", recipe.id));
            }
        });
    }

    fn check_idle_devices(&mut self) -> Result<(), String> {
        self.service_ready()?;
        let host = host(&self.app);
        if self.required.as_ref().is_none_or(|r| r.rig_gen != host.camera.generation()) {
            let req = self.compute_required();
            for w in req.warnings.iter().filter(|w| !self.config_warnings.contains(w)) {
                log(&self.app, "err", "配方", w.clone());
            }
            if req.warnings != self.config_warnings {
                self.config_warnings = req.warnings.clone();
                self.dirty = true;
            }
            if let (Ok(cams), Some(recipe)) = (&req.cams, self.current_recipe().filter(|_| host.settings().product_source == ProductSource::Manual)) {
                self.preapply_imaging(recipe, cams.clone());
            }
            self.required = Some(req);
        }
        let req = self.required.as_ref().unwrap();
        let down: Vec<String> = req.watch.iter().filter(|(_, cams)| !host.camera.all_ready(cams)).map(|(id, _)| id.clone()).collect();
        if down != self.camera_down || (!down.is_empty() && self.camera_checked.elapsed() > Duration::from_secs(1)) {
            self.camera_checked = Instant::now();
            let warnings: Vec<String> = req
                .watch
                .iter()
                .filter(|(id, _)| down.contains(id))
                .filter_map(|(id, cams)| host.camera.check_ready_at(cams).err().map(|e| format!("配方 {id} 暂时开不了工：{e}")))
                .collect();
            if warnings != self.camera_warnings {
                for w in warnings.iter().filter(|w| !self.camera_warnings.contains(w)) {
                    log(&self.app, "warn", "相机", w.clone());
                }
                self.camera_warnings = warnings;
                self.dirty = true;
            }
            self.camera_down = down;
        }
        let none_usable = !req.warnings.is_empty() && req.watch.is_empty();
        if none_usable || (!req.watch.is_empty() && self.camera_down.len() == req.watch.len()) {
            let first = self.camera_warnings.first().or(req.warnings.first()).cloned().unwrap_or_default();
            return Err(format!("没有一个配方开得了工（{first}）"));
        }
        match &req.cams {
            Ok(cams) => host.camera.check_ready_at(cams),
            Err(e) => Err(e.clone()),
        }
    }

    fn publish(&mut self) {
        self.dirty = false;
        self.published = Instant::now();
        let host = host(&self.app);
        let active = self.current_recipe();
        let locate_alarms = self.locate_health.alarms(self.stats_scope);
        let snapshot = Snapshot {
            stats_scope: self.stats_scope,
            candidate_debug: crate::offline::candidate(&self.app).map(|c| c.info.clone()),
            measurement_workers: self.workers.snapshot(),
            phase: self.phase,
            plc_locked: host.busy(),
            plc_handshake: Some(self.s7.view()),
            since: self.since,
            fault: self.fault.clone(),
            product_source: host.settings().product_source,
            active_recipe_id: active.as_ref().map(|r| r.id.clone()),
            trigger_mode: active.as_ref().map(|r| r.trigger_mode),
            part: self.part.as_ref().map(Part::view),
            result: self.result.clone(),
            stats: self.stats.clone(),
            stray_frames: self.stray,
            alarms: self.alarms.iter().chain(&self.config_warnings).chain(&self.camera_warnings).cloned()
                .chain(self.recording_alarm.as_ref().map(RecordingAlarm::message)).chain(locate_alarms.iter().map(LocateAlarm::message)).collect(),
            recording_alarm: self.recording_alarm.clone(),
            locate_alarms,
        };
        host.shared.lock().unwrap().snapshot = Some(snapshot.clone());
        let _ = self.app.emit("cycle://snapshot", snapshot);
    }

    async fn on_input(&mut self, input: Input) {
        self.dirty = true;
        match input {
            Input::Measured(m) => self.on_measured(m),
            Input::FrameMeasured(m) => self.on_frame_measured(m),
            Input::Reset => self.s7_reset_requested = true,
            Input::Refresh => self.required = None,
            Input::SiteChanged => {
                self.required = None;
                if matches!(self.phase, Phase::Idle | Phase::Fault) && self.s7.pending_kind() != PendingKind::Blocking {
                    self.part = None;
                    self.result = None;
                    self.active_recipe = None;
                    self.current_cycle_id = None;
                    applog::end_cycle();
                    self.done_at = None;
                    self.ack_alarmed = false;
                    self.alarms.clear();
                    self.fault = (self.phase == Phase::Fault).then(|| "站点模式已切换，等待当前 PLC 连接与复位".into());
                    self.stats_day = history::local_day();
                    self.stats_scope = HistoryScope::of(crate::site::record_source(&self.app));
                    self.stats = today_stats(&self.app, self.stats_scope);
                    self.stray = 0;
                    self.stray_times.clear();
                    self.camera_down.clear();
                    self.camera_warnings.clear();
                    self.config_warnings.clear();
                    let mut shared = host(&self.app).shared.lock().unwrap();
                    shared.part_recipe = None;
                    shared.measured.clear();
                    drop(shared);
                    crate::workspace::clear_live(&self.app);
                }
            },
        }
    }

    async fn start_part(&mut self, request: Request) {
        if host(&self.app).camera.recipe_capture().active() || self.s7.capture_id().is_some() { return; }
        let started = Instant::now();
        let settings = host(&self.app).settings();
        let mut budget = ArmBudget::with_start(started, settings.timeouts.arm());
        if let Err(error) = host(&self.app).audit.ready() {
            self.enter_fault("audit", format!("审计尚未就绪，拒绝布防：{error}"));
            return;
        }
        if crate::offline::candidate(&self.app).is_none() && crate::workspace::apply_pending(&self.app) { self.required = None; }
        if crate::workspace::activation_uncertain(&self.app) {
            self.enter_fault("arm", "生产生效版本尚未确认，禁止启动下一件".into());
            return;
        }
        self.set_phase(Phase::Validate);
        self.part = None;
        let scope = HistoryScope::of(crate::site::record_source(&self.app));
        if scope != self.stats_scope { self.stats_scope = scope; self.stats = today_stats(&self.app, scope); }
        self.current_cycle_id = None;
        self.result = None;
        crate::workspace::clear_live(&self.app);
        self.alarms.clear();
        for summary in self.noise.end_all(&codes::FRAME_STRAY_SUMMARY) {
            log_event(&self.app, summary.event, summary.msg, summary.fields);
        }
        host(&self.app).shared.lock().unwrap().measured.clear();
        budget.mark("entry-readiness-workspace");
        let app = self.app.clone();
        let engine = plc(&app);
        let (sn, code) = (request.sn.clone(), request.product_code as u32);
        let host = host(&app);
        let taken = host.cycle_ids.take(&app);
        let context = Fields { cycle_id: taken.as_ref().ok().cloned(), sn: Some(sn.clone()), request_seq: Some(request.request_seq), ..Fields::default() };
        applog::begin_cycle(context.clone());
        log_with(&app, "info", "plcState=10", format!("SN={sn} 型号={code}"),
            Fields::stage("partStart").with_data(json!({"productCode": code})));
        let cycle_id = match taken {
            Ok(id) => id,
            Err(reason) => return self.refuse(sn, None, fault::PROCESS_TIMEOUT, reason).await,
        };
        self.current_cycle_id = Some(cycle_id.clone());
        let site = PartSite { source: crate::site::record_source(&app), record: crate::site::record_mode(&app) };
        if let Err(error) = self.s7.bind_cycle_id(&cycle_id, site) {
            return self.refuse(sn, None, fault::PROCESS_TIMEOUT, format!("S7 事务无法绑定工件身份：{error}")).await;
        }
        if !self.workers.can_arm() {
            return self.refuse(sn, None, fault::PROCESS_TIMEOUT, "测量工作线程全部超时且尚未返回，拒绝布防".into()).await;
        }
        let recipe = if let Some(c) = crate::offline::candidate(&app) { Some(c.recipe.clone()) } else { match settings.product_source {
            ProductSource::Plc => host.recipes.list().into_iter().find(|r| r.product_code as u32 == code),
            ProductSource::Manual => settings.manual_recipe_id.as_deref().and_then(|id| host.recipe(id)),
        } };
        let Some(recipe) = recipe else {
            let reason = match (settings.product_source, settings.manual_recipe_id.as_deref()) {
                (ProductSource::Plc, _) => format!("产品代码 {code} 没有对应的配方"),
                (ProductSource::Manual, Some(id)) => missing_recipe(id),
                (ProductSource::Manual, None) => "未选择配方".to_string(),
            };
            return self.refuse(sn, None, fault::NO_RECIPE, reason).await;
        };
        self.active_recipe = Some(recipe.clone());
        applog::begin_cycle(Fields { recipe_id: Some(recipe.id.clone()), ..context });
        // 从这一刻起这个配方算在检测中（不能删、不能改编号）
        host.shared.lock().unwrap().part_recipe = Some(recipe.clone());
        let id = Some(recipe.id.clone());
        macro_rules! arm_remaining {
            ($stage:expr) => {
                match budget.remaining($stage) {
                    Ok(remaining) => remaining,
                    Err(reason) => {
                        log(&app, "err", "布防阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "refused", "trace": budget.trace()}).to_string());
                        return self.refuse(sn, id.clone(), fault::PROCESS_TIMEOUT, reason).await;
                    }
                }
            };
        }
        let n = recipe.shot_count();
        // 日志里的计划帧数：段配方为 ΣN_exp
        let planned = if recipe.is_burst() { recipe.expected_total().map_or(0, |n| n as usize) } else { n };
        let plan = match admission::admit(&request, &recipe, crate::plc::recipe_plan(host, &recipe)) {
            Ok(plan) => plan,
            Err((code, reason)) => return self.refuse(sn, id, code, reason).await,
        };
        if let Err(reason) = self.s7.validate_plan(&plan) {
            return self.refuse(sn, id, fault::PLAN_MISMATCH, reason).await;
        }
        if recipe.is_burst() && !self.frame_pool.health().can_arm() {
            return self.refuse(sn, id, fault::PROCESS_TIMEOUT, "段配方测量线程全部超时且尚未返回，拒绝布防".into()).await;
        }
        let production = match crate::production::ready(&app, &recipe) {
            Ok(prepared) => prepared,
            Err(reason) => return self.refuse(sn, id, fault::NO_RECIPE, reason).await,
        };
        budget.mark("identity-recipe-plan");
        arm_remaining!("before-verify");
        if let Some(prepared) = production.clone() {
            let checked = budget.run("prepared-verify", async {
                tauri::async_runtime::spawn_blocking(move || prepared.verify()).await.map_err(|error| error.to_string())
            }).await;
            match checked {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => return self.refuse(sn, id, fault::NO_RECIPE, format!("发布包核验失败：{reason}")).await,
                Err(reason) => {
                    log(&app, "err", "布防阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "refused", "trace": budget.trace()}).to_string());
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
        let imaging = {
            let (app, recipe, cams) = (app.clone(), recipe.clone(), cams.clone());
            budget.run("camera-imaging", async move {
                tauri::async_runtime::spawn_blocking(move || app.state::<CycleHost>().camera.apply_imaging(&cams, &recipe)).await.map_err(|error| error.to_string())
            }).await
        };
        match imaging {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return self.refuse(sn, id, fault::DEVICE_LOST, format!("相机曝光与增益下发失败：{reason}")).await,
            Err(reason) => {
                log(&app, "err", "布防阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "refused", "trace": budget.trace()}).to_string());
                return self.refuse(sn, id, fault::PROCESS_TIMEOUT, reason).await;
            }
        }
        let camera_ids = recipe.cameras();
        let mut identities: Vec<ArmCam> = Vec::new();
        for &cam in &cams {
            let Some(slot) = host.camera.slot(cam as usize) else {
                return self.refuse(sn, id, fault::DEVICE_LOST, "相机组已变化".into()).await;
            };
            let (identity, observed) = match slot.routing_state() {
                Ok(state) => state,
                Err(error) => return self.refuse(sn, id, fault::DEVICE_LOST, error).await,
            };
            if let Some(ledger) = observed { self.ledgers.merge(cam, &ledger); }
            identities.push(identity);
        }
        let (router, burst, issued) = {
            // 段配方：按 SDK 帧计数定每台相机的基线（D-SB-10），会话或计数来源不可信时拒绝布防（98）
            let policy = if real_parts(&app) { Policy::burst_production(None) } else { Policy::burst_offline(None) };
            let baselines = match arm_frames(&recipe.plan_cameras(), &self.ledgers, &policy, &identities) {
                Ok(baselines) => baselines,
                Err(error) => return self.refuse(sn, id, error.fault(), error.to_string()).await,
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
            (unrouted(), Some(Box::new(burst::BurstPart::arm(&recipe, baselines, policy.trusted))), None)
        };
        let bundle_id = production.as_ref().map(|prepared| prepared.bundle.id.clone());
        budget.mark("camera-router");
        arm_remaining!("before-recording");
        if let Err(reason) = host.audit.begin_recording(&cycle_id) {
            return self.refuse(sn, id, fault::PROCESS_TIMEOUT, format!("录制追溯屏障建立失败：{reason}")).await;
        }
        let source = crate::site::record_source(&app);
        let recording = host.recorders.begin(source, crate::site::record_mode(&app), &sn, recipe.clone(), &cycle_id, bundle_id.as_deref());
        self.part = Some(Part {
            cycle_id,
            bundle_id,
            production,
            candidate: crate::offline::candidate(&app),
            sn: sn.clone(),
            scenario: crate::offline::part_scenario(&app),
            frames: recipe.shots.iter().map(|shot| FrameView {
                shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, ..FrameView::waiting()
            }).collect(),
            measuring_since: vec![None; n],
            table: vec![PointState::Pending; recipe.point_count()],
            received: 0,
            extra: 0,
            queue: 0,
            router,
            burst,
            issued,
            device_triggers: None,
            routing_closed: false,
            image_measurement: crate::site::image_measurement(&app),
            proc_timeout: settings.timeouts.proc(),
            drain_timeout: settings.timeouts.drain(),
            armed_at: Instant::now(),
            end_at: None,
            fault: None,
            cams,
            camera_ids,
            rig_gen,
            source,
            recording,
            recipe: recipe.clone(),
        });
        budget.mark("recording-begin");
        arm_remaining!("before-audit-health");
        let audit = host.audit.clone();
        let health = arm_health_check(&mut budget, move || audit.health(), |error| host.audit.fail(error)).await;
        if let Err(reason) = health {
            log(&app, "err", "布防阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "refused", "trace": budget.trace()}).to_string());
            return self.refuse(sn, id, fault::PROCESS_TIMEOUT, format!("录制开始后追溯健康检查失败，未启动相机：{reason}")).await;
        }
        arm_remaining!("before-plc-arm");
        let r = budget.run_serial("plc-arm-confirmed", self.s7.arm_outputs(engine, &plan)).await;
        let elapsed = budget.elapsed();
        let committed = match r {
            Ok(()) => {
                let committed = self.s7.commit_arm(engine).await.map_err(|e| format!("布防后握手日志落盘失败：{e}"));
                budget.mark("plc-arm-journal");
                committed
            }
            Err(e) => Err(format!("布防写入失败或总预算超时：{e}")),
        };
        let mut trace = budget.trace();
        trace["elapsedMs"] = json!(elapsed.as_secs_f64() * 1000.0);
        trace["plcArm"] = json!(self.s7.arm_timing());
        if let Err(reason) = committed {
            log(&app, "err", "布防阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "fault", "trace": trace}).to_string());
            self.s7.fault(engine, reason.clone()).await;
            return self.enter_fault("arm", reason);
        }
        self.part.as_mut().unwrap().armed_at = Instant::now();
        self.set_phase(Phase::Acquire);
        log_with(&app, "info", "armed↑ busy↑", format!("{} · N={planned} · 布防耗时 {} ms", recipe.id, elapsed.as_millis()),
            Fields::stage("armed").with_data(json!({"armMs": elapsed.as_secs_f64() * 1000.0, "shots": planned})));
        log_with(&app, "info", "布防阶段", json!({"cycleId": self.current_cycle_id, "sn": sn, "status": "armed", "trace": trace}).to_string(),
            Fields::default().with_data(trace));
    }

    /// 校验没过，不布防，直接回写 ERR。
    async fn refuse(&mut self, sn: String, recipe_id: Option<String>, code: u16, reason: String) {
        log(&self.app, "err", "校验失败", format!("{reason}，不布防"));
        self.report(sn, recipe_id, Judgement::error(code, reason)).await
    }

    fn on_frame(&mut self, f: Frame) {
        let meta = FrameMeta::from(&f);
        self.ledgers.observe(&meta);
        let accepting = matches!(self.phase, Phase::Acquire | Phase::Drain);
        let slot = host(&self.app).camera.slot(f.cam as usize);
        if f.manual {
            if accepting {
                if let Some(part) = self.part.as_mut() {
                    let current = slot.as_ref().is_some_and(|slot| slot.routing_state().is_ok_and(|state| state.0.session == f.session));
                    if let Some(burst) = part.burst.as_mut() {
                        // 段配方：布防后的手动帧丢弃并告警，不判 ERR（它占了这台相机一个帧号，此后的帧组可能错开一格）
                        if let Some(camera) = burst.baseline(f.cam).filter(|base| current && f.frame_counter > *base)
                            .and_then(|_| part.cams.iter().position(|&c| c == f.cam)).filter(|&camera| burst.first_warning(camera, burst::Warning::Manual)) {
                            let message = format!("生产布防后收到相机 {} 的手动触发帧（帧计数 {}），已丢弃；该相机此后的帧组可能错开一格", burst.camera_name(camera), f.frame_counter);
                            log_with(&self.app, "warn", "手动帧", message, Fields { cycle_id: Some(part.cycle_id.clone()), ..Fields::default() });
                            self.dirty = true;
                        }
                    } else if part.router.baseline(f.cam).is_some_and(|base| f.trigger_counter > base) && current {
                        part.fault.get_or_insert((fault::EXTRA_FRAME, "生产布防后收到手动触发帧，拍照计划已被打断".into()));
                        self.dirty = true;
                    }
                }
            }
            return;
        }
        if !accepting || self.part.is_none() {
            self.stray += 1;
            self.dirty = true;
            let now = Instant::now();
            self.stray_times.push_back(now);
            self.stray_times.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
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
                &codes::FRAME_STRAY,
                &codes::FRAME_STRAY_SUMMARY,
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
        if self.part.as_ref().is_some_and(|part| part.burst.is_some()) {
            return self.on_burst_frame(f);
        }
        let part = self.part.as_mut().unwrap();
        let Some(route) = part.receive_frame(&f) else {
            let (camera, cycle_id) = (part.camera_id(f.cam), part.cycle_id.clone());
            let msg = format!(
                "工件 {cycle_id} 已到收尾截止，丢弃设备帧 {}",
                f.frame_counter
            );
            let fields = Fields {
                cycle_id: Some(cycle_id),
                camera: Some(camera.clone()),
                ..Fields::default()
            }
            .with_data(json!({"frameCounter": f.frame_counter}));
            self.noisy(
                &codes::FRAME_AFTER_DEADLINE,
                &codes::FRAME_AFTER_DEADLINE_SUMMARY,
                &camera,
                Tally::Range("frameCounter", f.frame_counter as i64),
                msg,
                fields,
            );
            return;
        };
        let Route::Bound { shot: k, ordinal } = route else {
            let msg = part.router.describe(&meta, &route);
            let (camera, cycle_id) = (part.camera_id(f.cam), part.cycle_id.clone());
            let mut data = json!({"reason": route_kind(&route), "frameCounter": f.frame_counter});
            if let Some(code) = route.fault() {
                data["faultCode"] = json!(code);
            }
            self.unrouted(
                camera,
                cycle_id,
                route.fault().is_some(),
                data,
                msg,
                f.frame_counter,
            );
            return;
        };
        let camera = part.camera_id(f.cam);
        if let Some(rec) = part.recording.as_mut() {
            host(&self.app).recorders.frame(rec, &f, &camera, k, ordinal);
        }
        let shot = &part.recipe.shots[k];
        let submitted_at = part.measuring_since[k].unwrap();
        let selected = shot.view;
        let skip = shot.skip;
        let expected_views = slot.as_ref().map_or(1, |s| s.config().view_count);
        let needs_image = expected_views > 1 || selected > 1 || !f.images.is_empty() || part.image_measurement;
        let selected_image = if needs_image { f.require_image(selected, expected_views).map(Some) } else { Ok(None) };
        let images = if needs_image {
            shot.enabled_views().into_iter().map(|view| f.require_image(view, expected_views).map(|image| (view, image)))
                .collect::<Result<Vec<_>, _>>()
        } else { Ok(Vec::new()) };
        let image = selected_image.as_ref().ok().cloned().flatten();
        crate::workspace::retain_live(&self.app, part.cycle_id.clone(), &part.recipe.revision_id, k, &image);
        let job = Job { cycle_id: part.cycle_id.clone(), shot_id: shot.id.clone(), camera,
            bundle_id: part.bundle_id.clone(), sn: part.sn.clone(), k, cam: f.cam, recipe: part.recipe.clone(),
            production: part.production.clone(), candidate: part.candidate.clone(),
            scenario: part.scenario, image, images: images.as_ref().ok().cloned().unwrap_or_default(), timeout: host(&self.app).settings().timeouts.proc(), submitted_at,
            image_measurement: part.image_measurement };
        if let Err(error) = images {
            return self.on_measured(Measured::failed(&job, format!("拍照点 {}：{error}", job.shot_id)));
        }
        if let Err(error) = selected_image {
            return self.on_measured(Measured::failed(&job, format!("拍照点 {}：{error}", job.shot_id)));
        }
        trace(&self.app, "帧到达", format!("cycle={} · {} · {} · 视角 {} · 设备触发 {} · 本件第 {} 次",
            job.cycle_id, job.shot_id, job.camera, selected, f.trigger_counter, ordinal),
            Fields { cycle_id: Some(job.cycle_id.clone()), view: Some(selected), ..Fields::shot(k, &job.shot_id, &job.camera) }
                .with_stage("frame").with_data(json!({"triggerCounter": f.trigger_counter, "frameCounter": f.frame_counter,
                    "session": f.session, "ordinal": ordinal, "arrivedMs": part.frames[k].arrived_ms})));
        if skip {
            let mut measured = Measured::empty(&job);
            measured.score = 1.0;
            self.on_measured(measured);
        } else {
            self.submit(job);
        }
    }

    /// 送测量队列；队列满了这一帧直接记为测量出错。
    fn submit(&mut self, job: Job) {
        self.dirty = true;
        match self.measure_tx.try_send(job) {
            Ok(()) => {}
            Err(error) => {
                let job = error.into_inner();
                self.on_measured(Measured::failed(&job, "测量队列已满：测量跟不上帧率"));
            }
        }
    }

    fn on_measured(&mut self, m: Measured) {
        if !matches!(self.phase, Phase::Acquire | Phase::Drain) { return; }
        self.dirty = true;
        let Some(part) = self.part.as_mut() else { return };
        let Some(m) = part.apply_result(m) else { return };
        self.dirty = true;
        // 告警与帧徽标同一来源：apply_result 写好的帧记录
        let subject = Subject { recipe_id: &part.recipe.id, revision_id: &part.recipe.revision_id, shot_id: &m.shot_id, k: m.k, camera: &m.camera };
        let changes = self.locate_health.observe(HistoryScope::of(part.source), &subject, &part.frames[m.k], &m.cycle_id, now_ms());
        for change in changes { log_locate_alarm(&self.app, change, &m.cycle_id); }
        let gaps = m.st.iter().filter(|&&s| s == measure::ST_GAP).count();
        let (level, ev, msg) = if let Some(e) = &m.error {
            ("err", "测量出错", format!("k={} · {e}", m.k))
        } else if !m.reference_failed.is_empty() {
            let views: Vec<_> = m.reference_failed.iter().map(|f| format!("图 {} {}", f.view, f.reason)).collect();
            ("err", "定位失败", format!("k={} {} · 有胶站没有位置基准 · 缺胶 {gaps} 点", m.k, views.join("，")))
        } else {
            let extra = if gaps > 0 { format!(" · 缺胶 {gaps} 点") } else { String::new() };
            (if gaps > 0 { "ng" } else { "info" }, "测量完成", format!("k={} 分数 {:.2} · {} 点 · {} ms{extra}", m.k, m.score, m.idx.len(), m.ms))
        };
        let fields = Fields { cycle_id: Some(m.cycle_id.clone()), ..Fields::shot(m.k, &m.shot_id, &m.camera) }.with_stage("measured")
            .with_data(json!({"ms": m.ms, "queueMs": m.queue_ms, "engineMs": m.engine_ms, "coreMs": m.core_ms,
                "score": m.score, "points": m.idx.len(), "gaps": gaps, "referenceFailed": m.reference_failed, "error": m.error}));
        if level == "info" { trace(&self.app, ev, msg, fields) } else { log_with(&self.app, level, ev, msg, fields) }
        let _ = self.app.emit("cycle://frame", &m);
        host(&self.app).shared.lock().unwrap().measured.push(m);
    }

    /// 段配方的一帧（D-SB-10）：按帧号配组；接受的帧录制、留预览并送测（所有帧都测，不裁帧）。上一件的迟到帧只记游离帧；
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
                &codes::FRAME_AFTER_DEADLINE,
                &codes::FRAME_AFTER_DEADLINE_SUMMARY,
                &camera,
                Tally::Range("frameCounter", f.frame_counter as i64),
                msg,
                fields,
            );
            return;
        };
        let cycle_id = part.cycle_id.clone();
        let burst = part.burst.as_mut().unwrap();
        let fields = |camera: &str, seq: Option<u64>| Fields { cycle_id: Some(cycle_id.clone()), camera: Some(camera.to_owned()), seq, ..Fields::default() };
        match arrival {
            burst::Arrival::Accepted { camera, seq } => {
                let camera_id = burst.camera_name(camera).to_owned();
                if let Some(rec) = part.recording.as_mut() { host(&app).recorders.burst_frame(rec, &f, camera, seq); }
                let image = f.single_image().cloned();
                crate::workspace::retain_live_frame(&app, &part.cycle_id, &part.recipe.revision_id, camera, seq, &image);
                let job = FrameJob { cycle_id: part.cycle_id.clone(), bundle_id: part.bundle_id.clone(), production: part.production.clone(),
                    candidate: part.candidate.clone(), camera_idx: camera, camera: camera_id.clone(), seq, image, image_measurement: part.image_measurement,
                    recipe: part.recipe.clone(), scenario: part.scenario, submitted_at: Instant::now(), timeout: part.proc_timeout };
                trace(&app, "帧到达", format!("cycle={cycle_id} · {camera_id} 第 {seq} 帧 · 帧计数 {}", f.frame_counter),
                    fields(&camera_id, Some(seq)).with_stage("frame")
                        .with_data(json!({"frameCounter": f.frame_counter, "triggerCounter": f.trigger_counter, "session": f.session})));
                self.frame_pool.submit(job);
            }
            burst::Arrival::Duplicate { camera, seq } => {
                if burst.first_warning(camera, burst::Warning::Duplicate) {
                    let camera_id = burst.camera_name(camera).to_owned();
                    log_with(&app, "warn", "重复帧号", format!("相机 {camera_id} 第 {seq} 帧重复到达，保留先到的那帧、丢弃后到的（本件同类情况不再逐条记录）"), fields(&camera_id, Some(seq)));
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
                let msg = format!("相机 {camera_id} 的帧（帧计数 {}）不属于本件：上一件的迟到帧或布防前那次打开的帧，记为游离帧", f.frame_counter);
                let fields =
                    fields(&camera_id, None).with_data(json!({"frameCounter": f.frame_counter}));
                self.noisy(
                    &codes::FRAME_LATE,
                    &codes::FRAME_LATE_SUMMARY,
                    &camera_id,
                    Tally::Range("frameCounter", f.frame_counter as i64),
                    msg,
                    fields,
                );
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

    /// 段配方一帧的测量结果：写进本件；没测成（队满、超时、失败）只记检测不完整，不判 ERR（D-SB-13）。
    fn on_frame_measured(&mut self, m: FrameMeasured) {
        if !matches!(self.phase, Phase::Acquire | Phase::Drain) { return; }
        let Some(part) = self.part.as_mut() else { return };
        let (ms, queue_ms, engine_ms) = (m.ms, m.queue_ms, m.engine_ms);
        let Some(metrics) = part.apply_burst(m) else { return };
        self.dirty = true;
        let camera = part.burst.as_ref().map_or("?", |b| b.camera_name(metrics.camera)).to_owned();
        let seq = metrics.seq;
        let fields = Fields { cycle_id: Some(part.cycle_id.clone()), camera: Some(camera.clone()), seq: Some(seq), ..Fields::default() }.with_stage("measured")
            .with_data(json!({"ms": ms, "queueMs": queue_ms, "engineMs": engine_ms, "state": metrics.state, "error": metrics.error}));
        let why = metrics.error.clone().unwrap_or_default();
        match metrics.state {
            FrameState::Measured => trace(&self.app, "测量完成", format!("{camera} 第 {seq} 帧 · 最大胶宽 {} · {ms} ms",
                metrics.width_max.map_or_else(|| "—".to_owned(), |w| format!("{w:.2} mm"))), fields),
            FrameState::OutOfCorridor => trace(&self.app, "走出走廊", format!("{camera} 第 {seq} 帧胶条走出走廊，记检测不完整"), fields),
            FrameState::Unmeasured => log_with(&self.app, "warn", "帧没测", format!("{camera} 第 {seq} 帧没测（{why}），记检测不完整"), fields),
            FrameState::Error | FrameState::Missing => log_with(&self.app, "warn", "测量出错", format!("{camera} 第 {seq} 帧测量失败（{why}），记检测不完整"), fields),
        }
    }

    async fn on_tick(&mut self) {
        let app = self.app.clone();
        self.flush_noise();
        app.state::<PlcHost>().flush_logs();
        let _link = host(&app).link_gate.lock().await;
        // 持有链路门期间站点模式不会切换：新事务按当前站点记来源
        self.s7.set_origin(if crate::offline::enabled(&app) {
            Origin::Offline
        } else {
            Origin::Real
        });
        if !host(&app).busy()
            && matches!(self.phase, Phase::Idle | Phase::Fault)
            && crate::offline::candidate(&self.app).is_none()
        {
            // 配置操作进行中就留到下一拍，不让空闲节拍排队
            if let Ok(_config) = host(&app).config_gate.try_lock() {
                if crate::workspace::apply_pending(&self.app) {
                    self.required = None;
                    self.dirty = true;
                }
            }
        }
        let connected = plc(&self.app).status().state == LinkState::Connected;
        let capture_host = host(&app).camera.recipe_capture().clone();
        if capture_host.active() && !connected { capture_host.fail("PLC 连接中断，请整圈重采"); }
        let scope = display_history_scope(crate::site::record_source(&app), self.part.as_ref().map(|p| p.source));
        if scope != self.stats_scope {
            self.stats_scope = scope;
            self.stats = today_stats(&app, scope);
            self.dirty = true;
        }
        self.sync_recording_alarm();
        {
            let mut teaching = host(&app).teaching.lock().await;
            let requested = teaching.view().phase == "requested";
            if requested && production_start_pending(plc(&app)).await.unwrap_or(false) {
                capture_host.interrupt("生产请求优先，本轮示教已取消");
                if teaching.cancel(plc(&app), 1).await.is_ok() {
                    host(&app).teaching_preempted.store(true, Ordering::SeqCst);
                }
            }
        }
        let cameras = if capture_host.active() {
            capture::capture_readiness(self.service_ready(), || self.capture_devices_ready())
        } else {
            match self.phase {
                // 在途的件只看它自己用的相机；等 PLC 确认结果时相机掉线不打断握手，回到空闲再查
                Phase::Acquire | Phase::Drain => match self.part.as_ref() {
                    Some(p) if p.rig_gen != host(&self.app).camera.generation() => {
                        Err("检测中增删了相机".into())
                    }
                    Some(p) => self.check_part_cams(p),
                    None => Ok(()),
                },
                Phase::Report | Phase::Release => Ok(()),
                _ => self.check_idle_devices(),
            }
        };
        let dropped = host(&self.app).camera.dropped_total();
        if dropped > self.dropped_seen {
            let added = dropped - self.dropped_seen;
            self.dropped_seen = dropped;
            let msg =
                format!("帧通道满，新丢弃 {added} 帧（累计 {dropped} 帧）：检测节拍处理不过来");
            let fields = Fields::default().with_data(json!({"dropped": added, "total": dropped}));
            self.noisy(
                &codes::FRAME_DROPPED,
                &codes::FRAME_DROPPED_SUMMARY,
                "",
                Tally::Sum("dropped", added),
                msg,
                fields,
            );
        }
        {
            let audit_ready = match host(&self.app).audit.readiness() {
                Ok(crate::audit::AuditReadiness::Ready) => true,
                Ok(crate::audit::AuditReadiness::WaitingRecording | crate::audit::AuditReadiness::WaitingStore) => false,
                Err(error) => {
                    let reason = format!("持久追溯故障：{error}");
                    self.s7.fault(plc(&app), reason.clone()).await;
                    if self.phase != Phase::Fault || self.fault.as_ref() != Some(&reason) {
                        self.enter_fault("audit", reason);
                    }
                    return;
                }
            };
            let teaching_requested = host(&app).teaching.lock().await.view().phase == "requested";
            let resetting = matches!(
                self.s7.phase(),
                SessionPhase::ResetRequired | SessionPhase::Fault
            ) || self.s7_reset_requested;
            let intake_ready = self.disk_space_ready()
                && (!resetting || self.check_idle_devices().is_ok())
                && (!host(&app).teaching_busy.load(Ordering::SeqCst)
                    || teaching_requested
                    || host(&app).teaching_preempted.load(Ordering::SeqCst));
            let devices = if resetting {
                self.service_ready()
            } else if teaching_requested || matches!(self.phase, Phase::Report | Phase::Release) {
                self.check_idle_devices()
            } else { cameras.clone() };
            let reset = std::mem::take(&mut self.s7_reset_requested);
            self.poll_teaching(reset, false).await;
            let previous = self.s7.phase();
            let previous_reset_blocked = self.s7.view().reset_blocked_reason;
            let had_ack = self.s7.acknowledged();
            let event = self.s7.poll_with_devices(plc(&app), reset, &devices, audit_ready, intake_ready).await;
            // 布防持有配置门直到下面 store_busy 登记在途，相机与配置操作要么在布防前完成，要么看到在途后拒绝
            let mut _config = None;
            if let Some(delivery) = s7_ack_delivery(had_ack, &self.s7).filter(|_| self.s7.capture_id().is_none()) {
                let result = match self.current_cycle_id.as_deref() {
                    Some(cycle_id) => host(&app).audit.delivery_durable(cycle_id, delivery).await,
                    None => Err("PLC 交付缺少工件身份".into()),
                };
                if let Err(error) = result {
                    self.s7.fault(plc(&app), format!("PLC 确认审计持久化失败：{error}")).await;
                    return self.enter_fault("audit", format!("PLC 确认审计持久化失败：{error}"));
                }
            }
            match event {
                SessionEvent::Ready => {
                    self.poll_teaching(true, true).await;
                    self.ledgers.clear_floors();
                    self.clear_fault();
                    self.alarms.clear();
                    self.set_phase(Phase::Idle);
                    log_event(
                        &app,
                        &applog::codes::S7_RESET_DONE,
                        "输入基线已核验，视觉就绪",
                        Fields::stage("reset"),
                    );
                    applog::end_cycle();
                }
                SessionEvent::Start(request) => {
                    let mut teaching = host(&app).teaching.lock().await;
                    let view = teaching.view();
                    if view.busy {
                        if view.phase == "requested" {
                            capture_host.interrupt("生产请求优先，本轮示教已取消");
                            if let Err(error) = teaching.cancel(plc(&app), 1).await {
                                self.s7.fault(plc(&app), error.clone()).await;
                                drop(teaching);
                                self.enter_fault("teach", error);
                                return;
                            }
                            host(&app).teaching_preempted.store(true, Ordering::SeqCst);
                        } else if !host(&app).teaching_preempted.load(Ordering::SeqCst) {
                            let error = "PLC 在示教已占用资源时提交生产请求".to_string();
                            teaching.force_fault(error.clone());
                            capture_host.interrupt(error.clone());
                            self.s7.fault(plc(&app), error.clone()).await;
                            drop(teaching);
                            self.enter_fault("teach", error);
                            return;
                        }
                    }
                    drop(teaching);
                    _config = Some(host(&app).config_gate.lock().await);
                    self.start_part(request).await;
                }
                SessionEvent::End(ended_at) => {
                    if let Some(part) = self.part.as_mut() {
                        part.end_at = Some(ended_at);
                    }
                    self.set_phase(Phase::Drain);
                    log_with(&app, "info", "plcState=20", "PLC 报运动结束，等待剩余帧", Fields::stage("partEnd"));
                }
                SessionEvent::Released => {
                    self.close_routing(None);
                    self.alarms.retain(|m| !m.starts_with("PLC 未确认"));
                    self.set_phase(Phase::Idle);
                    log_with(&app, "info", "S7 事务结束", "PLC 已锁存结果，视觉已回就绪", Fields::stage("released"));
                    applog::end_cycle();
                }
                SessionEvent::Fault(reason) => {
                    if self.s7.capture_id().is_some() { capture_host.fail(reason.clone()); }
                    self.enter_fault("s7", reason);
                },
                SessionEvent::Restored(reason) => {
                    log(&app, "warn", "S7 空闲恢复", reason.clone());
                    if !self.alarms.contains(&reason) { self.alarms.push(reason); }
                    self.dirty = true;
                }
                SessionEvent::None => {}
            }
            // 空闲等待提示（PLC 输入未释放、设备未就绪）跟着会话现状走，不进故障
            let notice = self.s7.notice().map(str::to_owned);
            let shown: Vec<String> = self.alarms.iter().filter(|m| m.starts_with(crate::plc_session::NOTICE)).cloned().collect();
            if shown.as_slice() != notice.as_slice() {
                self.alarms.retain(|m| !m.starts_with(crate::plc_session::NOTICE));
                if let Some(notice) = notice {
                    log(&app, "warn", "S7 握手", notice.clone());
                    self.alarms.push(notice);
                }
                self.dirty = true;
            }
            if self.s7.phase() == SessionPhase::Releasing && self.phase == Phase::Report {
                self.set_phase(Phase::Release);
            }
            if matches!(self.s7.phase(), SessionPhase::ResetRequired | SessionPhase::Fault) {
                let reason = self.s7.view().message.unwrap_or_else(|| "S7 需要明确复位".into());
                if self.phase != Phase::Fault || self.fault.as_ref() != Some(&reason) {
                    self.enter_fault("s7", reason);
                }
            }
            if previous != self.s7.phase() || previous_reset_blocked != self.s7.view().reset_blocked_reason { self.dirty = true; }
            host(&app).store_busy(self.phase, &self.s7);
        }
        self.poll_teaching(false, self.s7.phase() == SessionPhase::Idle)
            .await;
        let timeouts = host(&self.app).settings().timeouts;
        if matches!(self.phase, Phase::Acquire | Phase::Drain) {
            if let Some(part) = self.part.as_mut() {
                for k in part.expire(Instant::now(), timeouts.proc(), timeouts.drain()) {
                    let shot = &part.recipe.shots[k];
                    log_with(&self.app, "err", "拍照点超时", part.frames[k].error.clone().unwrap_or_default(),
                        Fields { cycle_id: Some(part.cycle_id.clone()), ..Fields::shot(k, &shot.id, &shot.camera) }.with_data(json!({"status": part.frames[k].status})));
                    self.dirty = true;
                }
                let expired = part.expire_burst(Instant::now(), timeouts.proc());
                if !expired.is_empty() {
                    let frames: Vec<String> = expired.iter().take(8).map(|(camera, seq)| format!("相机 {} 第 {seq} 帧", part.burst.as_ref().map_or("?", |b| b.camera_name(*camera)))).collect();
                    let more = if expired.len() > frames.len() { format!(" 等 {} 帧", expired.len()) } else { String::new() };
                    log_with(&self.app, "warn", "帧测量超时", format!("{}{more} 测量超时，记为没测（检测不完整）", frames.join("、")),
                        Fields { cycle_id: Some(part.cycle_id.clone()), ..Fields::default() });
                    self.dirty = true;
                }
            }
        }
        match self.phase {
            Phase::Acquire => {
                let Some(part) = self.part.as_mut() else { return };
                if part.armed_at.elapsed() > timeouts.motion() {
                    part.fault = Some((fault::MOTION_TIMEOUT, format!("布防后 {} s 内未收到运动结束（plcState=20）", timeouts.motion_ms / 1000)));
                    part.end_at = Some(Instant::now());
                    log(&self.app, "err", "运动超时", format!("T_motion {} ms", timeouts.motion_ms));
                    self.set_phase(Phase::Drain);
                }
            }
            Phase::Drain => {
                if self.part.as_ref().is_some_and(Part::complete) {
                    self.judge_part().await;
                }
            }
            Phase::Report => {
                if !self.ack_alarmed && self.done_at.is_some_and(|t| t.elapsed() > timeouts.ack()) {
                    self.ack_alarmed = true;
                    self.alarm(format!("PLC 未确认结果（超过 T_ack {} ms），保持 pcState=30", timeouts.ack_ms));
                }
            }
            Phase::Release if now_ms().saturating_sub(self.since) > timeouts.ack_ms as i64 => {
                self.alarm("PLC 已锁存结果，等待追溯落盘后回就绪；暂不接收下一件".into());
            }
            _ => {}
        }
    }

    async fn judge_part(&mut self) {
        if let Some(part) = self.part.as_ref() {
            if let Err(error) = self.check_part_routing(part) {
                self.part.as_mut().unwrap().fault.get_or_insert(error);
            }
        }
        self.set_phase(Phase::Judge);
        let Some(part) = self.part.as_mut() else { return };
        let mut warnings = Vec::new();
        let judgement = match part.burst_judgement() {
            Some(decided) => {
                let burst = part.burst.as_mut().unwrap();
                warnings = burst_warnings(&decided, |camera| burst.camera_name(camera).to_owned());
                let summary = burst::summary(&decided);
                burst.judgement = Some(decided);
                summary
            }
            None => part.judgement(),
        };
        let (sn, id, cycle_id) = (part.sn.clone(), part.recipe.id.clone(), part.cycle_id.clone());
        for (ev, message) in warnings {
            log_with(&self.app, "warn", ev, message, Fields { cycle_id: Some(cycle_id.clone()), ..Fields::default() });
        }
        self.report(sn, Some(id), judgement).await;
    }

    async fn report(&mut self, sn: String, recipe_id: Option<String>, judgement: Judgement) {
        // 旧拍照点配方：S7 没等到 partEnd 就判了 ERR（运动超时等）时手里还没有 PLC 报的触发数，等 PLC 确认结果时报的实际已发触发数
        // 再推下一件基线，按计划数假定会让下一件基线偏高、每件都缺帧。段配方（issued 为空）的下一件基线只看见到的帧计数，直接收尾
        self.close_routing(None);
        let app = self.app.clone();
        let mut drain_ms = None;
        let pending = PlcDelivery { state: PlcDeliveryState::Pending, updated_at: now_ms(), message: None };
        let record = self.recorded_part(&sn, recipe_id.as_deref(), &judgement, drain_ms, pending);
        let burst = record.as_ref().ok().and_then(|part| part.burst.as_ref()).map(|b| b.judgement.clone());
        let durable = match record {
            Ok(part) => host(&app).audit.insert_durable(part, host(&app).settings().timeouts.proc()).await.and_then(|_| self.record_without_part(&sn, &judgement)),
            Err(error) => Err(error),
        };
        let mut failure_saved = false;
        let r = match durable {
            Err(error) => Err(format!("审计记录未可靠保存，未提交 PLC 完成信号：{error}")),
            Ok(()) => {
                let sent = self.s7.report(plc(&app), judgement.plc_code, judgement.fault_code).await;
                self.done_at = sent.as_ref().ok().map(|_| Instant::now());
                drain_ms = sent.as_ref().ok().and_then(|_| self.part.as_ref().and_then(|part| part.end_at)).map(|end| end.elapsed().as_millis() as u64);
                let delivery = PlcDelivery { state: if sent.is_ok() { PlcDeliveryState::Submitted } else { PlcDeliveryState::Failed },
                    updated_at: now_ms(), message: sent.as_ref().err().map(|error| format!("回写 PLC 失败：{error}")) };
                let saved = match self.current_cycle_id.as_deref() {
                    Some(id) => host(&app).audit.submission_durable(id, delivery, drain_ms).await,
                    None => Err("工件身份缺失".into()),
                };
                failure_saved = sent.is_err() && saved.is_ok();
                sent.map_err(|error| format!("回写 PLC 失败：{error}")).and(saved.map_err(|error| format!("PLC 交付审计未可靠保存：{error}")))
            }
        };
        self.ack_alarmed = false;
        self.count(judgement.verdict);
        let level = match judgement.verdict {
            Verdict::Ok | Verdict::OkWithExcursion => "ok",
            Verdict::ErrInspect => "err",
            _ => "ng",
        };
        let fault = if judgement.fault_code > 0 { format!(" faultCode={}", judgement.fault_code) } else { String::new() };
        let delivery = if r.is_ok() { "done 已提交" } else { "交付异常，需人工确认" };
        log_with(&app, level, "检测结果", format!("resultCode={}{fault} snEcho={sn} · {delivery} · {}", judgement.plc_code, judgement.reason),
            Fields::stage("result").with_data(json!({"verdict": judgement.verdict, "plcCode": judgement.plc_code,
                "faultCode": judgement.fault_code, "drainMs": drain_ms, "delivered": r.is_ok()})));
        self.finish_recording(&sn, &judgement);
        self.result = Some(ResultView { candidate_debug: self.part.as_ref().and_then(|p| p.candidate.as_ref()).map(|c| c.info.clone()).or_else(|| crate::offline::candidate(&self.app).map(|c| c.info.clone())), cycle_id: self.current_cycle_id.clone(), sn, recipe_id, ts: now_ms(), drain_ms, judgement, burst });
        match r {
            Ok(()) => self.set_phase(Phase::Report),
            Err(reason) => {
                self.s7.fault(plc(&app), reason.clone()).await;
                if !failure_saved && self.current_cycle_id.is_some() { self.fail_delivery(&reason); }
                self.fault = Some(reason.clone());
                self.alarm(reason);
                self.set_phase(Phase::Fault);
            }
        }
    }

    fn close_routing(&mut self, plc_triggers: Option<u16>) {
        if let Some(part) = self.part.as_mut() { part.close_routing(&mut self.ledgers, plc_triggers); }
    }

    fn check_part_cams(&self, part: &Part) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        rig.check_ready_at(&part.cams)?;
        let identities = part.cams.iter().map(|&cam| {
            rig.slot(cam as usize).ok_or_else(|| "相机组已变化".to_string())?.routing_state().map(|state| state.0)
        }).collect::<Result<Vec<_>, _>>()?;
        match &part.burst {
            Some(burst) => burst.check_sessions(&identities),
            None => part.router.check_sessions(&identities),
        }
    }

    fn check_part_routing(&self, part: &Part) -> Result<(), (u16, String)> {
        self.check_part_cams(part).map_err(|reason| (fault::DEVICE_LOST, reason))?;
        let rig = &host(&self.app).camera;
        for &cam in &part.cams {
            let slot = rig.slot(cam as usize).ok_or_else(|| (fault::DEVICE_LOST, "相机组已变化".into()))?;
            let (_, observed) = slot.routing_state().map_err(|reason| (fault::DEVICE_LOST, reason))?;
            if let Some(ledger) = observed {
                match &part.burst {
                    Some(burst) => burst.check_observed(cam, &ledger)?,
                    None => part.router.check_observed(cam, &ledger)?,
                }
            }
        }
        Ok(())
    }

    fn finish_recording(&mut self, sn: &str, judgement: &Judgement) {
        let Some(rec) = self.part.as_mut().filter(|p| p.sn == sn).and_then(|p| p.recording.take()) else { return };
        let host = host(&self.app);
        let settings = host.settings();
        host.recorders.finish(rec, judgement.verdict, &judgement.reason, &settings);
    }

    fn recorded_part(&self, sn: &str, recipe_id: Option<&str>, judgement: &Judgement, drain_ms: Option<u64>, delivery: PlcDelivery) -> Result<RecordedPart, String> {
        let Some(cycle_id) = self.current_cycle_id.clone() else {
            return Err("工件身份尚未持久分配，不能关联历史与原图".into());
        };
        let part = self.part.as_ref().filter(|p| p.sn == sn);
        let recipe = part.map(|p| p.recipe.clone()).or_else(|| crate::offline::candidate(&self.app).filter(|c| recipe_id == Some(c.recipe.id.as_str())).map(|c| c.recipe.clone())).or_else(|| recipe_id.and_then(|id| host(&self.app).recipe(id)));
        let frames = part.map(|p| p.frames.clone()).unwrap_or_default();
        let burst = self.burst_outcome(sn, recipe.as_deref(), judgement);
        // 段配方：预期帧数取 ΣN_exp（待标定为 0），收到的帧数取帧组数，触发数取 PLC 报的 deviceTriggers
        let frames_expected = match &burst { Some(b) => b.expected_total.unwrap_or(0) as usize, None => recipe.as_ref().map_or(0, |r| r.shot_count()) };
        let table = part.map(|p| p.table.clone()).filter(|t| t.iter().any(|x| *x != PointState::Pending));
        let (received, triggers) = match &burst {
            Some(b) => (b.frames.iter().map(|f| f.seq).max().unwrap_or(0) as usize, b.device_triggers.map_or(0, u64::from)),
            None => part.map_or((0, 0), |p| (p.received, p.triggers())),
        };
        let shots = recorded_shots(recipe.as_deref(), &frames, &judgement.reason);
        let source = part.map_or_else(|| crate::site::record_source(&self.app), |p| p.source);
        let sample = (matches!(source, RecordSource::OfflineSample | RecordSource::OfflineCandidate)).then(|| crate::offline::claim_sample(&self.app, sn, &cycle_id)).flatten();
        Ok(RecordedPart { ts: now_ms(), sn: sn.to_owned(), recipe, judgement: judgement.clone(),
            drain_ms, frames, frames_expected, frames_received: received, triggers, table,
            software_version: self.app.package_info().version.to_string(), cycle_id: cycle_id.clone(),
            bundle_id: part.and_then(|p| p.bundle_id.clone()), delivery, shots, source, sample, burst })
    }

    /// 段配方的帧与区域记录：在途件按它的帧组与判定给出；没有在途件（拒绝布防）时按配方给出没有帧的记录。旧拍照点配方为 None。
    fn burst_outcome(&self, sn: &str, recipe: Option<&Recipe>, judgement: &Judgement) -> Option<BurstRecord> {
        match self.part.as_ref().filter(|p| p.sn == sn) {
            Some(part) => part.burst_record(judgement),
            None => recipe.and_then(|recipe| burst::empty_record(recipe, judgement)),
        }
    }

    fn record_without_part(&self, sn: &str, judgement: &Judgement) -> Result<(), String> {
        if self.part.as_ref().is_none_or(|part| part.sn != sn) {
            let cycle_id = self.current_cycle_id.clone().ok_or("工件身份缺失")?;
            let off = crate::site::record_mode(&self.app) == crate::settings::RecordMode::Off;
            host(&self.app).audit.recording(crate::recorder::RecordingOutcome { cycle_id, directory: None,
                files: Vec::new(), retention_errors: Vec::new(), available: false,
                state: if off { crate::recorder::RecordingState::Off } else { crate::recorder::RecordingState::Failed },
                errors: if off { Vec::new() } else { vec![format!("布防校验未通过，未开始录制：{}", judgement.reason)] } })?;
        }
        Ok(())
    }


    fn record(&self, sn: &str, recipe_id: Option<&str>, judgement: &Judgement, drain_ms: Option<u64>, delivery: PlcDelivery) -> Result<(), String> {
        host(&self.app).audit.insert(self.recorded_part(sn, recipe_id, judgement, drain_ms, delivery)?)?;
        self.record_without_part(sn, judgement)
    }

    fn delivery(&self, state: PlcDeliveryState, message: Option<String>) -> Result<(), String> {
        let cycle_id = self.current_cycle_id.as_deref().ok_or("PLC 交付缺少工件身份")?;
        host(&self.app).audit.delivery(cycle_id, PlcDelivery { state, updated_at: now_ms(), message })
    }

    fn fail_delivery(&self, reason: &str) {
        if let Err(error) = self.delivery(PlcDeliveryState::Failed, Some(reason.to_owned())) {
            log(&self.app, "err", "故障交付审计持久化失败", error);
        }
    }

    fn count(&mut self, v: Verdict) {
        let today = history::local_day();
        if today != self.stats_day {
            self.stats_day = today;
            self.stats = Stats::default();
            let app = self.app.clone();
            tauri::async_runtime::spawn_blocking(move || purge_history(&app));
        }
        self.stats.total += 1;
        match v {
            Verdict::Ok | Verdict::OkWithExcursion => self.stats.ok += 1,
            Verdict::ErrInspect => self.stats.err += 1,
            _ => self.stats.ng += 1,
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
            &self.app,
            &applog::codes::FAULT_REPEAT,
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
                &self.app,
                &applog::codes::FAULT_CLEARED,
                format!("故障解除：{reason}"),
                fields,
            );
        }
        self.fault = None;
        self.fault_source = None;
        self.fault_at = None;
    }

    /// 进入故障。已因同一来源同一原因处于故障时只计数（之前每拍重复写日志、重置故障计时）。
    fn enter_fault(&mut self, source: &'static str, reason: String) {
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
        self.close_routing(None);
        let production_pending = self.s7.request().is_some();
        let in_flight = production_pending
            && matches!(
                self.phase,
                Phase::Validate | Phase::Acquire | Phase::Drain | Phase::Judge
            );
        if in_flight {
            let sn = self.part.as_ref().map(|p| p.sn.clone()).or_else(|| self.s7.request().map(|r| r.sn.clone())).unwrap_or_default();
            let judgement = Judgement::error(fault::DEVICE_LOST, format!("{reason}，结果未回写"));
            self.count(judgement.verdict);
            if let Err(error) = self.record(&sn, self.part.as_ref().map(|p| p.recipe.id.clone()).as_deref(), &judgement, None,
                PlcDelivery { state: PlcDeliveryState::Failed, updated_at: now_ms(), message: Some(reason.clone()) }) {
                log(&self.app, "err", "中断工件审计持久化失败", error);
            }
            self.finish_recording(&sn, &judgement);
            log_with(&self.app, "err", "在途件中断", format!("SN {sn} 记 ERR 98，需人工处理该件"), Fields::stage("result"));
            let burst = self.burst_outcome(&sn, self.part.as_ref().map(|p| &*p.recipe), &judgement).map(|record| record.judgement);
            self.result = Some(ResultView { candidate_debug: self.part.as_ref().and_then(|p| p.candidate.as_ref()).map(|c| c.info.clone()).or_else(|| crate::offline::candidate(&self.app).map(|c| c.info.clone())), cycle_id: self.current_cycle_id.clone(), sn, recipe_id: self.part.as_ref().map(|p| p.recipe.id.clone()), ts: now_ms(), drain_ms: None, judgement, burst });
        }
        if production_pending && fault_fails_delivery(self.phase) && self.result.is_some() {
            self.fail_delivery(&reason);
        }
        let teach_phase = host(&self.app)
            .teaching
            .try_lock()
            .ok()
            .map(|t| t.view().phase);
        let fields = Fields { source: Some(source.into()), ..Fields::stage("fault") }
            .with_data(json!({"reason": reason, "s7Phase": format!("{:?}", self.s7.phase()), "teachPhase": teach_phase}));
        log_event(
            &self.app,
            &applog::codes::FAULT_ENTER,
            reason.clone(),
            fields,
        );
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

/// 段配方的件不经过拍照点路由：空计划的路由（没有相机，不绑任何帧）。
fn unrouted() -> ShotRouter {
    ShotRouter::arm(&Plan::new(std::iter::empty::<(&str, &str)>()), &Ledgers::default(), &Policy::offline(None), &[]).expect("空计划不需要相机")
}

/// 段配方判定里只告警的项（丢帧、帧数异常、疑似提前终止），写进运行日志。
fn burst_warnings(judgement: &BurstJudgement, camera: impl Fn(usize) -> String) -> Vec<(&'static str, String)> {
    use crate::frame_group::{CountStatus, LossReason};
    let mut out: Vec<(&'static str, String)> = judgement.alarms.iter().map(|alarm| {
        let name = camera(alarm.camera);
        let basis = match &alarm.reason {
            LossReason::Triggers { device_triggers } => format!("PLC 报 deviceTriggers {device_triggers}"),
            LossReason::Cameras { other, other_count } => format!("相机 {} 本件 {other_count} 帧", camera(*other)),
        };
        ("丢帧告警", format!("相机 {name} 本件 {} 帧，与{basis}相差超过 1：疑似丢帧（只告警，不改结论）", alarm.count))
    }).collect();
    if matches!(judgement.count_status, Some(CountStatus::Low | CountStatus::High)) || judgement.flags.frame_count_anomaly {
        out.push(("帧数异常", format!("整件 {} 个帧组，有区域帧组数超出预期 ± 容差（只告警，不改结论）", judgement.group_count)));
    }
    if judgement.flags.short_part {
        out.push(("疑似提前终止", format!("整件 {} 个帧组，少于 ΣN_exp − Σtol：疑似 PLC 提前报运动结束（只告警，不改结论）", judgement.group_count)));
    }
    out
}

/// 故障时只有已回写结果、仍在等 PLC 确认的工件改记交付失败；已确认、已释放的工件不再改写，交付失败进入故障的工件已在 report 里自行记失败。
fn fault_fails_delivery(phase: Phase) -> bool {
    phase == Phase::Report
}

/// 今日统计只算当前站点的范围：真机只算真机件，离线只算离线件。
pub fn display_history_scope(site: RecordSource, previous: Option<RecordSource>) -> HistoryScope {
    if site == RecordSource::OfflineSample && previous == Some(RecordSource::OfflineCandidate) { HistoryScope::Debug }
    else { HistoryScope::of(site) }
}

fn today_stats(app: &AppHandle, scope: HistoryScope) -> Stats {
    app.state::<Store>().counts_since(history::local_midnight_ms(), scope).map(Stats::from).unwrap_or_default()
}

#[tauri::command]
pub fn cycle_snapshot(cycle: State<'_, CycleHost>) -> Option<Snapshot> {
    cycle.shared.lock().unwrap().snapshot.clone()
}

#[tauri::command]
pub fn cycle_logs(cycle: State<'_, CycleHost>) -> Vec<LogLine> {
    cycle.shared.lock().unwrap().logs.iter().cloned().collect()
}

#[tauri::command]
pub fn cycle_part_data(cycle: State<'_, CycleHost>) -> Vec<Measured> {
    cycle.shared.lock().unwrap().measured.clone()
}

#[tauri::command]
pub fn cycle_recipes(cycle: State<'_, CycleHost>) -> Vec<RecipeSummary> {
    cycle.recipes.list().iter().map(|r| RecipeSummary::from(&**r)).collect()
}

/// 配方的运行数据。给了 revision_id 时要的是那一版：工件用的配方刚改过，这一件仍按开工时的快照画。
#[tauri::command]
pub fn cycle_layout(cycle: State<'_, CycleHost>, recipe_id: String, revision_id: Option<String>) -> Result<Arc<Recipe>, String> {
    if let Some(r) = cycle.shared.lock().unwrap().part_recipe.clone().filter(|r| r.id == recipe_id && revision_id.as_ref() == Some(&r.revision_id)) {
        return Ok(r);
    }
    let r = cycle.recipe(&recipe_id).ok_or_else(|| format!("配方不存在：{recipe_id}"))?;
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
pub fn cycle_save_settings(app: AppHandle, cycle: State<'_, CycleHost>, settings: CycleSettings) -> Result<(), String> {
    if let Some(id) = &settings.manual_recipe_id {
        cycle.recipe(id).ok_or("配方不存在")?;
    }
    cycle.save_settings(settings)?;
    measure::apply_settings(&app);
    let _ = app.emit("cycle://settings-changed", ());
    crate::commands::log_config(&app);
    Ok(())
}

/// 人工选择配方，只能在空闲或故障时切换，新配方从下一个工件起生效。
#[tauri::command]
pub fn cycle_select_recipe(cycle: State<'_, CycleHost>, recipe_id: String) -> Result<(), String> {
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在布防或处理其他配置操作，请稍后重试")?;
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

#[tauri::command]
pub fn cycle_reset(app: AppHandle, cycle: State<'_, CycleHost>) {
    let sent = cycle.tx.send(Input::Reset).map_err(|e| e.to_string());
    log_op(
        &app,
        &applog::codes::OP_RESET,
        json!({"phase": format!("{:?}", cycle.phase())}),
        &sent,
    );
}

/// 运行日志 op.*：操作员命令的参数摘要与结果。
pub fn log_op<T>(
    app: &AppHandle,
    event: &applog::codes::Event,
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
        app,
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
    tauri::async_runtime::spawn_blocking(move || readiness::gather(&app)).await.map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests;

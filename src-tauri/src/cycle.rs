mod admission;
mod burst;
mod interrupted;
pub mod readiness;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ly_plc::{now_ms, PlcEngine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, UnboundedReceiver, UnboundedSender};

use crate::applog::codes::{self, Event};
use crate::applog::throttle::{self, Tally, Throttle};
use crate::applog::{self, Fields};
use crate::arming::ArmBudget;
use crate::audit::{Audit, RecordedPart};
use crate::burst_judge::{BurstJudgement, FrameState};
use crate::camera::{CameraRig, FRAME_QUEUE};
use crate::frame::Frame;
use crate::handshake::Request;
use crate::history;
use crate::judge::{fault, Judgement, Verdict};
use crate::measure::{self, FrameJob, FrameMeasured, FramePool};
use crate::offline::Scenario;
use crate::plc::PlcHost;
use crate::plc_session::{Origin, PartSite, PendingKind, PlcSession, SessionEvent, SessionPhase, SessionView};
use crate::recipe::{Recipe, RecipeStore, TriggerMode};
use crate::recorder::{Recorder, Recorders, Recording};
use crate::recording_health::{AlarmChange, RecordingAlarm, RecordingHealth};
use crate::settings::{CycleSettings, ProductSource};
use crate::shot_router::{arm_frames, ArmCam, Floor, FrameMeta, Ledgers, Policy};
use crate::store::{
    BurstRecord, HistoryScope, PlcDelivery, PlcDeliveryState, RecordSource, Store, VerdictCounts,
};

pub(crate) fn s7_ack_delivery(had_ack: bool, session: &PlcSession) -> Option<PlcDelivery> {
    (!had_ack && session.acknowledged()).then(|| PlcDelivery {
        state: PlcDeliveryState::Acknowledged,
        updated_at: now_ms(),
        message: Some("PLC 结果序号已匹配确认".into()),
    })
}

pub enum Input {
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
    /// 段配方的判定；有值时 judgement 的 verdict / plc_code / fault_code / reason 与它相同。没有配方（拒收）时为 None
    pub burst: Option<BurstJudgement>,
    pub mode: crate::store::PartMode,
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
    pub maintenance: Option<crate::settings::Maintenance>,
    /// 维护采集进度，按配方
    pub collection: Vec<crate::store::CollectionProgress>,
    /// 维护试判统计，按配方（最新正式版本）
    pub trial: Vec<crate::store::TrialProgress>,
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
    pub trigger_mode: TriggerMode,
    pub cameras: Vec<String>,
    pub length: f32,
    /// PLC 计划版本
    pub plan_version: u32,
    /// PLC 触发次数 ΣN_exp（未标定为空）
    pub trigger_count: Option<usize>,
    pub segment_count: usize,
    /// 整件预期帧组数 ΣN_exp；未标定为空
    pub expected_total: Option<u32>,
    /// 整件帧数容差 Σtolerance
    pub tolerance_total: u32,
    /// 已标定（expected_total 非空）
    pub calibrated: bool,
    /// 示教件来源（RB-7）：取自生产配方文档；旧流程发布的配方为空
    pub teaching_piece: Option<crate::recipe::TeachingPieceRef>,
}

impl RecipeSummary {
    /// 配方库里一份生产配方的摘要，带配方文档里的示教件来源。
    pub fn listed(store: &RecipeStore, r: &Recipe) -> Self {
        Self { teaching_piece: store.doc(&r.id).and_then(|d| d.teaching_piece), ..Self::from(r) }
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
            plan_version: r.plan_version,
            trigger_count: expected_total.map(|n| n as usize),
            segment_count: r.path_segments.len(),
            expected_total,
            tolerance_total: r.tolerance_total(),
            calibrated: expected_total.is_some(),
            teaching_piece: None,
        }
    }
}

struct Shared {
    snapshot: Option<Snapshot>,
    logs: VecDeque<LogLine>,
    /// 当前（或刚结束的）一件用的配方快照
    part_recipe: Option<Arc<Recipe>>,
    settings: CycleSettings,
}

pub struct CycleHost {
    pub tx: UnboundedSender<Input>,
    /// PLC 链路门：S7 握手、PLC 配置/连接/手动写点互斥。节拍每个周期都持有。
    /// 两把门都要时先取 link_gate 再取 config_gate；只持 config_gate 时不能再取 link_gate。
    pub link_gate: tokio::sync::Mutex<()>,
    /// 布防与配置门：布防、切换生产配方与相机重配置、配方/设置/标定修改互斥。节拍只在布防到登记在途、切换待生效配方时持有。
    pub config_gate: tokio::sync::Mutex<()>,
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
        let recording_root = crate::log_storage::recording_project(&data)?;
        std::fs::create_dir_all(&recording_root).map_err(|e| e.to_string())?;
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
            recorder(recording_root.join("records"), HistoryScope::Real),
            recorder(
                recording_root.join("records-offline"),
                HistoryScope::Offline,
            ),
        );
        Ok(Self {
            camera,
            tx,
            link_gate: tokio::sync::Mutex::new(()),
            config_gate: tokio::sync::Mutex::new(()),
            recipes: RecipeStore::open_with_floors(
                data.join("recipes"),
                crate::storage::state(app).join("plan-version"),
                &app.state::<crate::store::Store>().recipe_version_floors()?,
            )?,
            recorders,
            cycle_ids: crate::cycle_ids::CycleIds::default(),
            audit,
            disk: crate::disk_guard::DiskGuard::new(data.clone())
                .with_recording_root(recording_root),
            recording_health,
            shared: Mutex::new(Shared {
                snapshot: None,
                logs: VecDeque::new(),
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
        let frame_tx = host.tx.clone();
        let frame_pool = measure::spawn_frame_worker(Arc::new(move |measured| { let _ = frame_tx.send(Input::FrameMeasured(measured)); }));
        let mut machine = Machine::new(app.clone(), frame_pool);
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
    let records = applog::dir(app)
        .map(|dir| {
            applog::cycle_records(&dir, cycle_id, started_at.unwrap_or_else(now_ms), 200).records
        })
        .unwrap_or_default();
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
        &records,
        |id| host.recipe(id),
        app.package_info().version.to_string(),
        now_ms(),
    );
    let label = site.source.label();
    let counted = part.mode == crate::store::PartMode::Production
        && interrupted::counts_in_stats(part.source, part.ts, scope, history::local_midnight_ms());
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

pub fn log(app: &AppHandle, level: &'static str, ev: impl Into<String>, msg: impl Into<String>) {
    log_with(app, level, ev, msg, Fields::default());
}

pub fn log_with(app: &AppHandle, level: &'static str, ev: impl Into<String>, msg: impl Into<String>, fields: Fields) {
    publish_log(app, level, applog::Level::from_tone(level), ev.into(), msg.into(), fields);
}

/// 逐帧的明细：界面事件流照常显示，日志文件记为 debug。
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
    mode: crate::store::PartMode,
    cycle_id: String,
    bundle_id: Option<String>,
    production: Option<Arc<crate::production::Prepared>>,
    candidate: Option<Arc<crate::candidate::Candidate>>,
    sn: String,
    recipe: Arc<Recipe>,
    scenario: Scenario,
    /// 帧组与每帧测量（`cycle::burst`）
    burst: Box<burst::BurstPart>,
    /// partEnd 时 PLC 报的设备触发数（协议 6 只作参考，可与计划数不同）；partEnd 前为空。
    /// 协议 7 不报触发数，恒为空，不参与下一件基线（D-SB-7）
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
        if self.routing_closed { return; }
        self.burst.close(ledgers);
        self.routing_closed = true;
    }

    fn camera_id(&self, cam: u8) -> String {
        self.cams.iter().position(|&c| c == cam).and_then(|i| self.camera_ids.get(i).cloned()).unwrap_or_else(|| format!("#{}", cam + 1))
    }

    fn complete(&self) -> bool {
        self.burst_complete(Instant::now())
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
            received: burst.camera_frames.iter().map(|&n| n as usize).sum(),
            triggers: self.triggers(),
            queue: burst.pending_measure,
            measured_frames: burst.measured_frames,
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

const COLLECTION_REFRESH: Duration = Duration::from_secs(5);

/// 维护模式下没建立检测就布防放行的件。
struct Uninspected {
    sn: String,
    recipe_id: Option<String>,
    judgement: Judgement,
    armed_at: Instant,
}

/// 段配方一帧的测量任务：用第 `segment` 段走廊量计划相机 `camera` 的第 `seq` 帧（到达送测与收尾补测共用）。
fn frame_job(
    part: &Part,
    camera: usize,
    camera_id: String,
    seq: u64,
    segment: u32,
    image: Option<Arc<crate::frame::FrameImage>>,
) -> FrameJob {
    FrameJob {
        cycle_id: part.cycle_id.clone(),
        bundle_id: part.bundle_id.clone(),
        production: part.production.clone(),
        candidate: part.candidate.clone(),
        camera_idx: camera,
        camera: camera_id,
        seq,
        segment,
        image,
        image_measurement: part.image_measurement,
        recipe: part.recipe.clone(),
        scenario: part.scenario,
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
    let devices: Vec<serde_json::Value> = cams.iter().zip(recipe.plan_cameras()).map(|(&cam, camera)| {
        let slot = host.camera.slot(cam as usize);
        json!({
            "cameraId": camera,
            "cameraConfig": slot.as_ref().map(|slot| slot.config()),
            "imaging": slot.as_ref().map(|slot| slot.recorded_imaging()),
            "deviceSession": identities.iter().find(|identity| identity.cam == cam).map(|identity| identity.session),
        })
    }).collect();
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
    /// 维护模式下没能建立检测的件（找不到配方、设备或发布包有问题）：照常布防放行，运动结束后回结果码 0
    uninspected: Option<Uninspected>,
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
    /// 维护采集进度、试判统计与上次从记录库读取的时刻（录制收尾是异步的，定时刷新）
    collection: (
        Vec<crate::store::CollectionProgress>,
        Vec<crate::store::TrialProgress>,
        Option<Instant>,
    ),
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
}

impl Machine {
    fn new(app: AppHandle, frame_pool: FramePool) -> Self {
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
            done_at: None,
            ack_alarmed: false,
            result: None,
            stats,
            collection: (Vec::new(), Vec::new(), None),
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

    /// 与配方无关的服务就绪：生产生效版本已确认、工件身份可分配。
    fn service_ready(&self) -> Result<(), String> {
        if crate::workspace::activation_uncertain(&self.app) { return Err("生产生效版本尚未确认，请先核实发布状态".into()); }
        host(&self.app).cycle_ids.ready(&self.app)
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
        // 维护模式下配方问题不拦来件：照常布防放行（MM-3），只有在用配方的相机全掉线才撤就绪
        let maintenance = host.settings().maintenance.is_some();
        let none_usable = !maintenance && !req.warnings.is_empty() && req.watch.is_empty();
        if none_usable || (!req.watch.is_empty() && self.camera_down.len() == req.watch.len()) {
            let first = self.camera_warnings.first().or(req.warnings.first()).cloned().unwrap_or_default();
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
        if host.settings().maintenance.is_none() {
            self.collection = (Vec::new(), Vec::new(), None);
        } else if self
            .collection
            .2
            .is_none_or(|at| at.elapsed() > COLLECTION_REFRESH)
        {
            let store = self.app.state::<Store>();
            self.collection = (
                store
                    .collection_progress(self.stats_scope)
                    .unwrap_or_default(),
                store.trial_progress(self.stats_scope).unwrap_or_default(),
                Some(Instant::now()),
            );
        }
        let active = self.current_recipe();
        let snapshot = Snapshot {
            stats_scope: self.stats_scope,
            candidate_debug: crate::offline::candidate(&self.app).map(|c| c.info.clone()),
            measurement_workers: self.frame_pool.health().snapshot(),
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
            alarms: self
                .alarms
                .iter()
                .chain(&self.config_warnings)
                .chain(&self.camera_warnings)
                .cloned()
                .chain(self.recording_alarm.as_ref().map(RecordingAlarm::message))
                .collect(),
            recording_alarm: self.recording_alarm.clone(),
            maintenance: host.settings().maintenance,
            collection: self.collection.0.clone(),
            trial: self.collection.1.clone(),
        };
        host.shared.lock().unwrap().snapshot = Some(snapshot.clone());
        let _ = self.app.emit("cycle://snapshot", snapshot);
    }

    async fn on_input(&mut self, input: Input) {
        self.dirty = true;
        match input {
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
                    drop(shared);
                    crate::workspace::clear_live(&self.app);
                }
            },
        }
    }

    async fn start_part(&mut self, request: Request) {
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
        self.uninspected = None;
        let scope = HistoryScope::of(crate::site::record_source(&self.app));
        if scope != self.stats_scope { self.stats_scope = scope; self.stats = today_stats(&self.app, scope); }
        self.current_cycle_id = None;
        self.result = None;
        crate::workspace::clear_live(&self.app);
        self.alarms.clear();
        for summary in self.noise.end_all(&codes::FRAME_STRAY_SUMMARY) {
            log_event(&self.app, summary.event, summary.msg, summary.fields);
        }
        budget.mark("entry-readiness-workspace");
        let app = self.app.clone();
        let engine = plc(&app);
        let (sn, code) = (request.sn.clone(), request.product_code as u32);
        let host = host(&app);
        let taken = host.cycle_ids.take(&app);
        let context = Fields { cycle_id: taken.as_ref().ok().cloned(), sn: Some(sn.clone()), request_seq: Some(request.request_seq), ..Fields::default() };
        applog::begin_cycle(context.clone());
        log_with(
            &app,
            "info",
            "plcState=10",
            format!("SN={sn} 型号={code}"),
            Fields::stage("partStart").with_data(json!({"productCode": code})),
        );
        let cycle_id = match taken {
            Ok(id) => id,
            Err(reason) => return self.refuse(sn, None, fault::PROCESS_TIMEOUT, reason).await,
        };
        self.current_cycle_id = Some(cycle_id.clone());
        let site = PartSite { source: crate::site::record_source(&app), record: crate::site::record_mode(&app) };
        if let Err(error) = self.s7.bind_cycle_id(&cycle_id, site) {
            return self.refuse(sn, None, fault::PROCESS_TIMEOUT, format!("S7 事务无法绑定工件身份：{error}")).await;
        }
        let maintenance = self.s7.part_maintenance();
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
        // 日志里的计划帧数 ΣN_exp
        let planned = recipe.expected_total().map_or(0, |n| n as usize);
        // 采集版只录图：不编拍照计划、不测量、没有发布包
        let plan = if collect {
            None
        } else {
            let plan =
                match admission::admit(&request, &recipe, crate::plc::recipe_plan(host, &recipe)) {
                    Ok(plan) => plan,
                    Err((code, reason)) => return self.refuse(sn, id, code, reason).await,
                };
            if let Err(reason) = self.s7.validate_plan(&plan) {
                return self.refuse(sn, id, fault::PLAN_MISMATCH, reason).await;
            }
            Some(plan)
        };
        if !collect && !self.frame_pool.health().can_arm() {
            return self
                .refuse(
                    sn,
                    id,
                    fault::PROCESS_TIMEOUT,
                    "测量线程全部超时且尚未返回，拒绝布防".into(),
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
        let burst = {
            // 按 SDK 帧计数定每台相机的基线（D-SB-10），会话或计数来源不可信时拒绝布防（98）
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
            let mut burst = burst::BurstPart::arm(&recipe, baselines, policy.trusted);
            burst.collect = collect;
            Box::new(burst)
        };
        let bundle_id = production.as_ref().map(|prepared| prepared.bundle.id.clone());
        budget.mark("camera-router");
        arm_remaining!("before-recording");
        if let Err(reason) = host.audit.begin_recording(&cycle_id) {
            return self.refuse(sn, id, fault::PROCESS_TIMEOUT, format!("录制追溯屏障建立失败：{reason}")).await;
        }
        let source = crate::site::record_source(&app);
        // 维护采集件是示教与标定的样本：不论录制设置都录全部帧
        let record = if collect {
            crate::settings::RecordMode::All
        } else {
            crate::site::record_mode(&app)
        };
        let mut recording = host.recorders.begin(
            source,
            record,
            &sn,
            recipe.clone(),
            &cycle_id,
            bundle_id.as_deref(),
        );
        if let Some(rec) = recording.as_mut() {
            annotate_recording(rec, host, mode, &recipe, &cams, &identities, &request);
        }
        self.part = Some(Part {
            mode,
            cycle_id,
            bundle_id,
            production,
            candidate: crate::offline::candidate(&app),
            sn: sn.clone(),
            scenario: crate::offline::part_scenario(&app),
            burst,
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
        let r = budget
            .run_serial(
                "plc-arm-confirmed",
                self.s7.arm_outputs(engine, plan.as_ref()),
            )
            .await;
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

    /// 校验没过，不布防，直接回写 ERR。维护模式下不拒收：照常布防，PLC 按未检放行，原因记进本件记录。
    async fn refuse(&mut self, sn: String, recipe_id: Option<String>, code: u16, reason: String) {
        if self.s7.part_maintenance() {
            return self.arm_uninspected(sn, recipe_id, code, reason).await;
        }
        log(&self.app, "err", "校验失败", format!("{reason}，不布防"));
        self.report(sn, recipe_id, Judgement::error(code, reason)).await
    }

    async fn arm_uninspected(
        &mut self,
        sn: String,
        recipe_id: Option<String>,
        code: u16,
        reason: String,
    ) {
        let app = self.app.clone();
        let reason = format!("维护模式：{reason}；照常布防放行，本件未检");
        log(&app, "warn", "维护模式", reason.clone());
        match self.part.as_mut() {
            Some(part) => {
                part.fault.get_or_insert((code, reason));
            }
            None => {
                self.uninspected = Some(Uninspected {
                    sn,
                    recipe_id,
                    judgement: uninspected_judgement(code, reason),
                    armed_at: Instant::now(),
                })
            }
        }
        let engine = plc(&app);
        let armed = match self.s7.arm_outputs(engine, None).await {
            Ok(()) => self
                .s7
                .commit_arm(engine)
                .await
                .map_err(|e| format!("布防后握手日志落盘失败：{e}")),
            Err(e) => Err(format!("维护模式布防失败：{e}")),
        };
        if let Err(reason) = armed {
            self.uninspected = None;
            self.s7.fault(engine, reason.clone()).await;
            return self.enter_fault("arm", reason);
        }
        if let Some(part) = self.part.as_mut() {
            part.armed_at = Instant::now();
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
                    let current = slot.as_ref().is_some_and(|slot| slot.routing_state().is_ok_and(|state| state.0.session == f.session));
                    let burst = &mut part.burst;
                    // 布防后的手动帧丢弃并告警，不判 ERR（它占了这台相机一个帧号，此后的帧组可能错开一格）
                    if let Some(camera) = burst
                        .baseline(f.cam)
                        .filter(|base| current && f.frame_counter > *base)
                        .and_then(|_| part.cams.iter().position(|&c| c == f.cam))
                        .filter(|&camera| burst.first_warning(camera, burst::Warning::Manual))
                    {
                        let message = format!("生产布防后收到相机 {} 的手动触发帧（帧计数 {}），已丢弃；该相机此后的帧组可能错开一格", burst.camera_name(camera), f.frame_counter);
                        log_with(
                            &self.app,
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
        self.on_burst_frame(f);
    }

    /// 一帧（D-SB-10）：按帧号配组；接受的帧录制、留预览并送测（所有帧都测，不裁帧）。上一件的迟到帧只记游离帧；
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
        let burst = &mut part.burst;
        let fields = |camera: &str, seq: Option<u64>| Fields { cycle_id: Some(cycle_id.clone()), camera: Some(camera.to_owned()), seq, ..Fields::default() };
        match arrival {
            burst::Arrival::Accepted { camera, seq } => {
                let camera_id = burst.camera_name(camera).to_owned();
                if let Some(rec) = part.recording.as_mut() { host(&app).recorders.burst_frame(rec, &f, camera, seq); }
                let image = f.single_image().cloned();
                crate::workspace::retain_live_frame(&app, &part.cycle_id, &part.recipe.revision_id, camera, seq, &image);
                burst.retain_image(camera, seq, image.clone());
                trace(&app, "帧到达", format!("cycle={cycle_id} · {camera_id} 第 {seq} 帧 · 帧计数 {}", f.frame_counter),
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

    /// 一帧的测量结果：写进本件；没测成（队满、超时、失败）只记检测不完整，不判 ERR（D-SB-13）。
    fn on_frame_measured(&mut self, m: FrameMeasured) {
        if !matches!(self.phase, Phase::Acquire | Phase::Drain) { return; }
        let Some(part) = self.part.as_mut() else { return };
        let (ms, queue_ms, engine_ms) = (m.ms, m.queue_ms, m.engine_ms);
        let Some(metrics) = part.apply_burst(m) else { return };
        self.dirty = true;
        let camera = part.burst.camera_name(metrics.camera).to_owned();
        let seq = metrics.seq;
        let fields = Fields { cycle_id: Some(part.cycle_id.clone()), camera: Some(camera.clone()), seq: Some(seq), ..Fields::default() }.with_stage("measured")
            .with_data(json!({"ms": ms, "queueMs": queue_ms, "engineMs": engine_ms, "state": metrics.state, "error": metrics.error}));
        let why = metrics.error.clone().unwrap_or_default();
        match metrics.state {
            FrameState::Measured => trace(
                &self.app,
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
                &self.app,
                "走出走廊",
                format!("{camera} 第 {seq} 帧胶条走出走廊，记检测不完整"),
                fields,
            ),
            FrameState::NotInspected => trace(
                &self.app,
                "本段不检测",
                format!("{camera} 第 {seq} 帧所在段不由这台相机检测，不测"),
                fields,
            ),
            FrameState::Unmeasured => log_with(
                &self.app,
                "warn",
                "帧没测",
                format!("{camera} 第 {seq} 帧没测（{why}），记检测不完整"),
                fields,
            ),
            FrameState::Error | FrameState::Missing => log_with(
                &self.app,
                "warn",
                "测量出错",
                format!("{camera} 第 {seq} 帧测量失败（{why}），记检测不完整"),
                fields,
            ),
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
        self.s7
            .set_maintenance(host(&app).settings().maintenance.is_some());
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
        let scope = display_history_scope(crate::site::record_source(&app), self.part.as_ref().map(|p| p.source));
        if scope != self.stats_scope {
            self.stats_scope = scope;
            self.stats = today_stats(&app, scope);
            self.dirty = true;
        }
        self.sync_recording_alarm();
        let cameras = match self.phase {
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
            let resetting = matches!(
                self.s7.phase(),
                SessionPhase::ResetRequired | SessionPhase::Fault
            ) || self.s7_reset_requested;
            let intake_ready =
                self.disk_space_ready() && (!resetting || self.check_idle_devices().is_ok());
            let devices = if resetting {
                self.service_ready()
            } else if matches!(self.phase, Phase::Report | Phase::Release) {
                self.check_idle_devices()
            } else { cameras.clone() };
            let reset = std::mem::take(&mut self.s7_reset_requested);
            let previous = self.s7.phase();
            let previous_reset_blocked = self.s7.view().reset_blocked_reason;
            let had_ack = self.s7.acknowledged();
            let event = self.s7.poll_with_devices(plc(&app), reset, &devices, audit_ready, intake_ready).await;
            // 布防持有配置门直到下面 store_busy 登记在途，相机与配置操作要么在布防前完成，要么看到在途后拒绝
            let mut _config = None;
            if let Some(delivery) = s7_ack_delivery(had_ack, &self.s7) {
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
                    _config = Some(host(&app).config_gate.lock().await);
                    self.start_part(request).await;
                }
                SessionEvent::End(ended_at) => {
                    if let Some(part) = self.part.as_mut() {
                        part.end_at = Some(ended_at);
                    }
                    self.set_phase(Phase::Drain);
                    log_with(
                        &app,
                        "info",
                        "plcState=20",
                        "PLC 报运动结束，等待剩余帧",
                        Fields::stage("partEnd"),
                    );
                }
                SessionEvent::Released => {
                    self.close_routing();
                    self.alarms.retain(|m| !m.starts_with("PLC 未确认"));
                    self.set_phase(Phase::Idle);
                    log_with(
                        &app,
                        "info",
                        "S7 事务结束",
                        "PLC 已锁存结果，视觉已回就绪",
                        Fields::stage("released"),
                    );
                    applog::end_cycle();
                }
                SessionEvent::Fault(reason) => self.enter_fault("s7", reason),
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
                        &self.app,
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
                Phase::Acquire if bare.armed_at.elapsed() > timeouts.motion() => {
                    log(
                        &self.app,
                        "err",
                        "运动超时",
                        format!("T_motion {} ms", timeouts.motion_ms),
                    );
                    self.set_phase(Phase::Drain);
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
                let Some(part) = self.part.as_mut() else { return };
                if part.armed_at.elapsed() > timeouts.motion() {
                    part.fault = Some((
                        fault::MOTION_TIMEOUT,
                        format!(
                            "布防后 {} s 内未收到运动结束（plcState=20）",
                            timeouts.motion_ms / 1000
                        ),
                    ));
                    part.end_at = Some(Instant::now());
                    log(&self.app, "err", "运动超时", format!("T_motion {} ms", timeouts.motion_ms));
                    self.set_phase(Phase::Drain);
                }
            }
            Phase::Drain => {
                if let Some(part) = self.part.as_mut() {
                    for (camera, seq, segment, image) in part.start_post_measure(Instant::now()) {
                        let camera_id = part.burst.camera_name(camera).to_owned();
                        trace(
                            &self.app,
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
                        self.frame_pool
                            .submit(frame_job(part, camera, camera_id, seq, segment, image));
                    }
                }
                if self.part.as_ref().is_some_and(Part::complete) {
                    self.judge_part().await;
                }
            }
            Phase::Report => {
                if !self.ack_alarmed && self.done_at.is_some_and(|t| t.elapsed() > timeouts.ack()) {
                    self.ack_alarmed = true;
                    self.alarm(format!(
                        "PLC 未确认结果（超过 T_ack {} ms），保持 pcState=30",
                        timeouts.ack_ms
                    ));
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
        let judgement = if part.mode == crate::store::PartMode::MaintenanceCollect {
            let groups = part.burst.group_count();
            let reason = match &part.fault {
                Some((_, why)) => format!("维护采集：只录图、不判定（{groups} 个帧组）；{why}"),
                None => format!("维护采集：只录图、不判定（{groups} 个帧组）"),
            };
            uninspected_judgement(0, reason)
        } else {
            let decided = part.burst_judgement();
            let burst = &mut part.burst;
            warnings = burst_warnings(&decided, |camera| burst.camera_name(camera).to_owned());
            let summary = burst::summary(&decided);
            burst.judgement = Some(decided);
            summary
        };
        let (sn, id, cycle_id) = (part.sn.clone(), part.recipe.id.clone(), part.cycle_id.clone());
        for (ev, message) in warnings {
            log_with(&self.app, "warn", ev, message, Fields { cycle_id: Some(cycle_id.clone()), ..Fields::default() });
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
        let pending = PlcDelivery {
            state: PlcDeliveryState::Pending,
            updated_at: now_ms(),
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
        let durable = match record {
            Ok(part) => host(&app).audit.insert_durable(part, host(&app).settings().timeouts.proc()).await.and_then(|_| self.record_without_part(&sn, &judgement)),
            Err(error) => Err(error),
        };
        let mut failure_saved = false;
        let r = match durable {
            Err(error) => Err(format!("审计记录未可靠保存，未提交 PLC 完成信号：{error}")),
            Ok(()) => {
                let (code, fault_code) = if maintenance {
                    (0, 0)
                } else {
                    (judgement.plc_code, judgement.fault_code)
                };
                let sent = self.s7.report(plc(&app), code, fault_code).await;
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
        if mode == crate::store::PartMode::Production {
            self.count(judgement.verdict);
        }
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
            "done 已提交"
        } else {
            "交付异常，需人工确认"
        };
        log_with(
            &app,
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
            candidate_debug: self
                .part
                .as_ref()
                .and_then(|p| p.candidate.as_ref())
                .map(|c| c.info.clone())
                .or_else(|| crate::offline::candidate(&self.app).map(|c| c.info.clone())),
            cycle_id: self.current_cycle_id.clone(),
            sn,
            recipe_id,
            ts: now_ms(),
            drain_ms,
            judgement,
            burst,
            mode,
        });
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

    fn close_routing(&mut self) {
        if let Some(part) = self.part.as_mut() {
            part.close_routing(&mut self.ledgers);
        }
    }

    fn check_part_cams(&self, part: &Part) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        rig.check_ready_at(&part.cams)?;
        let identities = part.cams.iter().map(|&cam| {
            rig.slot(cam as usize).ok_or_else(|| "相机组已变化".to_string())?.routing_state().map(|state| state.0)
        }).collect::<Result<Vec<_>, _>>()?;
        part.burst.check_sessions(&identities)
    }

    fn check_part_routing(&self, part: &Part) -> Result<(), (u16, String)> {
        self.check_part_cams(part).map_err(|reason| (fault::DEVICE_LOST, reason))?;
        let rig = &host(&self.app).camera;
        for &cam in &part.cams {
            let slot = rig.slot(cam as usize).ok_or_else(|| (fault::DEVICE_LOST, "相机组已变化".into()))?;
            let (_, observed) = slot.routing_state().map_err(|reason| (fault::DEVICE_LOST, reason))?;
            if let Some(ledger) = observed {
                part.burst.check_observed(cam, &ledger)?;
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

    fn recorded_part(
        &self,
        sn: &str,
        recipe_id: Option<&str>,
        judgement: &Judgement,
        drain_ms: Option<u64>,
        delivery: PlcDelivery,
        mode: crate::store::PartMode,
    ) -> Result<RecordedPart, String> {
        let Some(cycle_id) = self.current_cycle_id.clone() else {
            return Err("工件身份尚未持久分配，不能关联历史与原图".into());
        };
        let part = self.part.as_ref().filter(|p| p.sn == sn);
        let recipe = part.map(|p| p.recipe.clone()).or_else(|| crate::offline::candidate(&self.app).filter(|c| recipe_id == Some(c.recipe.id.as_str())).map(|c| c.recipe.clone())).or_else(|| recipe_id.and_then(|id| host(&self.app).recipe(id)));
        let burst = self.burst_outcome(sn, recipe.as_deref(), judgement);
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
        let source = part.map_or_else(|| crate::site::record_source(&self.app), |p| p.source);
        let sample = (matches!(
            source,
            RecordSource::OfflineSample | RecordSource::OfflineCandidate
        ))
        .then(|| crate::offline::claim_sample(&self.app, sn, &cycle_id))
        .flatten();
        Ok(RecordedPart {
            ts: now_ms(),
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
            sample,
            burst,
            mode,
        })
    }

    /// 帧与区域记录：在途件按它的帧组与判定给出；没有在途件（拒绝布防）时按配方给出没有帧的记录；没有配方为 None。
    fn burst_outcome(&self, sn: &str, recipe: Option<&Recipe>, judgement: &Judgement) -> Option<BurstRecord> {
        match self.part.as_ref().filter(|p| p.sn == sn) {
            Some(part) => Some(part.burst_record(judgement)),
            None => recipe.map(|recipe| burst::empty_record(recipe, judgement)),
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


    /// 没建立检测的件（拒收或维护模式未检）按会话锁存的模式记。
    fn part_mode(&self, sn: &str) -> crate::store::PartMode {
        self.part
            .as_ref()
            .filter(|part| part.sn == sn)
            .map(|part| part.mode)
            .unwrap_or(if self.s7.part_maintenance() {
                crate::store::PartMode::MaintenanceCollect
            } else {
                crate::store::PartMode::Production
            })
    }

    fn record(
        &self,
        sn: &str,
        recipe_id: Option<&str>,
        judgement: &Judgement,
        drain_ms: Option<u64>,
        delivery: PlcDelivery,
    ) -> Result<(), String> {
        host(&self.app).audit.insert(self.recorded_part(
            sn,
            recipe_id,
            judgement,
            drain_ms,
            delivery,
            self.part_mode(sn),
        )?)?;
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
        self.close_routing();
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
            let mode = self.part_mode(&sn);
            self.result = Some(ResultView {
                candidate_debug: self
                    .part
                    .as_ref()
                    .and_then(|p| p.candidate.as_ref())
                    .map(|c| c.info.clone())
                    .or_else(|| crate::offline::candidate(&self.app).map(|c| c.info.clone())),
                cycle_id: self.current_cycle_id.clone(),
                sn,
                recipe_id: self.part.as_ref().map(|p| p.recipe.id.clone()),
                ts: now_ms(),
                drain_ms: None,
                judgement,
                burst,
                mode,
            });
        }
        if production_pending && fault_fails_delivery(self.phase) && self.result.is_some() {
            self.fail_delivery(&reason);
        }
        let fields = Fields {
            source: Some(source.into()),
            ..Fields::stage("fault")
        }
        .with_data(json!({"reason": reason, "s7Phase": format!("{:?}", self.s7.phase())}));
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
        out.push((
            "疑似提前终止",
            format!(
                "整件 {} 个帧组，少于 ΣN_exp − Σtol：疑似 PLC 提前报运动结束（只告警，不改结论）",
                judgement.group_count
            ),
        ));
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
pub fn cycle_recipes(cycle: State<'_, CycleHost>) -> Vec<RecipeSummary> {
    cycle.recipes.list().iter().map(|r| RecipeSummary::listed(&cycle.recipes, r)).collect()
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
        &app,
        &codes::OP_MAINTENANCE,
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
        .map_err(|_| "正在布防或处理其他配置操作，请稍后重试")?;
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

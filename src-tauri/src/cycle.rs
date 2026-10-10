use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ly_plc::{now_ms, EdgeEvent, LinkState, PlcEngine, ProtocolKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender};

use crate::camera::{Acquisition, CameraRig, CameraSource, FRAME_QUEUE};
use crate::frame::Frame;
use crate::history;
use crate::handshake::Request;
use crate::inspection::{read_tag_u32, tag, tag_is_on, write_tag};
use crate::judge::{self, fault, Judgement, PointState, Verdict};
use crate::measure::{self, Job, Measured, WorkerHealth};
use crate::plc::PlcHost;
use crate::plc_session::{PlcSession, SessionEvent, SessionPhase, SessionView};
use crate::recipe::{Recipe, RecipeStore, TriggerMode};
use crate::recorder::{Recorder, Recording};
use crate::settings::{CycleSettings, ProductSource};
use crate::shot_router::{ArmCam, Floor, FrameMeta, Ledgers, Plan, Policy, Route, ShotRouter};
use crate::sim::{Scenario, SimCtl};
use crate::audit::{Audit, RecordedPart};
use crate::store::{PartShot, PlcDelivery, PlcDeliveryState, Store, VerdictCounts};

pub enum Input {
    Edge(EdgeEvent),
    Measured(Measured),
    Reset,
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
    Report,
    Release,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FrameStatus {
    Waiting,
    Measuring,
    Done,
    LocateFailed,
    /// 测量本身没做成（lyFlow 运行失败、缺示教资料等）
    Error,
    Missing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameView {
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

impl FrameView {
    fn waiting() -> Self {
        Self {
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
    pub cycle_id: String,
    pub bundle_hash: Option<String>,
    pub sn: u32,
    pub recipe_id: String,
    /// 本件配方快照的哈希：配方中途改了，界面仍按这一版画
    pub recipe_hash: String,
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
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultView {
    pub cycle_id: Option<String>,
    pub sn: u32,
    pub recipe_id: Option<String>,
    pub ts: i64,
    pub drain_ms: Option<u64>,
    #[serde(flatten)]
    pub judgement: Judgement,
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
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts: i64,
    pub level: &'static str,
    pub ev: String,
    pub msg: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeSummary {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub hash: String,
    pub product_code: u16,
    pub shot_count: usize,
    pub trigger_mode: TriggerMode,
    pub cameras: Vec<String>,
    pub length: f32,
}

impl From<&Recipe> for RecipeSummary {
    fn from(r: &Recipe) -> Self {
        Self {
            id: r.id.clone(),
            name: r.name.clone(),
            version: r.version,
            hash: r.hash.clone(),
            product_code: r.product_code,
            shot_count: r.shot_count(),
            trigger_mode: r.trigger_mode,
            cameras: r.cameras(),
            length: r.segments.iter().map(|g| g.length(r.spacing)).sum(),
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
    pub plc_gate: tokio::sync::Mutex<()>,
    pub camera: CameraRig,
    pub sim: SimCtl,
    pub recipes: RecipeStore,
    pub recorder: Recorder,
    pub cycle_ids: crate::cycle_ids::CycleIds,
    pub audit: Audit,
    settings_path: PathBuf,
    shared: Mutex<Shared>,
    rx: Mutex<Option<(UnboundedReceiver<Input>, Receiver<Frame>)>>,
    /// 节拍不在空闲或故障（开工那一刻就置上，比发布的快照早）
    busy: AtomicBool,
    settings_note: Option<String>,
}

impl CycleHost {
    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let settings_path = app.path().app_config_dir().map_err(|e| e.to_string())?.join("cycle.json");
        let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let (tx, rx) = unbounded_channel();
        let (frame_tx, frame_rx) = channel(FRAME_QUEUE);
        let (settings, settings_note) = CycleSettings::load(&settings_path);
        let audit = Audit::new(app);
        let record_audit = audit.clone();
        Ok(Self {
            camera: CameraRig::new(app, frame_tx)?,
            tx,
            plc_gate: tokio::sync::Mutex::new(()),
            sim: SimCtl::default(),
            recipes: RecipeStore::open(data.join("recipes"))?,
            recorder: Recorder::new(data.join("records"), Some(Arc::new(move |outcome| record_audit.recording(outcome)))),
            cycle_ids: crate::cycle_ids::CycleIds::default(),
            audit,
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
            settings_note,
        })
    }

    pub fn start(app: &AppHandle) {
        let host = app.state::<CycleHost>();
        let Some((mut rx, mut frames)) = host.rx.lock().unwrap().take() else { return };
        let (measure_tx, workers) = measure::spawn_worker(app.clone(), host.tx.clone());
        let mut machine = Machine::new(app.clone(), measure_tx, workers);
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
        if store.interrupted_recordings > 0 {
            log(app, "err", "录制恢复", format!("上次服务退出时有 {} 件原图尚未确认完整，已标记录制失败；原检测结论保留", store.interrupted_recordings));
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
        let gate = self.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试")?;
        self.save_settings_locked(settings, &gate)
    }

    pub(crate) fn save_settings_locked(&self, settings: CycleSettings, _gate: &tokio::sync::MutexGuard<'_, ()>) -> Result<(), String> {
        if self.busy() { return Err("在途事务结束或故障复位后才能修改检测设置".into()); }
        settings.validate()?;
        settings.save(&self.settings_path)?;
        self.shared.lock().unwrap().settings = settings;
        let _ = self.tx.send(Input::Refresh);
        Ok(())
    }
}

/// 删除超过保留天数的检测记录。
pub fn purge_history(app: &AppHandle) {
    let days = host(app).settings().history_days.max(1) as i64;
    match app.state::<Store>().purge_before(now_ms() - days * 86_400_000) {
        Ok(n) if n > 0 => log(app, "info", "记录清理", format!("删除 {days} 天前的 {n} 条检测记录")),
        Err(e) => log(app, "err", "记录清理", e),
        _ => {}
    }
}

fn plc(app: &AppHandle) -> &PlcEngine {
    app.state::<PlcHost>().inner().engine()
}

fn host(app: &AppHandle) -> &CycleHost {
    app.state::<CycleHost>().inner()
}

pub fn log(app: &AppHandle, level: &'static str, ev: impl Into<String>, msg: impl Into<String>) {
    let line = LogLine { ts: now_ms(), level, ev: ev.into(), msg: msg.into() };
    {
        let mut s = host(app).shared.lock().unwrap();
        s.logs.push_back(line.clone());
        if s.logs.len() > 300 {
            s.logs.pop_front();
        }
    }
    let _ = app.emit("cycle://log", line);
}

async fn put(app: &AppHandle, t: &str, v: Value) -> Result<(), String> {
    write_tag(plc(app), t, v).await
}

struct Part {
    run_id: u64,
    cycle_id: String,
    bundle_hash: Option<String>,
    production: Option<Arc<crate::production::Prepared>>,
    sn: u32,
    recipe: Arc<Recipe>,
    scenario: Scenario,
    frames: Vec<FrameView>,
    measuring_since: Vec<Option<Instant>>,
    table: Vec<PointState>,
    received: usize,
    extra: usize,
    queue: usize,
    router: ShotRouter,
    issued: Option<Vec<(String, u64)>>,
    issued_verified: bool,
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
    recording: Option<Recording>,
}

impl Part {
    fn n(&self) -> usize {
        self.recipe.shot_count()
    }

    fn triggers(&self) -> u64 {
        self.router.triggers()
    }

    fn camera_id(&self, cam: u8) -> String {
        self.cams.iter().position(|&c| c == cam).and_then(|i| self.camera_ids.get(i).cloned()).unwrap_or_else(|| format!("#{}", cam + 1))
    }

    fn complete(&self) -> bool {
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
        self.cycle_id == m.cycle_id && self.sn == m.sn && self.bundle_hash == m.bundle_hash
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
        if m.error.is_none() && m.located {
            let valid = m.score.is_finite() && (0.0..=1.0).contains(&m.score)
                && (m.px.is_empty() || (m.px.len() == owned.len() && m.px.iter().flatten().all(|v| v.is_finite())))
                && m.idx.len() == owned.len() && m.d.len() == owned.len() && m.w.len() == owned.len()
                && m.st.len() == owned.len() && m.idx.iter().zip(&owned).all(|(&actual, &expected)| actual as usize == expected)
                && m.st.iter().enumerate().all(|(i, &state)| state <= measure::ST_INVALID
                    && (state != measure::ST_OK || (m.d[i].is_finite() && m.w[i].is_finite())));
            if !valid { m.error = Some("测量输出与拍照点测点表不一致或含无效数值".into()); }
        }
        for (i, &j) in owned.iter().enumerate() {
            self.table[j] = if m.error.is_some() || !m.located { PointState::Invalid } else { m.point_state(i) };
        }
        self.queue = self.queue.saturating_sub(1);
        self.measuring_since[m.k] = None;
        let frame = &mut self.frames[m.k];
        frame.status = if m.error.is_some() { FrameStatus::Error }
            else if m.located { FrameStatus::Done } else { FrameStatus::LocateFailed };
        frame.error = m.error.clone();
        frame.score = m.error.is_none().then_some(m.score);
        frame.points = owned.len();
        frame.gap_points = if m.error.is_none() && m.located { m.st.iter().filter(|&&s| s == measure::ST_GAP).count() } else { 0 };
        frame.ms = Some(m.ms);
        if let Some(error) = &m.error {
            self.fault.get_or_insert((fault::PROCESS_TIMEOUT, format!("拍照点 {} 测量出错：{error}", m.shot_id)));
        }
        if m.error.is_some() || !m.located {
            m.idx.clear(); m.d.clear(); m.w.clear(); m.st.clear(); m.px.clear();
        }
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
        if let Some(frame) = self.frames.iter().find(|f| f.status == FrameStatus::LocateFailed) {
            return Judgement::error(fault::LOCATE_FAILED, format!("拍照点 {} 定位失败", frame.shot_id));
        }
        if !self.complete() {
            return Judgement::error(fault::PROCESS_TIMEOUT, "计划内拍照点尚未完成");
        }
        judge::judge(&self.recipe, &self.table)
    }

    fn view(&self) -> PartView {
        PartView {
            cycle_id: self.cycle_id.clone(),
            bundle_hash: self.bundle_hash.clone(),
            sn: self.sn,
            recipe_id: self.recipe.id.clone(),
            recipe_hash: self.recipe.hash.clone(),
            n: self.n(),
            received: self.received,
            triggers: self.triggers(),
            queue: self.queue,
            filled: self.table.iter().filter(|p| **p != PointState::Pending).count(),
            total: self.table.len(),
            frames: self.frames.clone(),
            measured_frames: self.frames.iter().filter(|f| matches!(f.status, FrameStatus::Done | FrameStatus::LocateFailed | FrameStatus::Error)).count(),
        }
    }
}

fn missing_recipe(id: &str) -> String {
    format!("选中的配方 {id} 不存在或没能加载（见配方页）")
}

/// PLC 用模拟器时没有实物：海康相机配模拟测量可以用来试触发、调曝光。
pub fn real_parts(app: &AppHandle) -> bool {
    plc(app).config().connection.protocol != ProtocolKind::Simulator
}

/// 配方用的相机此刻在相机组里的序号。相机不在相机组里、不是触发采集、
/// 实物检测（real_parts）时海康相机却没开图像测量时返回原因。
pub fn usable_cams(app: &AppHandle, recipe: &Recipe, real_parts: bool) -> Result<Vec<u8>, String> {
    let host = host(app);
    let cams = host.camera.resolve(&recipe.cameras())?;
    crate::vision::recipe_source(app, recipe)?;
    let image = host.settings().vision;
    for &c in &cams {
        let cfg = host.camera.slot(c as usize).ok_or("相机组改过了")?.config();
        if cfg.acquisition == Acquisition::FreeRun {
            return Err(format!("{}（{}）是连续采集，飞拍配方要用触发采集", cfg.name, cfg.id));
        }
        for shot in recipe.shots.iter().filter(|s| s.camera == cfg.id) {
            if shot.view > cfg.view_count {
                return Err(format!("拍照点 {} 选择视角 {}，设备 {} 只配置了 {} 个视角", shot.id, shot.view, cfg.id, cfg.view_count));
            }
        }
        // 模拟测量不看图，真实工件按模拟结果回写就是把实物判了 OK
        if real_parts && !image && cfg.source == CameraSource::Mvs {
            return Err(format!("{}（{}）是海康相机，飞拍的图像测量却没打开（系统设置）", cfg.name, cfg.id));
        }
    }
    Ok(cams)
}

pub fn runnable_cams(app: &AppHandle, recipe: &Recipe, real_parts: bool) -> Result<Vec<u8>, String> {
    recipe.ready()?;
    crate::production::ready(app, recipe)?;
    usable_cams(app, recipe, real_parts)
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

struct Machine {
    app: AppHandle,
    measure_tx: Sender<Job>,
    workers: Arc<WorkerHealth>,
    ledgers: Ledgers,
    phase: Phase,
    since: i64,
    part: Option<Part>,
    active_recipe: Option<Arc<Recipe>>,
    fault: Option<String>,
    fault_needs_reset: bool,
    done_at: Option<Instant>,
    ack_alarmed: bool,
    result: Option<ResultView>,
    stats: Stats,
    stats_day: i64,
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
    /// 通讯可能在两个 tick 之间完成断开/重连，仍需向新连接同步输出信号。
    plc_connected_since: Option<i64>,
    s7: PlcSession,
    s7_reset_requested: bool,
    run_id: u64,
    current_cycle_id: Option<String>,
}

impl Machine {
    fn new(app: AppHandle, measure_tx: Sender<Job>, workers: Arc<WorkerHealth>) -> Self {
        let stats = app.state::<Store>().counts_since(history::local_midnight_ms()).map(Stats::from).unwrap_or_default();
        let s7 = PlcSession::open(app.path().app_data_dir().expect("app data directory").join("plc-handshake.json"));
        let recovery = s7.recover_acknowledgements();
        for error in recovery.errors { log(&app, "err", "PLC 确认恢复", error); }
        let mut recovered = 0;
        let mut failed = 0;
        for receipt in recovery.receipts {
            match app.state::<Store>().recover_acknowledgement(&receipt.cycle_id, receipt.sn, receipt.request_seq, receipt.ts) {
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
        host(&app).busy.store(s7.pending(), Ordering::SeqCst);
        Self {
            app,
            measure_tx,
            workers,
            ledgers: Ledgers::default(),
            phase: Phase::Fault,
            since: now_ms(),
            part: None,
            active_recipe: None,
            fault: Some("PLC 未连接".into()),
            fault_needs_reset: false,
            done_at: None,
            ack_alarmed: false,
            result: None,
            stats,
            stats_day: history::local_day(),
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
            plc_connected_since: None,
            s7,
            s7_reset_requested: false,
            run_id: 0,
            current_cycle_id: None,
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        host(&self.app).busy.store(!matches!(phase, Phase::Idle | Phase::Fault) || (self.is_s7() && self.s7.pending()), Ordering::SeqCst);
        self.since = now_ms();
        self.dirty = true;
    }

    fn is_s7(&self) -> bool { plc(&self.app).config().connection.protocol == ProtocolKind::S7 }

    fn alarm(&mut self, msg: String) {
        if !self.alarms.contains(&msg) {
            log(&self.app, "err", "报警", msg.clone());
            self.alarms.push(msg);
            self.dirty = true;
        }
    }

    /// 当前配方（人工选择或上一件 PLC 下发的）。
    fn current_recipe(&self) -> Option<Arc<Recipe>> {
        let host = host(&self.app);
        let settings = host.settings();
        match settings.product_source {
            ProductSource::Manual => settings.manual_recipe_id.as_deref().and_then(|id| host.recipe(id)),
            ProductSource::Plc => self.active_recipe.clone(),
        }
    }

    fn compute_required(&self) -> Required {
        let host = host(&self.app);
        let rig_gen = host.camera.generation();
        let settings = host.settings();
        let real = real_parts(&self.app);
        match settings.product_source {
            ProductSource::Manual => {
                let cams = match (self.current_recipe(), settings.manual_recipe_id.as_deref()) {
                    (Some(r), _) => runnable_cams(&self.app, &r, real),
                    (None, Some(id)) => Err(missing_recipe(id)),
                    (None, None) => Ok(Vec::new()),
                };
                Required { rig_gen, cams, watch: Vec::new(), warnings: Vec::new() }
            }
            ProductSource::Plc => {
                let (mut watch, mut warnings) = (Vec::new(), Vec::new());
                for r in host.recipes.list() {
                    match runnable_cams(&self.app, &r, real) {
                        Ok(c) => watch.push((r.id.clone(), c)),
                        Err(e) => warnings.push(format!("配方 {} 开不了工：{e}", r.id)),
                    }
                }
                Required { rig_gen, cams: Ok(Vec::new()), watch, warnings }
            }
        }
    }

    /// 空闲、故障时的相机检查：没被配方用到的备用相机不拦着开工。
    fn check_idle_cams(&mut self) -> Result<(), String> {
        host(&self.app).cycle_ids.ready(&self.app)?;
        if !self.workers.can_arm() {
            return Err("测量工作线程全部超时且尚未返回，拒绝布防；需排查图像引擎或重启检测服务".into());
        }
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
        let snapshot = Snapshot {
            measurement_workers: self.workers.snapshot(),
            phase: self.phase,
            plc_locked: host.busy(),
            plc_handshake: self.is_s7().then(|| self.s7.view()),
            since: self.since,
            fault: self.fault.clone(),
            product_source: host.settings().product_source,
            active_recipe_id: active.as_ref().map(|r| r.id.clone()),
            trigger_mode: active.as_ref().map(|r| r.trigger_mode),
            part: self.part.as_ref().map(Part::view),
            result: self.result.clone(),
            stats: self.stats.clone(),
            stray_frames: self.stray,
            alarms: self.alarms.iter().chain(&self.config_warnings).chain(&self.camera_warnings).cloned().collect(),
        };
        host.shared.lock().unwrap().snapshot = Some(snapshot.clone());
        let _ = self.app.emit("cycle://snapshot", snapshot);
    }

    async fn on_input(&mut self, input: Input) {
        self.dirty = true;
        match input {
            Input::Edge(e) if e.rising && !self.is_s7() => self.on_edge(e).await,
            Input::Edge(_) => {}
            Input::Measured(m) => self.on_measured(m),
            Input::Reset if self.is_s7() => self.s7_reset_requested = true,
            Input::Reset => self.reset().await,
            Input::Refresh => self.required = None,
        }
    }

    async fn on_edge(&mut self, e: EdgeEvent) {
        let has = |t: &str| e.tags.iter().any(|x| x == t);
        if has(tag::PART_START) {
            if self.phase == Phase::Idle {
                self.start_part(None).await;
            } else {
                log(&self.app, "warn", "partStart↑", format!("当前状态 {:?}，忽略", self.phase));
            }
        } else if has(tag::PART_END) {
            if self.phase == Phase::Acquire {
                if let Some(p) = self.part.as_mut() {
                    p.end_at = Some(Instant::now());
                }
                self.set_phase(Phase::Drain);
                log(&self.app, "info", "partEnd↑", "运动结束，等待剩余帧");
            }
        } else if has(tag::RESULT_ACK) {
            if self.phase == Phase::Report {
                self.delivery(PlcDeliveryState::Acknowledged, Some("PLC 已确认结果（开发协议）".into()));
                let a = self.app.clone();
                let r = async {
                    put(&a, tag::DONE, json!(false)).await?;
                    put(&a, tag::BUSY, json!(false)).await
                }
                .await;
                if let Err(e) = r {
                    return self.enter_fault(format!("释放握手写入失败：{e}"));
                }
                self.alarms.retain(|m| !m.starts_with("PLC 未确认"));
                self.set_phase(Phase::Release);
                log(&self.app, "info", "resultAck↑", "PLC 已确认 · done↓ busy↓");
            }
        } else if has(tag::FAULT_RESET) {
            self.reset().await;
        }
    }

    async fn start_part(&mut self, request: Option<Request>) {
        if crate::workspace::apply_pending(&self.app) { self.required = None; }
        self.set_phase(Phase::Validate);
        self.part = None;
        self.current_cycle_id = None;
        self.result = None;
        crate::workspace::clear_live(&self.app);
        self.alarms.clear();
        host(&self.app).shared.lock().unwrap().measured.clear();
        let t0 = Instant::now();
        let app = self.app.clone();
        let engine = plc(&app);
        let sn = request.as_ref().map_or_else(|| read_tag_u32(engine, tag::PART_SN).unwrap_or(0), |r| r.sn);
        let code = request.as_ref().map_or_else(|| read_tag_u32(engine, tag::PRODUCT_CODE).unwrap_or(0), |r| r.product_code as u32);
        let count = request.as_ref().map_or_else(|| read_tag_u32(engine, tag::SHOT_COUNT).unwrap_or(0) as usize, |r| r.shot_count as usize);
        log(&app, "info", "partStart↑", format!("SN={sn} 产品代码={code} N={count}"));

        let host = host(&app);
        let settings = host.settings();
        let cycle_id = match host.cycle_ids.take(&app) {
            Ok(id) => id,
            Err(reason) => return self.refuse(sn, None, fault::PROCESS_TIMEOUT, reason).await,
        };
        self.current_cycle_id = Some(cycle_id.clone());
        if request.is_some() {
            if let Err(error) = self.s7.bind_cycle_id(&cycle_id) {
                return self.refuse(sn, None, fault::PROCESS_TIMEOUT, format!("S7 事务无法绑定工件身份：{error}")).await;
            }
        }
        if !self.workers.can_arm() {
            return self.refuse(sn, None, fault::PROCESS_TIMEOUT, "测量工作线程全部超时且尚未返回，拒绝布防".into()).await;
        }
        let recipe = match settings.product_source {
            ProductSource::Plc => host.recipes.list().into_iter().find(|r| r.product_code as u32 == code),
            ProductSource::Manual => settings.manual_recipe_id.as_deref().and_then(|id| host.recipe(id)),
        };
        let Some(recipe) = recipe else {
            let reason = match (settings.product_source, settings.manual_recipe_id.as_deref()) {
                (ProductSource::Plc, _) => format!("产品代码 {code} 没有对应的配方"),
                (ProductSource::Manual, Some(id)) => missing_recipe(id),
                (ProductSource::Manual, None) => "未选择配方".to_string(),
            };
            return self.refuse(sn, None, fault::NO_RECIPE, reason).await;
        };
        self.active_recipe = Some(recipe.clone());
        // 从这一刻起这个配方算在检测中（不能删、不能改编号）
        host.shared.lock().unwrap().part_recipe = Some(recipe.clone());
        let id = Some(recipe.id.clone());
        let n = recipe.shot_count();
        let plan = if request.is_some() {
            match crate::plc::recipe_plan(host, &recipe).and_then(|plan| self.s7.validate_plan(&plan).map(|_| plan)) {
                Ok(plan) if recipe.product_code as u32 == code => Some(plan),
                Ok(_) => return self.refuse(sn, id, fault::NO_RECIPE, "PLC 产品代码与所选配方不一致".into()).await,
                Err(reason) => return self.refuse(sn, id, fault::PLAN_MISMATCH, reason).await,
            }
        } else { None };
        if count != n {
            return self.refuse(sn, id, fault::SHOT_COUNT_MISMATCH, format!("PLC 下发拍照点数 {count}，配方 {} 为 {n}", recipe.id)).await;
        }
        let production = match crate::production::ready(&app, &recipe) {
            Ok(prepared) => prepared,
            Err(reason) => return self.refuse(sn, id, fault::NO_RECIPE, reason).await,
        };
        if let Some(prepared) = production.clone() {
            let checked = tokio::time::timeout(settings.timeouts.arm(), tauri::async_runtime::spawn_blocking(move || prepared.verify())).await;
            match checked {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(reason))) => return self.refuse(sn, id, fault::NO_RECIPE, format!("发布包核验失败：{reason}")).await,
                _ => return self.refuse(sn, id, fault::PROCESS_TIMEOUT, "布防前发布资源核验超时或异常".into()).await,
            }
        }
        let rig_gen = host.camera.generation();
        let cams = match runnable_cams(&app, &recipe, real_parts(&app)) {
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
        let policy = if self.is_s7() { Policy::production(None) } else { Policy::development(None) };
        let router = match ShotRouter::arm(&Plan::from_shots(&recipe.shots), &self.ledgers, &policy, &identities) {
            Ok(router) => router,
            Err(error) => return self.refuse(sn, id, error.fault(), error.to_string()).await,
        };
        for camera in &identities {
            self.ledgers.set_floor(&Floor { cam: camera.cam, camera: camera.camera.clone(), session: camera.session,
                baseline: router.baseline(camera.cam).unwrap_or(0) });
        }
        let issued = plan.as_ref().map(|p| p.camera_slots.iter().zip(p.camera_shots).filter(|(id, _)| !id.is_empty())
            .map(|(id, count)| (id.clone(), count as u64)).collect());
        let bundle_hash = production.as_ref().map(|prepared| prepared.bundle.hash.clone());
        let recording = host.recorder.begin(settings.record, sn, recipe.clone(), &cycle_id, bundle_hash.as_deref());
        host.camera.begin_part(&cams);
        self.run_id = self.run_id.wrapping_add(1);
        self.part = Some(Part {
            run_id: self.run_id,
            cycle_id,
            bundle_hash,
            production,
            sn,
            scenario: host.sim.part_scenario(),
            frames: recipe.shots.iter().map(|shot| FrameView {
                shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, ..FrameView::waiting()
            }).collect(),
            measuring_since: vec![None; n],
            table: vec![PointState::Pending; recipe.point_count()],
            received: 0,
            extra: 0,
            queue: 0,
            router,
            issued,
            issued_verified: false,
            routing_closed: false,
            image_measurement: settings.vision,
            proc_timeout: settings.timeouts.proc(),
            drain_timeout: settings.timeouts.drain(),
            armed_at: Instant::now(),
            end_at: None,
            fault: None,
            cams,
            camera_ids,
            rig_gen,
            recording,
            recipe: recipe.clone(),
        });
        let r = if let Some(plan) = &plan { self.s7.arm(engine, plan).await } else {
            async {
                put(&app, tag::BUSY, json!(true)).await?;
                put(&app, tag::ARMED, json!(true)).await
            }.await
        };
        if let Err(e) = r {
            let reason = format!("布防写入失败：{e}");
            if self.is_s7() { self.s7.fault(engine, reason.clone()).await; }
            return self.enter_fault(reason);
        }
        self.set_phase(Phase::Acquire);
        let elapsed = t0.elapsed();
        log(&app, "info", "armed↑ busy↑", format!("{} · N={n} · 布防耗时 {} ms", recipe.id, elapsed.as_millis()));
        if elapsed > host.settings().timeouts.arm() {
            log(&app, "warn", "布防慢", format!("超过 T_arm {} ms", host.settings().timeouts.arm_ms));
        }
    }

    /// 校验没过，不布防，直接回写 ERR。
    async fn refuse(&mut self, sn: u32, recipe_id: Option<String>, code: u16, reason: String) {
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
                    if part.router.baseline(f.cam).is_some_and(|base| f.trigger_counter > base)
                        && slot.as_ref().is_some_and(|slot| slot.routing_state().is_ok_and(|state| state.0.session == f.session)) {
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
            log(&self.app, "warn", "游离帧", format!("空闲时收到相机 {camera} 的帧（帧计数 {}），已记入设备计数账", f.frame_counter));
            if self.stray_times.len() >= 3 {
                self.alarm("1 分钟内游离帧 ≥ 3，检查 Line0 接线与输入滤波".into());
            }
            return;
        }
        self.dirty = true;
        let part = self.part.as_mut().unwrap();
        let Some(route) = part.receive_frame(&f) else {
            log(&self.app, "warn", "截止后帧", format!("工件 {} 已到收尾截止，丢弃设备帧 {}", part.cycle_id, f.frame_counter));
            return;
        };
        let Route::Bound { shot: k, ordinal } = route else {
            let reason = part.router.describe(&meta, &route);
            log(&self.app, if route.fault().is_some() { "err" } else { "warn" }, "帧归属", reason);
            return;
        };
        let camera = part.camera_id(f.cam);
        if let Some(rec) = part.recording.as_mut() {
            host(&self.app).recorder.frame(rec, &f, &camera, k, ordinal);
        }
        let shot = &part.recipe.shots[k];
        let submitted_at = part.measuring_since[k].unwrap();
        let selected = shot.view;
        let skip = shot.skip;
        let expected_views = slot.as_ref().map_or(1, |s| s.config().view_count);
        let needs_image = expected_views > 1 || selected > 1 || !f.images.is_empty() || part.image_measurement;
        let selected_image = if needs_image { f.require_image(selected, expected_views).map(Some) } else { Ok(None) };
        let image = selected_image.as_ref().ok().cloned().flatten();
        crate::workspace::retain_live(&self.app, part.cycle_id.clone(), &part.recipe.hash, k, &image);
        let job = Job { run_id: part.run_id, cycle_id: part.cycle_id.clone(), shot_id: shot.id.clone(), camera,
            bundle_hash: part.bundle_hash.clone(), sn: part.sn, k, cam: f.cam, recipe: part.recipe.clone(),
            production: part.production.clone(),
            scenario: part.scenario, image, timeout: host(&self.app).settings().timeouts.proc(), submitted_at,
            image_measurement: part.image_measurement };
        if let Err(error) = selected_image {
            return self.on_measured(Measured::failed(&job, format!("拍照点 {}：{error}", job.shot_id)));
        }
        log(&self.app, "info", "帧到达", format!("cycle={} · {} · {} · 视角 {} · 设备触发 {} · 本件第 {} 次",
            job.cycle_id, job.shot_id, job.camera, selected, f.trigger_counter, ordinal));
        if skip {
            let mut measured = Measured::empty(&job);
            measured.located = true;
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
            Err(TrySendError::Full(job)) | Err(TrySendError::Closed(job)) => {
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
        let gaps = m.st.iter().filter(|&&s| s == measure::ST_GAP).count();
        let (level, ev, msg) = if let Some(e) = &m.error {
            ("err", "测量出错", format!("k={} · {e}", m.k))
        } else if m.located {
            let extra = if gaps > 0 { format!(" · 缺胶 {gaps} 点") } else { String::new() };
            (if gaps > 0 { "ng" } else { "info" }, "测量完成", format!("k={} 分数 {:.2} · {} 点 · {} ms{extra}", m.k, m.score, m.idx.len(), m.ms))
        } else {
            ("err", "定位失败", format!("k={} 匹配分数 {:.2} < 0.60", m.k, m.score))
        };
        log(&self.app, level, ev, msg);
        let _ = self.app.emit("cycle://frame", &m);
        host(&self.app).shared.lock().unwrap().measured.push(m);
    }

    async fn on_tick(&mut self) {
        let app = self.app.clone();
        let _gate = host(&app).plc_gate.lock().await;
        if !host(&app).busy() && matches!(self.phase, Phase::Idle | Phase::Fault) && crate::workspace::apply_pending(&self.app) {
            self.required = None;
            self.dirty = true;
        }
        let plc_status = plc(&self.app).status();
        let connected = plc_status.state == LinkState::Connected;
        let new_connection = connected && self.plc_connected_since != Some(plc_status.since);
        self.plc_connected_since = connected.then_some(plc_status.since);
        if new_connection && !self.is_s7() && self.phase != Phase::Fault {
            self.enter_fault("PLC 连接已更新，重新同步握手信号".into());
        }
        let cameras = match self.phase {
            // 在途的件只看它自己用的相机；等 PLC 确认结果时相机掉线不打断握手，回到空闲再查
            Phase::Acquire | Phase::Drain => match self.part.as_ref() {
                Some(p) if p.rig_gen != host(&self.app).camera.generation() => Err("检测中增删了相机".into()),
                Some(p) => self.check_part_cams(p),
                None => Ok(()),
            },
            Phase::Report | Phase::Release => Ok(()),
            _ => self.check_idle_cams(),
        };
        let dropped = host(&self.app).camera.dropped_total();
        if dropped > self.dropped_seen {
            log(&self.app, "warn", "丢帧", format!("帧通道满，累计丢弃 {dropped} 帧：检测节拍处理不过来"));
            self.dropped_seen = dropped;
        }
        let s7 = self.is_s7();
        if s7 {
            let devices_ready = if matches!(self.phase, Phase::Report | Phase::Release) {
                self.check_idle_cams().is_ok()
            } else { cameras.is_ok() };
            let reset = std::mem::take(&mut self.s7_reset_requested);
            let previous = self.s7.phase();
            let had_ack = self.s7.acknowledged();
            let event = self.s7.poll(plc(&app), reset, devices_ready).await;
            if !had_ack && self.s7.acknowledged() {
                self.delivery(PlcDeliveryState::Acknowledged, Some("PLC 结果序号已匹配确认".into()));
            }
            match event {
                SessionEvent::Ready => {
                    self.ledgers.clear_floors();
                    self.fault = None;
                    self.fault_needs_reset = false;
                    self.alarms.clear();
                    self.set_phase(Phase::Idle);
                    log(&app, "ok", "S7 复位完成", "输入基线已核验，视觉就绪");
                }
                SessionEvent::Start(request) => self.start_part(Some(request)).await,
                SessionEvent::End(ended_at) => {
                    if let Some(part) = self.part.as_mut() {
                        part.end_at = Some(ended_at);
                        part.issued_verified = true;
                    }
                    self.set_phase(Phase::Drain);
                    log(&app, "info", "partEnd↑", "三路触发数量已核对，等待剩余帧");
                }
                SessionEvent::Released => {
                    self.delivery(PlcDeliveryState::Acknowledged, Some("PLC 结果序号已确认，双方握手已释放".into()));
                    self.alarms.retain(|m| !m.starts_with("PLC 未确认"));
                    self.set_phase(Phase::Idle);
                    log(&app, "info", "S7 事务结束", "结果序号已确认，PLC 输入已释放");
                }
                SessionEvent::Fault(reason) => self.enter_fault(reason),
                SessionEvent::None => {}
            }
            if self.s7.phase() == SessionPhase::Releasing && self.phase == Phase::Report {
                self.set_phase(Phase::Release);
            }
            if matches!(self.s7.phase(), SessionPhase::ResetRequired | SessionPhase::Fault) {
                let reason = self.s7.view().message.unwrap_or_else(|| "S7 需要明确复位".into());
                if self.phase != Phase::Fault || self.fault.as_ref() != Some(&reason) { self.enter_fault(reason); }
            }
            if previous != self.s7.phase() { self.dirty = true; }
            host(&app).busy.store(!matches!(self.phase, Phase::Idle | Phase::Fault) || self.s7.pending(), Ordering::SeqCst);
        } else if self.phase != Phase::Fault {
            if !connected {
                return self.enter_fault("PLC 未连接".into());
            }
            if let Err(e) = &cameras {
                return self.enter_fault(format!("相机未就绪：{e}"));
            }
        }
        // 等待自动恢复的故障：原因跟着现状走（PLC 连上了但相机还没好，就别还显示"PLC 未连接"）
        if !s7 && self.phase == Phase::Fault && !self.fault_needs_reset {
            let reason = if !connected { Some("PLC 未连接".to_string()) } else { cameras.as_ref().err().map(|e| format!("相机未就绪：{e}")) };
            if let Some(r) = reason.filter(|r| self.fault.as_deref() != Some(r.as_str())) {
                self.fault = Some(r);
                self.dirty = true;
            }
        }
        let connected = connected && cameras.is_ok();
        let timeouts = host(&self.app).settings().timeouts;
        if matches!(self.phase, Phase::Acquire | Phase::Drain) {
            if let Some(part) = self.part.as_mut() {
                for k in part.expire(Instant::now(), timeouts.proc(), timeouts.drain()) {
                    log(&self.app, "err", "拍照点超时", part.frames[k].error.clone().unwrap_or_default());
                    self.dirty = true;
                }
            }
        }
        match self.phase {
            Phase::Fault if !s7 && connected && !self.fault_needs_reset => self.recover().await,
            Phase::Acquire => {
                let Some(part) = self.part.as_mut() else { return };
                if part.armed_at.elapsed() > timeouts.motion() {
                    part.fault = Some((fault::MOTION_TIMEOUT, format!("布防后 {} s 内未收到 partEnd", timeouts.motion_ms / 1000)));
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
                    self.alarm(format!("PLC 未确认结果（超过 T_ack {} ms），保持 done", timeouts.ack_ms));
                }
            }
            Phase::Release if !s7 => {
                if !tag_is_on(plc(&self.app), tag::PART_START) {
                    self.set_phase(Phase::Idle);
                    log(&self.app, "info", "partStart↓", "回到空闲");
                }
            }
            Phase::Release if now_ms().saturating_sub(self.since) > timeouts.ack_ms as i64 => {
                self.alarm("结果已确认，等待 PLC 清除 partStart / partEnd / resultAck；暂不接收下一件".into());
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
        let judgement = part.judgement();
        let (sn, id) = (part.sn, part.recipe.id.clone());
        self.report(sn, Some(id), judgement).await;
    }

    async fn report(&mut self, sn: u32, recipe_id: Option<String>, judgement: Judgement) {
        self.close_routing();
        let app = self.app.clone();
        let r = if self.is_s7() {
            self.s7.report(plc(&app), judgement.plc_code, judgement.fault_code).await
        } else {
            async {
                put(&app, tag::ARMED, json!(false)).await?;
                put(&app, tag::RESULT_CODE, json!(judgement.plc_code)).await?;
                put(&app, tag::FAULT_CODE, json!(judgement.fault_code)).await?;
                put(&app, tag::RESULT_SN, json!(sn)).await?;
                put(&app, tag::DONE, json!(true)).await
            }.await
        };
        self.done_at = r.as_ref().ok().map(|_| Instant::now());
        self.ack_alarmed = false;
        self.count(judgement.verdict);
        let level = match judgement.verdict {
            Verdict::Ok | Verdict::OkWithExcursion => "ok",
            Verdict::ErrInspect => "err",
            _ => "ng",
        };
        let fault = if judgement.fault_code > 0 { format!(" faultCode={}", judgement.fault_code) } else { String::new() };
        let delivery = if r.is_ok() { "done 已提交" } else { "结果未确认送达" };
        log(&app, level, "检测结果", format!("resultCode={}{fault} resultSn={sn} · {delivery} · {}", judgement.plc_code, judgement.reason));
        let drain_ms = self.part.as_ref().and_then(|p| p.end_at).map(|t| t.elapsed().as_millis() as u64);
        let delivery = PlcDelivery { state: if r.is_ok() { PlcDeliveryState::Submitted } else { PlcDeliveryState::Failed },
            updated_at: now_ms(), message: r.as_ref().err().map(|error| format!("回写 PLC 失败：{error}")) };
        self.record(sn, recipe_id.as_deref(), &judgement, drain_ms, delivery);
        self.finish_recording(sn, &judgement);
        self.result = Some(ResultView { cycle_id: self.current_cycle_id.clone(), sn, recipe_id, ts: now_ms(), drain_ms, judgement });
        match r {
            Ok(()) => self.set_phase(Phase::Report),
            Err(error) => {
                let reason = format!("回写 PLC 失败，保留本件结果：{error}");
                if self.is_s7() { self.s7.fault(plc(&app), reason.clone()).await; }
                self.fault_needs_reset = true;
                self.fault = Some(reason.clone());
                self.alarm(reason);
                self.set_phase(Phase::Fault);
            }
        }
    }

    fn close_routing(&mut self) {
        let Some(part) = self.part.as_mut().filter(|part| !part.routing_closed) else { return };
        let issued = part.issued.as_ref().filter(|_| part.issued_verified)
            .map(|counts| counts.iter().map(|(id, n)| (id.as_str(), *n)).collect::<Vec<_>>());
        part.router.close(&mut self.ledgers, issued.as_deref());
        part.routing_closed = true;
    }

    fn check_part_cams(&self, part: &Part) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        rig.check_ready_at(&part.cams)?;
        let identities = part.cams.iter().map(|&cam| {
            rig.slot(cam as usize).ok_or_else(|| "相机组已变化".to_string())?.routing_state().map(|state| state.0)
        }).collect::<Result<Vec<_>, _>>()?;
        part.router.check_sessions(&identities)
    }

    fn check_part_routing(&self, part: &Part) -> Result<(), (u16, String)> {
        self.check_part_cams(part).map_err(|reason| (fault::DEVICE_LOST, reason))?;
        let rig = &host(&self.app).camera;
        for &cam in &part.cams {
            let slot = rig.slot(cam as usize).ok_or_else(|| (fault::DEVICE_LOST, "相机组已变化".into()))?;
            let (_, observed) = slot.routing_state().map_err(|reason| (fault::DEVICE_LOST, reason))?;
            if let Some(ledger) = observed { part.router.check_observed(cam, &ledger)?; }
        }
        Ok(())
    }

    fn finish_recording(&mut self, sn: u32, judgement: &Judgement) {
        let Some(rec) = self.part.as_mut().filter(|p| p.sn == sn).and_then(|p| p.recording.take()) else { return };
        let host = host(&self.app);
        let settings = host.settings();
        // 回放相机正在用的录制目录不能被滚动删除
        let in_use = host.camera.configs().into_iter().filter(|c| c.source == CameraSource::Replay).map(|c| PathBuf::from(c.replay_dir.trim())).collect();
        host.recorder.finish(rec, judgement.verdict, &judgement.reason, settings.record_keep, (settings.record_max_gb as f64 * 1e9) as u64, in_use);
    }

    fn record(&self, sn: u32, recipe_id: Option<&str>, judgement: &Judgement, drain_ms: Option<u64>, delivery: PlcDelivery) {
        let Some(cycle_id) = self.current_cycle_id.clone() else {
            log(&self.app, "err", "记录失败", "工件身份尚未持久分配，不能关联历史与原图");
            return;
        };
        let part = self.part.as_ref().filter(|p| p.sn == sn);
        let recipe = part.map(|p| p.recipe.clone()).or_else(|| recipe_id.and_then(|id| host(&self.app).recipe(id)));
        let frames = part.map(|p| p.frames.clone()).unwrap_or_default();
        let frames_expected = recipe.as_ref().map_or(0, |r| r.shot_count());
        let table = part.map(|p| p.table.clone()).filter(|t| t.iter().any(|x| *x != PointState::Pending));
        let (received, triggers) = part.map_or((0, 0), |p| (p.received, p.triggers()));
        let shots = recorded_shots(recipe.as_deref(), &frames, &judgement.reason);
        host(&self.app).audit.insert(RecordedPart { ts: now_ms(), sn, recipe, judgement: judgement.clone(),
            drain_ms, frames, frames_expected, frames_received: received, triggers, table,
            software_version: self.app.package_info().version.to_string(), cycle_id: cycle_id.clone(),
            bundle_hash: part.and_then(|p| p.bundle_hash.clone()), delivery, shots });
        if part.is_none() {
            let off = host(&self.app).settings().record == crate::settings::RecordMode::Off;
            host(&self.app).audit.recording(crate::recorder::RecordingOutcome { cycle_id, directory: None,
                files: Vec::new(), retention_errors: Vec::new(), available: false,
                state: if off { crate::recorder::RecordingState::Off } else { crate::recorder::RecordingState::Failed },
                errors: if off { Vec::new() } else { vec![format!("布防校验未通过，未开始录制：{}", judgement.reason)] } });
        }
    }

    fn delivery(&self, state: PlcDeliveryState, message: Option<String>) {
        if let Some(cycle_id) = &self.current_cycle_id {
            host(&self.app).audit.delivery(cycle_id, PlcDelivery { state, updated_at: now_ms(), message });
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

    fn enter_fault(&mut self, reason: String) {
        self.close_routing();
        let in_flight = matches!(self.phase, Phase::Validate | Phase::Acquire | Phase::Drain | Phase::Judge);
        if in_flight {
            let sn = self.part.as_ref().map(|p| p.sn).or_else(|| self.s7.request().map(|r| r.sn)).unwrap_or(0);
            let judgement = Judgement::error(fault::DEVICE_LOST, format!("{reason}，结果未回写"));
            self.count(judgement.verdict);
            self.record(sn, self.part.as_ref().map(|p| p.recipe.id.clone()).as_deref(), &judgement, None,
                PlcDelivery { state: PlcDeliveryState::Failed, updated_at: now_ms(), message: Some(reason.clone()) });
            self.finish_recording(sn, &judgement);
            log(&self.app, "err", "在途件中断", format!("SN {sn} 记 ERR 98，需人工处理该件"));
            self.result = Some(ResultView { cycle_id: self.current_cycle_id.clone(), sn, recipe_id: self.part.as_ref().map(|p| p.recipe.id.clone()), ts: now_ms(), drain_ms: None, judgement });
        }
        if !in_flight && self.result.is_some() {
            self.delivery(PlcDeliveryState::Failed, Some(reason.clone()));
        }
        self.fault_needs_reset = in_flight || self.is_s7();
        log(&self.app, "err", "故障", reason.clone());
        self.fault = Some(reason);
        self.set_phase(Phase::Fault);
    }

    async fn recover(&mut self) {
        let app = self.app.clone();
        let r = async {
            put(&app, tag::ARMED, json!(false)).await?;
            put(&app, tag::BUSY, json!(false)).await?;
            put(&app, tag::DONE, json!(false)).await?;
            put(&app, tag::VISION_READY, json!(true)).await
        }
        .await;
        match r {
            Ok(()) => {
                self.fault = None;
                self.alarms.clear();
                self.set_phase(Phase::Idle);
                log(&app, "ok", "visionReady↑", "视觉就绪，等待工件");
            }
            Err(e) => {
                self.fault_needs_reset = true;
                self.fault = Some(format!("无法写入视觉就绪信号：{e}"));
                self.dirty = true;
            }
        }
    }

    async fn reset(&mut self) {
        if self.phase != Phase::Fault {
            return;
        }
        self.fault_needs_reset = false;
        self.ledgers.clear_floors();
        log(&self.app, "info", "故障复位", "");
        if plc(&self.app).status().state == LinkState::Connected && self.check_idle_cams().is_ok() {
            self.recover().await;
        }
    }
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

/// 配方的运行数据。给了 hash 时要的是那一版：工件用的配方刚改过，这一件仍按开工时的快照画。
#[tauri::command]
pub fn cycle_layout(cycle: State<'_, CycleHost>, recipe_id: String, hash: Option<String>) -> Result<Arc<Recipe>, String> {
    if let Some(r) = cycle.shared.lock().unwrap().part_recipe.clone().filter(|r| hash.as_ref() == Some(&r.hash)) {
        return Ok(r);
    }
    let r = cycle.recipe(&recipe_id).ok_or_else(|| format!("配方不存在：{recipe_id}"))?;
    match hash {
        Some(h) if h != r.hash => Err(format!("配方 {recipe_id} 已经改过，找不到这一版")),
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
    Ok(())
}

/// 人工选择配方，只能在空闲或故障时切换，新配方从下一个工件起生效。
#[tauri::command]
pub fn cycle_select_recipe(cycle: State<'_, CycleHost>, recipe_id: String) -> Result<(), String> {
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试")?;
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
pub fn cycle_reset(cycle: State<'_, CycleHost>) {
    let _ = cycle.tx.send(Input::Reset);
}

#[cfg(test)]
pub(crate) mod tests;

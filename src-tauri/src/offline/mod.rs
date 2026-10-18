pub mod line;
pub mod sample_set;
mod scenario;
pub mod s7_server;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ly_plc::{now_ms, LinkState, PlcConfig};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::time::sleep;

use crate::camera::{CameraSource, SampleTarget, SimRender, TriggerPayload};
use crate::cycle::{CycleHost, Input, Phase};
use crate::judge::Verdict;
use crate::plc::PlcHost;
use crate::recipe::{Recipe, TriggerMode};
use crate::settings::RecordMode;
use crate::simimage::PoseError;
use crate::site::{Derivation, Environment, OfflineChoice, OfflineSession, Preconditions, SiteMode, SiteStatus, SourceKind};
use line::{Layout, Line, Outcome, PartOrder, Timing};
use s7_server::{Fault, S7Server};
use sample_set::{SampleLabel, SampleRef, SampleSet};

pub use scenario::Scenario;

const RECENT_RUNS: usize = 20;
const CAPTURE_PLAN_VERSION: u32 = 1;
const WARMUP_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfflineFault {
    Disconnect,
    HeartbeatStop,
    HeartbeatResume,
    RejectWrite,
    RejectRead,
    Delay,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunPurpose {
    Production,
    Capture,
    CandidateDebug,
}

/// 离线产线跑过的一件：PLC 字符串 partSn ↔ 样本件。标签只用于展示，不参与判定。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRun {
    pub ts: i64,
    pub part_sn: String,
    pub part_id: String,
    pub sample_sn: String,
    pub label: SampleLabel,
    pub purpose: RunPurpose,
    pub cycle_id: Option<String>,
    pub result_code: Option<u16>,
    pub fault_code: Option<u16>,
    pub verdict: Option<Verdict>,
    pub message: Option<String>,
    #[serde(skip)]
    reference: SampleRef,
}

struct Active {
    server: S7Server,
    line: Arc<Line>,
    session: OfflineSession,
    reconnect: bool,
}

/// 离线产线：进程内 S7 服务 + 虚拟 PLC / 机器人，PC 侧照常走生产 S7 握手。由站点模式进入与退出。
#[derive(Default)]
pub struct OfflineHost {
    active: Mutex<Option<Active>>,
    candidate: Mutex<Option<Arc<crate::candidate::Candidate>>>,
    last_candidate: Mutex<Option<Arc<crate::candidate::Candidate>>>,
    enabled: AtomicBool,
    last: Mutex<Option<OfflineChoice>>,
    running: AtomicBool,
    stop: AtomicBool,
    abort: Arc<AtomicBool>,
    continuous: AtomicBool,
    parts: AtomicU32,
    next_sn: AtomicU32,
    part_scenario: Mutex<Option<Scenario>>,
    message: Mutex<String>,
    samples: Mutex<Option<Arc<SampleSet>>>,
    cursor: AtomicUsize,
    preview_part: Mutex<Option<String>>,
    runs: Mutex<VecDeque<SampleRun>>,
}

impl OfflineHost {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn part_scenario(&self) -> Scenario {
        self.part_scenario.lock().unwrap().unwrap_or(Scenario::Normal)
    }

    pub fn sample_set(&self) -> Option<Arc<SampleSet>> {
        self.samples.lock().unwrap().clone()
    }

    fn set_samples(&self, set: Option<Arc<SampleSet>>) {
        *self.samples.lock().unwrap() = set;
        self.cursor.store(0, Ordering::SeqCst);
        *self.preview_part.lock().unwrap() = None;
    }

    pub fn last(&self) -> Option<OfflineChoice> {
        self.last.lock().unwrap().clone()
    }

    pub fn set_last(&self, choice: Option<OfflineChoice>) {
        *self.last.lock().unwrap() = choice;
    }

    fn kind(&self) -> Option<SourceKind> {
        self.active.lock().unwrap().as_ref().map(|a| a.session.kind())
    }

    pub fn status(&self) -> SiteStatus {
        let active = self.active.lock().unwrap();
        let set = self.sample_set();
        let source = active.as_ref().map(|a| a.session.kind());
        SiteStatus {
            mode: if active.is_some() { SiteMode::Offline } else { SiteMode::Real },
            source,
            last: self.last(),
            measurement: crate::site::measurement_of(source),
            port: active.as_ref().map(|a| a.server.port()),
            running: self.running.load(Ordering::SeqCst),
            continuous: self.continuous.load(Ordering::SeqCst),
            parts: self.parts.load(Ordering::SeqCst),
            message: self.message.lock().unwrap().clone(),
            heartbeat_stopped: active.as_ref().is_some_and(|a| a.server.heartbeat_stopped()),
            next_part: set.as_ref().map(|s| {
                let parts = &s.manifest().parts;
                parts[self.cursor.load(Ordering::SeqCst) % parts.len()].part_id.clone()
            }),
            sample_set: set.map(|s| s.summary()),
            recent: self.runs.lock().unwrap().iter().cloned().collect(),
        }
    }

    fn set_message(&self, app: &AppHandle, message: impl Into<String>) {
        *self.message.lock().unwrap() = message.into();
        let _ = app.emit("site://status", self.status());
    }

    async fn continue_after_pause(&self, continuous: bool, pause: Duration) -> bool {
        if !continuous || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        sleep(pause).await;
        !self.stop.load(Ordering::SeqCst)
    }

    /// 指定件或按顺序的下一件（到末尾回到第一件）；指定件之后从它的下一件接着走。
    fn pick_part(&self, set: &SampleSet, requested: Option<&str>) -> Result<String, String> {
        let parts = &set.manifest().parts;
        let index = match requested {
            Some(id) => parts.iter().position(|p| p.part_id == id).ok_or_else(|| format!("样本集里没有样本件 {id}"))?,
            None => self.cursor.load(Ordering::SeqCst) % parts.len(),
        };
        self.cursor.store((index + 1) % parts.len(), Ordering::SeqCst);
        *self.preview_part.lock().unwrap() = Some(parts[index].part_id.clone());
        Ok(parts[index].part_id.clone())
    }

    fn next_sn(&self) -> u32 {
        let base = (now_ms() / 1000 % 1_000_000_000) as u32;
        let _ = self.next_sn.compare_exchange(0, base, Ordering::SeqCst, Ordering::SeqCst);
        self.next_sn.fetch_add(1, Ordering::SeqCst)
    }

    fn sn_text(n: u32) -> String {
        format!("OFFLINE-{n:010}")
    }

    fn begin_run(&self, set: &SampleSet, part_id: &str, part_sn: &str, purpose: RunPurpose) {
        let Some(reference) = set.reference(part_id) else { return };
        let mut runs = self.runs.lock().unwrap();
        runs.push_front(SampleRun { ts: now_ms(), part_sn: part_sn.into(), part_id: reference.part_id.clone(), sample_sn: reference.sn.clone(),
            label: reference.label, purpose, cycle_id: None, result_code: None, fault_code: None, verdict: None, message: None, reference });
        runs.truncate(RECENT_RUNS);
    }

    fn finish_run(&self, part_sn: &str, result: &Result<Outcome, String>) {
        let mut runs = self.runs.lock().unwrap();
        let Some(run) = runs.iter_mut().find(|r| r.part_sn == part_sn) else { return };
        match result {
            Ok(outcome) => {
                run.result_code = Some(outcome.result_code);
                run.fault_code = Some(outcome.fault_code);
                run.verdict = (matches!(run.purpose, RunPurpose::Production | RunPurpose::CandidateDebug)).then(|| verdict_of(outcome.result_code)).flatten();
            }
            Err(error) => run.message = Some(error.clone()),
        }
    }

    /// 生产节拍按 PLC 下发的 partSn 认领样本件，记下 cycleId。只在离线期间生效：真机件不会认领到样本件。
    pub fn claim(&self, part_sn: &str, cycle_id: &str) -> Option<SampleRef> {
        if !self.enabled() { return None; }
        let mut runs = self.runs.lock().unwrap();
        let run = runs.iter_mut().find(|r| r.part_sn == part_sn && matches!(r.purpose, RunPurpose::Production | RunPurpose::CandidateDebug))?;
        run.cycle_id = Some(cycle_id.to_string());
        Some(run.reference.clone())
    }

    /// 结束本次离线会话：停用、清掉样本集与本次运行的样本件记录，交回会话供恢复真实环境。
    fn deactivate(&self) -> Option<Active> {
        self.enabled.store(false, Ordering::SeqCst);
        self.candidate.lock().unwrap().take();
        self.last_candidate.lock().unwrap().take();
        self.set_samples(None);
        self.runs.lock().unwrap().clear();
        self.active.lock().unwrap().take()
    }
}

/// 恢复真实 PLC 配置之后怎么处理连接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RestorePlan {
    Reconnect,
    StayDisconnected,
    /// 真实配置没能应用：保持断开并停到安全配置，不留在已关闭的离线端口上，需人工修复。
    Park(String),
}

pub(crate) fn restore_plan(applied: &Result<(), String>, reconnect: bool, valid: bool) -> RestorePlan {
    match applied {
        Err(error) => RestorePlan::Park(error.clone()),
        Ok(()) if reconnect && valid => RestorePlan::Reconnect,
        Ok(()) => RestorePlan::StayDisconnected,
    }
}

fn parked_plc() -> PlcConfig {
    PlcConfig { auto_connect: false, ..crate::inspection::default_plc_config() }
}

/// 把 PLC 恢复到真实配置；恢复失败时显式断开并停到安全配置。是否重连由调用方在环境全部恢复后执行。
async fn restore_real_plc(app: &AppHandle, plc: &PlcHost, cycle: &CycleHost, real: PlcConfig, reconnect: bool) -> RestorePlan {
    let valid = crate::plc::validate_config(&real).is_ok();
    let applied = apply_plc_config(app, plc, cycle, real).await;
    let plan = restore_plan(&applied, reconnect, valid);
    if let RestorePlan::Park(_) = plan {
        plc.engine().disconnect().await;
        let _ = apply_plc_config(app, plc, cycle, parked_plc()).await;
    }
    plan
}

fn verdict_of(code: u16) -> Option<Verdict> {
    [Verdict::Ok, Verdict::OkWithExcursion, Verdict::NgPosition, Verdict::NgAbsolute, Verdict::NgGap, Verdict::NgWidth, Verdict::ErrInspect]
        .into_iter().find(|v| v.plc_code() == code)
}

pub(crate) fn candidate(app: &AppHandle) -> Option<Arc<crate::candidate::Candidate>> {
    let offline = app.state::<OfflineHost>();
    if !offline.enabled() { return None; }
    let candidate = offline.candidate.lock().unwrap().clone();
    candidate
}

pub(crate) fn last_candidate(app: &AppHandle) -> Option<Arc<crate::candidate::Candidate>> {
    let offline = app.state::<OfflineHost>();
    let candidate = offline.last_candidate.lock().unwrap().clone();
    candidate
}

pub(crate) async fn run_candidate(app: AppHandle, candidate: Arc<crate::candidate::Candidate>, expected_set: Arc<SampleSet>, part_id: Option<String>) -> Result<(), String> {
    let offline = app.state::<OfflineHost>();
    let cycle = app.state::<CycleHost>();
    let plc = app.state::<PlcHost>();
    let _gate = cycle.plc_gate.lock().await;
    crate::candidate::check_start(offline.enabled(), offline.sample_set().is_some(), cycle.busy() || !matches!(cycle.phase(), Phase::Idle | Phase::Fault), cycle.camera.recipe_capture().active(), offline.running.load(Ordering::SeqCst))?;
    let line = connected_line(&plc, &offline)?;
    let set = offline.sample_set().ok_or("候选试运行仅支持离线样本集")?;
    if !Arc::ptr_eq(&set, &expected_set) { return Err("离线样本集已切换，请重新试运行".into()); }
    crate::workspace::recheck_debug_candidate(&app, &candidate)?;
    if let Some(id) = part_id.as_deref() { set.part(id).ok_or("样本件不存在")?; }
    let recipe = candidate.recipe.clone();
    let shots = set.manifest().recipe_shots(&recipe)?.into_iter().map(|s| s.shot_id.clone()).collect();
    let plan = crate::plc::recipe_plan(&cycle, &recipe)?;
    let cams = crate::cycle::usable_cams(&app, &recipe)?;
    cycle.camera.check_ready_at(&cams)?;
    start_running(&offline, false)?;
    *offline.last_candidate.lock().unwrap() = Some(candidate.clone());
    *offline.candidate.lock().unwrap() = Some(candidate);
    let _ = cycle.tx.send(crate::cycle::Input::Refresh);
    let order = PartOrder { sn: String::new(), product_code: recipe.product_code, plan_version: plan.plan_version,
        shot_count: plan.shot_count, device_shots: plan.device_shots };
    let run_app = app.clone();
    tauri::async_runtime::spawn(async move {
        run(run_app, line, recipe, order, Scenario::Normal, false, Some((set, shots, part_id))).await;
    });
    Ok(())
}

pub fn enabled(app: &AppHandle) -> bool {
    app.try_state::<OfflineHost>().is_some_and(|host| host.enabled())
}

pub fn source_kind(app: &AppHandle) -> Option<SourceKind> {
    app.try_state::<OfflineHost>().and_then(|host| host.kind())
}

pub fn applied_record(app: &AppHandle) -> Option<RecordMode> {
    let host = app.try_state::<OfflineHost>()?;
    let active = host.active.lock().unwrap();
    active.as_ref().map(|a| a.session.applied().record)
}

pub fn part_scenario(app: &AppHandle) -> Scenario {
    app.try_state::<OfflineHost>().map_or(Scenario::Normal, |host| host.part_scenario())
}

pub fn sample_set(app: &AppHandle) -> Option<Arc<SampleSet>> {
    app.try_state::<OfflineHost>().and_then(|host| host.sample_set())
}

/// 无生产触发身份时（软触发、站位取图）样本集相机出当前样本件里该相机的第一个拍照点。
pub fn preview_target(app: &AppHandle, camera_id: &str) -> Option<SampleTarget> {
    let host = app.try_state::<OfflineHost>()?;
    let set = host.sample_set()?;
    let part_id = host.preview_part.lock().unwrap().clone().unwrap_or_else(|| set.manifest().parts[0].part_id.clone());
    let shot_id = set.manifest().shots.iter().find(|s| s.camera_id == camera_id)?.shot_id.clone();
    Some(SampleTarget { set, part_id, shot_id })
}

pub fn claim_sample(app: &AppHandle, part_sn: &str, cycle_id: &str) -> Option<SampleRef> {
    app.try_state::<OfflineHost>()?.claim(part_sn, cycle_id)
}

async fn apply_plc_config(app: &AppHandle, plc: &PlcHost, cycle: &CycleHost, config: PlcConfig) -> Result<(), String> {
    plc.engine().disconnect().await;
    let result = plc.engine().apply_config(config).await;
    crate::inspection::invalidate_tags();
    let _ = cycle.tx.send(Input::Refresh);
    let _ = app.emit("plc://config", ());
    result
}

fn timing(app: &AppHandle, shot_interval: Duration) -> Timing {
    let timeouts = app.state::<CycleHost>().settings().timeouts;
    Timing {
        poll: Duration::from_millis(10),
        shot_interval,
        end_delay: Duration::from_millis(300),
        ack_delay: Duration::from_millis(150),
        settle: Duration::from_millis(300),
        arm_timeout: Duration::from_secs(5),
        result_timeout: timeouts.motion() + timeouts.drain() + timeouts.proc() + Duration::from_secs(5),
        ready_timeout: Duration::from_secs(10),
        release_timeout: Duration::from_secs(10),
    }
}

fn recipe_interval(recipe: &Recipe) -> Duration {
    Duration::from_millis(match recipe.trigger_mode { TriggerMode::Fly => 450, TriggerMode::Stop => 1100 })
}

fn switch_preconditions(cycle: &CycleHost, lyflow: Result<(), String>) -> Preconditions {
    Preconditions {
        busy: cycle.busy() || !matches!(cycle.phase(), Phase::Idle | Phase::Fault),
        capturing: cycle.camera.recipe_capture().active(),
        lyflow,
    }
}

fn apply_environment_rest(app: &AppHandle, cycle: &CycleHost, environment: &Environment) {
    cycle.camera.replace_temporarily(app, environment.cameras.clone());
    crate::measure::apply_settings(app);
    let _ = cycle.tx.send(crate::cycle::Input::SiteChanged);
}

/// 进入离线：起进程内 S7 服务，按来源派生 PLC / 相机组 / 测量 / 帧录制并临时应用；真实环境保存在会话里。
pub(crate) async fn enter(app: &AppHandle, set: Option<Arc<SampleSet>>, lyflow: Result<(), String>) -> Result<(), String> {
    let plc = app.state::<PlcHost>();
    let cycle = app.state::<CycleHost>();
    let offline = app.state::<OfflineHost>();
    let _gate = cycle.plc_gate.lock().await;
    if offline.enabled() { return Err("已处于离线模式；切换来源请先退出离线".into()); }
    let early = switch_preconditions(&cycle, Ok(()));
    crate::site::check_switch(early.busy, early.capturing)?;
    let real = Environment { plc: plc.engine().config(), cameras: cycle.camera.configs(), record: cycle.settings().record };
    let reconnect = real.plc.auto_connect || plc.engine().status().state == LinkState::Connected;
    let layout = Layout::from_config(&crate::inspection::default_plc_config())?;
    let memory = layout.memory();
    let server = S7Server::start(memory.clone(), Some((layout.heartbeat(), Duration::from_millis(500)))).await
        .map_err(|e| format!("离线 S7 服务启动失败：{e}"))?;
    let port = server.port();
    let derivation = match &set { Some(set) => Derivation::SampleSet(set.manifest()), None => Derivation::Synthetic };
    let session = OfflineSession::enter(real, derivation, port, switch_preconditions(&cycle, lyflow))?;
    let applied = session.applied().clone();
    let kind = session.kind();
    offline.set_samples(set);
    offline.runs.lock().unwrap().clear();
    offline.parts.store(0, Ordering::SeqCst);
    *offline.active.lock().unwrap() = Some(Active { server, line: Arc::new(Line::new(memory, layout)), session, reconnect });
    offline.enabled.store(true, Ordering::SeqCst);
    if let Err(error) = apply_plc_config(app, &plc, &cycle, applied.plc.clone()).await {
        let Some(active) = offline.deactivate() else { return Err(error) };
        let plan = restore_real_plc(app, &plc, &cycle, active.session.leave().plc, active.reconnect).await;
        drop(active.server);
        return match plan {
            RestorePlan::Reconnect => { plc.engine().connect().await; Err(error) }
            RestorePlan::StayDisconnected => Err(error),
            RestorePlan::Park(restore) => Err(format!("{error}；回滚真机 PLC 配置也失败：{restore}，PLC 已断开，需人工修复 PLC 配置")),
        };
    }
    apply_environment_rest(app, &cycle, &applied);
    plc.engine().connect().await;
    let what = match (kind, offline.sample_set()) {
        (SourceKind::SampleSet, Some(set)) => format!("样本集 {}（{} 件 × {} 个拍照点，缺图 {} 张），lyFlow 测量", set.manifest().name,
            set.manifest().parts.len(), set.manifest().shots.len(), set.missing().len()),
        _ => "合成图，模拟测量".to_string(),
    };
    offline.set_message(app, format!("已进入离线·{what}；S7 127.0.0.1:{port}"));
    crate::cycle::log(app, "warn", "站点模式", format!("进入离线·{what}：PLC 临时连接 127.0.0.1:{port}，相机组临时生成 {} 台，帧录制临时保留全部；真实配置保留，退出后恢复", applied.cameras.len()));
    Ok(())
}

/// 退出离线：恢复进入前的 PLC、相机组与帧录制。
pub(crate) async fn leave(app: &AppHandle) -> Result<(), String> {
    let plc = app.state::<PlcHost>();
    let cycle = app.state::<CycleHost>();
    let offline = app.state::<OfflineHost>();
    let _gate = cycle.plc_gate.lock().await;
    if !offline.enabled() { return Ok(()); }
    if offline.running.load(Ordering::SeqCst) { return Err("离线产线仍在运行，先停止".into()); }
    let pre = switch_preconditions(&cycle, Ok(()));
    crate::site::check_switch(pre.busy, pre.capturing)?;
    let Some(active) = offline.deactivate() else { return Ok(()) };
    let real = active.session.leave();
    let plan = restore_real_plc(app, &plc, &cycle, real.plc.clone(), active.reconnect).await;
    apply_environment_rest(app, &cycle, &real);
    drop(active.server);
    if plan == RestorePlan::Reconnect { plc.engine().connect().await; }
    match plan {
        RestorePlan::Park(error) => {
            let message = format!("故障·需人工修复：已退出离线，但真机 PLC 配置无法恢复（{error}）；PLC 已断开并停在安全配置，请在 PLC 页修正配置后保存");
            offline.set_message(app, message.clone());
            crate::cycle::log(app, "err", "站点模式", message.clone());
            Err(message)
        }
        _ => {
            offline.set_message(app, "已退出离线，恢复真机 PLC、相机组与帧录制设置");
            crate::cycle::log(app, "info", "站点模式", "已退出离线，恢复真机 PLC、相机组与帧录制设置");
            Ok(())
        }
    }
}

/// 进入离线后由虚拟 PLC 像现场操作员一样复位一次，让节拍从"新连接待复位"进入空闲。
pub(crate) fn reset_after_enter(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let offline = app.state::<OfflineHost>();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let line = loop {
            if !offline.enabled() { return; }
            if let Ok(line) = connected_line(&app.state::<PlcHost>(), &offline) { break line; }
            if std::time::Instant::now() > until {
                offline.set_message(&app, "离线 S7 服务 10 s 内未连接，未能自动复位");
                return;
            }
            sleep(Duration::from_millis(50)).await;
        };
        let result = line.ensure_ready(&timing(&app, Duration::ZERO), &AtomicBool::new(false)).await;
        if !offline.enabled() { return; }
        match result {
            Ok(()) => crate::cycle::log(&app, "info", "站点模式", "离线 PLC 已发 faultReset，视觉就绪"),
            Err(error) => offline.set_message(&app, format!("离线 PLC 自动复位未完成：{error}")),
        }
    });
}

fn connected_line(plc: &PlcHost, offline: &OfflineHost) -> Result<Arc<Line>, String> {
    if !offline.enabled() { return Err("未进入离线模式：在离线控制台进入离线".into()); }
    if plc.engine().status().state != LinkState::Connected { return Err("离线 S7 服务尚未连接".into()); }
    offline.active.lock().unwrap().as_ref().map(|a| a.line.clone()).ok_or_else(|| "未进入离线模式：在离线控制台进入离线".into())
}

fn start_running(offline: &OfflineHost, continuous: bool) -> Result<(), String> {
    if offline.running.swap(true, Ordering::SeqCst) { return Err("离线产线已在运行".into()); }
    offline.stop.store(false, Ordering::SeqCst);
    offline.abort.store(false, Ordering::SeqCst);
    offline.continuous.store(continuous, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn offline_run(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>,
    recipe_id: String, scenario: Scenario, continuous: bool, part_id: Option<String>) -> Result<(), String> {
    let line = connected_line(&plc, &offline)?;
    let recipe = cycle.recipe(&recipe_id).ok_or("配方不存在")?;
    let plan = crate::plc::recipe_plan(&cycle, &recipe)?;
    let samples = offline.sample_set();
    if samples.is_some() && !scenario.handshake() {
        return Err("样本集图像是现场原图，只能选握手类工况（正常、传输丢帧、触发数不符、运动超时、ACK 不匹配）".into());
    }
    let warming = match crate::cycle::runnable_cams(&app, &recipe) {
        Ok(_) => false,
        Err(_) if crate::production::warming(&app, &recipe) => true,
        Err(error) => return Err(error),
    };
    if let (Some(set), Some(id)) = (&samples, part_id.as_deref()) {
        set.part(id).ok_or_else(|| format!("样本集里没有样本件 {id}"))?;
    }
    let sample_shots = samples.as_ref().map(|set| set.manifest().recipe_shots(&recipe)
        .map(|shots| shots.into_iter().map(|s| s.shot_id.clone()).collect::<Vec<_>>())).transpose()
        .map_err(|reason| format!("样本集与配方不符：{reason}"))?;
    start_running(&offline, continuous)?;
    let order = PartOrder { sn: String::new(), product_code: recipe.product_code, plan_version: plan.plan_version,
        shot_count: plan.shot_count, device_shots: plan.device_shots };
    tauri::async_runtime::spawn(async move {
        if warming && !wait_warmup(&app, &recipe).await { return finish(&app, &app.state::<OfflineHost>()); }
        run(app, line, recipe, order, scenario, continuous, samples.zip(sample_shots).map(|(set, shots)| (set, shots, part_id))).await
    });
    Ok(())
}

/// 为正在等待 PLC 开始的整圈采集轮次跑一圈样本件：每次触发同时发往本轮全部相机，第 k 次触发取各相机在样本集里的第 k 个拍照点。
#[tauri::command]
pub fn offline_capture(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>,
    part_id: Option<String>) -> Result<(), String> {
    let line = connected_line(&plc, &offline)?;
    let set = offline.sample_set().ok_or("未选择离线样本集")?;
    let round = cycle.camera.recipe_capture().current()
        .filter(|r| r.state == crate::recipe_capture::CaptureState::WaitingStart)
        .ok_or("没有等待 PLC 开始的整圈采集：先在整圈采集页点“开始接收”")?;
    let mut feeds = Vec::with_capacity(round.cameras.len());
    for camera in &round.cameras {
        let cam = cycle.camera.require(&camera.camera_id)?;
        let config = cycle.camera.slot(cam as usize).ok_or("采集设备不存在")?.config();
        if config.source != CameraSource::SampleSet { return Err(format!("采集相机 {} 不是样本集来源", config.id)); }
        let shots: Vec<_> = set.manifest().shots.iter().filter(|s| s.camera_id == config.id).collect();
        if let Some(shot) = shots.iter().find(|s| (1..=config.view_count).any(|v| !s.views.contains(&v))) {
            return Err(format!("样本集拍照点 {} 的视角 {:?} 与采集相机 {}（{} 个视角）不符", shot.shot_id, shot.views, config.id, config.view_count));
        }
        feeds.push((cam, config.id, shots.iter().map(|s| s.shot_id.clone()).collect::<Vec<_>>()));
    }
    let n = feeds[0].2.len();
    if let Some((_, id, shots)) = feeds.iter().find(|(_, _, shots)| shots.len() != n || shots.is_empty()) {
        return Err(format!("样本集里相机 {id} 有 {} 个拍照点，与其他采集相机（{n} 个）不一致或为空", shots.len()));
    }
    if n > 64 { return Err(format!("整圈采集最多 64 次触发，样本集每台相机有 {n} 个拍照点；请用 --shots 子集生成样本集")); }
    if let Some(id) = part_id.as_deref() { set.part(id).ok_or_else(|| format!("样本集里没有样本件 {id}"))?; }
    start_running(&offline, false)?;
    let n = n as u16;
    let order = PartOrder { sn: String::new(), product_code: 0, plan_version: CAPTURE_PLAN_VERSION, shot_count: n, device_shots: n };
    let feeds = feeds.into_iter().map(|(cam, _, shots)| (cam, shots)).collect();
    tauri::async_runtime::spawn(capture(app, line, set, part_id, feeds, order));
    Ok(())
}

#[tauri::command]
pub fn offline_stop(app: AppHandle, offline: State<'_, OfflineHost>, now: bool) {
    offline.stop.store(true, Ordering::SeqCst);
    offline.continuous.store(false, Ordering::SeqCst);
    if now { offline.abort.store(true, Ordering::SeqCst); }
    if offline.running.load(Ordering::SeqCst) {
        offline.set_message(&app, if now { "已请求立即停止，PLC 撤销本件请求…" } else { "已请求停止，等待本件完成…" });
    }
}

#[tauri::command]
pub fn offline_inject(app: AppHandle, offline: State<'_, OfflineHost>, fault: OfflineFault) -> Result<(), String> {
    {
        let active = offline.active.lock().unwrap();
        let active = active.as_ref().ok_or("离线模式未启用")?;
        let delay = app.state::<PlcHost>().engine().config().connection.timeout_ms.saturating_mul(2).max(500);
        active.server.inject(match fault {
            OfflineFault::Disconnect => Fault::Disconnect,
            OfflineFault::HeartbeatStop => Fault::HeartbeatStop,
            OfflineFault::HeartbeatResume => Fault::HeartbeatResume,
            OfflineFault::RejectWrite => Fault::RejectWrite,
            OfflineFault::RejectRead => Fault::RejectRead,
            OfflineFault::Delay => Fault::Delay(Duration::from_millis(delay)),
            OfflineFault::Clear => Fault::Clear,
        });
    }
    crate::cycle::log(&app, "warn", "离线故障注入", format!("{fault:?}"));
    let _ = app.emit("site://status", offline.status());
    Ok(())
}

/// 第 k 次触发的样本集取图身份：样本件 + 第 k 个拍照点编号。
pub fn sample_payload(set: &Arc<SampleSet>, part_id: &str, shot_id: &str) -> TriggerPayload {
    TriggerPayload::Sample(SampleTarget { set: set.clone(), part_id: part_id.to_string(), shot_id: shot_id.to_string() })
}

fn sim_payload(recipe: &Arc<Recipe>, part: u32, scenario: Scenario, k: usize) -> SimRender {
    let r = |i: u32| ((part.wrapping_mul(2_654_435_761).wrapping_add(i * 40_503) >> 8) % 1000) as f32 / 500.0 - 1.0;
    let pose = PoseError { dx: 4.0 * r(1), dy: 4.0 * r(2), deg: 0.3 * r(3) };
    let n = recipe.shot_count();
    SimRender {
        recipe: recipe.clone(),
        k,
        scenario,
        pose: if scenario.locate_fail_frame(n) == Some(k) { PoseError { dx: pose.dx + 130.0, dy: pose.dy + 130.0, ..pose } } else { pose },
        seed: part as u64 * 16 + k as u64,
    }
}

/// 第 trigger 次 PLC 触发：同时触发这次 Pose 的全部相机，各取自己的拍照点。
/// 样本集图按 `sample_shots[配方拍照点下标]` 取（见 [`sample_set::Manifest::recipe_shots`]）。
fn trigger_shot(app: &AppHandle, recipe: &Arc<Recipe>, groups: &[Vec<usize>], part: u32, scenario: Scenario, sample: Option<(&Arc<SampleSet>, &[String], &str)>, trigger: usize, lost: bool) {
    let cycle = app.state::<CycleHost>();
    for &k in groups.get(trigger).map_or(&[][..], Vec::as_slice) {
        let shot = &recipe.shots[k];
        let Ok(cam) = cycle.camera.require(&shot.camera) else { continue };
        let payload = match sample {
            Some((set, sample_shots, part_id)) => sample_payload(set, part_id, &sample_shots[k]),
            None => TriggerPayload::Sim(sim_payload(recipe, part, scenario, k)),
        };
        let _ = cycle.camera.trigger(cam, lost, Some(payload));
    }
}

/// 刚发布的配方先预热发布资源与图像引擎：有界等待预热完成再开工，不直接拒绝。
async fn wait_warmup(app: &AppHandle, recipe: &Recipe) -> bool {
    let offline = app.state::<OfflineHost>();
    offline.set_message(app, format!("等待预热：{} 的发布资源与图像引擎预热完成后开始运行（最多 {} s）", recipe.id, WARMUP_WAIT.as_secs()));
    let until = std::time::Instant::now() + WARMUP_WAIT;
    while crate::production::warming(app, recipe) && std::time::Instant::now() < until && !offline.stop.load(Ordering::SeqCst) {
        sleep(Duration::from_millis(100)).await;
    }
    if offline.stop.load(Ordering::SeqCst) { offline.set_message(app, "离线产线已停止"); return false; }
    match crate::cycle::runnable_cams(app, recipe) {
        Ok(_) => true,
        Err(error) => { offline.set_message(app, format!("已中止：{error}")); false }
    }
}

async fn run(app: AppHandle, line: Arc<Line>, recipe: Arc<Recipe>, order: PartOrder, scenario: Scenario, continuous: bool,
    samples: Option<(Arc<SampleSet>, Vec<String>, Option<String>)>) {
    let offline = app.state::<OfflineHost>();
    let timing = timing(&app, recipe_interval(&recipe));
    let mut seed = (now_ms() % 997) as u32;
    let (set, sample_shots, mut requested) = samples.map_or((None, Vec::new(), None), |(set, shots, part)| (Some(set), shots, part));
    let groups = match crate::plc_plan::trigger_groups(&recipe) {
        Ok(groups) => groups,
        Err(error) => { offline.set_message(&app, format!("已中止：{error}")); return finish(&app, &offline); }
    };
    while !offline.stop.load(Ordering::SeqCst) {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let s = scenario.resolve(seed >> 8);
        let how = if crate::site::image_measurement(&app) { "（lyFlow 测量）" } else { "（模拟测量）" };
        let part_id = match &set {
            Some(set) => match offline.pick_part(set, requested.take().as_deref()) {
                Ok(id) => Some(id),
                Err(error) => { offline.set_message(&app, format!("已中止：{error}")); break; }
            },
            None => None,
        };
        let what = part_id.as_ref().map_or(String::new(), |id| format!(" · 样本件 {id}"));
        offline.set_message(&app, format!("运行中：{}{what}{how}", recipe.id));
        if let Err(error) = line.ensure_ready(&timing, &offline.abort).await {
            offline.set_message(&app, format!("已中止：{error}"));
            break;
        }
        let number = offline.next_sn();
        let debug = candidate(&app).is_some();
        let sn = if debug { format!("DBG-{number:08}") } else { OfflineHost::sn_text(number) };
        if let (Some(set), Some(id)) = (&set, &part_id) { offline.begin_run(set, id, &sn, if debug { RunPurpose::CandidateDebug } else { RunPurpose::Production }); }
        *offline.part_scenario.lock().unwrap() = Some(s);
        let order = PartOrder { sn: sn.clone(), ..order.clone() };
        let sample = set.as_ref().zip(part_id.as_deref()).map(|(set, part_id)| (set, sample_shots.as_slice(), part_id));
        let shot = |k, lost| trigger_shot(&app, &recipe, &groups, number, s, sample, k, lost);
        let result = line.run_part(&timing, &order, s, shot, &offline.abort).await;
        *offline.part_scenario.lock().unwrap() = None;
        offline.parts.fetch_add(1, Ordering::SeqCst);
        offline.finish_run(&sn, &result);
        match result {
            Ok(outcome) => offline.set_message(&app, format!("SN {sn}{what} 完成，resultCode = {}", outcome.result_code)),
            Err(error) => {
                offline.set_message(&app, format!("SN {sn}{what} 已中止：{error}"));
                break;
            }
        }
        if !offline.continue_after_pause(continuous, Duration::from_millis(800)).await {
            break;
        }
    }
    finish(&app, &offline);
}

async fn capture(app: AppHandle, line: Arc<Line>, set: Arc<SampleSet>, requested: Option<String>, feeds: Vec<(u8, Vec<String>)>, order: PartOrder) {
    let offline = app.state::<OfflineHost>();
    let timing = timing(&app, Duration::from_millis(300));
    let result = async {
        let part_id = offline.pick_part(&set, requested.as_deref())?;
        offline.set_message(&app, format!("整圈采集运行中：样本件 {part_id}"));
        line.ensure_ready(&timing, &offline.abort).await?;
        let sn = OfflineHost::sn_text(offline.next_sn());
        offline.begin_run(&set, &part_id, &sn, RunPurpose::Capture);
        let order = PartOrder { sn: sn.clone(), ..order };
        let trigger = |k: usize, lost| {
            for (cam, shots) in &feeds {
                let Some(shot_id) = shots.get(k) else { continue };
                let _ = app.state::<CycleHost>().camera.trigger(*cam, lost, Some(sample_payload(&set, &part_id, shot_id)));
            }
        };
        let result = line.run_part(&timing, &order, Scenario::Normal, trigger, &offline.abort).await;
        offline.parts.fetch_add(1, Ordering::SeqCst);
        offline.finish_run(&sn, &result);
        result.map(|outcome| format!("整圈采集 SN {sn} · 样本件 {part_id} 完成，resultCode = {}", outcome.result_code))
            .map_err(|error| format!("整圈采集 SN {sn} · 样本件 {part_id} 已中止：{error}"))
    }.await;
    offline.set_message(&app, result.unwrap_or_else(|error| error));
    finish(&app, &offline);
}

fn finish(app: &AppHandle, offline: &OfflineHost) {
    if candidate(app).is_some() && app.state::<CycleHost>().busy() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            while app.state::<CycleHost>().busy() { tokio::time::sleep(Duration::from_millis(50)).await; }
            finish_idle(&app, &app.state::<OfflineHost>());
        });
    } else { finish_idle(app, offline); }
}

fn finish_idle(app: &AppHandle, offline: &OfflineHost) {
    offline.continuous.store(false, Ordering::SeqCst);
    {
        let mut message = offline.message.lock().unwrap();
        if message.starts_with("已请求") { *message = "离线产线已停止".into(); }
    }
    offline.candidate.lock().unwrap().take();
    let _ = app.state::<CycleHost>().tx.send(crate::cycle::Input::Refresh);
    offline.running.store(false, Ordering::SeqCst);
    let _ = app.emit("site://status", offline.status());
}

#[cfg(test)]
mod tests;

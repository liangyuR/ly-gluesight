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
use crate::cycle::{CycleHost, Input};
use crate::judge::Verdict;
use crate::plc::PlcHost;
use crate::recipe::{Recipe, TriggerMode};
use crate::simimage::PoseError;
use line::{Layout, Line, Outcome, PartOrder, Timing};
use s7_server::{Fault, S7Server};
use sample_set::{SampleLabel, SampleRef, SampleSet, SampleSetSummary};

pub use scenario::Scenario;

const RECENT_RUNS: usize = 20;
const CAPTURE_PLAN_VERSION: u32 = 1;

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
}

/// 离线产线跑过的一件：PLC 数值 partSn ↔ 样本件。标签只用于展示，不参与判定。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRun {
    pub ts: i64,
    pub part_sn: u32,
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineStatus {
    pub enabled: bool,
    pub port: Option<u16>,
    pub running: bool,
    pub continuous: bool,
    pub parts: u32,
    pub message: String,
    pub heartbeat_stopped: bool,
    pub sample_set: Option<SampleSetSummary>,
    pub sample_error: Option<String>,
    pub next_part: Option<String>,
    pub recent: Vec<SampleRun>,
}

struct Active {
    server: S7Server,
    line: Arc<Line>,
    previous: PlcConfig,
    reconnect: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedSampleSet {
    dir: String,
}

/// 离线产线：进程内 S7 服务 + 虚拟 PLC / 机器人，PC 侧照常走生产 S7 握手。
#[derive(Default)]
pub struct OfflineHost {
    active: Mutex<Option<Active>>,
    enabled: AtomicBool,
    running: AtomicBool,
    stop: AtomicBool,
    abort: Arc<AtomicBool>,
    continuous: AtomicBool,
    parts: AtomicU32,
    next_sn: AtomicU32,
    part_scenario: Mutex<Option<Scenario>>,
    message: Mutex<String>,
    samples: Mutex<Option<Arc<SampleSet>>>,
    sample_error: Mutex<Option<String>>,
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

    pub fn status(&self) -> OfflineStatus {
        let active = self.active.lock().unwrap();
        let set = self.sample_set();
        OfflineStatus {
            enabled: self.enabled(),
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
            sample_error: self.sample_error.lock().unwrap().clone(),
            recent: self.runs.lock().unwrap().iter().cloned().collect(),
        }
    }

    fn set_message(&self, app: &AppHandle, message: impl Into<String>) {
        *self.message.lock().unwrap() = message.into();
        let _ = app.emit("offline://status", self.status());
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

    fn begin_run(&self, set: &SampleSet, part_id: &str, part_sn: u32, purpose: RunPurpose) {
        let Some(reference) = set.reference(part_id) else { return };
        let mut runs = self.runs.lock().unwrap();
        runs.push_front(SampleRun { ts: now_ms(), part_sn, part_id: reference.part_id.clone(), sample_sn: reference.sn.clone(),
            label: reference.label, purpose, cycle_id: None, result_code: None, fault_code: None, verdict: None, message: None, reference });
        runs.truncate(RECENT_RUNS);
    }

    fn finish_run(&self, part_sn: u32, result: &Result<Outcome, String>) {
        let mut runs = self.runs.lock().unwrap();
        let Some(run) = runs.iter_mut().find(|r| r.part_sn == part_sn) else { return };
        match result {
            Ok(outcome) => {
                run.result_code = Some(outcome.result_code);
                run.fault_code = Some(outcome.fault_code);
                run.verdict = (run.purpose == RunPurpose::Production).then(|| verdict_of(outcome.result_code)).flatten();
            }
            Err(error) => run.message = Some(error.clone()),
        }
    }

    /// 生产节拍按 PLC 下发的 partSn 认领样本件，记下 cycleId。
    pub fn claim(&self, part_sn: u32, cycle_id: &str) -> Option<SampleRef> {
        let mut runs = self.runs.lock().unwrap();
        let run = runs.iter_mut().find(|r| r.part_sn == part_sn && r.purpose == RunPurpose::Production)?;
        run.cycle_id = Some(cycle_id.to_string());
        Some(run.reference.clone())
    }
}

fn verdict_of(code: u16) -> Option<Verdict> {
    [Verdict::Ok, Verdict::OkWithExcursion, Verdict::NgPosition, Verdict::NgAbsolute, Verdict::NgGap, Verdict::NgWidth, Verdict::ErrInspect]
        .into_iter().find(|v| v.plc_code() == code)
}

pub fn enabled(app: &AppHandle) -> bool {
    app.try_state::<OfflineHost>().is_some_and(|host| host.enabled())
}

pub fn part_scenario(app: &AppHandle) -> Scenario {
    app.try_state::<OfflineHost>().map_or(Scenario::Normal, |host| host.part_scenario())
}

pub fn sample_set(app: &AppHandle) -> Option<Arc<SampleSet>> {
    app.try_state::<OfflineHost>().and_then(|host| host.sample_set())
}

/// 无生产触发身份时（软触发、站位取图）样本集相机出当前样本件的第一个拍照点。
pub fn preview_target(app: &AppHandle) -> Option<SampleTarget> {
    let host = app.try_state::<OfflineHost>()?;
    let set = host.sample_set()?;
    let part_id = host.preview_part.lock().unwrap().clone().unwrap_or_else(|| set.manifest().parts[0].part_id.clone());
    let shot_id = set.manifest().shots.first()?.shot_id.clone();
    Some(SampleTarget { set, part_id, shot_id })
}

pub fn claim_sample(app: &AppHandle, part_sn: u32, cycle_id: &str) -> Option<SampleRef> {
    app.try_state::<OfflineHost>()?.claim(part_sn, cycle_id)
}

fn sample_set_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("offline-sample-set.json"))
}

/// 启动时恢复上次选择的样本集；读不了只记原因，不阻塞启动。
pub fn restore_sample_set(app: &AppHandle) {
    let Some(host) = app.try_state::<OfflineHost>() else { return };
    let saved = sample_set_file(app).and_then(|path| crate::fsio::read_json::<SavedSampleSet>(&path));
    match saved {
        Ok(Some(saved)) => match SampleSet::load(std::path::Path::new(&saved.dir)) {
            Ok(set) => host.set_samples(Some(Arc::new(set))),
            Err(error) => *host.sample_error.lock().unwrap() = Some(format!("上次选择的样本集 {} 无法加载：{error}", saved.dir)),
        },
        Ok(None) => {}
        Err(error) => *host.sample_error.lock().unwrap() = Some(error),
    }
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

#[tauri::command]
pub fn offline_status(offline: State<'_, OfflineHost>) -> OfflineStatus {
    offline.status()
}

#[tauri::command]
pub async fn offline_enable(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>) -> Result<OfflineStatus, String> {
    let _gate = cycle.plc_gate.lock().await;
    if offline.enabled() { return Ok(offline.status()); }
    if cycle.busy() { return Err("PLC 仍有在途事务，完成握手或复位后再启用离线模式".into()); }
    let previous = plc.engine().config();
    let reconnect = previous.auto_connect || plc.engine().status().state == LinkState::Connected;
    let mut config = crate::inspection::default_plc_config();
    let layout = Layout::from_config(&config)?;
    let memory = layout.memory();
    let server = S7Server::start(memory.clone(), Some((layout.heartbeat(), Duration::from_millis(500)))).await
        .map_err(|e| format!("离线 S7 服务启动失败：{e}"))?;
    config.connection.host = "127.0.0.1".into();
    config.connection.port = server.port();
    config.auto_connect = false;
    crate::plc::validate_config(&config)?;
    if let Err(error) = apply_plc_config(&app, &plc, &cycle, config).await {
        let _ = apply_plc_config(&app, &plc, &cycle, previous).await;
        return Err(error);
    }
    let port = server.port();
    *offline.active.lock().unwrap() = Some(Active { server, line: Arc::new(Line::new(memory, layout)), previous, reconnect });
    offline.enabled.store(true, Ordering::SeqCst);
    plc.engine().connect().await;
    offline.set_message(&app, format!("离线模式已启用：S7 127.0.0.1:{port}，DB100 一期点表"));
    crate::cycle::log(&app, "warn", "离线模式", format!("PLC 已切换到进程内 S7 服务 127.0.0.1:{port}，原配置保留，停用后恢复"));
    Ok(offline.status())
}

#[tauri::command]
pub async fn offline_disable(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>) -> Result<OfflineStatus, String> {
    let _gate = cycle.plc_gate.lock().await;
    if !offline.enabled() { return Ok(offline.status()); }
    if offline.running.load(Ordering::SeqCst) { return Err("离线产线仍在运行，先停止".into()); }
    if cycle.busy() { return Err("离线事务未结束：等本件释放或复位故障后再停用".into()); }
    let Some(active) = offline.active.lock().unwrap().take() else { return Ok(offline.status()) };
    offline.enabled.store(false, Ordering::SeqCst);
    let previous = active.previous.clone();
    apply_plc_config(&app, &plc, &cycle, previous.clone()).await?;
    drop(active.server);
    if active.reconnect && crate::plc::validate_config(&previous).is_ok() {
        plc.engine().connect().await;
    }
    offline.set_message(&app, "离线模式已停用，已恢复原 PLC 配置");
    crate::cycle::log(&app, "info", "离线模式", "已停用，恢复原 PLC 配置");
    Ok(offline.status())
}

#[tauri::command]
pub async fn offline_select_sample_set(app: AppHandle, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>, dir: Option<String>) -> Result<OfflineStatus, String> {
    if offline.running.load(Ordering::SeqCst) { return Err("离线产线运行中，停止后再切换样本集".into()); }
    if cycle.busy() || cycle.camera.recipe_capture().active() { return Err("检测或整圈采集进行中，结束后再切换样本集".into()); }
    let path = sample_set_file(&app)?;
    match dir.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) {
        Some(dir) => {
            let loaded = tauri::async_runtime::spawn_blocking({
                let dir = dir.clone();
                move || SampleSet::load(std::path::Path::new(&dir))
            }).await.map_err(|e| e.to_string())?;
            let set = match loaded {
                Ok(set) => set,
                Err(error) => {
                    *offline.sample_error.lock().unwrap() = Some(error.clone());
                    return Err(error);
                }
            };
            let note = format!("样本集 {}（{}）：{} 件 × {} 个拍照点，缺图 {} 张", set.manifest().name, set.manifest().id,
                set.manifest().parts.len(), set.manifest().shots.len(), set.missing().len());
            crate::fsio::write_atomic(&path, &serde_json::to_string_pretty(&SavedSampleSet { dir }).map_err(|e| e.to_string())?)?;
            offline.set_samples(Some(Arc::new(set)));
            *offline.sample_error.lock().unwrap() = None;
            crate::cycle::log(&app, "info", "离线样本集", note.clone());
            offline.set_message(&app, note);
        }
        None => {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("清除样本集选择失败：{e}")),
            }
            offline.set_samples(None);
            *offline.sample_error.lock().unwrap() = None;
            offline.set_message(&app, "已清除离线样本集");
        }
    }
    let _ = cycle.tx.send(Input::Refresh);
    let _ = app.emit("camera://changed", ());
    Ok(offline.status())
}

fn connected_line(plc: &PlcHost, offline: &OfflineHost) -> Result<Arc<Line>, String> {
    if !offline.enabled() { return Err("离线模式未启用".into()); }
    if plc.engine().status().state != LinkState::Connected { return Err("离线 S7 服务尚未连接".into()); }
    offline.active.lock().unwrap().as_ref().map(|a| a.line.clone()).ok_or_else(|| "离线模式未启用".into())
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
    for c in crate::cycle::runnable_cams(&app, &recipe, false)? {
        let cam = cycle.camera.slot(c as usize).ok_or("相机不存在")?.config();
        if cam.source == CameraSource::Mvs && cam.trigger_source != "Software" {
            return Err("相机触发源为 Line0，离线产线发不出硬触发；改为 Software 或切换到模拟相机".into());
        }
        if samples.is_some() != (cam.source == CameraSource::SampleSet) {
            return Err(if samples.is_some() {
                format!("已选择离线样本集，相机 {} 需改为样本集来源；不用样本集时先清除样本集", cam.id)
            } else {
                format!("相机 {} 是样本集来源，先在离线产线中选择样本集目录", cam.id)
            });
        }
    }
    if let (Some(set), Some(id)) = (&samples, part_id.as_deref()) {
        set.part(id).ok_or_else(|| format!("样本集里没有样本件 {id}"))?;
    }
    start_running(&offline, continuous)?;
    let order = PartOrder { sn: 0, product_code: recipe.product_code, plan_version: plan.plan_version,
        shot_count: plan.shot_count, device_shots: plan.device_shots };
    tauri::async_runtime::spawn(run(app, line, recipe, order, scenario, continuous, samples.map(|set| (set, part_id))));
    Ok(())
}

/// 为正在等待 PLC 开始的整圈采集轮次跑一圈样本件：拍照点数 = 样本集拍照点数，触发全部发往采集设备。
#[tauri::command]
pub fn offline_capture(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>,
    part_id: Option<String>) -> Result<(), String> {
    let line = connected_line(&plc, &offline)?;
    let set = offline.sample_set().ok_or("未选择离线样本集")?;
    let round = cycle.camera.recipe_capture().current()
        .filter(|r| r.state == crate::recipe_capture::CaptureState::WaitingStart)
        .ok_or("没有等待 PLC 开始的整圈采集：先在整圈采集页点“开始接收”")?;
    let cam = cycle.camera.require(&round.camera_id)?;
    let config = cycle.camera.slot(cam as usize).ok_or("采集设备不存在")?.config();
    if config.source != CameraSource::SampleSet { return Err(format!("采集设备 {} 不是样本集来源", config.id)); }
    let shots = &set.manifest().shots;
    if let Some(shot) = shots.iter().find(|s| s.camera_id != config.id || (1..=config.view_count).any(|v| !s.views.contains(&v))) {
        return Err(format!("样本集拍照点 {} 的相机或视角（{}，{:?}）与采集设备 {}（{} 个视角）不符", shot.shot_id, shot.camera_id, shot.views, config.id, config.view_count));
    }
    if shots.len() > 64 { return Err(format!("整圈采集最多 64 个拍照点，样本集有 {} 个；请用 --shots 子集生成样本集", shots.len())); }
    if let Some(id) = part_id.as_deref() { set.part(id).ok_or_else(|| format!("样本集里没有样本件 {id}"))?; }
    start_running(&offline, false)?;
    let n = shots.len() as u16;
    let order = PartOrder { sn: 0, product_code: 0, plan_version: CAPTURE_PLAN_VERSION, shot_count: n, device_shots: n };
    tauri::async_runtime::spawn(capture(app, line, set, part_id, cam, order));
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
    let _ = app.emit("offline://status", offline.status());
    Ok(())
}

/// 第 k 次触发的样本集取图身份：样本件 + 第 k 个拍照点编号。
pub fn sample_payload(set: &Arc<SampleSet>, part_id: &str, shot_id: &str) -> TriggerPayload {
    TriggerPayload::Sample(SampleTarget { set: set.clone(), part_id: part_id.to_string(), shot_id: shot_id.to_string() })
}

fn sim_payload(recipe: &Arc<Recipe>, sn: u32, scenario: Scenario, k: usize) -> SimRender {
    let r = |i: u32| ((sn.wrapping_mul(2_654_435_761).wrapping_add(i * 40_503) >> 8) % 1000) as f32 / 500.0 - 1.0;
    let pose = PoseError { dx: 4.0 * r(1), dy: 4.0 * r(2), deg: 0.3 * r(3) };
    let n = recipe.shot_count();
    SimRender {
        recipe: recipe.clone(),
        k,
        scenario,
        pose: if scenario.locate_fail_frame(n) == Some(k) { PoseError { dx: pose.dx + 130.0, dy: pose.dy + 130.0, ..pose } } else { pose },
        seed: sn as u64 * 16 + k as u64,
    }
}

fn trigger_shot(app: &AppHandle, recipe: &Arc<Recipe>, sn: u32, scenario: Scenario, vision: bool, sample: Option<(&Arc<SampleSet>, &str)>, k: usize, lost: bool) {
    let cycle = app.state::<CycleHost>();
    let Some(shot) = recipe.shots.get(k) else { return };
    let Ok(cam) = cycle.camera.require(&shot.camera) else { return };
    let tricam = cycle.camera.slot(cam as usize).is_some_and(|s| s.config().view_count == 3);
    let payload = match sample {
        Some((set, part_id)) => Some(sample_payload(set, part_id, &shot.id)),
        None => (vision || tricam).then(|| TriggerPayload::Sim(sim_payload(recipe, sn, scenario, k))),
    };
    let _ = cycle.camera.trigger(cam, lost, payload);
}

async fn run(app: AppHandle, line: Arc<Line>, recipe: Arc<Recipe>, order: PartOrder, scenario: Scenario, continuous: bool,
    samples: Option<(Arc<SampleSet>, Option<String>)>) {
    let offline = app.state::<OfflineHost>();
    let timing = timing(&app, recipe_interval(&recipe));
    let mut seed = (now_ms() % 997) as u32;
    let (set, mut requested) = samples.map_or((None, None), |(set, part)| (Some(set), part));
    while !offline.stop.load(Ordering::SeqCst) {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let s = scenario.resolve(seed >> 8);
        let settings = app.state::<CycleHost>().settings();
        let vision = settings.vision || settings.record != crate::settings::RecordMode::Off;
        let how = if settings.vision { "（lyFlow 测量）" } else { "（模拟测量）" };
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
        let sn = offline.next_sn();
        if let (Some(set), Some(id)) = (&set, &part_id) { offline.begin_run(set, id, sn, RunPurpose::Production); }
        *offline.part_scenario.lock().unwrap() = Some(s);
        let order = PartOrder { sn, ..order.clone() };
        let sample = set.as_ref().zip(part_id.as_deref());
        let shot = |k, lost| trigger_shot(&app, &recipe, sn, s, vision, sample, k, lost);
        let result = line.run_part(&timing, &order, s, shot, &offline.abort).await;
        *offline.part_scenario.lock().unwrap() = None;
        offline.parts.fetch_add(1, Ordering::SeqCst);
        offline.finish_run(sn, &result);
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

async fn capture(app: AppHandle, line: Arc<Line>, set: Arc<SampleSet>, requested: Option<String>, cam: u8, order: PartOrder) {
    let offline = app.state::<OfflineHost>();
    let timing = timing(&app, Duration::from_millis(300));
    let result = async {
        let part_id = offline.pick_part(&set, requested.as_deref())?;
        offline.set_message(&app, format!("整圈采集运行中：样本件 {part_id}"));
        line.ensure_ready(&timing, &offline.abort).await?;
        let sn = offline.next_sn();
        offline.begin_run(&set, &part_id, sn, RunPurpose::Capture);
        let order = PartOrder { sn, ..order };
        let shots = &set.manifest().shots;
        let trigger = |k: usize, lost| {
            let Some(shot) = shots.get(k) else { return };
            let _ = app.state::<CycleHost>().camera.trigger(cam, lost, Some(sample_payload(&set, &part_id, &shot.shot_id)));
        };
        let result = line.run_part(&timing, &order, Scenario::Normal, trigger, &offline.abort).await;
        offline.parts.fetch_add(1, Ordering::SeqCst);
        offline.finish_run(sn, &result);
        result.map(|outcome| format!("整圈采集 SN {sn} · 样本件 {part_id} 完成，resultCode = {}", outcome.result_code))
            .map_err(|error| format!("整圈采集 SN {sn} · 样本件 {part_id} 已中止：{error}"))
    }.await;
    offline.set_message(&app, result.unwrap_or_else(|error| error));
    finish(&app, &offline);
}

fn finish(app: &AppHandle, offline: &OfflineHost) {
    offline.continuous.store(false, Ordering::SeqCst);
    {
        let mut message = offline.message.lock().unwrap();
        if message.starts_with("已请求") { *message = "离线产线已停止".into(); }
    }
    offline.running.store(false, Ordering::SeqCst);
    let _ = app.emit("offline://status", offline.status());
}

#[cfg(test)]
mod tests;

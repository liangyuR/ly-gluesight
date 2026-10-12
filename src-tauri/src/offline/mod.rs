pub mod line;
mod scenario;
pub mod s7_server;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ly_plc::{now_ms, LinkState, PlcConfig};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::time::sleep;

use crate::camera::{CameraSource, SimRender};
use crate::cycle::{CycleHost, Input};
use crate::plc::PlcHost;
use crate::recipe::{Recipe, TriggerMode};
use crate::simimage::PoseError;
use line::{Layout, Line, PartOrder, Timing};
use s7_server::{Fault, S7Server};

pub use scenario::Scenario;

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
}

struct Active {
    server: S7Server,
    line: Arc<Line>,
    previous: PlcConfig,
    reconnect: bool,
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
}

impl OfflineHost {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn part_scenario(&self) -> Scenario {
        self.part_scenario.lock().unwrap().unwrap_or(Scenario::Normal)
    }

    pub fn status(&self) -> OfflineStatus {
        let active = self.active.lock().unwrap();
        OfflineStatus {
            enabled: self.enabled(),
            port: active.as_ref().map(|a| a.server.port()),
            running: self.running.load(Ordering::SeqCst),
            continuous: self.continuous.load(Ordering::SeqCst),
            parts: self.parts.load(Ordering::SeqCst),
            message: self.message.lock().unwrap().clone(),
            heartbeat_stopped: active.as_ref().is_some_and(|a| a.server.heartbeat_stopped()),
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
}

pub fn enabled(app: &AppHandle) -> bool {
    app.try_state::<OfflineHost>().is_some_and(|host| host.enabled())
}

pub fn part_scenario(app: &AppHandle) -> Scenario {
    app.try_state::<OfflineHost>().map_or(Scenario::Normal, |host| host.part_scenario())
}

async fn apply_plc_config(app: &AppHandle, plc: &PlcHost, cycle: &CycleHost, config: PlcConfig) -> Result<(), String> {
    plc.engine().disconnect().await;
    let result = plc.engine().apply_config(config).await;
    crate::inspection::invalidate_tags();
    let _ = cycle.tx.send(Input::Refresh);
    let _ = app.emit("plc://config", ());
    result
}

fn timing(app: &AppHandle, recipe: &Recipe) -> Timing {
    let timeouts = app.state::<CycleHost>().settings().timeouts;
    Timing {
        poll: Duration::from_millis(10),
        shot_interval: Duration::from_millis(match recipe.trigger_mode { TriggerMode::Fly => 450, TriggerMode::Stop => 1100 }),
        end_delay: Duration::from_millis(300),
        ack_delay: Duration::from_millis(150),
        settle: Duration::from_millis(300),
        arm_timeout: Duration::from_secs(5),
        result_timeout: timeouts.motion() + timeouts.drain() + timeouts.proc() + Duration::from_secs(5),
        ready_timeout: Duration::from_secs(10),
        release_timeout: Duration::from_secs(10),
    }
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
pub fn offline_run(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, offline: State<'_, OfflineHost>,
    recipe_id: String, scenario: Scenario, continuous: bool) -> Result<(), String> {
    if !offline.enabled() { return Err("离线模式未启用".into()); }
    if plc.engine().status().state != LinkState::Connected { return Err("离线 S7 服务尚未连接".into()); }
    let recipe = cycle.recipe(&recipe_id).ok_or("配方不存在")?;
    let plan = crate::plc::recipe_plan(&cycle, &recipe)?;
    for c in crate::cycle::runnable_cams(&app, &recipe, false)? {
        let cam = cycle.camera.slot(c as usize).ok_or("相机不存在")?.config();
        if cam.source == CameraSource::Mvs && cam.trigger_source != "Software" {
            return Err("相机触发源为 Line0，离线产线发不出硬触发；改为 Software 或切换到模拟相机".into());
        }
    }
    let line = offline.active.lock().unwrap().as_ref().map(|a| a.line.clone()).ok_or("离线模式未启用")?;
    if offline.running.swap(true, Ordering::SeqCst) { return Err("离线产线已在运行".into()); }
    offline.stop.store(false, Ordering::SeqCst);
    offline.abort.store(false, Ordering::SeqCst);
    offline.continuous.store(continuous, Ordering::SeqCst);
    let order = PartOrder { sn: 0, product_code: recipe.product_code, plan_version: plan.plan_version,
        shot_count: plan.shot_count, device_shots: plan.device_shots };
    tauri::async_runtime::spawn(run(app, line, recipe, order, scenario, continuous));
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

fn trigger_shot(app: &AppHandle, recipe: &Arc<Recipe>, sn: u32, scenario: Scenario, vision: bool, k: usize, lost: bool) {
    let cycle = app.state::<CycleHost>();
    let Some(cam) = recipe.shots.get(k).and_then(|shot| cycle.camera.require(&shot.camera).ok()) else { return };
    let tricam = cycle.camera.slot(cam as usize).is_some_and(|s| s.config().view_count == 3);
    let r = |i: u32| ((sn.wrapping_mul(2_654_435_761).wrapping_add(i * 40_503) >> 8) % 1000) as f32 / 500.0 - 1.0;
    let pose = PoseError { dx: 4.0 * r(1), dy: 4.0 * r(2), deg: 0.3 * r(3) };
    let n = recipe.shot_count();
    let render = (vision || tricam).then(|| SimRender {
        recipe: recipe.clone(),
        k,
        scenario,
        pose: if scenario.locate_fail_frame(n) == Some(k) { PoseError { dx: pose.dx + 130.0, dy: pose.dy + 130.0, ..pose } } else { pose },
        seed: sn as u64 * 16 + k as u64,
    });
    let _ = cycle.camera.trigger(cam, lost, render);
}

async fn run(app: AppHandle, line: Arc<Line>, recipe: Arc<Recipe>, order: PartOrder, scenario: Scenario, continuous: bool) {
    let offline = app.state::<OfflineHost>();
    let timing = timing(&app, &recipe);
    let mut seed = (now_ms() % 997) as u32;
    while !offline.stop.load(Ordering::SeqCst) {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let s = scenario.resolve(seed >> 8);
        let settings = app.state::<CycleHost>().settings();
        let vision = settings.vision || settings.record != crate::settings::RecordMode::Off;
        let how = if settings.vision { "（lyFlow 测量）" } else { "（模拟测量）" };
        offline.set_message(&app, format!("运行中：{}{how}", recipe.id));
        if let Err(error) = line.ensure_ready(&timing, &offline.abort).await {
            offline.set_message(&app, format!("已中止：{error}"));
            break;
        }
        let base = (now_ms() / 1000 % 1_000_000_000) as u32;
        let _ = offline.next_sn.compare_exchange(0, base, Ordering::SeqCst, Ordering::SeqCst);
        let sn = offline.next_sn.fetch_add(1, Ordering::SeqCst);
        *offline.part_scenario.lock().unwrap() = Some(s);
        let order = PartOrder { sn, ..order.clone() };
        let shot = |k, lost| trigger_shot(&app, &recipe, sn, s, vision, k, lost);
        let result = line.run_part(&timing, &order, s, shot, &offline.abort).await;
        *offline.part_scenario.lock().unwrap() = None;
        offline.parts.fetch_add(1, Ordering::SeqCst);
        match result {
            Ok(outcome) => offline.set_message(&app, format!("SN {sn} 完成，resultCode = {}", outcome.result_code)),
            Err(error) => {
                offline.set_message(&app, format!("SN {sn} 已中止：{error}"));
                break;
            }
        }
        if !offline.continue_after_pause(continuous, Duration::from_millis(800)).await {
            break;
        }
    }
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

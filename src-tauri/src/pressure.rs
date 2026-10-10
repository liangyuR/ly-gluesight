use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ly_plc::{LinkState, ProtocolKind};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::Sender;

use crate::camera::{Acquisition, CameraSource, FRAME_QUEUE};
use crate::cycle::{CycleHost, Phase};
use crate::measure::{Job, MEASURE_QUEUE};
use crate::plc::PlcHost;

const IDENTIFIER: &str = "com.xyzrobotics.tujiaovision.p0-tests.pressure";
static CONTROL: OnceLock<Arc<Controller>> = OnceLock::new();

#[derive(Default)]
struct Gates {
    callback_until: Option<Instant>,
    measure_until: Option<Instant>,
    callback_consumer_held: bool,
    held_job: Option<HeldJob>,
    queue_full: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldJob {
    cycle_id: String,
    shot_id: String,
    sn: u32,
    k: usize,
}

#[derive(Default)]
struct Controller {
    gates: Mutex<Gates>,
    measure_tx: Mutex<Option<Sender<Job>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    schema_version: u8,
    identifier: &'static str,
    enabled: bool,
    callback_capacity: usize,
    measure_capacity: usize,
    callback_queued: usize,
    measure_queued: usize,
    callback_hold: bool,
    measure_hold: bool,
    callback_consumer_held: bool,
    measure_blocked_consumer: bool,
    hold_remaining_ms: u64,
    held_job: Option<HeldJob>,
    measure_queue_full: u64,
    cameras: Vec<CameraCounters>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraCounters {
    id: String,
    session: u64,
    counter_source: Option<crate::frame::CounterSource>,
    last_trigger_counter: Option<u64>,
}

fn active(until: Option<Instant>) -> bool { until.is_some_and(|until| until > Instant::now()) }

fn validate_instance(identifier: &str, debug: bool) -> Result<(), String> {
    if !debug || identifier != IDENTIFIER {
        return Err("压力测试 feature 仅允许独立 pressure 调试实例".into());
    }
    Ok(())
}

pub fn init(app: &AppHandle) -> Result<(), String> {
    validate_instance(&app.config().identifier, cfg!(debug_assertions))?;
    CONTROL.set(Arc::new(Controller::default())).map_err(|_| "压力门控已初始化".to_string())
}

fn control(app: &AppHandle) -> Result<&'static Arc<Controller>, String> {
    validate_instance(&app.config().identifier, cfg!(debug_assertions))?;
    CONTROL.get().ok_or_else(|| "压力门控未初始化".into())
}

fn require_plc(app: &AppHandle) -> Result<(), String> {
    let plc = app.state::<PlcHost>();
    let engine = plc.engine();
    let connection = engine.config().connection;
    if connection.protocol != ProtocolKind::ModbusTcp || connection.host != "127.0.0.1"
        || engine.status().state != LinkState::Connected {
        return Err("压力门控要求已连接 127.0.0.1 的外部 Modbus TCP 测试 PLC".into());
    }
    Ok(())
}

pub fn frame_receive_enabled() -> bool {
    let Some(control) = CONTROL.get() else { return true };
    let mut gates = control.gates.lock().unwrap();
    let hold = active(gates.callback_until);
    gates.callback_consumer_held = hold;
    !hold
}

pub fn register_measure_queue(tx: Sender<Job>) {
    if let Some(control) = CONTROL.get() { *control.measure_tx.lock().unwrap() = Some(tx); }
}

pub async fn wait_measure(job: &Job) {
    let Some(control) = CONTROL.get() else { return };
    loop {
        {
            let mut gates = control.gates.lock().unwrap();
            if !active(gates.measure_until) {
                gates.held_job = None;
                return;
            }
            gates.held_job = Some(HeldJob {
                cycle_id: job.cycle_id.clone(), shot_id: job.shot_id.clone(), sn: job.sn, k: job.k,
            });
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub fn measure_queue_full() {
    if let Some(control) = CONTROL.get() {
        let mut gates = control.gates.lock().unwrap();
        gates.queue_full = gates.queue_full.saturating_add(1);
    }
}

#[tauri::command]
pub fn pressure_test_status(app: AppHandle) -> Result<Status, String> {
    let control = control(&app)?;
    let cycle = app.state::<CycleHost>();
    let cameras = cycle.camera.configs().into_iter().map(|config| {
        let cam = cycle.camera.require(&config.id)?;
        let slot = cycle.camera.slot(cam as usize).ok_or("相机不存在")?;
        let (arm, ledger) = slot.routing_state()?;
        Ok(CameraCounters { id: config.id, session: arm.session,
            counter_source: ledger.as_ref().and_then(|ledger| ledger.source),
            last_trigger_counter: ledger.and_then(|ledger| ledger.last) })
    }).collect::<Result<Vec<_>, String>>()?;
    let gates = control.gates.lock().unwrap();
    let callback_hold = active(gates.callback_until);
    let measure_hold = active(gates.measure_until);
    let remaining = [gates.callback_until, gates.measure_until].into_iter().flatten()
        .map(|until| until.saturating_duration_since(Instant::now())).max().unwrap_or_default();
    let measure_queued = control.measure_tx.lock().unwrap().as_ref()
        .map_or(0, |tx| MEASURE_QUEUE - tx.capacity());
    Ok(Status {
        schema_version: 1, identifier: IDENTIFIER, enabled: true,
        callback_capacity: FRAME_QUEUE, measure_capacity: MEASURE_QUEUE,
        callback_queued: cycle.camera.queued_frames(), measure_queued,
        callback_hold, measure_hold,
        callback_consumer_held: callback_hold && gates.callback_consumer_held,
        measure_blocked_consumer: measure_hold && gates.held_job.is_some(),
        hold_remaining_ms: remaining.as_millis().min(u64::MAX as u128) as u64,
        held_job: gates.held_job.clone(), measure_queue_full: gates.queue_full, cameras,
    })
}

#[tauri::command]
pub fn pressure_test_configure(app: AppHandle, callback_hold: bool, measure_hold: bool, timeout_ms: u64) -> Result<Status, String> {
    let control = control(&app)?;
    if !(100..=10_000).contains(&timeout_ms) { return Err("门控自动释放时间必须为 100–10000 ms".into()); }
    if callback_hold || measure_hold {
        require_plc(&app)?;
        let cycle = app.state::<CycleHost>();
        if cycle.busy() || cycle.phase() != Phase::Idle || cycle.sim.status().running {
            return Err("只能在空闲且内置模拟节拍停止时设置压力门控".into());
        }
    }
    {
        let mut gates = control.gates.lock().unwrap();
        let until = Instant::now() + Duration::from_millis(timeout_ms);
        gates.callback_consumer_held = false;
        gates.callback_until = callback_hold.then_some(until);
        gates.measure_until = measure_hold.then_some(until);
    }
    pressure_test_status(app)
}

#[tauri::command]
pub fn pressure_test_emit(app: AppHandle, camera_id: String, count: usize, sn: Option<u32>) -> Result<Status, String> {
    let control = control(&app)?;
    require_plc(&app)?;
    if !(1..=65).contains(&count) { return Err("单次真实回放触发数量必须为 1–65".into()); }
    let cycle = app.state::<CycleHost>();
    if cycle.sim.status().running || !cycle.settings().vision {
        return Err("压力测试要求启用图像测量并停止内置模拟节拍".into());
    }
    let snapshot = crate::cycle::cycle_snapshot(cycle.clone()).ok_or("检测状态未就绪")?;
    match snapshot.phase {
        Phase::Idle if !cycle.busy() && sn.is_none() && active(control.gates.lock().unwrap().callback_until) => {}
        Phase::Acquire if sn.is_some() && snapshot.part.as_ref().is_some_and(|part| Some(part.sn) == sn) => {}
        _ => return Err("触发只能填充已门控的空闲队列或匹配当前采集工件 SN".into()),
    }
    let cam = cycle.camera.require(&camera_id)?;
    let slot = cycle.camera.slot(cam as usize).ok_or("回放相机不存在")?;
    let config = slot.config();
    if config.source != CameraSource::Replay || config.acquisition != Acquisition::Triggered || !slot.is_ready() {
        return Err("压力触发仅接受已就绪的触发式 Replay 相机".into());
    }
    for _ in 0..count {
        if !cycle.camera.trigger(cam, false, None) { return Err("真实回放相机拒绝触发".into()); }
    }
    pressure_test_status(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_feature_rejects_production_and_release_instances() {
        assert!(validate_instance(IDENTIFIER, true).is_ok());
        assert!(validate_instance("com.xyzrobotics.tujiaovision", true).is_err());
        assert!(validate_instance("com.xyzrobotics.tujiaovision.p0-tests.audit", true).is_err());
        assert!(validate_instance(IDENTIFIER, false).is_err());
    }

    #[test]
    fn expired_holds_release_without_external_reset() {
        assert!(!active(None));
        assert!(!active(Some(Instant::now() - Duration::from_millis(1))));
        assert!(active(Some(Instant::now() + Duration::from_secs(1))));
    }
}

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ly_plc::{now_ms, ProtocolKind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::time::{sleep, Instant};

use crate::camera::{CameraSource, SimRender};
use crate::cycle::CycleHost;
use crate::inspection::{read_tag_u32, tag, tag_is_on, write_tag};
use crate::plc::PlcHost;
use crate::recipe::{PathSpec, Recipe, TriggerMode};
use crate::simimage::PoseError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scenario {
    Normal,
    Excursion,
    Gap,
    LostFrame,
    LocateFail,
    CountMismatch,
    Random,
}

impl Scenario {
    pub fn lost_frame(self, n: usize) -> Option<usize> {
        (self == Scenario::LostFrame).then(|| 3.min(n - 1))
    }

    pub fn locate_fail_frame(self, n: usize) -> Option<usize> {
        (self == Scenario::LocateFail).then(|| 4.min(n - 1))
    }

    /// 在 k1 / k2 归属分界处两侧各放一个缺胶点：单帧看都不超限，按弧长合并后超限。
    pub fn gap_points(self, recipe: &Recipe) -> Vec<usize> {
        if self != Scenario::Gap {
            return Vec::new();
        }
        let k = &recipe.points.k;
        (0..k.len() - 1).find(|&j| k[j] == 1 && k[j + 1] == 2).map(|j| vec![j, j + 1]).unwrap_or_default()
    }

    fn resolve(self, seed: u32) -> Scenario {
        if self != Scenario::Random {
            return self;
        }
        match seed % 25 {
            0 => Scenario::Gap,
            1 => Scenario::LostFrame,
            2 => Scenario::LocateFail,
            3 | 4 => Scenario::Excursion,
            _ => Scenario::Normal,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimStatus {
    pub running: bool,
    pub continuous: bool,
    pub parts: u32,
    pub message: String,
}

/// 模拟 PLC 与机器人：写入工件信息、等待布防、发触发、给 partEnd、确认结果。仅在 PLC 协议为模拟器时可用。
#[derive(Default)]
pub struct SimCtl {
    running: AtomicBool,
    stop: AtomicBool,
    continuous: AtomicBool,
    parts: AtomicU32,
    next_sn: AtomicU32,
    part_scenario: Mutex<Option<Scenario>>,
    /// 外部演示 Robot 每件已经接受的触发序号，防止重试产生重复帧。
    external_shots: Mutex<Option<(u32, usize)>>,
    message: Mutex<String>,
}

impl SimCtl {
    pub fn part_scenario(&self) -> Scenario {
        self.part_scenario.lock().unwrap().unwrap_or(Scenario::Normal)
    }

    pub fn status(&self) -> SimStatus {
        SimStatus {
            running: self.running.load(Ordering::SeqCst),
            continuous: self.continuous.load(Ordering::SeqCst),
            parts: self.parts.load(Ordering::SeqCst),
            message: self.message.lock().unwrap().clone(),
        }
    }

    fn set_message(&self, app: &AppHandle, message: impl Into<String>) {
        *self.message.lock().unwrap() = message.into();
        let _ = app.emit("sim://status", self.status());
    }

    async fn continue_after_pause(&self, continuous: bool, pause: Duration) -> bool {
        if !continuous || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        sleep(pause).await;
        // “本件后停止”也可能在两件之间按下；休息结束后再次确认，不能多开一件。
        !self.stop.load(Ordering::SeqCst)
    }
}

async fn wait_for(app: &AppHandle, timeout: Duration, cond: impl Fn(&AppHandle) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond(app) {
            return true;
        }
        sleep(Duration::from_millis(10)).await;
    }
    false
}

fn on(app: &AppHandle, t: &str) -> bool {
    tag_is_on(app.state::<PlcHost>().engine(), t)
}

async fn put(app: &AppHandle, t: &str, v: serde_json::Value) -> Result<(), String> {
    write_tag(app.state::<PlcHost>().engine(), t, v).await
}

async fn run_part(app: &AppHandle, recipe: &Arc<Recipe>, scenario: Scenario, vision: bool) -> Result<String, String> {
    let cycle = app.state::<CycleHost>();
    let n = recipe.shot_count();
    let base = (now_ms() / 1000 % 1_000_000_000) as u32;
    let _ = cycle.sim.next_sn.compare_exchange(0, base, Ordering::SeqCst, Ordering::SeqCst);
    let sn = cycle.sim.next_sn.fetch_add(1, Ordering::SeqCst);
    *cycle.sim.part_scenario.lock().unwrap() = Some(scenario);

    let shot_count = if scenario == Scenario::CountMismatch { n - 1 } else { n };
    put(app, tag::PART_SN, json!(sn)).await?;
    put(app, tag::PRODUCT_CODE, json!(recipe.product_code)).await?;
    put(app, tag::SHOT_COUNT, json!(shot_count)).await?;
    put(app, tag::PART_START, json!(true)).await?;

    if !wait_for(app, Duration::from_secs(3), |a| on(a, tag::ARMED) || on(a, tag::DONE)).await {
        return Err("3 s 内未收到 armed 或 done".into());
    }
    if on(app, tag::ARMED) {
        let interval = match recipe.trigger_mode {
            TriggerMode::Fly => 450,
            TriggerMode::Stop => 1100,
        };
        let cams = recipe.shots.iter().map(|s| cycle.camera.require(&s.camera)).collect::<Result<Vec<_>, _>>()?;
        let lost = scenario.lost_frame(n);
        let locate_fail = scenario.locate_fail_frame(n);
        // 机器人每件的定位偏差：±0.4 mm、±0.15°（模板定位要吸收它）
        let r = |i: u32| ((sn.wrapping_mul(2_654_435_761).wrapping_add(i * 40_503) >> 8) % 1000) as f64 / 500.0 - 1.0;
        let pose = PoseError { dx: 0.4 * r(1), dy: 0.4 * r(2), deg: 0.15 * r(3) };
        for k in 0..n {
            sleep(Duration::from_millis(interval)).await;
            let render = vision.then(|| SimRender {
                recipe: recipe.clone(),
                k,
                scenario,
                // 定位失败场景：这一帧斜着偏开 15 mm，超出模板搜索范围，也没有哪条直边还能对上
                pose: if locate_fail == Some(k) { PoseError { dx: pose.dx + 15.0, dy: pose.dy + 15.0, ..pose } } else { pose },
                seed: sn as u64 * 16 + k as u64,
            });
            let _ = cycle.camera.trigger(cams[k], lost == Some(k), render);
        }
        sleep(Duration::from_millis(300)).await;
        put(app, tag::PART_END, json!(true)).await?;
    }

    finish_part(app, sn).await
}

/// 等结果、确认、复位握手信号。
async fn finish_part(app: &AppHandle, sn: u32) -> Result<String, String> {
    if !wait_for(app, Duration::from_secs(10), |a| on(a, tag::DONE)).await {
        return Err("10 s 内未收到 done".into());
    }
    let code = read_tag_u32(app.state::<PlcHost>().engine(), tag::RESULT_CODE).unwrap_or(0);
    sleep(Duration::from_millis(150)).await;
    put(app, tag::RESULT_ACK, json!(true)).await?;
    wait_for(app, Duration::from_secs(3), |a| !on(a, tag::DONE)).await;
    for t in [tag::PART_START, tag::PART_END, tag::RESULT_ACK] {
        put(app, t, json!(false)).await?;
    }
    Ok(format!("SN {sn} 完成，resultCode = {code}"))
}

pub async fn run(app: AppHandle, recipe: Arc<Recipe>, scenario: Scenario, continuous: bool) {
    let sim = &app.state::<CycleHost>().sim;
    let mut seed = (now_ms() % 997) as u32;
    while !sim.stop.load(Ordering::SeqCst) {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let s = scenario.resolve(seed >> 8);
        // 模拟相机只在真要看图（lyFlow 测量或帧录制）时才合成飞拍图像，一帧 5 MP 很费 CPU
        let settings = app.state::<CycleHost>().settings();
        let lyflow = settings.vision;
        let recording = settings.record != crate::settings::RecordMode::Off;
        let vision = (lyflow || recording) && matches!(recipe.path, Some(PathSpec::RoundedRect { .. }));
        if vision && lyflow {
            sim.set_message(&app, format!("准备 {} 的模拟示教资料…", recipe.id));
            let (a, r) = (app.clone(), recipe.clone());
            let prepared = tauri::async_runtime::spawn_blocking(move || crate::vision::assets_for(&a, &r).map(|_| ())).await;
            if let Ok(Err(e)) | Err(e) = prepared.map_err(|e| e.to_string()) {
                sim.set_message(&app, format!("已中止：{e}"));
                break;
            }
        }
        let how = if lyflow { "（lyFlow 测量）" } else { "（模拟测量）" };
        sim.set_message(&app, format!("运行中：{}{how}", recipe.id));
        let result = run_part(&app, &recipe, s, vision).await;
        *sim.part_scenario.lock().unwrap() = None;
        sim.parts.fetch_add(1, Ordering::SeqCst);
        match result {
            Ok(msg) => sim.set_message(&app, msg),
            Err(e) => {
                for t in [tag::PART_START, tag::PART_END, tag::RESULT_ACK] {
                    let _ = put(&app, t, json!(false)).await;
                }
                sim.set_message(&app, format!("已中止：{e}"));
                break;
            }
        }
        if !sim.continue_after_pause(continuous, Duration::from_millis(800)).await {
            break;
        }
    }
    sim.continuous.store(false, Ordering::SeqCst);
    {
        let mut message = sim.message.lock().unwrap();
        if *message == "已请求停止，等待本件完成…" {
            *message = "模拟节拍已停止".into();
        }
    }
    sim.running.store(false, Ordering::SeqCst);
    let _ = app.emit("sim://status", sim.status());
}

#[tauri::command]
pub fn sim_status(cycle: State<'_, CycleHost>) -> SimStatus {
    cycle.sim.status()
}

#[tauri::command]
pub fn sim_start(
    app: AppHandle,
    plc: State<'_, PlcHost>,
    cycle: State<'_, CycleHost>,
    recipe_id: String,
    scenario: Scenario,
    continuous: bool,
) -> Result<(), String> {
    if plc.engine().config().connection.protocol != ProtocolKind::Simulator {
        return Err("模拟节拍只能在 PLC 协议为“模拟器”时使用".into());
    }
    if plc.engine().status().state != ly_plc::LinkState::Connected {
        return Err("PLC 模拟器未连接".into());
    }
    let recipe = cycle.recipe(&recipe_id).ok_or("配方不存在")?;
    // 采集方式、图像测量与开工时同一套检查；模拟节拍还要能发出触发
    for c in crate::cycle::runnable_cams(&app, &recipe, false)? {
        let cam = cycle.camera.slot(c as usize).ok_or("相机不存在")?.config();
        if cam.source == CameraSource::Mvs && cam.trigger_source != "Software" {
            return Err("相机触发源为 Line0，模拟节拍发不出硬触发；改为 Software 或切换到模拟相机".into());
        }
    }
    if cycle.sim.running.swap(true, Ordering::SeqCst) {
        return Err("模拟节拍已在运行".into());
    }
    cycle.sim.stop.store(false, Ordering::SeqCst);
    cycle.sim.continuous.store(continuous, Ordering::SeqCst);
    tauri::async_runtime::spawn(run(app, recipe, scenario, continuous));
    Ok(())
}

#[tauri::command]
pub fn sim_stop(app: AppHandle, cycle: State<'_, CycleHost>) {
    cycle.sim.stop.store(true, Ordering::SeqCst);
    cycle.sim.continuous.store(false, Ordering::SeqCst);
    if cycle.sim.running.load(Ordering::SeqCst) {
        cycle.sim.set_message(&app, "已请求停止，等待本件完成…");
    }
}

/// 演示 Robot 的虚拟相机触发线；PLC 握手仍通过独立 Modbus TCP 服务。
/// 仅独立调试演示实例开放，不接受实体相机，也不写 PLC 点位或伪造测量结果。
#[tauri::command]
pub fn sim_robot_trigger(
    app: AppHandle,
    plc: State<'_, PlcHost>,
    cycle: State<'_, CycleHost>,
    sn: u32,
    k: usize,
    scenario: Scenario,
) -> Result<(), String> {
    if !cfg!(debug_assertions) || app.config().identifier != "com.xyzrobotics.gluesight.robot-plc-demo" {
        return Err("外部模拟触发仅供 Robot PLC 独立调试演示实例使用".into());
    }
    let connection = plc.engine().config().connection;
    if connection.protocol != ProtocolKind::ModbusTcp || connection.host != "127.0.0.1" {
        return Err("演示触发要求连接 127.0.0.1 的 Modbus TCP 模拟 PLC".into());
    }
    if plc.engine().status().state != ly_plc::LinkState::Connected || cycle.sim.running.load(Ordering::SeqCst) {
        return Err("模拟 PLC 未连接或内置模拟节拍正在运行".into());
    }
    let snapshot = crate::cycle::cycle_snapshot(cycle.clone()).ok_or("检测状态未就绪")?;
    if snapshot.phase != crate::cycle::Phase::Acquire {
        return Err("Robot 只能在已布防的采集阶段触发".into());
    }
    let part = snapshot.part.ok_or("没有正在检测的工件")?;
    if part.sn != sn || k >= part.n {
        return Err("Robot 工件 SN 或拍照序号与当前工件不符".into());
    }
    let recipe = crate::cycle::cycle_layout(cycle.clone(), part.recipe_id, Some(part.recipe_hash))?;
    let cam = cycle.camera.require(&recipe.shots.get(k).ok_or("拍照序号超出配方")?.camera)?;
    if cycle.camera.slot(cam as usize).ok_or("相机不存在")?.config().source != CameraSource::Sim {
        return Err("外部演示触发只允许模拟相机".into());
    }
    if !cycle.settings().vision {
        return Err("演示联调需要启用 lyFlow 图像测量".into());
    }
    let mut sequence = cycle.sim.external_shots.lock().unwrap();
    let next = sequence.filter(|(active_sn, _)| *active_sn == sn).map_or(0, |(_, next)| next);
    if k != next {
        return Err(format!("Robot 触发重复或乱序：应为 k{next}，收到 k{k}"));
    }
    let scenario = scenario.resolve(sn);
    let pose = if scenario.locate_fail_frame(part.n) == Some(k) {
        PoseError { dx: 15.0, dy: 15.0, deg: 0.0 }
    } else {
        PoseError { dx: 0.0, dy: 0.0, deg: 0.0 }
    };
    let render = SimRender { recipe, k, scenario, pose, seed: sn as u64 * 16 + k as u64 };
    if !cycle.camera.trigger(cam, scenario.lost_frame(part.n) == Some(k), Some(render)) {
        return Err("模拟相机未接受触发".into());
    }
    *sequence = Some((sn, k + 1));
    crate::cycle::log(&app, "info", "外部 Robot", format!("SN {sn} · k{k} · {scenario:?}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_during_inter_part_pause_does_not_start_another_part() {
        let sim = SimCtl::default();
        let (start_next, ()) = tokio::join!(
            biased;
            sim.continue_after_pause(true, Duration::from_millis(20)),
            async { sim.stop.store(true, Ordering::SeqCst); },
        );
        assert!(!start_next, "stop accepted between parts must prevent the next part");
    }

    #[tokio::test]
    async fn only_unstopped_continuous_runs_start_another_part() {
        let sim = SimCtl::default();
        assert!(!sim.continue_after_pause(false, Duration::ZERO).await);
        assert!(sim.continue_after_pause(true, Duration::ZERO).await);
        sim.stop.store(true, Ordering::SeqCst);
        assert!(!sim.continue_after_pause(true, Duration::ZERO).await);
    }
}

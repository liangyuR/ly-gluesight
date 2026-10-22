use super::line::{Layout, Line, Outcome, PartOrder, Pulse, PulseProfile, Timing};
use super::s7_server::S7Server;
use super::test_dir::TestDir;
use super::*;
use crate::burst_judge::BurstJudgement;
use crate::cycle::tests::{burst_frame, judge_burst_at_drain_deadline};
use crate::frame_group::{LossAlarm, LossReason};
use crate::judge::fault;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::plc_session::{Origin, PendingKind, PlcSession, SessionEvent, SessionPhase};
use crate::inspection::tag;
use ly_plc::{EventSink, Logbook, PlcEngine};
use std::time::Instant;

const HANG_GUARD: Duration = Duration::from_secs(120);
/// PC 主动断开后 3.5 s 自动重连（plc_session 的 RECONNECT_AFTER_STOP）再留余量：观察这么久没有重连，才算不会再自动重连。
const RECONNECT_AFTER_STOP_WINDOW: Duration = Duration::from_millis(4000);

const TIMING: Timing = Timing {
    poll: Duration::from_millis(5),
    shot_interval: Duration::from_millis(20),
    end_delay: Duration::from_millis(20),
    ack_delay: Duration::from_millis(20),
    settle: Duration::from_millis(150),
    arm_timeout: HANG_GUARD,
    result_timeout: HANG_GUARD,
    ready_timeout: HANG_GUARD,
    release_timeout: HANG_GUARD,
};

/// 测试台的脉冲频率：间隔 10 ms，一件几十毫秒跑完。
const BENCH_HZ: f64 = 100.0;

struct Bench {
    _server: S7Server,
    line: Line,
    engine: PlcEngine,
    session: PlcSession,
    plan: PlcPlan,
    intake: Arc<AtomicBool>,
    directory: TestDir,
    /// 段配方最近一件 PC 侧的区域判定
    judged: Option<BurstJudgement>,
}

/// 三台相机都交付了这一脉冲的帧（传输丢帧的除外）。
fn all_cameras(pulse: &Pulse) -> Vec<usize> {
    (0..3).filter(|camera| !pulse.lost.contains(camera)).collect()
}

impl Bench {
    async fn new(name: &str) -> Self {
        Self::new_with(name, |_| {}).await
    }

    async fn new_with(name: &str, before_connect: impl FnOnce(&S7Server)) -> Self {
        Self::start(name, Duration::from_millis(20), Duration::from_millis(100), before_connect).await
    }

    /// 旧拍照点配方的测试台：单相机 cam1、4 个拍照点各一次触发。
    async fn legacy(name: &str) -> Self {
        let mut bench = Self::new(name).await;
        bench.plan = PlcPlan::compile("part-A".into(), 7, vec!["cam1".into()], (1..=4).map(|i| PlanShot {
            shot_id: format!("P{i}"), pose_id: format!("P{i}"), camera_ids: vec!["cam1".into()],
        }).collect()).unwrap();
        bench
    }

    /// 段计划的一件：一个直线段、名义帧数 = ΣN_exp，不波动、不丢帧。
    fn order(&self, sn: &str) -> PartOrder {
        let profile = PulseProfile { hz: BENCH_HZ, segments: vec![(crate::plan::SegmentKind::Straight, u32::from(self.plan.shot_count))],
            head_tail_jitter: 0, corner_jitter: 0, seed: 0 };
        PartOrder { profile: Some(profile), ..PartOrder::legacy(sn.into(), 3, self.plan.plan_version, self.plan.shot_count, self.plan.device_shots) }
    }

    /// `poll`：PC 轮询周期；`heartbeat`：虚拟 PLC 心跳翻转间隔。
    async fn start(name: &str, poll: Duration, heartbeat: Duration, before_connect: impl FnOnce(&S7Server)) -> Self {
        let mut config = crate::inspection::default_plc_config();
        let layout = Layout::from_config(&config).unwrap();
        let memory = layout.memory();
        let server = S7Server::start(memory.clone(), Some((layout.heartbeat(), heartbeat))).await.unwrap();
        config.connection.host = "127.0.0.1".into();
        config.connection.port = server.port();
        config.connection.timeout_ms = 5000;
        config.connection.poll_interval_ms = poll.as_millis() as u64;
        config.connection.reconnect_interval_ms = 200;
        config.heartbeat.interval_ms = 100;
        let directory = TestDir::new("offline-tests", name);
        let sink: EventSink = Arc::new(|_| {});
        let engine = PlcEngine::new(config, Arc::new(Logbook::open(&directory.join("plc.db"), sink.clone(), 1).unwrap()), sink);
        before_connect(&server);
        engine.connect().await;
        // 段计划：三台相机共用一个触发，shotCount = deviceShots = ΣN_exp = 4
        let plan = PlcPlan::from_recipe(&crate::recipe::samples_burst().build().unwrap()).unwrap();
        Self { _server: server, line: Line::new(memory, layout), engine, session: PlcSession::open(directory.join("session.json")), plan,
            intake: Arc::new(AtomicBool::new(true)), directory, judged: None }
    }

    /// 跑一件正常件：段计划按脉冲串，旧拍照点计划按触发。
    async fn run(&mut self, scenario: Scenario) -> Result<Outcome, String> {
        if self.plan.is_burst() {
            let order = self.order(" a-4242 ~");
            return self.run_burst_with(order, scenario, all_cameras).await?.ok_or_else(|| "PLC 没有提交 partStart".into());
        }
        self.run_with(" a-4242 ~".into(), scenario, |_, lost| !lost).await
    }

    /// 段配方：虚拟 PLC 按 `order` 的脉冲剖面跑一件，PC 侧按生产节拍驱动 S7 会话：开始 → 布防 → partEnd 后用实收帧
    /// 走段配方节拍判定（[`judge_burst_at_drain_deadline`]）并回写，判定存进 `judged`。
    /// `fire(pulse)` 模拟相机侧：返回这一脉冲真正交付了帧的相机（计划相机下标），帧计数 = 脉冲序号。
    async fn run_burst_with(&mut self, order: PartOrder, scenario: Scenario, mut fire: impl FnMut(&Pulse) -> Vec<usize>) -> Result<Option<Outcome>, String> {
        self.until_heartbeat_live().await;
        let Self { line, engine, session, plan, intake, judged, .. } = self;
        let delivered = Mutex::new(Vec::new());
        let finished = AtomicBool::new(false);
        let abort = AtomicBool::new(false);
        let plc = async {
            let result = match line.ensure_ready(&TIMING, &abort).await {
                Ok(()) => line.run_burst(&TIMING, &order, scenario, |pulse| {
                    let cameras = fire(pulse);
                    delivered.lock().unwrap().extend(cameras.into_iter().map(|camera| burst_frame(camera as u8, pulse.seq)));
                }, &abort).await,
                Err(error) => Err(error),
            };
            finished.store(true, Ordering::SeqCst);
            result
        };
        let pc = async {
            let until = Instant::now() + HANG_GUARD;
            while !finished.load(Ordering::SeqCst) || !matches!(session.phase(), SessionPhase::Idle | SessionPhase::Fault) {
                assert!(Instant::now() < until, "会话未收敛：{:?}", session.view());
                match session.poll_gated(engine, false, true, true, intake.load(Ordering::SeqCst)).await {
                    SessionEvent::Start(_) => session.arm(engine, plan).await.unwrap(),
                    SessionEvent::End(_, triggers) => {
                        let judgement = judge_burst_at_drain_deadline(delivered.lock().unwrap().clone(), triggers);
                        session.report(engine, judgement.plc_code, judgement.fault_code).await.unwrap();
                        *judged = Some(judgement);
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::join!(plc, pc).0
    }

    /// 虚拟 PLC 的 faultReset 上升沿只会被会话处理一次：会话还没观察到心跳翻转就按复位，这次复位被拒且不再重发。
    /// 先让会话轮询到心跳翻转，复位时机就只取决于会话状态，不取决于机器多忙。
    async fn until_heartbeat_live(&mut self) {
        let until = Instant::now() + HANG_GUARD;
        while !self.session.heartbeat_live() {
            assert!(Instant::now() < until, "会话长时间没有观察到 PLC 心跳翻转：{:?}", self.session.view());
            self.session.poll_gated(&self.engine, false, true, true, self.intake.load(Ordering::SeqCst)).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 旧拍照点配方：虚拟 PLC 跑一件，PC 侧按生产节拍的顺序驱动 S7 会话：开始 → 布防 → partEnd 后按实收帧判定回写。
    /// `deliver(k, lost)` 模拟相机侧：返回这次触发是否真的交付了一帧。
    async fn run_with(&mut self, sn: String, scenario: Scenario, mut deliver: impl FnMut(usize, bool) -> bool) -> Result<Outcome, String> {
        self.until_heartbeat_live().await;
        let Self { line, engine, session, plan, intake, .. } = self;
        let delivered = Mutex::new(Vec::new());
        let finished = AtomicBool::new(false);
        let abort = AtomicBool::new(false);
        let order = PartOrder::legacy(sn, 3, plan.plan_version, plan.shot_count, plan.device_shots);
        let plc = async {
            let result = match line.ensure_ready(&TIMING, &abort).await {
                Ok(()) => line.run_part(&TIMING, &order, scenario, |k, lost| { let ok = deliver(k, lost); delivered.lock().unwrap().push(!ok) }, &abort).await,
                Err(error) => Err(error),
            };
            finished.store(true, Ordering::SeqCst);
            result
        };
        let pc = async {
            let until = Instant::now() + HANG_GUARD;
            while !finished.load(Ordering::SeqCst) || !matches!(session.phase(), SessionPhase::Idle | SessionPhase::Fault) {
                assert!(Instant::now() < until, "会话未收敛：{:?}", session.view());
                match session.poll_gated(engine, false, true, true, intake.load(Ordering::SeqCst)).await {
                    SessionEvent::Start(_) => session.arm(engine, plan).await.unwrap(),
                    SessionEvent::End(_, _) => {
                        let judgement = crate::cycle::tests::judge_delivered(&delivered.lock().unwrap());
                        session.report(engine, judgement.plc_code, judgement.fault_code).await.unwrap();
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::join!(plc, pc).0
    }
}

#[tokio::test]
async fn offline_line_over_s7_completes_parts_with_matched_ack_and_returns_to_idle() {
    let mut bench = Bench::new("normal").await;
    for seq in [1, 2] {
        let outcome = bench.run(Scenario::Normal).await.unwrap();
        assert_eq!(outcome, Outcome { request_seq: seq, sn: " a-4242 ~".into(), result_code: 1, fault_code: 0 });
        assert_eq!(bench.session.phase(), SessionPhase::Idle);
        assert!(!bench.session.pending());
        assert_eq!(bench.line.read(tag::RESULT_SEQ), seq);
        assert_eq!(bench.line.read(tag::ACK_SEQ), seq);
        assert_eq!(bench.line.read(tag::DEVICE_TRIGGERS), 4);
        for field in [tag::PART_START, tag::PART_END, tag::RESULT_ACK, tag::DONE, tag::BUSY, tag::ARMED, tag::VISION_FAULT] {
            assert_eq!(bench.line.read(field), 0, "{field}");
        }
    }
    bench.engine.disconnect().await;
}

/// S-12：计入布防预算的布防输出确认既不等下一个轮询周期，也不等握手日志落盘（落盘在 commit_arm）。
/// 刚轮询完就布防：等下一个周期要将近整个周期，等落盘要等到阻塞线程放行，都超出上限；上限留足负载余量。
/// PLC 心跳翻转间隔长于轮询周期，每个电平都会被读到，心跳活性不受轮询相位影响。
#[test]
fn arm_outputs_confirm_without_waiting_for_a_poll_period_or_the_journal() {
    const POLL_PERIOD: Duration = Duration::from_millis(600);
    const LIMIT: Duration = Duration::from_millis(450);
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap().block_on(async {
        let mut bench = Bench::start("arm-latency", POLL_PERIOD, Duration::from_millis(900), |_| {}).await;
        let order = bench.order("SN-ARM");
        let Bench { line, engine, session, plan, .. } = &mut bench;
        let until = Instant::now() + Duration::from_secs(15);
        while !matches!(session.poll(engine, true, true).await, SessionEvent::Ready) {
            assert!(Instant::now() < until, "复位未完成：{:?}", session.view());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let abort = AtomicBool::new(false);
        let plc = async { line.ensure_ready(&TIMING, &abort).await?; line.run_burst(&TIMING, &order, Scenario::Normal, |_| {}, &abort).await };
        let pc = async {
            let until = Instant::now() + Duration::from_secs(15);
            while !matches!(session.poll(engine, false, true).await, SessionEvent::Start(_)) {
                assert!(Instant::now() < until, "未收到开始请求：{:?}", session.view());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let (started, started_wait) = tokio::sync::oneshot::channel();
            let journal_blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                let _ = wait.recv_timeout(Duration::from_secs(10));
            });
            started_wait.await.unwrap();
            let polled = engine.status().poll_count;
            while engine.status().poll_count == polled { tokio::time::sleep(Duration::from_millis(1)).await; }
            let begun = Instant::now();
            session.arm_outputs(engine, plan).await.unwrap();
            let elapsed = begun.elapsed();
            release.send(()).unwrap();
            journal_blocker.await.unwrap();
            session.commit_arm(engine).await.unwrap();
            assert_eq!(session.phase(), SessionPhase::Acquiring);
            abort.store(true, Ordering::SeqCst);
            elapsed
        };
        let (_, elapsed) = tokio::join!(plc, pc);
        assert!(elapsed < LIMIT, "布防输出确认用了 {elapsed:?}，轮询周期 {POLL_PERIOD:?}");
        bench.engine.disconnect().await;
    });
}

#[tokio::test]
async fn low_disk_during_part_still_releases_then_withholds_ready_until_space_recovers() {
    let mut bench = Bench::new("low-disk").await;
    let intake = bench.intake.clone();
    let order = bench.order(" a-4242 ~");
    let outcome = bench.run_burst_with(order, Scenario::Normal, |pulse| { intake.store(false, Ordering::SeqCst); all_cameras(pulse) }).await.unwrap().unwrap();
    assert_eq!((outcome.request_seq, outcome.result_code), (1, 1));
    assert_eq!(bench.session.phase(), SessionPhase::Idle);
    assert!(!bench.session.pending());
    assert_eq!(bench.line.read(tag::VISION_READY), 0, "释放时空间不足不能先置就绪再撤销");
    for _ in 0..3 {
        let event = bench.session.poll_gated(&bench.engine, false, true, true, false).await;
        assert!(matches!(event, SessionEvent::None), "{event:?}");
        assert_eq!(bench.line.read(tag::VISION_READY), 0);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    bench.intake.store(true, Ordering::SeqCst);
    let event = bench.session.poll_gated(&bench.engine, false, true, true, true).await;
    assert!(matches!(event, SessionEvent::None), "{event:?}");
    assert_eq!(bench.line.read(tag::VISION_READY), 1);
    assert_eq!(bench.run(Scenario::Normal).await.unwrap().request_seq, 2);
    bench.engine.disconnect().await;
}

#[tokio::test]
async fn low_disk_fault_reset_still_completes_but_keeps_ready_low_until_space_recovers() {
    let mut bench = Bench::new("low-disk-reset").await;
    bench.until_heartbeat_live().await;
    {
        let Bench { line, engine, session, .. } = &mut bench;
        let abort = AtomicBool::new(false);
        let pc = async {
            let until = Instant::now() + HANG_GUARD;
            while !matches!(session.poll_gated(engine, false, true, true, false).await, SessionEvent::Ready) {
                assert!(Instant::now() < until, "空间不足时复位未完成：{:?}", session.view());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            abort.store(true, Ordering::SeqCst);
        };
        let (ready, ()) = tokio::join!(line.ensure_ready(&TIMING, &abort), pc);
        assert!(ready.is_err(), "空间不足时复位后不能置 visionReady");
    }
    assert_eq!(bench.session.phase(), SessionPhase::Idle);
    assert!(!bench.session.pending());
    assert_eq!(bench.line.read(tag::VISION_READY), 0);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(matches!(bench.session.poll_gated(&bench.engine, false, true, true, false).await, SessionEvent::None));
    assert_eq!(bench.line.read(tag::VISION_READY), 0);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(matches!(bench.session.poll_gated(&bench.engine, false, true, true, true).await, SessionEvent::None));
    assert_eq!(bench.line.read(tag::VISION_READY), 1);
    assert_eq!(bench.run(Scenario::Normal).await.unwrap().request_seq, 1);
    bench.engine.disconnect().await;
}

async fn poll_until(bench: &mut Bench, reset: bool, devices: &Result<(), String>, what: &str, accept: impl Fn(&SessionEvent, &PlcSession) -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        let event = bench.session.poll_with_devices(&bench.engine, reset, devices, true, true).await;
        if accept(&event, &bench.session) { return; }
        assert!(Instant::now() < until, "{what}：{event:?} {:?}", bench.session.view());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// S-13 问题二：置故障输出确认不了时 PC 主动断开停心跳，过了 PLC 心跳失活阈值自动重连，重连后仍须明确复位。
#[tokio::test]
async fn pc_initiated_disconnect_reconnects_after_the_heartbeat_loss_window_and_waits_for_an_explicit_reset() {
    let mut bench = Bench::new("pc-disconnect").await;
    poll_until(&mut bench, true, &Ok(()), "首次复位", |event, _| matches!(event, SessionEvent::Ready)).await;
    for _ in 0..60 { bench._server.inject(s7_server::Fault::RejectWrite); }
    poll_until(&mut bench, true, &Ok(()), "置故障输出失败后应断开", |_, _| true).await;
    let disconnected = Instant::now();
    assert_eq!(bench.engine.status().state, LinkState::Disconnected, "{:?}", bench.session.view());
    bench._server.inject(s7_server::Fault::Clear);
    while bench.engine.status().state != LinkState::Connected {
        bench.session.poll_with_devices(&bench.engine, false, &Ok(()), true, true).await;
        if bench.engine.status().state != LinkState::Disconnected { break; }
        assert!(disconnected.elapsed() < HANG_GUARD, "主动断开后没有自动重连：{:?}", bench.session.view());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(disconnected.elapsed() >= Duration::from_millis(3000), "PLC 判 PC 心跳失活之前就重连了：{:?}", disconnected.elapsed());
    poll_until(&mut bench, false, &Ok(()), "重连后待复位", |_, session| session.phase() == SessionPhase::ResetRequired).await;
    for _ in 0..10 {
        bench.session.poll_with_devices(&bench.engine, false, &Ok(()), true, true).await;
        assert_ne!(bench.session.phase(), SessionPhase::Idle, "重连后不能自行回到就绪");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(bench.line.read(tag::VISION_READY), 0);
    poll_until(&mut bench, true, &Ok(()), "明确复位", |event, _| matches!(event, SessionEvent::Ready)).await;
    bench.engine.disconnect().await;
}

/// PR #134 审查：PLC 能读不能写时，自动重连只做一次，重连后仍确认不了置故障就停在断开，不再反复占用 PLC 连接；确认置故障后，下一次主动断开仍会自动重连。
#[tokio::test]
async fn persistent_write_rejection_reconnects_once_then_stays_disconnected_until_stop_is_confirmed() {
    let mut bench = Bench::new("reconnect-once").await;
    poll_until(&mut bench, true, &Ok(()), "首次复位", |event, _| matches!(event, SessionEvent::Ready)).await;
    for _ in 0..100_000 { bench._server.inject(s7_server::Fault::RejectWrite); }
    poll_until(&mut bench, true, &Ok(()), "置故障输出失败后应断开", |_, _| true).await;
    assert_eq!(bench.engine.status().state, LinkState::Disconnected, "{:?}", bench.session.view());
    // 到点自动重连一次；新连接上仍确认不了置故障，再次断开
    let (first, watch) = (bench.engine.status().since, Instant::now());
    let second = loop {
        bench.session.poll_with_devices(&bench.engine, false, &Ok(()), true, true).await;
        let status = bench.engine.status();
        if status.state == LinkState::Disconnected && status.since != first { break status.since; }
        assert!(watch.elapsed() < HANG_GUARD, "持续拒写时没有自动重连：{:?}", bench.session.view());
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // 此后过了自动重连的间隔也停在断开，不再重连
    let watch = Instant::now();
    while watch.elapsed() < RECONNECT_AFTER_STOP_WINDOW {
        bench.session.poll_with_devices(&bench.engine, false, &Ok(()), true, true).await;
        let status = bench.engine.status();
        assert!(status.state == LinkState::Disconnected && status.since == second, "持续拒写时应只自动重连一次：{:?}", bench.session.view());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    bench._server.inject(s7_server::Fault::Clear);
    bench.engine.connect().await;
    poll_until(&mut bench, true, &Ok(()), "人工连接后明确复位", |event, _| matches!(event, SessionEvent::Ready)).await;
    for _ in 0..60 { bench._server.inject(s7_server::Fault::RejectWrite); }
    poll_until(&mut bench, true, &Ok(()), "再次置故障输出失败后应断开", |_, _| true).await;
    assert_eq!(bench.engine.status().state, LinkState::Disconnected, "{:?}", bench.session.view());
    bench._server.inject(s7_server::Fault::Clear);
    let until = Instant::now() + HANG_GUARD;
    while bench.engine.status().state == LinkState::Disconnected {
        assert!(Instant::now() < until, "确认置故障后再次主动断开没有自动重连：{:?}", bench.session.view());
        bench.session.poll_with_devices(&bench.engine, false, &Ok(()), true, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bench.engine.disconnect().await;
}

/// D-CF-9：设备未就绪时复位被拒，提示要带上具体原因（如没有可开工配方）。
#[tokio::test]
async fn reset_refused_for_unready_devices_names_the_reason() {
    let mut bench = Bench::new("reset-reason").await;
    let reason = "没有一个配方开得了工（配方 A：所选生产版本没有不可变发布包）";
    poll_until(&mut bench, true, &Err(reason.to_string()), "复位应被拒并带上设备未就绪的原因",
        |event, _| matches!(event, SessionEvent::Fault(m) if m.contains(reason))).await;
    assert_eq!(bench.session.phase(), SessionPhase::Fault);
    bench.engine.disconnect().await;
}

#[tokio::test]
async fn new_connection_before_first_poll_waits_instead_of_faulting_and_disconnecting() {
    let mut bench = Bench::new_with("first-poll", |server| {
        server.inject(s7_server::Fault::Delay(Duration::from_millis(200)));
        server.inject(s7_server::Fault::Delay(Duration::from_millis(200)));
    }).await;
    let until = Instant::now() + HANG_GUARD;
    while bench.engine.status().state != LinkState::Connected {
        assert!(Instant::now() < until, "未连上离线 S7 服务");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(bench.engine.status().poll_count, 0);
    let event = bench.session.poll(&bench.engine, false, true).await;
    assert!(!matches!(event, SessionEvent::Fault(_)), "{event:?}");
    assert_eq!(bench.engine.status().state, LinkState::Connected);
    bench.engine.disconnect().await;
}

/// 旧拍照点配方（旧节拍保留）：传输丢帧按旧判定判缺帧 ERR；协议 6 起少发一次触发（CountMismatch）不再是握手故障，
/// 本件按实收帧照常判结果（缺帧 ERR）并回到空闲；ACK 序号不匹配是握手故障。段配方的同类工况见下方脉冲串用例。
#[tokio::test]
async fn legacy_recipe_line_scenarios_end_in_err_result_or_handshake_fault() {
    for (scenario, expected, phase) in [
        (Scenario::LostFrame, Some((90, fault::MISSING_FRAME)), SessionPhase::Idle),
        (Scenario::CountMismatch, Some((90, fault::MISSING_FRAME)), SessionPhase::Idle),
        (Scenario::AckMismatch, None, SessionPhase::Fault),
    ] {
        let mut bench = Bench::legacy(&format!("{scenario:?}")).await;
        let result = bench.run(scenario).await;
        assert_eq!(result.as_ref().ok().map(|o| (o.result_code, o.fault_code)), expected, "{scenario:?}: {result:?}");
        assert_eq!(bench.session.phase(), phase, "{scenario:?}: {:?}", bench.session.view());
        assert_eq!(bench.line.read(tag::PART_START), 0, "{scenario:?}");
        bench.engine.disconnect().await;
    }
}

// ───────────── 段配方：脉冲串（S7 闭环，计划 ΣN_exp 4、Σtol 1，三台相机共用触发） ─────────────

/// 规格 §2/§6：帧数在 ΣN_exp ± Σtol 内为正常，超出只记帧数异常、结论不变。虚拟 PLC 按带首尾 ±1 波动的剖面发脉冲串，
/// deviceTriggers 写实际脉冲数；PC 只按实收帧组数比较：容差内 → OK 且无帧数异常，容差外 → 仍 OK、标帧数异常。种子覆盖两类。
#[tokio::test]
async fn jittered_pulse_train_within_tolerance_is_ok_and_beyond_tolerance_only_alarms() {
    let mut bench = Bench::new("pulse-jitter").await;
    let (expected, tolerance) = (u64::from(bench.plan.shot_count), u64::from(bench.plan.tolerance_total));
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..8 {
        let mut order = bench.order(&format!("SN-JITTER-{seed}"));
        let profile = order.profile.as_mut().unwrap();
        (profile.head_tail_jitter, profile.seed) = (Scenario::PulseJitter.jitter().0, seed);
        let pulses: u64 = profile.counts().iter().sum();
        let outcome = bench.run_burst_with(order, Scenario::PulseJitter, all_cameras).await.unwrap().unwrap();
        let judged = bench.judged.take().unwrap();
        let beyond = pulses.abs_diff(expected) > tolerance;
        assert_eq!((outcome.result_code, judged.verdict, u64::from(judged.group_count), judged.flags.frame_count_anomaly), (1, Verdict::Ok, pulses, beyond),
            "种子 {seed}：{pulses} 个脉冲");
        assert_eq!(u64::from(bench.line.read(tag::DEVICE_TRIGGERS)), pulses, "种子 {seed}：deviceTriggers 写实际脉冲数");
        seen.insert(beyond);
    }
    assert_eq!(seen.len(), 2, "种子要同时覆盖容差内与容差外");
    bench.engine.disconnect().await;
}

/// 规格 §0/§3：三台相机之一缺帧时该组按其余相机照常判，组和整件标检测不完整，不影响 OK/NG，也不判 ERR。
/// 虚拟 PLC 按 cameraLost 工况只让第 2 台相机丢一帧。
#[tokio::test]
async fn single_camera_loss_yields_incomplete_ok_not_err() {
    let mut bench = Bench::new("camera-lost").await;
    let mut order = bench.order("SN-CAMERA-LOST");
    order.lost = Scenario::CameraLost.lost_frames(u64::from(bench.plan.shot_count));
    let outcome = bench.run_burst_with(order, Scenario::CameraLost, all_cameras).await.unwrap().unwrap();
    let judged = bench.judged.take().unwrap();
    assert_eq!((outcome.result_code, outcome.fault_code), (1, 0));
    assert_eq!((judged.verdict, judged.flags.incomplete), (Verdict::Ok, true));
    let region = &judged.regions[0];
    assert_eq!((region.camera_frames.as_slice(), region.incomplete_groups.as_slice()), ([4, 3, 4].as_slice(), [3].as_slice()), "只有第 2 台相机缺第 3 帧");
    assert_eq!(bench.session.phase(), SessionPhase::Idle);
    bench.engine.disconnect().await;
}

/// 规格 §3/§5：deviceTriggers 只作参考，计数不符不再是故障。countMismatch 工况下虚拟 PLC 多计 2 个脉冲（deviceTriggers
/// 比各相机帧数多 2）：三台相机都告警，不改结论、不升级故障，件后会话回到空闲。相差 1 不告警、相差 2 告警的阈值由
/// frame_group::tests::camera_count_differing_by_more_than_one_raises_loss_alarm 保护，这里不重复。
#[tokio::test]
async fn plc_counting_two_extra_pulses_alarms_every_camera_without_fault() {
    let mut bench = Bench::new("plc-counts-2-extra").await;
    let order = bench.order("SN-GAP");
    let outcome = bench.run_burst_with(order, Scenario::CountMismatch, all_cameras).await.unwrap().unwrap();
    let judged = bench.judged.take().unwrap();
    let alarms: Vec<LossAlarm> = (0..3).map(|camera| LossAlarm { camera, count: 4, reason: LossReason::Triggers { device_triggers: 6 } }).collect();
    assert_eq!(judged.alarms, alarms);
    assert_eq!((outcome.result_code, judged.verdict, judged.fault_code), (1, Verdict::Ok, 0));
    assert_eq!(bench.session.phase(), SessionPhase::Idle, "{:?}", bench.session.view());
    bench.engine.disconnect().await;
}

/// 规格 §0、默认做法 3：PLC 提前置 partEnd（提前终止）时 PC 用已收到的帧照常判 OK/NG；帧组数 < ΣN_exp − Σtol
/// 时标疑似提前终止，同时是帧数异常。
#[tokio::test]
async fn early_part_end_is_judged_with_short_part_flag_and_frame_count_anomaly() {
    let mut bench = Bench::new("early-end").await;
    let early = Scenario::EarlyEnd.early_end_after(u64::from(bench.plan.shot_count), bench.plan.tolerance_total);
    let order = PartOrder { early_end_after: early, ..bench.order("SN-EARLY-END") };
    let outcome = bench.run_burst_with(order, Scenario::EarlyEnd, all_cameras).await.unwrap().unwrap();
    let judged = bench.judged.take().unwrap();
    assert_eq!(bench.line.read(tag::DEVICE_TRIGGERS), 2, "PLC 只发了 2 个脉冲就结束");
    assert_eq!((outcome.result_code, judged.verdict, judged.group_count), (1, Verdict::Ok, 2));
    assert_eq!((judged.flags.short_part, judged.flags.frame_count_anomaly), (true, true));
    bench.engine.disconnect().await;
}

/// D-SB-28：段配方把样本集 v1 解释为每台相机按清单顺序的帧序列——第 seq 个脉冲取各相机自己的第 seq 帧；各相机帧数可以不等，
/// 帧数少的相机在后面的帧组缺帧（检测不完整，不判 ERR）。脉冲数取样本帧数最多的那台，按生产的取图函数取图核对像素。
#[tokio::test]
async fn sample_set_line_serves_each_camera_its_ordered_frames() {
    use super::sample_set::{fixture::Fixture, SampleSet};
    let frames = [4u8, 4, 3];
    let fixture = Fixture::burst(&frames);
    let set = Arc::new(SampleSet::load(&fixture.0).unwrap());
    let recipe = crate::recipe::samples_burst().build().unwrap();
    let feeds = sample_feeds(&recipe, &set).unwrap();
    let mut bench = Bench::new("burst-samples").await;
    let profile = burst_profile(&recipe, &feeds, Scenario::Normal, BENCH_HZ, 0);
    let order = PartOrder { profile: Some(profile), ..bench.order("SN-SAMPLES") };
    let fetched = Mutex::new(Vec::new());
    let outcome = bench.run_burst_with(order, Scenario::Normal, |pulse| feeds.iter().filter_map(|feed| {
        let Some(TriggerPayload::Sample(target)) = sample_frame_payload(&set, "p02", feed.frames.as_ref().unwrap(), pulse.seq) else { return None };
        fetched.lock().unwrap().push((feed.camera_idx, pulse.seq, target.images(&feed.camera, 1).unwrap()[0].pixels[0]));
        Some(feed.camera_idx)
    }).collect()).await.unwrap().unwrap();
    let expected: Vec<(usize, u64, u8)> = (1..=4u8).flat_map(|seq| (0..3).filter(move |&c| seq <= frames[c])
        .map(move |c| (c, u64::from(seq), Fixture::burst_pixel(2, seq, c as u8 + 1)))).collect();
    assert_eq!(fetched.into_inner().unwrap(), expected);
    let judged = bench.judged.take().unwrap();
    assert_eq!((outcome.result_code, judged.group_count, judged.flags.incomplete), (1, 4, true));
    bench.engine.disconnect().await;
}

/// 规格 §0：相机上限 66 fps，离线触发间隔最短 15 ms；上限 5000 ms 不变。
#[test]
fn pace_interval_of_15ms_is_accepted_and_14ms_rejected() {
    for (interval, accepted) in [(14, false), (15, true), (5000, true), (5001, false)] {
        assert_eq!(Pace { shot_interval_ms: interval, pause_ms: 0 }.validate().is_ok(), accepted, "{interval} ms");
    }
}

/// 虚拟 PLC 像操作员一样反复按复位，直到视觉就绪或 PC 侧收敛：复位沿早于会话确认心跳翻转会被拒，单次脉冲不保证成功。
async fn press_reset_until_ready(line: &Line, abort: &AtomicBool) -> Result<(), String> {
    let retry = Timing { ready_timeout: Duration::from_secs(1), ..TIMING };
    loop {
        match line.ensure_ready(&retry, abort).await {
            Err(_) if !abort.load(Ordering::SeqCst) => continue,
            result => return result,
        }
    }
}

/// 采集中途崩溃后重启：握手日志按事务来源决定能否由离线虚拟 PLC 复位（S-13）。
#[tokio::test]
async fn crash_left_pending_is_reset_by_the_virtual_plc_only_when_it_came_from_offline() {
    for origin in [Origin::Offline, Origin::Real] {
        let mut bench = Bench::new(&format!("crash-{origin:?}")).await;
        bench.session.set_origin(origin);
        {
            let order = bench.order("SN-CRASH");
            let Bench { line, engine, session, plan, .. } = &mut bench;
            let abort = AtomicBool::new(false);
            let plc = async { press_reset_until_ready(line, &abort).await?; line.run_burst(&TIMING, &order, Scenario::Normal, |_| {}, &abort).await };
            let pc = async {
                let until = Instant::now() + Duration::from_secs(10);
                while !matches!(session.poll_gated(engine, false, true, true, true).await, SessionEvent::Start(_)) {
                    assert!(Instant::now() < until, "{origin:?}：未收到开始请求：{:?}", session.view());
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                session.arm(engine, plan).await.unwrap();
                abort.store(true, Ordering::SeqCst);
            };
            let (aborted, ()) = tokio::join!(plc, pc);
            assert!(aborted.is_err());
        }
        let journal = bench.directory.join("session.json");
        let mut restarted = PlcSession::open(journal.clone());
        let expected = if origin == Origin::Offline { PendingKind::Offline } else { PendingKind::Blocking };
        assert_eq!(restarted.pending_kind(), expected, "{origin:?}");
        if origin == Origin::Real {
            let mut legacy: serde_json::Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
            legacy["pending"].as_object_mut().unwrap().remove("origin");
            let path = bench.directory.join("legacy.json");
            std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
            assert_eq!(PlcSession::open(path).pending_kind(), PendingKind::Blocking, "未记来源的旧日志不能当离线事务处理");
        }
        restarted.set_origin(Origin::Offline);
        let Bench { line, engine, .. } = &bench;
        let abort = AtomicBool::new(false);
        let pc = async {
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                let event = restarted.poll_gated(engine, false, true, true, true).await;
                if matches!(event, SessionEvent::Ready) { break; }
                // 真机事务：心跳已活、设备就绪时虚拟 PLC 按复位仍被拒，即离线复位不了它
                if origin == Origin::Real && matches!(event, SessionEvent::Fault(_)) && restarted.heartbeat_live() { break; }
                assert!(Instant::now() < until, "{origin:?}：复位未收敛：{:?}", restarted.view());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            abort.store(true, Ordering::SeqCst);
        };
        let _ = tokio::join!(press_reset_until_ready(line, &abort), pc);
        if origin == Origin::Offline {
            assert_eq!(restarted.phase(), SessionPhase::Idle);
            assert!(!restarted.pending());
            let audit = std::fs::read_to_string(journal.with_extension("audit.jsonl")).unwrap();
            assert!(audit.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
                .any(|r| r["action"] == "resetCompleted" && r["pending"]["origin"] == "offline" && r["pending"]["request"]["sn"] == "SN-CRASH"));
        } else {
            assert_eq!(restarted.phase(), SessionPhase::Fault);
            assert!(restarted.pending(), "真机事务不能被离线复位清掉");
        }
        bench.engine.disconnect().await;
    }
}

#[tokio::test]
async fn stop_during_inter_part_pause_does_not_start_another_part() {
    let offline = OfflineHost::default();
    assert!(!offline.continue_after_pause(false, Duration::ZERO).await);
    assert!(offline.continue_after_pause(true, Duration::ZERO).await);
    let (start_next, ()) = tokio::join!(
        biased;
        offline.continue_after_pause(true, Duration::from_millis(20)),
        async { offline.stop.store(true, Ordering::SeqCst); },
    );
    assert!(!start_next, "stop accepted between parts must prevent the next part");
}

/// 心跳故障后视觉还没观察到新的心跳翻转时第一次 faultReset 会被拒；虚拟 PLC 要再按复位，不能一直等到超时。
#[tokio::test]
async fn virtual_plc_presses_reset_again_after_a_reset_refused_before_the_heartbeat_recovered() {
    let mut bench = Bench::new_with("reset-retry", |server| server.inject(s7_server::Fault::HeartbeatStop)).await;
    let Bench { line, engine, session, _server: server, .. } = &mut bench;
    let abort = AtomicBool::new(false);
    let refused = AtomicBool::new(false);
    let pc = async {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let event = session.poll(engine, false, true).await;
            if matches!(event, SessionEvent::Ready) { return; }
            if matches!(event, SessionEvent::Fault(_)) && !session.heartbeat_live() && !refused.swap(true, Ordering::SeqCst) {
                server.inject(s7_server::Fault::HeartbeatResume);
            }
            assert!(Instant::now() < until, "复位未完成：{:?}", session.view());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let (ready, ()) = tokio::join!(line.ensure_ready(&TIMING, &abort), pc);
    assert!(refused.load(Ordering::SeqCst), "第一次复位应因心跳未翻转被拒");
    assert!(ready.is_ok(), "{ready:?}");
    bench.engine.disconnect().await;
}

/// 旧拍照点配方（旧节拍保留）：第 k 次触发按 件 / 拍照点 / 视角取样本图，按 partSn 认领样本件。
#[tokio::test]
async fn sample_set_line_fetches_each_trigger_by_part_shot_and_view_and_claims_the_sample_by_part_sn() {
    use super::sample_set::{fixture::Fixture, SampleLabel, SampleSet};
    let fixture = Fixture::new(&[1, 2, 3]);
    let set = Arc::new(SampleSet::load(&fixture.0).unwrap());
    let shots: Vec<String> = set.manifest().shots.iter().map(|s| s.shot_id.clone()).collect();
    let fetch = |part: &str, k: usize| match sample_payload(&set, part, &shots[k]) {
        TriggerPayload::Sample(target) => target.images("cam1", 3),
        TriggerPayload::Sim(_) | TriggerPayload::Burst(_) => unreachable!(),
    };
    let pixels = |images: &[Arc<crate::frame::FrameImage>]| images.iter().map(|i| i.pixels[0]).collect::<Vec<_>>();
    let offline = OfflineHost::default();
    offline.enabled.store(true, Ordering::SeqCst);
    let mut bench = Bench::legacy("samples").await;
    for (requested, part, number, sn, expected) in [
        (Some("p02"), "p02", 2, 501, (1, 0, Verdict::Ok)),
        (None, "p01", 1, 502, (1, 0, Verdict::Ok)),
    ] {
        let sn = OfflineHost::sn_text(sn);
        assert_eq!(offline.pick_part(&set, requested).unwrap(), part);
        offline.begin_run(&set, part, &sn, RunPurpose::Production);
        let received = Mutex::new(Vec::new());
        let result = bench.run_with(sn.clone(), Scenario::Normal, |k, lost| {
            let images = (!lost).then(|| fetch(part, k).ok()).flatten();
            received.lock().unwrap().push((k, images.as_deref().map(pixels)));
            images.is_some()
        }).await;
        offline.finish_run(&sn, &result);
        let outcome = result.unwrap();
        assert_eq!((&outcome.sn, outcome.result_code, outcome.fault_code), (&sn, expected.0, expected.1), "{part}");
        for (k, images) in received.into_inner().unwrap() {
            let wanted = Some((1..=3).map(|v| Fixture::pixel(number, k as u8 + 1, v)).collect::<Vec<_>>());
            assert_eq!(images, wanted, "{part} k={k}");
        }
        let reference = offline.claim(&sn, &format!("cycle-{sn}")).unwrap();
        assert_eq!((reference.part_id.as_str(), reference.label), (part, if part == "p01" { SampleLabel::Ok } else { SampleLabel::Ng }));
        let run = offline.status().recent.into_iter().find(|r| r.part_sn == sn).unwrap();
        assert_eq!((run.verdict, run.cycle_id), (Some(expected.2), Some(format!("cycle-{sn}"))));
    }
    for k in [3, 0, 2, 1] {
        assert_eq!(pixels(&fetch("p01", k).unwrap()), (1..=3).map(|v| Fixture::pixel(1, k as u8 + 1, v)).collect::<Vec<_>>(), "乱序触发 k={k}");
    }
    bench.engine.disconnect().await;
}

#[test]
fn real_part_with_the_same_part_sn_after_leaving_offline_gets_no_sample_reference() {
    use super::sample_set::{fixture::Fixture, SampleSet};
    let fixture = Fixture::new(&[1]);
    let set = SampleSet::load(&fixture.0).unwrap();
    let offline = OfflineHost::default();
    offline.enabled.store(true, Ordering::SeqCst);
    offline.begin_run(&set, "p01", "SN-777", RunPurpose::Production);
    assert!(offline.deactivate().is_none());
    assert!(offline.claim("SN-777", "real-cycle").is_none());
    assert!(offline.status().recent.is_empty());
    offline.begin_run(&set, "p01", "SN-778", RunPurpose::Production);
    assert!(offline.claim("SN-778", "real-cycle").is_none(), "未启用离线时不认领样本件");
}

#[test]
fn restoring_the_real_plc_reconnects_only_after_a_valid_config_was_applied_and_parks_on_failure() {
    let failed = Err("配置无效".to_string());
    for (applied, reconnect, valid, expected) in [
        (&Ok(()), true, true, RestorePlan::Reconnect),
        (&Ok(()), false, true, RestorePlan::StayDisconnected),
        (&Ok(()), true, false, RestorePlan::StayDisconnected),
        (&failed, true, true, RestorePlan::Park("配置无效".into())),
    ] {
        assert_eq!(restore_plan(applied, reconnect, valid), expected, "{applied:?} {reconnect} {valid}");
    }
}

/// plc.db 的握手信号变化带上当时的 requestSeq，按件追溯 PLC 日志（日志库本身没有 cycleId）。
#[tokio::test]
async fn plc_log_value_changes_carry_the_request_seq_of_their_part() {
    let mut bench = Bench::new("log-context").await;
    let until = Instant::now() + Duration::from_secs(8);
    while !matches!(bench.session.poll(&bench.engine, true, true).await, SessionEvent::Ready) {
        assert!(Instant::now() < until, "复位未完成：{:?}", bench.session.view());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for seq in [1, 2] { assert_eq!(bench.run(Scenario::Normal).await.unwrap().request_seq, seq); }
    let armed = crate::handshake::Contract::validate(&bench.engine.config()).unwrap().point_id(tag::ARMED).unwrap().to_owned();
    let until = Instant::now() + Duration::from_secs(5);
    for seq in [1, 2] {
        let query = ly_plc::LogQuery { point_id: Some(armed.clone()), context: Some(format!("requestSeq={seq}")), ..Default::default() };
        loop {
            let page = bench.engine.logbook().query(&query).unwrap();
            let changed = |to: &str| page.items.iter().any(|e| e.new_value.as_deref() == Some(to));
            if changed("true") && changed("false") { break; }
            assert!(Instant::now() < until, "requestSeq={seq} 的 armed 置位与撤销没有带上该件的上下文：{:?}", page.items);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    bench.engine.disconnect().await;
}

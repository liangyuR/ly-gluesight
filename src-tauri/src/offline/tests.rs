use super::line::{Layout, Line, Outcome, PartOrder, Timing};
use super::s7_server::S7Server;
use super::*;
use crate::judge::fault;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::plc_session::{Origin, PendingKind, PlcSession, SessionEvent, SessionPhase};
use crate::inspection::tag;
use ly_plc::{EventSink, Logbook, PlcEngine};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const HANG_GUARD: Duration = Duration::from_secs(120);

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

struct Bench {
    _server: S7Server,
    line: Line,
    engine: PlcEngine,
    session: PlcSession,
    plan: PlcPlan,
    intake: Arc<AtomicBool>,
    directory: std::path::PathBuf,
}

impl Bench {
    async fn new(name: &str) -> Self {
        Self::new_with(name, |_| {}).await
    }

    async fn new_with(name: &str, before_connect: impl FnOnce(&S7Server)) -> Self {
        let mut config = crate::inspection::default_plc_config();
        let layout = Layout::from_config(&config).unwrap();
        let memory = layout.memory();
        let server = S7Server::start(memory.clone(), Some((layout.heartbeat(), Duration::from_millis(100)))).await.unwrap();
        config.connection.host = "127.0.0.1".into();
        config.connection.port = server.port();
        config.connection.timeout_ms = 5000;
        config.connection.poll_interval_ms = 20;
        config.connection.reconnect_interval_ms = 200;
        config.heartbeat.interval_ms = 100;
        let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/offline-tests")
            .join(format!("{name}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&directory).unwrap();
        let sink: EventSink = Arc::new(|_| {});
        let engine = PlcEngine::new(config, Arc::new(Logbook::open(&directory.join("plc.db"), sink.clone(), 1).unwrap()), sink);
        before_connect(&server);
        engine.connect().await;
        let plan = PlcPlan::compile("part-A".into(), 7, vec!["cam1".into()], (1..=4).map(|i| PlanShot {
            shot_id: format!("P{i}"), pose_id: format!("P{i}"), camera_ids: vec!["cam1".into()],
        }).collect()).unwrap();
        Self { _server: server, line: Line::new(memory, layout), engine, session: PlcSession::open(directory.join("session.json")), plan,
            intake: Arc::new(AtomicBool::new(true)), directory }
    }

    async fn run(&mut self, scenario: Scenario) -> Result<Outcome, String> {
        self.run_with(" a-4242 ~".into(), scenario, |_, lost| !lost).await
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

    /// 虚拟 PLC 跑一件，PC 侧按生产节拍的顺序驱动 S7 会话：开始 → 布防 → partEnd 后按实收帧判定回写。
    /// `deliver(k, lost)` 模拟相机侧：返回这次触发是否真的交付了一帧。
    async fn run_with(&mut self, sn: String, scenario: Scenario, mut deliver: impl FnMut(usize, bool) -> bool) -> Result<Outcome, String> {
        self.until_heartbeat_live().await;
        let Self { line, engine, session, plan, intake, .. } = self;
        let delivered = Mutex::new(Vec::new());
        let finished = AtomicBool::new(false);
        let abort = AtomicBool::new(false);
        let order = PartOrder { sn, product_code: 3, plan_version: plan.plan_version,
            shot_count: plan.shot_count, device_shots: plan.device_shots };
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
                    SessionEvent::End(_) => {
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

#[tokio::test]
async fn low_disk_during_part_still_releases_then_withholds_ready_until_space_recovers() {
    let mut bench = Bench::new("low-disk").await;
    let intake = bench.intake.clone();
    let outcome = bench.run_with(" a-4242 ~".into(), Scenario::Normal, |_, lost| { intake.store(false, Ordering::SeqCst); !lost }).await.unwrap();
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
        assert!(disconnected.elapsed() < Duration::from_secs(6), "主动断开后没有自动重连：{:?}", bench.session.view());
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
    let (mut reconnects, mut since, watch) = (0, bench.engine.status().since, Instant::now());
    while watch.elapsed() < Duration::from_secs(9) {
        bench.session.poll_with_devices(&bench.engine, false, &Ok(()), true, true).await;
        let status = bench.engine.status();
        if status.state == LinkState::Disconnected && status.since != since { reconnects += 1; since = status.since; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(reconnects, 1, "持续拒写时应只自动重连一次：{:?}", bench.session.view());
    assert_eq!(bench.engine.status().state, LinkState::Disconnected, "{:?}", bench.session.view());

    bench._server.inject(s7_server::Fault::Clear);
    bench.engine.connect().await;
    poll_until(&mut bench, true, &Ok(()), "人工连接后明确复位", |event, _| matches!(event, SessionEvent::Ready)).await;
    for _ in 0..60 { bench._server.inject(s7_server::Fault::RejectWrite); }
    poll_until(&mut bench, true, &Ok(()), "再次置故障输出失败后应断开", |_, _| true).await;
    assert_eq!(bench.engine.status().state, LinkState::Disconnected, "{:?}", bench.session.view());
    bench._server.inject(s7_server::Fault::Clear);
    let until = Instant::now() + Duration::from_secs(6);
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
    let devices = Err("没有一个配方开得了工（配方 A：所选生产版本没有不可变发布包）".to_string());
    poll_until(&mut bench, true, &devices, "复位应被拒", |event, _| matches!(event, SessionEvent::Fault(m) if m.contains("设备尚未就绪"))).await;
    let message = bench.session.view().message.unwrap();
    assert!(message.contains("设备尚未就绪") && message.contains("所选生产版本没有不可变发布包"), "{message}");
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

#[tokio::test]
async fn offline_line_scenarios_end_in_err_result_or_handshake_fault() {
    for (scenario, expected, phase) in [
        (Scenario::LostFrame, Some((90, fault::MISSING_FRAME)), SessionPhase::Idle),
        (Scenario::CountMismatch, None, SessionPhase::Fault),
        (Scenario::AckMismatch, None, SessionPhase::Fault),
    ] {
        let mut bench = Bench::new(&format!("{scenario:?}")).await;
        let result = bench.run(scenario).await;
        assert_eq!(result.as_ref().ok().map(|o| (o.result_code, o.fault_code)), expected, "{scenario:?}: {result:?}");
        assert_eq!(bench.session.phase(), phase, "{scenario:?}: {:?}", bench.session.view());
        assert_eq!(bench.line.read(tag::PART_START), 0, "{scenario:?}");
        bench.engine.disconnect().await;
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
            let Bench { line, engine, session, plan, .. } = &mut bench;
            let abort = AtomicBool::new(false);
            let order = PartOrder { sn: "SN-CRASH".into(), product_code: 3, plan_version: plan.plan_version,
                shot_count: plan.shot_count, device_shots: plan.device_shots };
            let plc = async { press_reset_until_ready(line, &abort).await?; line.run_part(&TIMING, &order, Scenario::Normal, |_, _| {}, &abort).await };
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
                if matches!(restarted.poll_gated(engine, false, true, true, true).await, SessionEvent::Ready) { break; }
                if restarted.view().message.is_some_and(|m| m.contains("不能由离线虚拟 PLC 复位")) { break; }
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

#[tokio::test]
async fn sample_set_line_fetches_each_trigger_by_part_shot_and_view_and_claims_the_sample_by_part_sn() {
    use super::sample_set::{fixture::Fixture, SampleLabel, SampleSet};
    let fixture = Fixture::new(&[1, 2, 3]);
    let set = Arc::new(SampleSet::load(&fixture.0).unwrap());
    let shots: Vec<String> = set.manifest().shots.iter().map(|s| s.shot_id.clone()).collect();
    let fetch = |part: &str, k: usize| match sample_payload(&set, part, &shots[k]) {
        TriggerPayload::Sample(target) => target.images("cam1", 3),
        TriggerPayload::Sim(_) => unreachable!(),
    };
    let pixels = |images: &[Arc<crate::frame::FrameImage>]| images.iter().map(|i| i.pixels[0]).collect::<Vec<_>>();
    let offline = OfflineHost::default();
    offline.enabled.store(true, Ordering::SeqCst);
    let mut bench = Bench::new("samples").await;
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

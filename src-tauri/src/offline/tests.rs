use super::line::{Layout, Line, Outcome, PartOrder, Timing};
use super::s7_server::S7Server;
use super::*;
use crate::judge::fault;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::plc_session::{PlcSession, SessionEvent, SessionPhase};
use crate::inspection::tag;
use ly_plc::{EventSink, Logbook, PlcEngine};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const TIMING: Timing = Timing {
    poll: Duration::from_millis(5),
    shot_interval: Duration::from_millis(20),
    end_delay: Duration::from_millis(20),
    ack_delay: Duration::from_millis(20),
    settle: Duration::from_millis(150),
    arm_timeout: Duration::from_secs(5),
    result_timeout: Duration::from_secs(5),
    ready_timeout: Duration::from_secs(8),
    release_timeout: Duration::from_secs(5),
};

struct Bench {
    _server: S7Server,
    line: Line,
    engine: PlcEngine,
    session: PlcSession,
    plan: PlcPlan,
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
        config.connection.timeout_ms = 250;
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
        let plan = PlcPlan::compile("part-A".into(), 7, "cam1".into(), (1..=4).map(|i| PlanShot {
            shot_id: format!("P{i}"), pose_id: format!("P{i}"), camera_id: "cam1".into(),
        }).collect()).unwrap();
        Self { _server: server, line: Line::new(memory, layout), engine, session: PlcSession::open(directory.join("session.json")), plan }
    }

    async fn run(&mut self, scenario: Scenario) -> Result<Outcome, String> {
        self.run_with(4242, scenario, |_, lost| !lost).await
    }

    /// 虚拟 PLC 跑一件，PC 侧按生产节拍的顺序驱动 S7 会话：开始 → 布防 → partEnd 后按实收帧判定回写。
    /// `deliver(k, lost)` 模拟相机侧：返回这次触发是否真的交付了一帧。
    async fn run_with(&mut self, sn: u32, scenario: Scenario, mut deliver: impl FnMut(usize, bool) -> bool) -> Result<Outcome, String> {
        let Self { line, engine, session, plan, .. } = self;
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
            let until = Instant::now() + Duration::from_secs(15);
            while !finished.load(Ordering::SeqCst) || !matches!(session.phase(), SessionPhase::Idle | SessionPhase::Fault) {
                assert!(Instant::now() < until, "会话未收敛：{:?}", session.view());
                match session.poll(engine, false, true).await {
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
        assert_eq!(outcome, Outcome { request_seq: seq, sn: 4242, result_code: 1, fault_code: 0 });
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
async fn new_connection_before_first_poll_waits_instead_of_faulting_and_disconnecting() {
    let mut bench = Bench::new_with("first-poll", |server| {
        server.inject(s7_server::Fault::Delay(Duration::from_millis(200)));
        server.inject(s7_server::Fault::Delay(Duration::from_millis(200)));
    }).await;
    let until = Instant::now() + Duration::from_secs(3);
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
async fn sample_set_line_fetches_each_trigger_by_part_shot_and_view_and_missing_image_is_a_missing_frame() {
    use super::sample_set::{fixture::Fixture, SampleLabel, SampleSet};
    let fixture = Fixture::new(&[1, 2, 3]);
    std::fs::remove_file(fixture.0.join("parts/p02/S002_cam1_v2.png")).unwrap();
    let set = Arc::new(SampleSet::load(&fixture.0).unwrap());
    let shots: Vec<String> = set.manifest().shots.iter().map(|s| s.shot_id.clone()).collect();
    let fetch = |part: &str, k: usize| match sample_payload(&set, part, &shots[k]) {
        TriggerPayload::Sample(target) => target.images("cam1", 3),
        TriggerPayload::Sim(_) => unreachable!(),
    };
    let pixels = |images: &[Arc<crate::frame::FrameImage>]| images.iter().map(|i| i.pixels[0]).collect::<Vec<_>>();
    let offline = OfflineHost::default();
    let mut bench = Bench::new("samples").await;
    for (requested, part, number, sn, expected) in [
        (Some("p02"), "p02", 2, 501, (90, fault::MISSING_FRAME, Verdict::ErrInspect)),
        (None, "p01", 1, 502, (1, 0, Verdict::Ok)),
    ] {
        assert_eq!(offline.pick_part(&set, requested).unwrap(), part);
        offline.begin_run(&set, part, sn, RunPurpose::Production);
        let received = Mutex::new(Vec::new());
        let result = bench.run_with(sn, Scenario::Normal, |k, lost| {
            let images = (!lost).then(|| fetch(part, k).ok()).flatten();
            received.lock().unwrap().push((k, images.as_deref().map(pixels)));
            images.is_some()
        }).await;
        offline.finish_run(sn, &result);
        let outcome = result.unwrap();
        assert_eq!((outcome.sn, outcome.result_code, outcome.fault_code), (sn, expected.0, expected.1), "{part}");
        for (k, images) in received.into_inner().unwrap() {
            let wanted = (part != "p02" || k != 1).then(|| (1..=3).map(|v| Fixture::pixel(number, k as u8 + 1, v)).collect::<Vec<_>>());
            assert_eq!(images, wanted, "{part} k={k}");
        }
        let reference = offline.claim(sn, &format!("cycle-{sn}")).unwrap();
        assert_eq!((reference.part_id.as_str(), reference.label), (part, if part == "p01" { SampleLabel::Ok } else { SampleLabel::Ng }));
        let run = offline.status().recent.into_iter().find(|r| r.part_sn == sn).unwrap();
        assert_eq!((run.verdict, run.cycle_id), (Some(expected.2), Some(format!("cycle-{sn}"))));
    }
    for k in [3, 0, 2, 1] {
        assert_eq!(pixels(&fetch("p01", k).unwrap()), (1..=3).map(|v| Fixture::pixel(1, k as u8 + 1, v)).collect::<Vec<_>>(), "乱序触发 k={k}");
    }
    bench.engine.disconnect().await;
}

use super::line::{Layout, Line, Outcome, PartOrder, Pulse, PulseProfile, Timing};
use super::s7_server::S7Server;
use super::test_dir::TestDir;
use super::*;
use crate::burst_judge::BurstJudgement;
use crate::cycle::tests::{burst_frame, judge_burst_at_drain_deadline};
use crate::inspection::tag;
use crate::plc_plan::PlcPlan;
use crate::plc_session::{PlcSession, SessionEvent, SessionPhase};
use ly_plc::{EventSink, Logbook, PlcEngine};
use std::time::Instant;

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

/// 测试台的脉冲频率：间隔 10 ms，一件几十毫秒跑完。
const BENCH_HZ: f64 = 100.0;

struct Bench {
    _server: S7Server,
    line: Line,
    engine: PlcEngine,
    session: PlcSession,
    plan: PlcPlan,
    intake: Arc<AtomicBool>,
    _directory: TestDir,
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

    /// 段计划的一件：一个直线段、名义帧数 = ΣN_exp，不波动、不丢帧。
    fn order(&self, sn: &str) -> PartOrder {
        let profile = PulseProfile {
            hz: BENCH_HZ,
            segments: vec![(
                crate::plan::SegmentKind::Straight,
                u32::from(self.plan.shot_count),
            )],
            head_tail_jitter: 0,
            corner_jitter: 0,
            seed: 0,
        };
        PartOrder {
            profile: Some(profile),
            ..PartOrder::new(sn.into(), 3, self.plan.shot_count)
        }
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
        Self {
            _server: server,
            line: Line::new(memory, layout),
            engine,
            session: PlcSession::open(directory.join("session.json")),
            plan,
            intake: Arc::new(AtomicBool::new(true)),
            _directory: directory,
            judged: None,
        }
    }

    /// 段配方：虚拟 PLC 按 `order` 的脉冲剖面跑一件，PC 侧按生产节拍驱动 S7 会话：开始 → 布防 → 运动结束后用实收帧
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
                    SessionEvent::End(_) => {
                        let judgement =
                            judge_burst_at_drain_deadline(delivered.lock().unwrap().clone(), None);
                        session
                            .report(engine, judgement.plc_code, judgement.fault_code)
                            .await
                            .unwrap();
                        *judged = Some(judgement);
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::join!(plc, pc).0
    }

    /// 会话还没观察到 PLC 心跳变化时，虚拟 PLC 的复位请求只能等着；先让会话看到心跳，复位时机就只取决于会话状态。
    /// 不取决于机器多忙。
    async fn until_heartbeat_live(&mut self) {
        let until = Instant::now() + HANG_GUARD;
        while !self.session.heartbeat_live() {
            assert!(Instant::now() < until, "会话长时间没有观察到 PLC 心跳翻转：{:?}", self.session.view());
            self.session.poll_gated(&self.engine, false, true, true, self.intake.load(Ordering::SeqCst)).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

}

// ───────────── 段配方：脉冲串（S7 闭环，计划 ΣN_exp 4、Σtol 1，三台相机共用触发） ─────────────

/// 虚拟 PLC 与会话按协议 7 闭环：启动时 PC 未就绪，PLC 现场复位让 PC 就绪；一件走完 PLC 回 0、PC 回 10。
#[tokio::test]
async fn simulator_resets_pc_then_part_completes_back_to_idle() {
    let mut bench = Bench::new("normal").await;
    let order = bench.order("SN-OK");
    let outcome = bench
        .run_burst_with(order, Scenario::Normal, all_cameras)
        .await
        .unwrap()
        .unwrap();
    let judged = bench.judged.take().unwrap();
    assert_eq!(
        (outcome.result_code, outcome.sn.as_str(), judged.verdict),
        (1, "SN-OK", Verdict::Ok)
    );
    assert_eq!(
        bench.session.phase(),
        SessionPhase::Idle,
        "{:?}",
        bench.session.view()
    );
    assert_eq!(
        (
            bench.line.read(tag::PLC_STATE),
            bench.line.read(tag::PC_STATE)
        ),
        (0, 10)
    );
    bench.engine.disconnect().await;
}

/// PLC 核对回显不符置 90：会话进入故障并保留事务；下一件前虚拟 PLC 现场复位，会话清事务回就绪。
#[tokio::test]
async fn simulator_echo_mismatch_faults_then_plc_reset_restores_ready() {
    let mut bench = Bench::new("echo-mismatch").await;
    let order = bench.order("SN-ACK");
    assert!(bench
        .run_burst_with(order, Scenario::AckMismatch, all_cameras)
        .await
        .is_err());
    assert_eq!(
        bench.session.phase(),
        SessionPhase::Fault,
        "{:?}",
        bench.session.view()
    );
    assert!(bench.session.pending());
    let order = bench.order("SN-NEXT");
    let outcome = bench
        .run_burst_with(order, Scenario::Normal, all_cameras)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.sn, "SN-NEXT");
    assert_eq!(
        bench.session.phase(),
        SessionPhase::Idle,
        "{:?}",
        bench.session.view()
    );
    assert!(!bench.session.pending());
    bench.engine.disconnect().await;
}

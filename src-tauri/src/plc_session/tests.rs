use super::*;
use std::sync::{mpsc, Arc};
use ly_plc::{EventSink, Logbook};
use serde_json::Value;
use crate::offline::line::Layout;
use crate::offline::s7_server::{Fault, S7Server, SharedMemory, WriteTarget};
use crate::offline::test_dir::TestDir;

/// PC 区（DB2）上的 pcState（DBW4）
const PC_STATE_WORD: WriteTarget = WriteTarget {
    db: 2,
    byte: 4,
    bit: None,
};

/// 进程内 S7 服务扮演 PLC：测试直接改它的 DB，会话经真实 S7 通讯读写同一块内存。
struct TestPlc {
    server: S7Server,
    memory: SharedMemory,
    layout: Layout,
}

impl TestPlc {
    async fn start() -> Self {
        let layout = Layout::from_config(&crate::inspection::default_plc_config()).unwrap();
        let memory = layout.memory();
        let server = S7Server::start(memory.clone(), Some((layout.heartbeat(), Duration::from_millis(100)))).await.unwrap();
        Self { server, memory, layout }
    }

    fn get(&self, field: &str) -> u32 { self.layout.get(&self.memory.lock().unwrap(), field) }

    fn pc(&self) -> u16 {
        self.get(tag::PC_STATE) as u16
    }

    fn text(&self, field: &str) -> String { self.layout.get_text(&self.memory.lock().unwrap(), field) }

    fn set(&self, values: &[(&str, u32)]) {
        let mut memory = self.memory.lock().unwrap();
        for (field, value) in values { self.layout.set(&mut memory, field, *value); }
    }

    fn plc_state(&self, value: u16) {
        self.set(&[(tag::PLC_STATE, value as u32)]);
    }

    /// 数据先写，最后置 plcState=10
    fn request(&self, seq: u32, sn: &str) {
        let mut memory = self.memory.lock().unwrap();
        self.layout.set_text(&mut memory, tag::SN, sn);
        for (field, value) in [
            (tag::REQUEST_SEQ, seq),
            (tag::PRODUCT_CODE, 1),
            (tag::PLC_STATE, state::PLC_REQUEST as u32),
        ] {
            self.layout.set(&mut memory, field, value);
        }
    }

    /// PLC 核对回显后锁存结果
    fn ack(&self) {
        assert_eq!(self.pc(), state::PC_RESULT);
        assert_eq!(self.get(tag::SEQ_ECHO), self.get(tag::REQUEST_SEQ));
        assert_eq!(
            self.text(tag::SN_ECHO),
            self.text(tag::SN),
            "snEcho 与 sn 不一致时 PLC 不会确认"
        );
        self.plc_state(state::PLC_CONFIRMED);
    }

    fn fault(&self, fault: Fault) { self.server.inject(fault); }
}

struct Rig {
    plc: TestPlc,
    engine: PlcEngine,
    session: PlcSession,
    plan: PlcPlan,
    _directory: TestDir,
}

impl Rig {
    async fn new(name: &str) -> Self {
        let plc = TestPlc::start().await;
        let directory = TestDir::new("s7-session-tests", name);
        let mut config = crate::inspection::default_plc_config();
        config.connection.host = "127.0.0.1".into();
        config.connection.port = plc.server.port();
        config.connection.timeout_ms = 250;
        config.connection.poll_interval_ms = 20;
        config.connection.reconnect_interval_ms = 200;
        config.heartbeat.interval_ms = 100;
        let sink: EventSink = Arc::new(|_| {});
        let logbook = Arc::new(Logbook::open(&directory.join("plc.db"), sink.clone(), 1).unwrap());
        let engine = PlcEngine::new(config, logbook, sink);
        let session = PlcSession::open(directory.join("session.json"));
        let plan = PlcPlan::from_recipe(&crate::recipe::samples_burst().build().unwrap()).unwrap();
        let mut rig = Self {
            plc,
            engine,
            session,
            plan,
            _directory: directory,
        };
        rig.engine.connect().await;
        rig.connected().await;
        rig.live().await;
        assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready), "{:?}", rig.session.view());
        assert_eq!(rig.plc.pc(), state::PC_READY);
        rig
    }

    async fn connected(&self) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let status = self.engine.status();
            if status.state == LinkState::Connected && status.poll_count >= 2 { return; }
            assert!(Instant::now() < until, "PLC not connected: {status:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn live(&mut self) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.session.poll(&self.engine, false, true).await;
            if self.session.heartbeat.lock().unwrap().is_live(now_ms()) { return; }
            assert!(Instant::now() < until, "PLC heartbeat not live: {:?}", self.session.view());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn phase(&mut self, phase: SessionPhase) {
        let until = Instant::now() + Duration::from_secs(6);
        while self.session.phase() != phase {
            let event = self.session.poll(&self.engine, false, true).await;
            if self.session.phase() == phase { return; }
            assert!(Instant::now() < until, "expected {phase:?}, got {:?}: {event:?}", self.session.view());
            if self.session.phase() == SessionPhase::Fault && phase != SessionPhase::Fault {
                panic!("unexpected fault: {:?}", self.session.view());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn fresh(&self) {
        let count = self.engine.status().poll_count;
        let until = Instant::now() + Duration::from_secs(3);
        while self.engine.status().poll_count < count + 2 {
            assert!(Instant::now() < until, "fresh PLC poll timeout");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn request(&mut self, seq: u32, sn: &str) {
        self.plc.request(seq, sn);
        self.phase(SessionPhase::Validating).await;
        assert_eq!(self.session.request().unwrap().request_seq, seq);
    }

    /// 接件登记：核对计划后进入采集，不写 PLC，pcState 保持 10。
    async fn intake(&mut self) {
        self.session.validate_plan(&self.plan).unwrap();
        self.session.commit_intake(&self.engine).await.unwrap();
        assert_eq!(
            (self.session.phase(), self.plc.pc()),
            (SessionPhase::Acquiring, state::PC_READY)
        );
    }

    async fn end(&mut self) {
        self.plc.plc_state(state::PLC_MOTION_END);
        self.fresh().await;
        let event = self.session.poll(&self.engine, false, true).await;
        assert!(
            matches!(event, SessionEvent::End(_)),
            "expected part end, got {event:?}"
        );
        assert_eq!(self.session.phase(), SessionPhase::Draining);
    }

    async fn report(&mut self) {
        self.session.report(&self.engine, 1, 0).await.unwrap();
        assert_eq!(self.session.phase(), SessionPhase::AwaitAck);
        assert_eq!(self.plc.pc(), state::PC_RESULT);
    }

    /// PLC 锁存结果 → PC 回就绪 → PLC 回空闲
    async fn ack_release(&mut self) {
        self.plc.ack();
        let until = Instant::now() + Duration::from_secs(6);
        while self.plc.pc() != state::PC_READY {
            let event = self.session.poll(&self.engine, false, true).await;
            assert!(
                Instant::now() < until,
                "PC 未回就绪：{:?} {event:?}",
                self.session.view()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(self.session.phase(), SessionPhase::Idle);
        assert!(!self.session.pending());
        self.plc.plc_state(state::PLC_IDLE);
    }

    fn audit(&self) -> Vec<Value> {
        std::fs::read_to_string(self.session.path.with_extension("audit.jsonl")).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    async fn finish(&mut self) {
        self.engine.disconnect().await;
    }
}

/// 协议 8 正常一件：PLC 锁存结果（30）前 PC 保持 30，锁存后才回就绪并清事务；序号连续递增。
#[tokio::test]
async fn s7_wire_part_returns_ready_only_after_plc_latches_result() {
    let mut rig = Rig::new("normal-part").await;
    for seq in [1, 2] {
        rig.request(seq, "SN-50").await;
        rig.intake().await;
        rig.end().await;
        rig.report().await;
        rig.fresh().await;
        rig.session.poll(&rig.engine, false, true).await;
        assert_eq!(
            (rig.session.phase(), rig.plc.pc()),
            (SessionPhase::AwaitAck, state::PC_RESULT),
            "PLC 未锁存前不得回就绪"
        );
        rig.ack_release().await;
    }
    assert!(rig
        .audit()
        .iter()
        .any(|r| r["action"] == "acknowledged" && r["pending"]["request"]["requestSeq"] == 2));
    rig.finish().await;
}

/// 软件复位：PLC 不在空闲时 PC 先置 99，PLC 回 0 之前不得置就绪。
#[tokio::test]
async fn software_reset_waits_for_plc_idle_before_ready() {
    let mut rig = Rig::new("software-reset").await;
    rig.plc.set(&[
        (tag::PLC_REASON, 9),
        (tag::PLC_STATE, state::PLC_FAULT as u32),
    ]);
    rig.fresh().await;
    rig.session.poll(&rig.engine, false, true).await;
    assert!(
        rig.session
            .notice()
            .is_some_and(|n| n.contains("plcReason=9")),
        "{:?}",
        rig.session.view()
    );
    assert!(matches!(
        rig.session.poll(&rig.engine, true, true).await,
        SessionEvent::None
    ));
    assert_eq!(rig.plc.pc(), state::PC_RESET);
    rig.fresh().await;
    assert!(matches!(
        rig.session.poll(&rig.engine, false, true).await,
        SessionEvent::None
    ));
    assert_eq!(rig.plc.pc(), state::PC_RESET);
    rig.plc.set(&[
        (tag::PLC_REASON, 0),
        (tag::PLC_STATE, state::PLC_IDLE as u32),
    ]);
    rig.fresh().await;
    assert!(matches!(
        rig.session.poll(&rig.engine, false, true).await,
        SessionEvent::Ready
    ));
    assert_eq!(rig.plc.pc(), state::PC_READY);
    rig.finish().await;
}

/// 协议 8 PC 在任何状态都可置 99：在途时软件复位，可复位预检不拦（录制未落盘也不拦）；
/// 本件按故障中断后不必再点一次，PC 接着置 99，PLC 回空闲即就绪。
#[tokio::test]
async fn software_reset_while_part_in_flight_interrupts_part_then_resets() {
    let mut rig = Rig::new("in-flight-reset").await;
    rig.request(1, "SN-R").await;
    rig.intake().await;
    rig.fresh().await;
    rig.session
        .poll_gated(&rig.engine, false, true, false, true)
        .await;
    assert_eq!(
        rig.session.view().reset_blocked_reason,
        None,
        "{:?}",
        rig.session.view()
    );
    let event = rig.session.poll(&rig.engine, true, true).await;
    assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
    let until = Instant::now() + Duration::from_secs(3);
    while rig.plc.pc() != state::PC_RESET {
        rig.session.poll(&rig.engine, false, true).await;
        assert!(
            Instant::now() < until,
            "未接着发起软件复位：{:?}",
            rig.session.view()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    rig.plc.plc_state(state::PLC_IDLE);
    rig.fresh().await;
    assert!(
        matches!(
            rig.session.poll(&rig.engine, false, true).await,
            SessionEvent::Ready
        ),
        "{:?}",
        rig.session.view()
    );
    assert_eq!(rig.plc.pc(), state::PC_READY);
    assert!(!rig.session.pending());
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_disconnect_after_result_applied_never_commits_success() {
    let mut rig = Rig::new("disconnect-after-apply").await;
    rig.request(1, "SN-50").await;
    rig.intake().await;
    rig.end().await;
    rig.plc.fault(Fault::DisconnectAfterWriteTo(PC_STATE_WORD));
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.plc.pc(), state::PC_RESULT);
    assert!(rig.session.pending());
    let saved: Value = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
    assert_eq!(saved["pending"]["result"]["requestSeq"], 1);
    if rig.engine.status().state == LinkState::Disconnected { rig.engine.connect().await; }
    rig.connected().await;
    rig.live().await;
    assert!(matches!(rig.session.phase(), SessionPhase::ResetRequired | SessionPhase::Fault));
    rig.plc.plc_state(state::PLC_IDLE);
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert!(!rig.session.pending());
    assert!(rig.audit().iter().any(|r| r["action"] == "resetCompleted" && r["pending"]["result"]["requestSeq"] == 1));
    rig.finish().await;
}

/// 慢落盘期间 PLC 心跳停：不等落盘结束就置 pcState=90；之后心跳恢复也不自行退出故障，未结事务保留。
#[tokio::test]
async fn s7_wire_heartbeat_fault_stops_outputs_before_slow_io_finishes() {
    let mut rig = Rig::new("heartbeat-during-io").await;
    rig.request(1, "SN-50").await;
    rig.intake().await;
    rig.plc.fault(Fault::HeartbeatStop);
    let (release, wait) = mpsc::channel();
    let operation = rig.session.blocking_io(&rig.engine, move || {
        wait.recv_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())
    });
    let observe = async {
        let until = Instant::now() + Duration::from_secs(8);
        while rig.plc.pc() != state::PC_FAULT {
            assert!(Instant::now() < until, "stop outputs before releasing the blocked IO task");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        release.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(operation, observe);
    assert!(result.is_err());
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    rig.plc.fault(Fault::HeartbeatResume);
    rig.live().await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.pending());
    rig.finish().await;
}

/// 协议 8 第九节：维护件按 pcMode=1 回写，结果码只能是 0；切换模式不算外部改写，在途件按接件时锁存的模式完成。
#[tokio::test]
async fn maintenance_part_reports_with_pc_mode_1_and_accepts_only_result_code_0() {
    let mut rig = Rig::new("maintenance").await;
    rig.session.set_maintenance(true);
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::None), "{event:?}");
    assert_eq!(
        (rig.plc.pc(), rig.plc.get(tag::PC_MODE)),
        (state::PC_READY, 1)
    );
    rig.request(1, "SN-M1").await;
    rig.session.set_maintenance(false);
    assert!(rig.session.part_maintenance());
    rig.session.commit_intake(&rig.engine).await.unwrap();
    rig.end().await;
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    rig.session.report(&rig.engine, 0, 0).await.unwrap();
    assert_eq!(
        (
            rig.plc.pc(),
            rig.plc.get(tag::RESULT_CODE),
            rig.plc.get(tag::PC_MODE)
        ),
        (state::PC_RESULT, 0, 1)
    );
    rig.ack_release().await;
    assert_eq!(
        rig.plc.get(tag::PC_MODE),
        0,
        "退出维护模式后空闲输出回到正常模式"
    );
    rig.finish().await;
}

/// 协议 8 第六节：PLC 在 10 不接受结果，PLC 报工件结束（20）前不能发布任何结果（含检测错误 90）；
/// 正常件也不能回结果码 0（0 只属于维护模式，PLC 判非法）。
#[tokio::test]
async fn normal_part_reports_only_after_part_end_and_never_code_0() {
    let mut rig = Rig::new("normal-no-zero").await;
    rig.request(1, "SN-N1").await;
    rig.intake().await;
    assert!(rig.session.report(&rig.engine, 90, 95).await.is_err());
    assert_eq!(
        (rig.session.phase(), rig.plc.pc()),
        (SessionPhase::Acquiring, state::PC_READY)
    );
    rig.end().await;
    assert!(rig.session.report(&rig.engine, 0, 0).await.is_err());
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}

/// PLC 来件后不等 PC：接件登记还没完成工件就已结束（plcState=20），登记照常完成，下一次轮询即报工件结束。
#[tokio::test]
async fn part_end_before_intake_commit_is_reported_after_commit() {
    let mut rig = Rig::new("end-during-intake").await;
    rig.request(1, "SN-E1").await;
    rig.plc.plc_state(state::PLC_MOTION_END);
    rig.fresh().await;
    rig.intake().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}

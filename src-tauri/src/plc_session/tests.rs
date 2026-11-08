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

    /// PLC 复位信号，模板默认地址 DB4.DBX14.0
    fn reset_signal(&self, level: bool) {
        self.memory
            .lock()
            .unwrap()
            .set_bit(4, 14, 0, level)
            .unwrap();
    }
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
        Self::with_files(name, |_| {}).await
    }

    /// `prepare` 在打开会话前改写会话目录（如预置损坏的握手日志）。
    async fn with_files(name: &str, prepare: impl FnOnce(&std::path::Path)) -> Self {
        let plc = TestPlc::start().await;
        let directory = TestDir::new("s7-session-tests", name);
        prepare(&directory);
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
        assert!(matches!(rig.session.poll(&rig.engine, true).await, SessionEvent::Ready), "{:?}", rig.session.view());
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
            self.session.poll(&self.engine, false).await;
            if self.session.heartbeat.lock().unwrap().is_live(now_ms()) { return; }
            assert!(Instant::now() < until, "PLC heartbeat not live: {:?}", self.session.view());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn phase(&mut self, phase: SessionPhase) {
        let until = Instant::now() + Duration::from_secs(6);
        while self.session.phase() != phase {
            let event = self.session.poll(&self.engine, false).await;
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
        let event = self.session.poll(&self.engine, false).await;
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
            let event = self.session.poll(&self.engine, false).await;
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
        rig.session.poll(&rig.engine, false).await;
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

/// 空闲时 plcState 在 20 与 0 间来回跳：等待提示保持，不随每次跳变出现、消失；PLC 稳定回空闲后才清除。
#[tokio::test]
async fn idle_notice_holds_while_plc_state_flaps() {
    let mut rig = Rig::new("idle-flap").await;
    for plc in [
        state::PLC_MOTION_END,
        state::PLC_IDLE,
        state::PLC_MOTION_END,
        state::PLC_IDLE,
    ] {
        rig.plc.plc_state(plc);
        rig.fresh().await;
        rig.session.poll(&rig.engine, false).await;
        assert!(
            rig.session.notice().is_some(),
            "plcState={plc}：{:?}",
            rig.session.view()
        );
    }
    tokio::time::sleep(NOTICE_HOLD).await;
    rig.fresh().await;
    rig.session.poll(&rig.engine, false).await;
    assert_eq!(rig.session.notice(), None);
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
    rig.session.poll(&rig.engine, false).await;
    assert!(
        rig.session
            .notice()
            .is_some_and(|n| n.contains("plcReason=9")),
        "{:?}",
        rig.session.view()
    );
    assert!(matches!(
        rig.session.poll(&rig.engine, true).await,
        SessionEvent::None
    ));
    assert_eq!(rig.plc.pc(), state::PC_RESET);
    rig.fresh().await;
    assert!(matches!(
        rig.session.poll(&rig.engine, false).await,
        SessionEvent::None
    ));
    assert_eq!(rig.plc.pc(), state::PC_RESET);
    rig.plc.set(&[
        (tag::PLC_REASON, 0),
        (tag::PLC_STATE, state::PLC_IDLE as u32),
    ]);
    rig.fresh().await;
    assert!(matches!(
        rig.session.poll(&rig.engine, false).await,
        SessionEvent::Ready
    ));
    assert_eq!(rig.plc.pc(), state::PC_READY);
    rig.finish().await;
}

/// 现场复位时 PLC 把序号归 1，且晚于 PC 完成复位：复位后的第一件照常接件。
#[tokio::test]
async fn request_seq_restarting_after_reset_is_accepted() {
    let mut rig = Rig::new("seq-restart").await;
    rig.request(5, "SN-50").await;
    rig.intake().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.fresh().await;
    assert!(
        matches!(
            rig.session.poll(&rig.engine, true).await,
            SessionEvent::Ready
        ),
        "{:?}",
        rig.session.view()
    );
    rig.request(1, "SN-50").await;
    rig.finish().await;
}

/// 只在 PLC 端复位、PC 未经复位：PC 空闲时读到序号回退即清零下限并记审计，下一件照常接件。
#[tokio::test]
async fn request_seq_rewinding_while_idle_is_accepted_and_audited() {
    let mut rig = Rig::new("seq-rewind-idle").await;
    rig.request(5, "SN-50").await;
    rig.intake().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.plc.set(&[(tag::REQUEST_SEQ, 0)]);
    rig.fresh().await;
    rig.session.poll(&rig.engine, false).await;
    assert!(rig.audit().iter().any(|r| r["action"] == "plcSeqRestarted"));
    rig.request(1, "SN-50").await;
    rig.finish().await;
}

/// PLC 复位信号由 0 变 1 等同操作员软件复位：PC 置 99，PLC 回空闲后就绪；信号保持为 1 不再触发。
#[tokio::test]
async fn reset_signal_rising_edge_runs_software_reset() {
    let mut rig = Rig::new("reset-signal").await;
    rig.plc.set(&[
        (tag::PLC_REASON, 9),
        (tag::PLC_STATE, state::PLC_FAULT as u32),
    ]);
    rig.fresh().await;
    rig.session.poll(&rig.engine, false).await;
    assert!(!rig.session.reset_signal(&rig.engine));
    rig.plc.reset_signal(true);
    rig.fresh().await;
    let reset = rig.session.reset_signal(&rig.engine);
    assert!(reset, "{:?}", rig.session.view());
    rig.session.poll(&rig.engine, reset).await;
    assert_eq!(rig.plc.pc(), state::PC_RESET);
    rig.plc.set(&[
        (tag::PLC_REASON, 0),
        (tag::PLC_STATE, state::PLC_IDLE as u32),
    ]);
    rig.fresh().await;
    assert!(!rig.session.reset_signal(&rig.engine));
    assert!(matches!(
        rig.session.poll(&rig.engine, false).await,
        SessionEvent::Ready
    ));
    assert_eq!(rig.plc.pc(), state::PC_READY);
    rig.finish().await;
}

/// 复位信号卡在 1：第一次读到就是 1、之后一直为 1 都不算上升沿，PC 不替现场清故障。
#[tokio::test]
async fn reset_signal_already_high_never_triggers_reset() {
    let mut rig = Rig::new("reset-signal-held").await;
    rig.plc.set(&[
        (tag::PLC_REASON, 9),
        (tag::PLC_STATE, state::PLC_FAULT as u32),
    ]);
    rig.plc.reset_signal(true);
    for _ in 0..5 {
        rig.fresh().await;
        assert!(
            !rig.session.reset_signal(&rig.engine),
            "{:?}",
            rig.session.view()
        );
        rig.session.poll(&rig.engine, false).await;
    }
    assert_eq!(
        rig.session.view().reset_blocked_reason,
        None,
        "复位条件满足时仍不触发，才说明是按边沿判断"
    );
    assert_ne!(rig.plc.pc(), state::PC_RESET);
    rig.finish().await;
}

impl Rig {
    /// 按节拍的顺序：先问自动复位，再带着结果轮询；PC 置 99 后 PLC 清事务回空闲。返回是否到了就绪。
    async fn auto_poll(&mut self, rounds: usize) -> bool {
        for _ in 0..rounds {
            self.fresh().await;
            if self.plc.pc() == state::PC_RESET {
                self.plc.set(&[(tag::PLC_REASON, 0), (tag::PLC_STATE, state::PLC_IDLE as u32)]);
                self.fresh().await;
            }
            let reset = self.session.auto_reset();
            if matches!(self.session.poll(&self.engine, reset).await, SessionEvent::Ready) {
                return true;
            }
        }
        false
    }
}

/// 握手日志损坏、留有未提交临时文件都不挡复位：两个文件改名到旁边保留，会话以空日志启动并复位到就绪。
#[tokio::test]
async fn unusable_journal_files_are_set_aside_and_reset_reaches_ready() {
    let mut rig = Rig::with_files("journal-set-aside", |dir| {
        std::fs::write(dir.join("session.json"), "{ 不是 JSON").unwrap();
        std::fs::write(dir.join("session.pending.tmp"), "{").unwrap();
    })
    .await;
    assert!(rig.session.archived().is_some());
    let dir = rig.session.path.parent().unwrap().to_owned();
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n == "session.pending.tmp"), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("session.corrupt-") && n.ends_with(".json")), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("session.pending-") && n.ends_with(".tmp")), "{names:?}");
    rig.request(1, "SN-J1").await;
    rig.finish().await;
}

/// 在途件遇协议故障（PLC 中途报 90）：不等操作员，自动复位置 99，PLC 回空闲后就绪，在途事务记入审计后清除。
#[tokio::test]
async fn protocol_fault_with_pending_part_auto_resets_to_ready() {
    let mut rig = Rig::new("auto-reset-pending").await;
    rig.request(1, "SN-50").await;
    rig.intake().await;
    rig.plc.set(&[(tag::PLC_REASON, 9), (tag::PLC_STATE, state::PLC_FAULT as u32)]);
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    assert!(rig.auto_poll(20).await, "{:?}", rig.session.view());
    assert_eq!(rig.plc.pc(), state::PC_READY);
    assert!(!rig.session.pending());
    assert!(rig.audit().iter().any(|r| r["action"] == "resetCompleted" && r["pending"]["request"]["requestSeq"] == 1));
    rig.finish().await;
}

/// 反复故障：连续自动复位 3 次后进入退避，复位条件满足也不再立刻复位；退避期满后照常自动复位，不永久放弃。
#[tokio::test]
async fn repeated_faults_back_off_auto_reset_then_resume() {
    let mut rig = Rig::new("auto-reset-backoff").await;
    for _ in 0..AUTO_RESET_BURST {
        rig.session.fault(&rig.engine, "测试故障".into()).await;
        assert!(rig.auto_poll(10).await, "{:?}", rig.session.view());
    }
    rig.session.fault(&rig.engine, "测试故障".into()).await;
    assert!(!rig.auto_poll(10).await);
    assert_eq!(rig.session.view().reset_blocked_reason, None, "复位条件满足仍不复位，才说明是退避在拦");
    rig.session.auto_last = Instant::now().checked_sub(AUTO_RESET_BACKOFF);
    assert!(rig.auto_poll(10).await, "{:?}", rig.session.view());
    rig.finish().await;
}

/// 协议 8 PC 在任何状态都可置 99：在途时软件复位，可复位预检不拦；
/// 本件按故障中断后不必再点一次，PC 接着置 99，PLC 回空闲即就绪。
#[tokio::test]
async fn software_reset_while_part_in_flight_interrupts_part_then_resets() {
    let mut rig = Rig::new("in-flight-reset").await;
    rig.request(1, "SN-R").await;
    rig.intake().await;
    rig.fresh().await;
    rig.session.poll(&rig.engine, false).await;
    assert_eq!(
        rig.session.view().reset_blocked_reason,
        None,
        "{:?}",
        rig.session.view()
    );
    let event = rig.session.poll(&rig.engine, true).await;
    assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
    let until = Instant::now() + Duration::from_secs(3);
    while rig.plc.pc() != state::PC_RESET {
        rig.session.poll(&rig.engine, false).await;
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
            rig.session.poll(&rig.engine, false).await,
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
    assert!(matches!(rig.session.poll(&rig.engine, true).await, SessionEvent::Ready));
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
    let event = rig.session.poll(&rig.engine, false).await;
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

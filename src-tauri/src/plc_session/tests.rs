use super::*;
use std::sync::{mpsc, Arc};
use ly_plc::{EventSink, Logbook};
use serde_json::Value;
use crate::offline::line::Layout;
use crate::offline::s7_server::{Fault, S7Server, SharedMemory, WriteTarget};
use crate::offline::test_dir::TestDir;

/// PLC 心跳超时（HeartbeatWatch 的 3 s）再留一点余量：慢落盘要超过它才能证明落盘期间仍在观察心跳。
const OUTLAST_HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(3300);

/// PC 区（DB2）上的位：视觉输出
const fn pc_bit(bit: u8) -> WriteTarget { WriteTarget { db: 2, byte: 0, bit: Some(bit) } }

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

    fn on(&self, field: &str) -> bool { self.get(field) == 1 }

    fn text(&self, field: &str) -> String { self.layout.get_text(&self.memory.lock().unwrap(), field) }

    fn set(&self, values: &[(&str, u32)]) {
        let mut memory = self.memory.lock().unwrap();
        for (field, value) in values { self.layout.set(&mut memory, field, *value); }
    }

    /// PLC 清掉自己的握手输入（不看视觉输出）
    fn neutral(&self) { self.set(&[("partStart", 0), ("partEnd", 0), ("resultAck", 0)]); }

    fn request(&self, plan: &PlcPlan, seq: u32, sn: &str) {
        let mut memory = self.memory.lock().unwrap();
        self.layout.set_text(&mut memory, "partSn", sn);
        for (field, value) in [("protocolVersion", crate::handshake::PROTOCOL_VERSION as u32), ("requestSeq", seq), ("productCode", 1),
            ("shotCount", plan.shot_count as u32), ("planVersion", plan.plan_version), ("planReserved", 0),
            ("deviceShots", plan.device_shots as u32), ("deviceTriggers", 0), ("partEnd", 0), ("resultAck", 0), ("partStart", 1)] {
            self.layout.set(&mut memory, field, value);
        }
    }

    /// PLC 按当前 resultSeq 确认结果
    fn ack(&self) {
        assert_eq!(self.text("resultSn"), self.text("partSn"), "resultSn 与 partSn 不一致时 PLC 不会确认");
        self.set(&[("ackSeq", self.get("resultSeq")), ("resultAck", 1)]);
    }

    /// 视觉清除 done / busy 后 PLC 释放握手
    fn release(&self) {
        assert!(!self.on("done") && !self.on("busy"), "视觉清除 done / busy 前 PLC 不会释放");
        self.neutral();
    }

    fn raw(&self, db: u16, byte: u32, len: usize) -> Vec<u8> { self.memory.lock().unwrap().read(db, byte, len).unwrap() }

    fn write_raw(&self, db: u16, byte: u32, data: &[u8]) { self.memory.lock().unwrap().write(db, byte, data).unwrap(); }

    fn fault(&self, fault: Fault) { self.server.inject(fault); }
}

struct Rig { plc: TestPlc, engine: PlcEngine, session: PlcSession, plan: PlcPlan, directory: TestDir }

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
        // 段计划：三台相机共用一个触发，shotCount = deviceShots = ΣN_exp = 4
        let plan = PlcPlan::from_recipe(&crate::recipe::samples_burst().build().unwrap()).unwrap();
        let mut rig = Self { plc, engine, session, plan, directory };
        rig.engine.connect().await;
        rig.connected().await;
        rig.live().await;
        assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready), "{:?}", rig.session.view());
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
        self.plc.request(&self.plan, seq, sn);
        self.phase(SessionPhase::Validating).await;
        assert_eq!(self.session.request().unwrap().request_seq, seq);
    }

    async fn arm(&mut self) {
        self.session.validate_plan(&self.plan).unwrap();
        self.session.arm(&self.engine, &self.plan).await.unwrap();
        assert_eq!(self.session.phase(), SessionPhase::Acquiring);
    }

    async fn end(&mut self) -> Instant { self.end_counts(self.plan.device_shots).await.0 }

    /// PLC 置 partEnd 并报设备触发数；返回 End 事件的时刻与其带出的触发数。
    async fn end_counts(&mut self, counts: u16) -> (Instant, Option<u16>) {
        self.plc.set(&[("partEnd", 1), ("deviceTriggers", counts as u32)]);
        self.fresh().await;
        let event = self.session.poll(&self.engine, false, true).await;
        let SessionEvent::End(ended_at, triggers) = event else { panic!("expected partEnd, got {event:?}") };
        assert_eq!(self.session.phase(), SessionPhase::Draining);
        (ended_at, triggers)
    }

    async fn report(&mut self) {
        self.session.report(&self.engine, 1, 0).await.unwrap();
        assert_eq!(self.session.phase(), SessionPhase::AwaitAck);
        assert!(self.plc.on("done"));
        assert!(self.plc.on("busy"));
        assert!(!self.plc.on("visionReady"));
        assert_eq!(self.plc.get("resultSeq"), self.session.request().unwrap().request_seq);
    }

    async fn ack_release(&mut self) {
        self.plc.ack();
        self.phase(SessionPhase::Releasing).await;
        assert!(self.session.pending());
        assert!(self.session.journal.pending.as_ref().unwrap().acknowledged);
        self.plc.release();
        self.phase(SessionPhase::Idle).await;
        assert!(!self.session.pending());
    }

    fn audit(&self) -> Vec<Value> {
        std::fs::read_to_string(self.session.path.with_extension("audit.jsonl")).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    async fn finish(&mut self) {
        self.engine.disconnect().await;
    }
}

/// 协议 6：partEnd 时设备触发数少于或多于计划不再拒绝完成（只作参考），End 带出 PLC 报的触发数，照常发布结果、确认并释放。
#[tokio::test]
async fn s7_wire_part_end_with_trigger_count_unlike_plan_completes_and_reports_triggers() {
    let mut rig = Rig::new("end-count-unlike-plan").await;
    assert_eq!(rig.plan.device_shots, 4);
    for (seq, counts) in [(1, 3), (2, 5)] {
        rig.request(seq, "SN-50").await;
        rig.arm().await;
        let (_, reported) = rig.end_counts(counts).await;
        assert_eq!(reported, Some(counts), "deviceTriggers={counts}");
        rig.report().await;
        rig.ack_release().await;
    }
    rig.finish().await;
}

/// 升级到协议 6 后，握手日志里协议 5 的在途事务过不了当前协议的校验：原样归档为 `.v5.bak`，会话以空事务启动并照常要求明确复位，
/// 复位不被它挡住；ACK 恢复也不把审计里的协议 5 事务报成日志错误。
#[tokio::test]
async fn journal_pending_from_protocol_5_is_archived_and_requires_reset() {
    let mut rig = Rig::new("protocol-5-journal").await;
    let cycle = "00000000000000000000000000000005";
    let pending = Pending { origin: Some(Origin::Real), capture_id: None, request: Request { protocol_version: 5, request_seq: 9,
        sn: "SN-V5".into(), product_code: 1, shot_count: 4, plan_version: 7, device_shots: 4 }, result: None,
        phase: SessionPhase::Acquiring, cycle_id: Some(cycle.into()), acknowledged: false, started_at: 1000, site: None };
    let original = serde_json::to_vec(&Journal { version: 1, last_request_seq: 9, pending: Some(pending.clone()), last_resolution: None }).unwrap();
    std::fs::write(&rig.session.path, &original).unwrap();
    let mut audit = serde_json::to_vec(&json!({"version": 1, "ts": 2000, "action": "resetRequested", "lastRequestSeq": 9, "pending": pending})).unwrap();
    audit.push(b'\n');
    crate::handshake_audit::append(&rig.session.path.with_extension("audit.jsonl"), &audit).unwrap();

    rig.session = PlcSession::open(rig.session.path.clone());
    assert!(!rig.session.pending(), "{:?}", rig.session.pending_summary());
    assert_eq!(std::fs::read(rig.session.path.with_extension("v5.bak")).unwrap(), original);
    let recovered = rig.session.recover_acknowledgements(&[cycle.to_owned()].into_iter().collect());
    assert!(recovered.errors.is_empty() && recovered.receipts.is_empty(), "{recovered:?}");

    rig.live().await;
    assert_eq!(rig.session.phase(), SessionPhase::ResetRequired, "未复位前不能接件");
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready), "{:?}", rig.session.view());
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_plan_mismatch_refusal_still_requires_ack() {
    let mut rig = Rig::new("plan-mismatch").await;
    rig.request(1, "SN-50").await;
    let mut wrong = rig.plan.clone();
    wrong.plan_version += 1;
    assert!(rig.session.validate_plan(&wrong).is_err());
    assert!(rig.session.arm(&rig.engine, &wrong).await.is_err());
    assert!(!rig.plc.on("armed"));
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    rig.session.report(&rig.engine, 90, 94).await.unwrap();
    assert!(!rig.plc.on("visionReady"));
    assert!(rig.plc.on("busy"));
    assert!(rig.plc.on("done"));
    rig.ack_release().await;
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_early_ack_and_active_reset_preserve_pending() {
    let mut rig = Rig::new("early-ack-reset").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.plc.set(&[("ackSeq", 1), ("resultAck", 1)]);
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Fault(_)));
    assert!(rig.session.pending());
    rig.plc.neutral();
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    rig.request(2, "SN-50").await;
    rig.arm().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Fault(_)));
    assert!(rig.session.pending());
    assert_eq!(rig.session.request().unwrap().request_seq, 2);
    assert!(rig.plc.on("busy"));
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_result_write_rejection_retains_envelope_and_fault_is_not_repeated() {
    let mut rig = Rig::new("result-write-reject").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.plc.fault(Fault::RejectWriteTo(WriteTarget { db: 2, byte: 4, bit: None }));
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.session.result().unwrap().result_code, 1);
    assert!(!rig.plc.on("done"));
    assert!(rig.plc.on("busy"));
    let modified = std::fs::metadata(&rig.session.path).unwrap().modified().unwrap();
    // 故障输出已确认后不再重写：再写 visionFault 就会撞上这个定点拒写
    rig.plc.fault(Fault::RejectWriteTo(pc_bit(4)));
    for _ in 0..5 {
        rig.session.poll(&rig.engine, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(modified, std::fs::metadata(&rig.session.path).unwrap().modified().unwrap());
    assert_eq!(rig.plc.server.armed_write_faults(), 1, "故障输出不应重复写入");
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_fault_best_effort_continues_after_ready_rejection() {
    let mut rig = Rig::new("fault-best-effort").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.plc.fault(Fault::RejectWriteTo(pc_bit(0)));
    rig.session.fault(&rig.engine, "injected fault".into()).await;
    assert_eq!(rig.plc.server.armed_write_faults(), 0, "visionReady 的写入应已被拒");
    assert!(!rig.plc.on("armed"));
    assert!(rig.plc.on("visionFault"));
    assert!(rig.plc.on("busy"));
    assert!(rig.session.pending());
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_disconnect_after_done_applied_never_commits_success() {
    let mut rig = Rig::new("disconnect-after-apply").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.plc.fault(Fault::DisconnectAfterWriteTo(pc_bit(3)));
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.plc.on("done"));
    assert!(rig.session.pending());
    let saved: Value = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
    assert_eq!(saved["pending"]["result"]["requestSeq"], 1);
    if rig.engine.status().state == LinkState::Disconnected { rig.engine.connect().await; }
    rig.connected().await;
    rig.live().await;
    assert!(matches!(rig.session.phase(), SessionPhase::ResetRequired | SessionPhase::Fault));
    assert!(rig.plc.on("done"));
    rig.plc.neutral();
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert!(!rig.session.pending());
    assert!(rig.audit().iter().any(|r| r["action"] == "resetCompleted" && r["pending"]["result"]["requestSeq"] == 1));
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_release_write_failure_does_not_finish_transaction() {
    let mut rig = Rig::new("release-write-reject").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.fault(Fault::RejectWriteTo(pc_bit(2)));
    rig.plc.ack();
    rig.phase(SessionPhase::Fault).await;
    assert!(!rig.plc.on("done"));
    assert!(rig.plc.on("busy"));
    assert!(rig.session.journal.pending.as_ref().unwrap().acknowledged);
    assert_eq!(rig.session.result().unwrap().request_seq, 1);
    assert!(!rig.audit().iter().any(|r| r["action"] == "acknowledged"));
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_ack_journal_write_failure_preserves_acknowledgement() {
    let mut rig = Rig::new("ack-journal-write-failure").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    let temporary = rig.session.path.with_extension("pending.tmp");
    std::fs::create_dir(&temporary).unwrap();
    assert!(!rig.session.acknowledged());
    rig.plc.ack();
    rig.fresh().await;

    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.acknowledged());
    assert!(rig.session.pending());
    assert_eq!(rig.session.result().unwrap().request_seq, 1);
    assert_eq!(rig.session.journal.pending.as_ref().unwrap().phase, SessionPhase::Fault);
    let durable: Journal = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
    assert!(!durable.pending.unwrap().acknowledged);
    assert!(rig.plc.on("done"));
    assert!(rig.plc.on("busy"));
    assert!(rig.plc.on("visionFault"));
    assert!(!rig.plc.on("visionReady"));

    std::fs::remove_dir(&temporary).unwrap();
    rig.plc.neutral();
    rig.fresh().await;
    rig.session.poll(&rig.engine, false, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.acknowledged());
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert!(rig.audit().iter().any(|record| record["action"] == "resetCompleted"
        && record["pending"]["acknowledged"] == true && record["pending"]["result"]["requestSeq"] == 1));
    rig.finish().await;
}

#[test]
fn s7_wire_ack_journal_monitor_failure_preserves_acknowledgement() {
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap().block_on(async {
        let mut rig = Rig::new("ack-journal-monitor-failure").await;
        rig.request(1, "SN-50").await;
        rig.arm().await;
        rig.end().await;
        rig.report().await;
        assert!(!rig.session.acknowledged());
        rig.plc.ack();
        rig.plc.fault(Fault::HeartbeatStop);
        rig.fresh().await;
        let (release, wait) = mpsc::channel();
        let (started, started_wait) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        started_wait.await.unwrap();

        // ACK 落盘卡在阻塞线程池里；心跳失活后监视必须先置故障，确认后才放行落盘
        let observe = async {
            let until = Instant::now() + Duration::from_secs(8);
            while !rig.plc.on("visionFault") {
                assert!(Instant::now() < until, "monitor must fault before the ACK journal write completes");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(rig.plc.on("done"));
            assert!(rig.plc.on("busy"));
            release.send(()).unwrap();
        };
        let (event, ()) = tokio::join!(rig.session.poll(&rig.engine, false, true), observe);
        blocker.await.unwrap();
        assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
        assert_eq!(rig.session.phase(), SessionPhase::Fault);
        assert!(rig.session.acknowledged());
        assert!(rig.session.pending());
        assert_eq!(rig.session.result().unwrap().request_seq, 1);
        let durable: Journal = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
        let pending = durable.pending.unwrap();
        assert!(pending.acknowledged);
        assert_eq!(pending.phase, SessionPhase::Fault);

        rig.plc.fault(Fault::HeartbeatResume);
        rig.plc.neutral();
        rig.live().await;
        assert_eq!(rig.session.phase(), SessionPhase::Fault);
        assert!(rig.session.acknowledged());
        assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
        assert!(rig.audit().iter().any(|record| record["action"] == "resetCompleted"
            && record["pending"]["acknowledged"] == true && record["pending"]["result"]["requestSeq"] == 1));
        rig.finish().await;
    });
}

#[tokio::test]
async fn s7_wire_reset_conditions_change_reports_live_blocker_without_clearing_pending() {
    let mut rig = Rig::new("reset-readiness").await;
    rig.request(1, "SN-RESET").await;
    rig.arm().await;
    rig.session = PlcSession::open(rig.session.path.clone());
    rig.live().await;
    assert!(rig.session.view().reset_blocked_reason.unwrap().contains("partStart"));
    rig.plc.neutral();
    rig.fresh().await;

    // 同一条复位前置契约：每个阻塞条件均须实时反映，不能靠点击后才发现。
    let current = crate::handshake::PROTOCOL_VERSION as u32;
    for (protocol, devices, audit_ready, expected) in [
        (3, Ok(()), true, Some("协议版本")),
        (current, Err("cam2 未连接".to_string()), true, Some("cam2")),
        (current, Ok(()), false, Some("持久化")),
        (current, Ok(()), true, None),
    ] {
        rig.plc.set(&[("protocolVersion", protocol)]);
        rig.fresh().await;
        rig.session.poll_with_devices(&rig.engine, false, &devices, audit_ready, true).await;
        let view = rig.session.view();
        match expected {
            Some(fragment) => assert!(view.reset_blocked_reason.as_deref().is_some_and(|reason| reason.contains(fragment)), "{view:?}"),
            None => assert!(view.reset_blocked_reason.is_none(), "{view:?}"),
        }
        assert!(rig.session.pending(), "只读复位检查不能清除未结事务");
        assert_eq!(view.phase, SessionPhase::ResetRequired);
    }
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert!(!rig.session.pending());
    assert!(!rig.plc.on("visionFault"));
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_corrupt_journal_cannot_reset_or_overwrite_evidence() {
    let mut rig = Rig::new("corrupt-journal").await;
    let original = b"{\"version\":1,\"pending\":broken";
    std::fs::write(&rig.session.path, original).unwrap();
    rig.session = PlcSession::open(rig.session.path.clone());
    rig.live().await;
    rig.session.poll(&rig.engine, true, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.pending());
    assert_eq!(std::fs::read(&rig.session.path).unwrap(), original);
    assert!(!rig.plc.on("visionReady"));
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_accepted_outputs_and_done_are_continuously_verified() {
    let mut rig = Rig::new("output-tamper").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.plc.set(&[("acceptedSeq", 55)]);
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    rig.plc.neutral();
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    rig.request(2, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.set(&[("done", 0)]);
    rig.phase(SessionPhase::Fault).await;
    assert_eq!(rig.session.result().unwrap().request_seq, 2);
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_slow_durable_io_keeps_observing_heartbeat() {
    let mut rig = Rig::new("slow-durable-io").await;
    rig.session.blocking_io(&rig.engine, || {
        std::thread::sleep(OUTLAST_HEARTBEAT_TIMEOUT);
        Ok(())
    }).await.unwrap();
    rig.session.poll(&rig.engine, false, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(rig.session.heartbeat.lock().unwrap().is_live(now_ms()));
    rig.finish().await;
}

#[test]
fn s7_wire_slow_end_journal_does_not_extend_frame_deadline() {
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap().block_on(async {
        let mut rig = Rig::new("slow-end-frame-deadline").await;
        rig.request(1, "SN-50").await;
        rig.arm().await;
        rig.plc.set(&[("partEnd", 1), ("deviceTriggers", 4)]);
        rig.fresh().await;

        let (release, wait) = mpsc::channel();
        let (started, started_wait) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(8)).unwrap();
        });
        started_wait.await.unwrap();

        let late_frame = async {
            tokio::time::sleep(Duration::from_millis(350)).await;
            let frame = crate::frame::Frame {
                cam: 0, session: 1, counter: crate::frame::CounterSource::Synthetic,
                frame_counter: 9004, trigger_counter: 4, lost_packets: 0, ts: now_ms(), manual: false, images: Vec::new(),
            };
            release.send(()).unwrap();
            frame
        };
        let judgement = crate::cycle::tests::judge_last_frame_during_end(
            rig.session.poll(&rig.engine, false, true), late_frame,
        ).await;
        blocker.await.unwrap();
        assert_eq!(judgement.verdict, crate::judge::Verdict::ErrInspect);
        assert_eq!(judgement.fault_code, crate::judge::fault::MISSING_FRAME);
        rig.session.report(&rig.engine, judgement.plc_code, judgement.fault_code).await.unwrap();
        assert_eq!(rig.plc.get("resultCode"), 90);
        assert_eq!(rig.plc.get("faultCode"), 91);
        assert!(rig.plc.on("done"));
        rig.finish().await;
    });
}

/// 慢落盘期间 PLC 心跳停：不等落盘结束就撤销布防、置故障；之后心跳恢复也不自行退出故障，未结事务保留。
#[tokio::test]
async fn s7_wire_heartbeat_fault_stops_outputs_before_slow_io_finishes() {
    let mut rig = Rig::new("heartbeat-during-io").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.plc.fault(Fault::HeartbeatStop);
    let (release, wait) = mpsc::channel();
    let operation = rig.session.blocking_io(&rig.engine, move || {
        wait.recv_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())
    });
    let observe = async {
        let until = Instant::now() + Duration::from_secs(8);
        while rig.plc.on("armed") || !rig.plc.on("visionFault") {
            assert!(Instant::now() < until, "stop outputs before releasing the blocked IO task");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(rig.plc.on("busy"));
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

#[tokio::test]
async fn s7_wire_audit_failure_blocks_destructive_reset() {
    let mut rig = Rig::new("audit-failure").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.session.fault(&rig.engine, "operator recovery".into()).await;
    rig.plc.neutral();
    rig.fresh().await;
    let audit = rig.session.path.with_extension("audit.jsonl");
    std::fs::rename(&audit, rig.directory.join("preserved-audit.jsonl")).unwrap();
    std::fs::create_dir(&audit).unwrap();
    rig.session.poll(&rig.engine, true, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.plc.on("done"));
    assert_eq!(rig.session.result().unwrap().result_code, 1);
    rig.finish().await;
}

fn assert_idle_outputs(plc: &TestPlc, ready: bool) {
    assert_eq!(plc.on("visionReady"), ready);
    assert_eq!(plc.get("pcProtocolVersion"), crate::handshake::PROTOCOL_VERSION as u32);
    for tag in ["armed", "busy", "done", "visionFault"] { assert!(!plc.on(tag), "{tag}"); }
}

#[tokio::test]
async fn s7_wire_idle_db_reinit_restores_outputs_without_reset() {
    let mut rig = Rig::new("idle-db-reinit").await;
    // PLC 重启或 DB 重新初始化：PC 区（DB2 字节 0–19）读回全 0，visionReady=0
    rig.plc.write_raw(2, 0, &[0u8; 20]);
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Restored(_)), "{event:?}");
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(rig.session.notice().is_none());
    assert_idle_outputs(&rig.plc, true);
    assert!(rig.audit().iter().any(|record| record["action"] == "idleOutputsRestored" && record["pending"].is_null()));
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_broken_sn_headers_reset_heals_result_sn_only_after_part_sn_restored() {
    let (broken, empty) = ([0u8, 0], [50u8, 0]);
    let mut rig = Rig::new("sn-header-reset").await;
    rig.plc.write_raw(2, 20, &broken);
    rig.plc.write_raw(4, 38, &broken);
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::Fault(_)));
    rig.session.poll(&rig.engine, true, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.plc.raw(2, 20, 2), broken);
    rig.plc.write_raw(4, 38, &empty);
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready), "{:?}", rig.session.view());
    assert_eq!(rig.plc.raw(2, 20, 2), empty);
    assert_idle_outputs(&rig.plc, true);
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_idle_repeated_output_overwrite_faults_on_third_restore() {
    let mut rig = Rig::new("idle-overwrite").await;
    for _ in 0..2 {
        rig.plc.set(&[("busy", 1), ("visionFault", 1)]);
        rig.fresh().await;
        let event = rig.session.poll(&rig.engine, false, true).await;
        assert!(matches!(event, SessionEvent::Restored(_)), "{event:?}");
        assert_idle_outputs(&rig.plc, true);
    }
    rig.plc.set(&[("done", 1)]);
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.plc.on("visionFault"));
    assert!(!rig.session.pending());
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert_idle_outputs(&rig.plc, true);
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_idle_plc_leftovers_wait_without_fault_until_released() {
    let mut rig = Rig::new("idle-plc-leftovers").await;
    rig.plc.set(&[("partEnd", 1)]);
    rig.fresh().await;
    for _ in 0..3 {
        assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(rig.session.notice().is_some(), "{:?}", rig.session.view());
    assert_idle_outputs(&rig.plc, true);
    rig.plc.set(&[("partEnd", 0)]);
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
    assert!(rig.session.notice().is_none() && rig.session.view().message.is_none());
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;

    // 带着未清的 resultAck 再发 partStart：分不清确认属于哪一件，判握手故障
    rig.plc.set(&[("resultAck", 1)]);
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
    assert!(rig.session.notice().is_some());
    rig.plc.set(&[("partStart", 1)]);
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
    assert!(!rig.session.pending());
    rig.plc.release();
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_devices_not_ready_while_idle_withdraws_ready_and_recovers() {
    let mut rig = Rig::new("idle-devices").await;
    for _ in 0..3 {
        assert!(matches!(rig.session.poll(&rig.engine, false, false).await, SessionEvent::None));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(rig.session.notice().is_some());
    assert_idle_outputs(&rig.plc, false);
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
    assert!(rig.session.notice().is_none());
    assert_idle_outputs(&rig.plc, true);
    assert!(!rig.audit().iter().any(|record| record["action"] == "idleOutputsRestored"));

    // 结果已确认后设备掉线：事务照常结束，不置就绪；设备恢复后补上
    rig.request(1, "SN-50").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.ack();
    rig.phase(SessionPhase::Releasing).await;
    rig.plc.release();
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, false, false).await, SessionEvent::Released(Some(4))));
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(!rig.session.pending());
    assert_idle_outputs(&rig.plc, false);
    assert!(rig.session.notice().is_some());
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
    assert_idle_outputs(&rig.plc, true);

    // 就绪撤销前 PLC 已经发来新件：照常接下事务，由节拍拒收判 ERR，不判握手故障
    rig.plc.request(&rig.plan, 2, "SN-51");
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, false).await;
    assert!(matches!(event, SessionEvent::Start(ref request) if request.request_seq == 2), "{event:?}");
    rig.session.report(&rig.engine, 90, crate::judge::fault::DEVICE_LOST).await.unwrap();
    rig.ack_release().await;
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_early_err_release_reports_plc_issued_triggers() {
    let mut rig = Rig::new("early-err-triggers").await;
    rig.request(1, "SN-50").await;
    rig.arm().await;
    // 运动中途停了：设备只发了 1 个触发，没有 partEnd；PC 运动超时提前判 ERR 97
    rig.plc.set(&[("deviceTriggers", 1)]);
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, false, true).await, SessionEvent::None));
    rig.session.report(&rig.engine, 90, crate::judge::fault::MOTION_TIMEOUT).await.unwrap();
    rig.plc.ack();
    rig.phase(SessionPhase::Releasing).await;
    rig.plc.release();
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Released(Some(1))), "{event:?}");
    // 下一件的请求清零了触发数，正常完成时报计划数
    rig.request(2, "SN-51").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.ack();
    rig.phase(SessionPhase::Releasing).await;
    rig.plc.release();
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Released(Some(4))), "{event:?}");
    rig.finish().await;
}

#[test]
fn unfinished_temporary_journal_blocks_recovery_even_when_main_file_exists() {
    let directory = TestDir::new("s7-session-tests", "unfinished-journal");
    let path = directory.join("session.json");
    std::fs::write(&path, serde_json::to_vec(&Journal { version: 1, ..Journal::default() }).unwrap()).unwrap();
    std::fs::write(path.with_extension("pending.tmp"), b"unfinished transaction").unwrap();
    let session = PlcSession::open(path);
    assert!(session.pending());
    assert!(session.load_error.unwrap().contains("未提交"));
}

#[tokio::test]
async fn s7_wire_reconnect_between_validation_and_arm_cannot_commit() {
    let mut rig = Rig::new("reconnect-before-arm").await;
    rig.request(1, "SN-50").await;
    let original = rig.session.connection.unwrap();
    rig.engine.disconnect().await;
    rig.engine.connect().await;
    rig.connected().await;
    assert_ne!(rig.engine.status().since, original);
    assert!(rig.session.arm(&rig.engine, &rig.plan).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.pending());
    assert!(!rig.plc.on("armed"));
    assert_eq!(rig.plc.get("acceptedSeq"), 0);
    rig.finish().await;
}


#[tokio::test]
async fn s7_wire_recording_audit_wait_holds_release_then_recovers_without_reset() {
    let mut rig = Rig::new("recording-audit-wait").await;
    rig.request(1, "SN-12345").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.ack();
    rig.phase(SessionPhase::Releasing).await;
    rig.plc.release();
    rig.fresh().await;
    for _ in 0..4 {
        assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, false).await, SessionEvent::None));
        assert_eq!(rig.session.phase(), SessionPhase::Releasing);
        assert!(rig.session.pending() && rig.session.acknowledged());
        assert!(!rig.plc.on("visionReady"));
        assert!(!rig.plc.on("visionFault"));
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let durable = PlcSession::open(rig.session.path.clone());
    assert!(durable.pending() && durable.acknowledged());
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, true).await, SessionEvent::Released(Some(4))));
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(!rig.session.pending());
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, false).await, SessionEvent::None));
    assert!(!rig.plc.on("visionReady"));
    rig.fresh().await;
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, true).await, SessionEvent::None));
    assert!(rig.plc.on("visionReady"));
    rig.request(2, "SN-12345").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}

#[tokio::test]
async fn s7_wire_acquire_and_drain_audit_wait_does_not_mask_device_fault() {
    for draining in [false, true] {
        let mut rig = Rig::new(if draining { "drain-audit-device-fault" } else { "acquire-audit-device-fault" }).await;
        rig.request(1, "SN-12345").await;
        rig.arm().await;
        if draining { rig.end().await; }
        let event = rig.session.poll_with_audit(&rig.engine, false, false, false).await;
        assert!(matches!(event, SessionEvent::Fault(_)), "{event:?}");
        assert_eq!(rig.session.phase(), SessionPhase::Fault);
        assert!(rig.session.pending());
        assert!(!rig.plc.on("visionReady"));
        rig.finish().await;
    }
}

#[tokio::test]
async fn s7_wire_idle_audit_recovery_keeps_ready_low_until_devices_recover() {
    let mut rig = Rig::new("idle-audit-device-recovery").await;
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, false).await, SessionEvent::None));
    assert!(!rig.plc.on("visionReady"));
    rig.fresh().await;
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, false, false).await, SessionEvent::None));
    assert!(!rig.plc.on("visionReady"));
    rig.fresh().await;
    for _ in 0..3 {
        assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, false, true).await, SessionEvent::None));
        assert_eq!(rig.session.phase(), SessionPhase::Idle);
        for tag in ["visionReady", "visionFault", "done", "busy", "armed"] {
            assert!(!rig.plc.on(tag), "{tag}");
        }
        assert!(rig.session.notice().is_some());
        rig.fresh().await;
    }
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, true).await, SessionEvent::None));
    assert!(rig.plc.on("visionReady"));
    assert!(rig.session.notice().is_none());
    rig.request(1, "SN-12345").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}


#[tokio::test]
async fn s7_wire_released_does_not_create_second_ack_delivery_after_ready() {
    use crate::store::{PartRecord, PlcDelivery, PlcDeliveryState, Store};
    let mut rig = Rig::new("released-single-delivery").await;
    let cycle = "1123456789abcdef0123456789abcdef";
    rig.request(1, "SN-12345").await;
    rig.session.bind_cycle_id(cycle, PartSite::default()).unwrap();
    rig.arm().await;
    rig.end().await;
    let judgement = crate::judge::Judgement::error(crate::judge::fault::MISSING_FRAME, "协议专项保留原判定");
    rig.session.report(&rig.engine, judgement.plc_code, judgement.fault_code).await.unwrap();
    assert_eq!(rig.session.phase(), SessionPhase::AwaitAck);
    let store = Store::open(&rig.directory.join("single-ack.sqlite")).unwrap();
    let submitted = PlcDelivery { state: PlcDeliveryState::Submitted, updated_at: 1, message: Some("结果已提交".into()) };
    store.insert(&PartRecord { ts: 1, sn: "SN-12345", recipe: None, judgement: &judgement, drain_ms: None,
        frames: &[], frames_expected: 4, frames_received: 0, triggers: 4, table: None,
        software_version: "single-ack-test", cycle_id: Some(cycle), bundle_id: None, delivery: &submitted, shots: &[], source: crate::store::RecordSource::Real, sample: None }).unwrap();
    rig.plc.ack();
    let until = Instant::now() + Duration::from_secs(6);
    let mut deliveries = 0;
    while rig.session.phase() != SessionPhase::Releasing {
        let had_ack = rig.session.acknowledged();
        let event = rig.session.poll_with_audit(&rig.engine, false, true, false).await;
        assert!(!matches!(event, SessionEvent::Fault(_)), "{event:?}");
        if let Some(delivery) = crate::cycle::s7_ack_delivery(had_ack, &rig.session) {
            assert!(store.update_delivery(cycle, &delivery).unwrap());
            deliveries += 1;
        }
        assert!(Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(deliveries, 1);
    let accepted = store.detail_by_cycle(cycle).unwrap().unwrap().summary.delivery;
    assert_eq!(accepted.state, PlcDeliveryState::Acknowledged);
    assert_eq!(accepted.message.as_deref(), Some("PLC 结果序号已匹配确认"));
    rig.plc.release();
    rig.fresh().await;
    let had_ack = rig.session.acknowledged();
    assert!(matches!(rig.session.poll_with_audit(&rig.engine, false, true, true).await, SessionEvent::Released(Some(4))));
    assert!(rig.plc.on("visionReady"));
    assert!(crate::cycle::s7_ack_delivery(had_ack, &rig.session).is_none());
    assert_eq!(store.detail_by_cycle(cycle).unwrap().unwrap().summary.delivery, accepted);
    assert_eq!(deliveries, 1);
    drop(store);
    rig.request(2, "SN-12345").await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}

#[test]
fn s7_wire_arm_deadline_settles_original_journal_before_fault_without_late_overwrite() {
    use std::sync::atomic::{AtomicBool, Ordering};
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap().block_on(async {
        let mut rig = Rig::new("arm-deadline-journal-settlement").await;
        rig.request(1, "SN-12345").await;
        let cycle = "223456789abcdef0123456789abcdef0";
        rig.session.bind_cycle_id(cycle, PartSite::default()).unwrap();
        let path = rig.session.path.clone();
        let temporary = path.with_extension("pending.tmp");
        let (release, wait) = mpsc::channel();
        let (started, started_wait) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(8)).unwrap();
        });
        started_wait.await.unwrap();
        let settled = AtomicBool::new(false);
        let mut budget = crate::arming::ArmBudget::with_start(Instant::now(), Duration::from_millis(35));
        let arm = async {
            let result = budget.run_serial("s7-arm-journal", rig.session.arm(&rig.engine, &rig.plan)).await;
            settled.store(true, Ordering::SeqCst);
            result
        };
        let observe = async {
            let until = Instant::now() + Duration::from_secs(3);
            while !rig.plc.on("armed") {
                assert!(Instant::now() < until, "S7 arm outputs never reached the real PLC");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
            assert!(!settled.load(Ordering::SeqCst), "deadline must not detach the original journal task");
            let waiting: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(waiting.pending.unwrap().phase, SessionPhase::Validating);
            assert!(!temporary.exists(), "the queued journal must not race a fault writer");
            release.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(arm, observe);
        blocker.await.unwrap();
        assert!(result.unwrap_err().contains("s7-arm-journal"));
        assert!(settled.load(Ordering::SeqCst));
        assert_eq!(rig.session.phase(), SessionPhase::Acquiring);
        let completed: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(completed.pending.unwrap().phase, SessionPhase::Acquiring);
        assert!(!temporary.exists());
        rig.session.fault(&rig.engine, "arm total deadline exceeded after serial settlement".into()).await;
        assert_eq!(rig.session.phase(), SessionPhase::Fault);
        assert!(!rig.plc.on("armed"));
        assert!(!rig.plc.on("visionReady"));
        assert!(rig.plc.on("visionFault"));
        tokio::time::sleep(Duration::from_millis(150)).await;
        let durable: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let pending = durable.pending.unwrap();
        assert_eq!(pending.phase, SessionPhase::Fault);
        assert_eq!(pending.cycle_id.as_deref(), Some(cycle));
        assert_eq!(pending.request.request_seq, 1);
        assert_eq!(pending.request.sn, "SN-12345");
        assert!(!temporary.exists());
        let restarted = PlcSession::open(path);
        assert!(restarted.load_error.is_none());
        assert_eq!(restarted.journal.pending.unwrap().phase, SessionPhase::Fault);
        rig.finish().await;
    });
}

/// 示教采集事务绑定采集轮次而不是工件：capture_id 落盘，结果发出后仍保持，PLC 确认并释放后才解绑。
#[tokio::test]
async fn s7_wire_teaching_capture_holds_capture_id_until_release() {
    let mut rig = Rig::new("teaching-capture").await;
    rig.request(1, "SN-777").await;
    rig.session.bind_capture_id("capture-round-1").unwrap();
    assert!(rig.session.bind_cycle_id("00000000000000000000000000000001", PartSite::default()).is_err());
    rig.arm().await;
    assert_eq!(rig.session.capture_id(), Some("capture-round-1"));
    let persisted: Journal = serde_json::from_str(&std::fs::read_to_string(&rig.session.path).unwrap()).unwrap();
    assert_eq!(persisted.pending.as_ref().unwrap().capture_id.as_deref(), Some("capture-round-1"));
    assert!(persisted.pending.unwrap().cycle_id.is_none());
    rig.end().await;
    rig.report().await;
    assert!(rig.session.pending());
    assert_eq!(rig.session.capture_id(), Some("capture-round-1"));
    rig.ack_release().await;
    assert!(rig.session.capture_id().is_none());
    rig.finish().await;
}

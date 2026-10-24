use super::*;
use std::sync::{mpsc, Arc};
use ly_plc::{EventSink, Logbook};
use serde_json::Value;
use crate::offline::line::Layout;
use crate::offline::s7_server::{Fault, S7Server, SharedMemory, WriteTarget};
use crate::offline::test_dir::TestDir;

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
        // 段计划：三台相机共用一个触发，shotCount = deviceShots = ΣN_exp = 4
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
async fn s7_wire_part_end_ignores_reserved_trigger_count() {
    let mut rig = Rig::new("end-count-unlike-plan").await;
    assert_eq!(rig.plan.device_shots, 4);
    for (seq, counts) in [(1, 3), (2, 5)] {
        rig.request(seq, "SN-50").await;
        rig.arm().await;
        let (_, reported) = rig.end_counts(counts).await;
        assert_eq!(reported, Some(0), "预留字段不参与采集：deviceTriggers={counts}");
        rig.report().await;
        rig.ack_release().await;
    }
    rig.finish().await;
}

#[tokio::test]
async fn plc_reset_high_waits_for_release_then_completes_without_production_intake() {
    let mut rig = Rig::new("reset-without-production").await;
    rig.plc.set(&[("faultReset", 1)]);
    rig.fresh().await;
    assert!(matches!(
        rig.session
            .poll_gated(&rig.engine, false, true, true, false)
            .await,
        SessionEvent::None
    ));
    assert!(rig.plc.on("visionFault"));
    assert!(!rig.plc.on("visionReady"));
    assert_eq!(rig.session.phase(), SessionPhase::ResetRequired);
    rig.plc.set(&[("faultReset", 0)]);
    rig.fresh().await;
    assert!(matches!(
        rig.session
            .poll_gated(&rig.engine, false, true, true, false)
            .await,
        SessionEvent::Ready
    ));
    assert!(!rig.plc.on("visionFault"));
    assert!(!rig.plc.on("visionReady"));
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert_eq!(rig.audit().last().unwrap()["action"], "resetCompleted");
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

fn teaching_plan(rig: &Rig) -> PlcPlan {
    let cameras = ["cam1", "cam2", "cam3"].map(String::from).to_vec();
    PlcPlan::teaching("recipe-a".into(), rig.plan.plan_version, cameras).unwrap()
}

/// 规格 §5：段配方的示教采集事务绑定采集轮次后，不核对 shotCount / deviceShots（可为 0、两者不等、超过 64），照常布防。
/// 0 是下限；32767 是线协议能写的最大值，且与 deviceShots 不等、超过旧上限 64（同一类的 65 / 200 不再列）。
#[tokio::test]
async fn s7_wire_teaching_plan_in_a_capture_transaction_arms_with_any_plc_counts() {
    let mut rig = Rig::new("teaching-plan-counts").await;
    let teaching = teaching_plan(&rig);
    for (seq, (shot_count, device_shots)) in [(1, (0, 0)), (2, (i16::MAX as u16, 1))] {
        rig.plc.request(&PlcPlan { shot_count, device_shots, ..teaching.clone() }, seq, "SN-TEACH");
        rig.phase(SessionPhase::Validating).await;
        rig.session.bind_capture_id(&format!("capture-round-{seq}")).unwrap();
        rig.session.arm(&rig.engine, &teaching).await.unwrap_or_else(|e| panic!("shotCount={shot_count} deviceShots={device_shots}：{e}"));
        rig.end_counts(device_shots).await;
        rig.report().await;
        rig.ack_release().await;
    }
    rig.finish().await;
}

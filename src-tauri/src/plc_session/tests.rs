use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{SystemTime, UNIX_EPOCH};
use ly_plc::{EventSink, Logbook};
use serde_json::Value;
use crate::plc_plan::PlanShot;

struct TestPlc {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<Value>,
    sequence: u64,
    port: u16,
    directory: PathBuf,
}

impl TestPlc {
    fn start(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let directory = root.join("target/s7-session-tests").join(format!("{name}-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut command = Command::new(std::env::var("S7_TEST_PYTHON").unwrap_or_else(|_| "python".into()));
        command.arg(root.join("../scripts/s7-handshake/s7_test_plc.py"))
            .args(["--port", "0", "--db", "100"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().expect("start Python S7 test PLC (set S7_TEST_PYTHON if needed)");
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str(&line) else { continue };
                if tx.send(value).is_err() { break; }
            }
        });
        let mut plc = Self { child, input, output, sequence: 0, port: 0, directory };
        let ready = plc.output.recv_timeout(Duration::from_secs(5)).expect("S7 test PLC ready");
        assert_eq!(ready["host"], "127.0.0.1");
        plc.port = ready["port"].as_u64().unwrap() as u16;
        plc.control(json!({"op":"heartbeat", "interval_ms":100}));
        plc
    }

    fn control(&mut self, mut command: Value) -> Value {
        self.sequence += 1;
        command["id"] = json!(self.sequence);
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        let response = self.output.recv_timeout(Duration::from_secs(5)).expect("S7 control response");
        assert_eq!(response["id"], self.sequence);
        assert_eq!(response["ok"], true, "{response}");
        response["result"].clone()
    }

    fn fields(&mut self) -> Value { self.control(json!({"op":"status"}))["fields"].clone() }

    fn values(&mut self, values: Value) { self.control(json!({"op":"plc_values", "values":values})); }

    fn fault(&mut self, byte: u16, bit: Option<u8>, kind: &str, after_apply: bool) {
        let mut command = json!({"op":"fault", "function":"write", "kind":kind, "db":100, "byte":byte});
        if let Some(bit) = bit { command["bit"] = json!(bit); }
        if kind == "disconnect" { command["after_apply"] = json!(after_apply); }
        self.control(command);
    }

    fn save(&mut self, label: &str) {
        let status = self.control(json!({"op":"status"}));
        let trace = self.control(json!({"op":"trace"}));
        std::fs::write(self.directory.join(format!("{label}-status.json")), serde_json::to_vec_pretty(&status).unwrap()).unwrap();
        std::fs::write(self.directory.join(format!("{label}-wire.json")), serde_json::to_vec_pretty(&trace).unwrap()).unwrap();
    }
}

impl Drop for TestPlc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Rig { plc: TestPlc, engine: PlcEngine, session: PlcSession, plan: PlcPlan }

impl Rig {
    async fn new(name: &str) -> Self {
        let plc = TestPlc::start(name);
        let mut config = crate::inspection::s7_phase1_config(100).unwrap();
        config.connection.host = "127.0.0.1".into();
        config.connection.port = plc.port;
        config.connection.timeout_ms = 250;
        config.connection.poll_interval_ms = 20;
        config.connection.reconnect_interval_ms = 200;
        config.heartbeat.interval_ms = 100;
        let sink: EventSink = Arc::new(|_| {});
        let logbook = Arc::new(Logbook::open(&plc.directory.join("plc.db"), sink.clone(), 1).unwrap());
        let engine = PlcEngine::new(config, logbook, sink);
        let session = PlcSession::open(plc.directory.join("session.json"));
        let plan = PlcPlan::compile("part-A".into(), 7, ["cam1".into(), "cam2".into(), "cam3".into()],
            ["cam1", "cam2", "cam3", "cam1"].into_iter().enumerate().map(|(i, camera)| PlanShot {
                shot_id: format!("P{}", i + 1), pose_id: format!("P{}", i + 1), camera_id: camera.into(),
            }).collect()).unwrap();
        let mut rig = Self { plc, engine, session, plan };
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

    async fn request(&mut self, seq: u32, sn: u32) {
        self.plc.control(json!({"op":"plc_request", "values":{
            "protocolVersion":1,"requestSeq":seq,"partSn":sn,"productCode":1,
            "shotCount":self.plan.shot_count,"planVersion":self.plan.plan_version,"planHash":self.plan.plan_hash,
            "camera1Shots":self.plan.camera_shots[0],"camera2Shots":self.plan.camera_shots[1],"camera3Shots":self.plan.camera_shots[2],
            "camera1Triggers":0,"camera2Triggers":0,"camera3Triggers":0,"partEnd":false,"resultAck":false
        }}));
        self.phase(SessionPhase::Validating).await;
        assert_eq!(self.session.request().unwrap().request_seq, seq);
    }

    async fn arm(&mut self) {
        self.session.validate_plan(&self.plan).unwrap();
        self.session.arm(&self.engine, &self.plan).await.unwrap();
        assert_eq!(self.session.phase(), SessionPhase::Acquiring);
    }

    async fn end(&mut self) -> Instant { self.end_counts([2, 1, 1]).await }

    async fn end_counts(&mut self, counts: [u16; 3]) -> Instant {
        self.plc.values(json!({"partEnd":true,"camera1Triggers":counts[0],"camera2Triggers":counts[1],"camera3Triggers":counts[2]}));
        self.fresh().await;
        let event = self.session.poll(&self.engine, false, true).await;
        let SessionEvent::End(ended_at) = event else { panic!("expected partEnd, got {event:?}") };
        assert_eq!(self.session.phase(), SessionPhase::Draining);
        ended_at
    }

    async fn reject_end_and_reset(&mut self, counts: [u16; 3]) {
        assert_eq!(self.session.phase(), SessionPhase::Acquiring);
        let request = self.session.request().unwrap().clone();
        let cycle_id = self.session.cycle_id().map(str::to_owned);
        let before = self.plc.fields();
        self.plc.values(json!({"partEnd":true,"camera1Triggers":counts[0],"camera2Triggers":counts[1],"camera3Triggers":counts[2]}));
        self.fresh().await;
        let event = self.session.poll(&self.engine, false, true).await;
        assert!(matches!(event, SessionEvent::Fault(ref error) if error.contains("PLC 实际触发数")
            && error.contains(&format!("{counts:?}")) && error.contains(&format!("{:?}", request.camera_shots))), "{event:?}");
        assert_eq!(self.session.phase(), SessionPhase::Fault);
        assert!(self.session.pending());
        assert_eq!(self.session.request(), Some(&request));
        assert_eq!(self.session.cycle_id(), cycle_id.as_deref());
        assert!(self.session.result().is_none());
        assert!(!self.session.acknowledged());
        let durable: Journal = serde_json::from_slice(&std::fs::read(&self.session.path).unwrap()).unwrap();
        let pending = durable.pending.unwrap();
        assert_eq!(pending.phase, SessionPhase::Fault);
        assert_eq!(pending.request, request);
        assert_eq!(pending.cycle_id, cycle_id);
        assert!(pending.result.is_none());
        assert!(!pending.acknowledged);
        let fields = self.plc.fields();
        assert_eq!(fields["armed"], false);
        assert_eq!(fields["visionReady"], false);
        assert_eq!(fields["visionFault"], true);
        assert_eq!(fields["busy"], true);
        assert_eq!(fields["done"], false);
        for tag in ["resultCode", "faultCode", "resultSn", "resultSeq"] { assert_eq!(fields[tag], before[tag], "{tag}"); }
        assert!(self.session.report(&self.engine, 1, 0).await.is_err());
        assert!(self.session.result().is_none());
        assert_eq!(self.plc.fields()["done"], false);
        let active_reset = self.session.poll(&self.engine, true, true).await;
        assert!(matches!(active_reset, SessionEvent::Fault(ref error) if error.contains("复位前 PLC 必须释放")), "{active_reset:?}");
        assert!(self.session.pending());
        assert_eq!(self.session.request(), Some(&request));
        assert_eq!(self.plc.fields()["busy"], true);
        self.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
        self.fresh().await;
        assert!(matches!(self.session.poll(&self.engine, false, true).await, SessionEvent::None));
        assert_eq!(self.session.phase(), SessionPhase::Fault);
        assert!(self.session.pending());
        assert!(matches!(self.session.poll(&self.engine, true, true).await, SessionEvent::Ready));
        assert_eq!(self.session.phase(), SessionPhase::Idle);
        assert!(!self.session.pending());
        let durable: Journal = serde_json::from_slice(&std::fs::read(&self.session.path).unwrap()).unwrap();
        assert!(durable.pending.is_none());
        let fields = self.plc.fields();
        assert_eq!(fields["visionReady"], true);
        for tag in ["armed", "busy", "done", "visionFault"] { assert_eq!(fields[tag], false, "{tag}"); }
        assert!(self.audit().iter().any(|record| record["action"] == "resetCompleted"
            && record["pending"]["request"] == json!(request) && record["pending"]["cycleId"] == json!(cycle_id)
            && record["pending"]["result"].is_null() && record["pending"]["acknowledged"] == false));
    }

    async fn report(&mut self) {
        self.session.report(&self.engine, 1, 0).await.unwrap();
        assert_eq!(self.session.phase(), SessionPhase::AwaitAck);
        let fields = self.plc.fields();
        assert_eq!(fields["done"], true);
        assert_eq!(fields["busy"], true);
        assert_eq!(fields["visionReady"], false);
        assert_eq!(fields["resultSeq"], self.session.request().unwrap().request_seq);
    }

    async fn ack_release(&mut self) {
        self.plc.control(json!({"op":"plc_ack"}));
        self.phase(SessionPhase::Releasing).await;
        assert!(self.session.pending());
        assert!(self.session.journal.pending.as_ref().unwrap().acknowledged);
        self.plc.control(json!({"op":"plc_release"}));
        self.phase(SessionPhase::Idle).await;
        assert!(!self.session.pending());
    }

    fn audit(&self) -> Vec<Value> {
        std::fs::read_to_string(self.session.path.with_extension("audit.jsonl")).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    async fn finish(&mut self) {
        self.plc.save("final");
        println!("S7 artifacts: {}", self.plc.directory.display());
        self.engine.disconnect().await;
    }
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_same_sn_distinct_sequences_and_held_result() {
    let mut rig = Rig::new("same-sn").await;
    for sequence in [1, 2] {
        rig.request(sequence, 12345).await;
        rig.arm().await;
        rig.end().await;
        rig.report().await;
        for _ in 0..5 { rig.session.poll(&rig.engine, false, true).await; }
        assert_eq!(rig.session.phase(), SessionPhase::AwaitAck);
        assert_eq!(rig.plc.fields()["done"], true);
        rig.ack_release().await;
        assert_eq!(rig.plc.fields()["resultSn"], 12345);
        assert_eq!(rig.plc.fields()["resultSeq"], sequence);
    }
    let records = rig.audit().into_iter().filter(|r| r["action"] == "acknowledged").collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["pending"]["request"]["requestSeq"], 1);
    assert_eq!(records[1]["pending"]["result"]["requestSeq"], 2);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_end_extra_count_rejects_completion_and_requires_neutral_reset() {
    let mut rig = Rig::new("end-extra-count").await;
    assert_eq!(rig.plan.camera_shots, [2, 1, 1]);
    rig.request(1, 50).await;
    rig.session.bind_cycle_id("00000000000000000000000000000001").unwrap();
    rig.arm().await;
    rig.reject_end_and_reset([3, 1, 1]).await;
    rig.request(2, 50).await;
    rig.session.bind_cycle_id("00000000000000000000000000000002").unwrap();
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_end_unused_slots_reject_completion_and_require_neutral_reset() {
    let mut rig = Rig::new("end-unused-slot").await;
    let mut shots = rig.plan.shots.clone();
    for shot in &mut shots { shot.camera_id = "cam1".into(); }
    rig.plan = PlcPlan::compile("part-A".into(), 7, ["cam1".into(), String::new(), String::new()], shots).unwrap();
    assert_eq!(rig.plan.camera_shots, [4, 0, 0]);
    for (index, unused_slot) in [1usize, 2].into_iter().enumerate() {
        let sequence = index as u32 * 2 + 1;
        rig.request(sequence, 50).await;
        rig.session.bind_cycle_id(&format!("{sequence:032x}")).unwrap();
        rig.arm().await;
        let mut counts = [4, 0, 0];
        counts[unused_slot] = 1;
        rig.reject_end_and_reset(counts).await;
        rig.request(sequence + 1, 50).await;
        rig.session.bind_cycle_id(&format!("{:032x}", sequence + 1)).unwrap();
        rig.arm().await;
        rig.end_counts([4, 0, 0]).await;
        rig.report().await;
        rig.ack_release().await;
    }
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_plan_mismatch_refusal_still_requires_ack() {
    let mut rig = Rig::new("plan-mismatch").await;
    rig.request(1, 50).await;
    let mut wrong = rig.plan.clone();
    wrong.plan_hash ^= 1;
    assert!(rig.session.validate_plan(&wrong).is_err());
    assert!(rig.session.arm(&rig.engine, &wrong).await.is_err());
    assert_eq!(rig.plc.fields()["armed"], false);
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    rig.session.report(&rig.engine, 90, 94).await.unwrap();
    assert_eq!(rig.plc.fields()["visionReady"], false);
    assert_eq!(rig.plc.fields()["busy"], true);
    assert_eq!(rig.plc.fields()["done"], true);
    rig.ack_release().await;
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_late_ack_cannot_release_next_same_sn() {
    let mut rig = Rig::new("late-ack").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.ack_release().await;
    rig.request(2, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.control(json!({"op":"plc_ack","values":{"ackSeq":1}}));
    rig.phase(SessionPhase::Fault).await;
    assert_eq!(rig.plc.fields()["done"], true);
    assert_eq!(rig.plc.fields()["busy"], true);
    assert_eq!(rig.session.result().unwrap().request_seq, 2);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_early_ack_and_active_reset_preserve_pending() {
    let mut rig = Rig::new("early-ack-reset").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.plc.control(json!({"op":"plc_ack","values":{"ackSeq":1}}));
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Fault(_)));
    assert!(rig.session.pending());
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    rig.request(2, 50).await;
    rig.arm().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Fault(_)));
    assert!(rig.session.pending());
    assert_eq!(rig.session.request().unwrap().request_seq, 2);
    assert_eq!(rig.plc.fields()["busy"], true);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_result_write_rejection_retains_envelope_and_fault_is_not_repeated() {
    let mut rig = Rig::new("result-write-reject").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.plc.fault(68, None, "reject", false);
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.session.result().unwrap().result_code, 1);
    assert_eq!(rig.plc.fields()["done"], false);
    assert_eq!(rig.plc.fields()["busy"], true);
    let modified = std::fs::metadata(&rig.session.path).unwrap().modified().unwrap();
    let writes_before = rig.plc.control(json!({"op":"status"}))["counts"]["writes"].as_u64().unwrap();
    for _ in 0..5 {
        rig.session.poll(&rig.engine, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let writes_after = rig.plc.control(json!({"op":"status"}))["counts"]["writes"].as_u64().unwrap();
    assert_eq!(modified, std::fs::metadata(&rig.session.path).unwrap().modified().unwrap());
    assert!(writes_after - writes_before <= 2, "only independent PC heartbeat may write again");
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_fault_best_effort_continues_after_ready_rejection() {
    let mut rig = Rig::new("fault-best-effort").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.plc.fault(64, Some(0), "reject", false);
    rig.session.fault(&rig.engine, "injected fault".into()).await;
    let fields = rig.plc.fields();
    assert_eq!(fields["armed"], false);
    assert_eq!(fields["visionFault"], true);
    assert_eq!(fields["busy"], true);
    assert!(rig.session.pending());
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_disconnect_after_done_applied_never_commits_success() {
    let mut rig = Rig::new("disconnect-after-apply").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.plc.fault(64, Some(3), "disconnect", true);
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.plc.fields()["done"], true);
    assert!(rig.session.pending());
    let saved: Value = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
    assert_eq!(saved["pending"]["result"]["requestSeq"], 1);
    if rig.engine.status().state == LinkState::Disconnected { rig.engine.connect().await; }
    rig.connected().await;
    rig.live().await;
    assert!(matches!(rig.session.phase(), SessionPhase::ResetRequired | SessionPhase::Fault));
    assert_eq!(rig.plc.fields()["done"], true);
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert!(!rig.session.pending());
    assert!(rig.audit().iter().any(|r| r["action"] == "resetCompleted" && r["pending"]["result"]["requestSeq"] == 1));
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_release_write_failure_does_not_finish_transaction() {
    let mut rig = Rig::new("release-write-reject").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.fault(64, Some(2), "reject", false);
    rig.plc.control(json!({"op":"plc_ack"}));
    rig.phase(SessionPhase::Fault).await;
    assert_eq!(rig.plc.fields()["done"], false);
    assert_eq!(rig.plc.fields()["busy"], true);
    assert!(rig.session.journal.pending.as_ref().unwrap().acknowledged);
    assert_eq!(rig.session.result().unwrap().request_seq, 1);
    assert!(!rig.audit().iter().any(|r| r["action"] == "acknowledged"));
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_ack_journal_write_failure_preserves_acknowledgement() {
    let mut rig = Rig::new("ack-journal-write-failure").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    let temporary = rig.session.path.with_extension("pending.tmp");
    std::fs::create_dir(&temporary).unwrap();
    assert!(!rig.session.acknowledged());
    rig.plc.control(json!({"op":"plc_ack"}));
    rig.fresh().await;

    let event = rig.session.poll(&rig.engine, false, true).await;
    assert!(matches!(event, SessionEvent::Fault(ref error) if error.contains("无法保存握手事务")), "{event:?}");
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.acknowledged());
    assert!(rig.session.pending());
    assert_eq!(rig.session.result().unwrap().request_seq, 1);
    assert_eq!(rig.session.journal.pending.as_ref().unwrap().phase, SessionPhase::Fault);
    let durable: Journal = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
    assert!(!durable.pending.unwrap().acknowledged);
    let fields = rig.plc.fields();
    assert_eq!(fields["done"], true);
    assert_eq!(fields["busy"], true);
    assert_eq!(fields["visionFault"], true);
    assert_eq!(fields["visionReady"], false);

    std::fs::remove_dir(&temporary).unwrap();
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
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
#[ignore = "requires Python and local loopback S7 fixture"]
fn s7_wire_ack_journal_monitor_failure_preserves_acknowledgement() {
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap().block_on(async {
        let mut rig = Rig::new("ack-journal-monitor-failure").await;
        rig.request(1, 50).await;
        rig.arm().await;
        rig.end().await;
        rig.report().await;
        assert!(!rig.session.acknowledged());
        rig.plc.control(json!({"op":"plc_ack"}));
        rig.plc.control(json!({"op":"heartbeat","enabled":false}));
        rig.fresh().await;
        let (release, wait) = mpsc::channel();
        let (started, started_wait) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(8)).unwrap();
        });
        started_wait.await.unwrap();

        let observe = async {
            tokio::time::sleep(Duration::from_millis(3600)).await;
            let fields = rig.plc.fields();
            assert_eq!(fields["visionFault"], true, "monitor must fault before the ACK journal write completes");
            assert_eq!(fields["done"], true);
            assert_eq!(fields["busy"], true);
            release.send(()).unwrap();
        };
        let (event, ()) = tokio::join!(rig.session.poll(&rig.engine, false, true), observe);
        blocker.await.unwrap();
        assert!(matches!(event, SessionEvent::Fault(ref error) if error.contains("心跳")), "{event:?}");
        assert_eq!(rig.session.phase(), SessionPhase::Fault);
        assert!(rig.session.acknowledged());
        assert!(rig.session.pending());
        assert_eq!(rig.session.result().unwrap().request_seq, 1);
        let durable: Journal = serde_json::from_slice(&std::fs::read(&rig.session.path).unwrap()).unwrap();
        let pending = durable.pending.unwrap();
        assert!(pending.acknowledged);
        assert_eq!(pending.phase, SessionPhase::Fault);

        rig.plc.control(json!({"op":"heartbeat","enabled":true}));
        rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
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
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_heartbeat_stop_faults_without_clearing_pending() {
    let mut rig = Rig::new("heartbeat-stop").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.plc.control(json!({"op":"heartbeat","enabled":false}));
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.view().message.unwrap().contains("心跳"));
    assert_eq!(rig.plc.fields()["armed"], false);
    assert_eq!(rig.plc.fields()["busy"], true);
    assert!(rig.session.pending());
    rig.plc.control(json!({"op":"heartbeat","enabled":true}));
    rig.live().await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_restart_pending_requires_neutral_explicit_reset_and_audit() {
    let mut rig = Rig::new("restart-pending").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.session = PlcSession::open(rig.session.path.clone());
    rig.live().await;
    assert!(rig.session.pending());
    assert_eq!(rig.session.phase(), SessionPhase::ResetRequired);
    assert_eq!(rig.plc.fields()["done"], true);
    rig.session.poll(&rig.engine, true, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.plc.fields()["done"], true);
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
    rig.fresh().await;
    rig.session.poll(&rig.engine, false, true).await;
    assert!(rig.session.pending());
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert_eq!(rig.plc.fields()["done"], false);
    assert!(!rig.session.pending());
    assert!(rig.audit().iter().any(|r| r["action"] == "resetCompleted" && r["pending"]["result"]["resultCode"] == 1));
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready), "Idle permits an explicit neutral reset: {:?}", rig.session.view());
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_restart_after_persisted_ack_recovers_exact_cycle_before_and_after_reset() {
    let mut rig = Rig::new("restart-ack-cycle").await;
    let cycle_id = "0123456789abcdef0123456789abcdef";
    rig.request(7, 50).await;
    rig.session.bind_cycle_id(cycle_id).unwrap();
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.control(json!({"op":"plc_ack"}));
    rig.phase(SessionPhase::Releasing).await;
    assert!(rig.session.acknowledged());

    rig.session = PlcSession::open(rig.session.path.clone());
    assert_eq!(rig.session.cycle_id(), Some(cycle_id));
    let recovered = rig.session.recover_acknowledgements();
    assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
    assert_eq!(recovered.receipts.len(), 1);
    assert_eq!(recovered.receipts[0].cycle_id, cycle_id);
    assert_eq!(recovered.receipts[0].sn, 50);
    assert_eq!(recovered.receipts[0].request_seq, 7);
    assert!(recovered.receipts[0].ts > 0);
    assert_eq!(rig.session.recover_acknowledgements(), recovered);
    rig.live().await;
    assert_eq!(rig.session.phase(), SessionPhase::ResetRequired);
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    assert!(!rig.session.pending());
    rig.session = PlcSession::open(rig.session.path.clone());
    assert_eq!(rig.session.cycle_id(), None);
    let audit_recovered = rig.session.recover_acknowledgements();
    assert!(audit_recovered.errors.is_empty(), "{:?}", audit_recovered.errors);
    assert_eq!(audit_recovered.receipts.len(), 1);
    assert_eq!(audit_recovered.receipts[0].cycle_id, cycle_id);
    assert_eq!(audit_recovered.receipts[0].sn, 50);
    assert_eq!(audit_recovered.receipts[0].request_seq, 7);
    assert!(audit_recovered.receipts[0].ts >= recovered.receipts[0].ts);
    assert_eq!(rig.session.recover_acknowledgements(), audit_recovered);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
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
    assert_eq!(rig.plc.fields()["visionReady"], false);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_accepted_outputs_and_done_are_continuously_verified() {
    let mut rig = Rig::new("output-tamper").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.plc.values(json!({"acceptedSeq":55}));
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
    rig.fresh().await;
    assert!(matches!(rig.session.poll(&rig.engine, true, true).await, SessionEvent::Ready));
    rig.request(2, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.plc.values(json!({"done":false}));
    rig.phase(SessionPhase::Fault).await;
    assert_eq!(rig.session.result().unwrap().request_seq, 2);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_slow_durable_io_keeps_observing_heartbeat() {
    let mut rig = Rig::new("slow-durable-io").await;
    rig.session.blocking_io(&rig.engine, || {
        std::thread::sleep(Duration::from_millis(3500));
        Ok(())
    }).await.unwrap();
    rig.session.poll(&rig.engine, false, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(rig.session.heartbeat.lock().unwrap().is_live(now_ms()));
    rig.finish().await;
}

#[test]
#[ignore = "requires Python and local loopback S7 fixture"]
fn s7_wire_slow_end_journal_does_not_extend_frame_deadline() {
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap().block_on(async {
        let mut rig = Rig::new("slow-end-frame-deadline").await;
        rig.request(1, 50).await;
        rig.arm().await;
        rig.plc.values(json!({"partEnd":true,"camera1Triggers":2,"camera2Triggers":1,"camera3Triggers":1}));
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
                frame_counter: 9002, trigger_counter: 2, lost_packets: 0, ts: now_ms(), manual: false, images: Vec::new(),
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
        let fields = rig.plc.fields();
        assert_eq!(fields["resultCode"], 90);
        assert_eq!(fields["faultCode"], 91);
        assert_eq!(fields["done"], true);
        rig.finish().await;
    });
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_heartbeat_fault_stops_outputs_before_slow_io_finishes() {
    let mut rig = Rig::new("heartbeat-during-io").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.plc.control(json!({"op":"heartbeat","enabled":false}));
    let (release, wait) = mpsc::channel();
    let operation = rig.session.blocking_io(&rig.engine, move || {
        wait.recv_timeout(Duration::from_secs(8)).map_err(|e| e.to_string())
    });
    let observe = async {
        tokio::time::sleep(Duration::from_millis(3600)).await;
        let fields = rig.plc.fields();
        assert_eq!(fields["armed"], false, "stop outputs before releasing the blocked IO task");
        assert_eq!(fields["visionFault"], true);
        assert_eq!(fields["busy"], true);
        release.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(operation, observe);
    assert!(result.unwrap_err().contains("心跳"));
    rig.phase(SessionPhase::Fault).await;
    assert!(rig.session.pending());
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_unconfirmable_fault_stops_pc_heartbeat() {
    let mut rig = Rig::new("unconfirmable-fault").await;
    rig.request(1, 50).await;
    rig.arm().await;
    for bit in [0, 1, 4] { rig.plc.fault(64, Some(bit), "reject", false); }
    rig.session.fault(&rig.engine, "stop outputs rejected".into()).await;
    assert_eq!(rig.engine.status().state, LinkState::Disconnected);
    assert!(rig.session.pending());
    let writes = rig.plc.control(json!({"op":"status"}))["counts"]["writes"].clone();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(rig.plc.control(json!({"op":"status"}))["counts"]["writes"], writes);
    rig.finish().await;
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_audit_failure_blocks_destructive_reset() {
    let mut rig = Rig::new("audit-failure").await;
    rig.request(1, 50).await;
    rig.arm().await;
    rig.end().await;
    rig.report().await;
    rig.session.fault(&rig.engine, "operator recovery".into()).await;
    rig.plc.values(json!({"partStart":false,"partEnd":false,"resultAck":false}));
    rig.fresh().await;
    let audit = rig.session.path.with_extension("audit.jsonl");
    std::fs::rename(&audit, rig.plc.directory.join("preserved-audit.jsonl")).unwrap();
    std::fs::create_dir(&audit).unwrap();
    rig.session.poll(&rig.engine, true, true).await;
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert_eq!(rig.plc.fields()["done"], true);
    assert_eq!(rig.session.result().unwrap().result_code, 1);
    rig.finish().await;
}

#[test]
fn unfinished_temporary_journal_blocks_recovery_even_when_main_file_exists() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/s7-session-tests")
        .join(format!("unfinished-journal-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("session.json");
    std::fs::write(&path, serde_json::to_vec(&Journal { version: 1, ..Journal::default() }).unwrap()).unwrap();
    std::fs::write(path.with_extension("pending.tmp"), b"unfinished transaction").unwrap();
    let session = PlcSession::open(path);
    assert!(session.pending());
    assert!(session.load_error.unwrap().contains("未提交"));
}

#[tokio::test]
#[ignore = "requires Python and local loopback S7 fixture"]
async fn s7_wire_reconnect_between_validation_and_arm_cannot_commit() {
    let mut rig = Rig::new("reconnect-before-arm").await;
    rig.request(1, 50).await;
    let original = rig.session.connection.unwrap();
    rig.engine.disconnect().await;
    rig.engine.connect().await;
    rig.connected().await;
    assert_ne!(rig.engine.status().since, original);
    assert!(rig.session.arm(&rig.engine, &rig.plan).await.is_err());
    assert_eq!(rig.session.phase(), SessionPhase::Fault);
    assert!(rig.session.pending());
    assert_eq!(rig.plc.fields()["armed"], false);
    assert_eq!(rig.plc.fields()["acceptedSeq"], 0);
    rig.finish().await;
}

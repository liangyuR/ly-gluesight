use super::*;
use crate::offline::s7_server::{Fault, S7Server, SharedMemory, WriteTarget};
use crate::offline::test_dir::TestDir;
use crate::offline::test_layout::Layout;
use ly_plc::{EventSink, Logbook};
use std::sync::Arc;

/// PC 区（DB2）上的 resultSeq（DBD8）
const RESULT_SEQ_DWORD: WriteTarget = WriteTarget {
    db: 2,
    byte: 8,
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
        let server = S7Server::start(
            memory.clone(),
            Some((layout.heartbeat(), Duration::from_millis(100))),
        )
        .await
        .unwrap();
        Self {
            server,
            memory,
            layout,
        }
    }

    fn get(&self, field: &str) -> u32 {
        self.layout.get(&self.memory.lock().unwrap(), field)
    }

    fn text(&self, field: &str) -> String {
        self.layout.get_text(&self.memory.lock().unwrap(), field)
    }

    fn set(&self, values: &[(&str, u32)]) {
        let mut memory = self.memory.lock().unwrap();
        for (field, value) in values {
            self.layout.set(&mut memory, field, *value);
        }
    }

    /// 工件到达：型号、SN、序号先写，最后置 inspecting=1
    fn arrive(&self, seq: u32, sn: &str) {
        let mut memory = self.memory.lock().unwrap();
        self.layout.set_text(&mut memory, tag::SN, sn);
        for (field, value) in [
            (tag::PRODUCT_CODE, 1),
            (tag::PART_SEQ, seq),
            (tag::INSPECTING, 1),
        ] {
            self.layout.set(&mut memory, field, value);
        }
    }

    /// 工件结束
    fn done(&self) {
        self.set(&[(tag::INSPECTING, 0)]);
    }

    /// PLC 按协议取到的结果：没有在检件、resultSeq 等于本件序号时为 (resultCode, faultCode)
    fn result(&self) -> Option<(u32, u32)> {
        (self.get(tag::INSPECTING) == 0 && self.get(tag::RESULT_SEQ) == self.get(tag::PART_SEQ))
            .then(|| (self.get(tag::RESULT_CODE), self.get(tag::FAULT_CODE)))
    }

    fn fault(&self, fault: Fault) {
        self.server.inject(fault);
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
        Self::with(name, |_| {}, |_| {}).await
    }

    /// `files` 在打开会话前改写会话目录（如预置损坏的握手日志），`plc` 在 PC 连上前布置 PLC 的 DB。
    async fn with(name: &str, files: impl FnOnce(&Path), prepare: impl FnOnce(&TestPlc)) -> Self {
        let plc = TestPlc::start().await;
        prepare(&plc);
        let directory = TestDir::new("s7-session-tests", name);
        files(&directory);
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
        rig.ready().await;
        rig
    }

    /// 轮询到出现满足 `wanted` 的事件；期间会话掉到未就绪（`wanted` 等的不是就绪）即失败。
    async fn until<T>(&mut self, what: &str, wanted: impl Fn(SessionEvent) -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(6);
        loop {
            let event = self.session.poll(&self.engine).await;
            let text = format!("{event:?}");
            if let Some(found) = wanted(event) {
                return found;
            }
            assert!(
                Instant::now() < until,
                "等不到{what}：{text} {:?}",
                self.session.view()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 不需要任何操作，通讯条件满足后自动就绪。
    async fn ready(&mut self) {
        self.until("通讯就绪", |event| {
            matches!(event, SessionEvent::Ready).then_some(())
        })
        .await;
        assert_eq!(self.session.phase(), SessionPhase::Idle);
    }

    async fn not_ready(&mut self) {
        let until = Instant::now() + Duration::from_secs(6);
        while self.session.phase() != SessionPhase::NotReady {
            self.session.poll(&self.engine).await;
            assert!(
                Instant::now() < until,
                "会话没有回到未就绪：{:?}",
                self.session.view()
            );
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

    /// 连续几次新轮询都没有事件：PC 没有接件，也没有离开当前阶段。
    async fn quiet(&mut self) {
        for _ in 0..5 {
            self.fresh().await;
            let event = self.session.poll(&self.engine).await;
            assert!(
                matches!(event, SessionEvent::None),
                "{event:?} {:?}",
                self.session.view()
            );
        }
    }

    async fn started(&mut self) -> Request {
        let request = self
            .until("新件", |event| match event {
                SessionEvent::Start(request) => Some(request),
                _ => None,
            })
            .await;
        assert_eq!(self.session.phase(), SessionPhase::Validating);
        request
    }

    async fn arrive(&mut self, seq: u32, sn: &str) {
        self.plc.arrive(seq, sn);
        let request = self.started().await;
        assert_eq!((request.request_seq, request.sn.as_str()), (seq, sn));
    }

    /// 接件登记：核对计划后进入采集，不写 PLC。
    async fn admit(&mut self) {
        self.session.validate_plan(&self.plan).unwrap();
        self.session.admit(None).await;
        assert_eq!(self.session.phase(), SessionPhase::Acquiring);
    }

    async fn end(&mut self) {
        self.plc.done();
        self.until("工件结束", |event| {
            matches!(event, SessionEvent::End(_)).then_some(())
        })
        .await;
        assert_eq!(self.session.phase(), SessionPhase::Draining);
    }

    /// 一件走到收尾：到达、登记、结束。
    async fn part(&mut self, seq: u32, sn: &str) {
        self.arrive(seq, sn).await;
        self.admit().await;
        self.end().await;
    }

    async fn finish(&mut self) {
        self.engine.disconnect().await;
    }

    async fn part_events(&self, seq: u32, sn: &str, final_event: &str) -> Vec<serde_json::Value> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let query = ly_plc::LogQuery {
            categories: vec!["part".into()],
            context: Some(format!("partSeq={seq}")),
            ..Default::default()
        };
        loop {
            let page = self.engine.logbook().query(&query).unwrap();
            let events: Vec<serde_json::Value> = page
                .items
                .iter()
                .rev()
                .map(|row| serde_json::from_str(row.new_value.as_deref().unwrap()).unwrap())
                .collect();
            if events
                .last()
                .is_some_and(|e| e["event"] == final_event && e["sn"] == sn)
            {
                return events;
            }
            assert!(Instant::now() < deadline, "工件通讯事件：{events:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// 协议 9 正常一件：PC 接件、登记都不写 PLC；工件结束后写结果，PLC 按序号取到；写完 PC 即回空闲，不等 PLC 任何确认，下一件照常。
#[tokio::test]
async fn part_result_is_taken_by_sequence_and_pc_returns_idle_without_plc_confirmation() {
    let mut rig = Rig::new("normal-part").await;
    for (seq, sn, code) in [(1, "SN-A", 1), (2, "SN-B", 12)] {
        rig.arrive(seq, sn).await;
        rig.admit().await;
        assert_ne!(rig.plc.get(tag::RESULT_SEQ), seq, "工件结束前不得写结果");
        rig.end().await;
        rig.session.report(&rig.engine, code, 0).await.unwrap();
        assert_eq!(rig.plc.result(), Some((code as u32, 0)));
        assert_eq!(rig.plc.text(tag::SN_ECHO), sn);
        let events = rig.part_events(seq, sn, "resultWritten").await;
        assert_eq!(
            events.iter().map(|e| &e["event"]).collect::<Vec<_>>(),
            ["received", "ended", "resultWritten"]
        );
        assert!(events
            .iter()
            .all(|e| e["sn"] == sn && e["receivedAt"] == events[0]["receivedAt"]));
        assert_eq!(events[2]["resultCode"], code);
        assert_eq!(
            (rig.session.phase(), rig.session.pending()),
            (SessionPhase::Idle, false)
        );
    }
    rig.finish().await;
}

/// PC 就绪时 PLC 已经在检的件没有帧计数基线：不登记、不回结果，等它结束；下一件照常接。
#[tokio::test]
async fn part_already_inspecting_when_pc_becomes_ready_is_skipped_and_next_part_taken() {
    let mut rig = Rig::with("skip-in-flight", |_| {}, |plc| plc.arrive(5, "SN-EARLY")).await;
    assert!(rig
        .session
        .notice()
        .is_some_and(|notice| notice.contains("5")));
    rig.quiet().await;
    rig.plc.done();
    rig.quiet().await;
    assert_eq!(rig.plc.result(), None);
    assert_eq!(rig.session.notice(), None);
    rig.arrive(6, "SN-NEXT").await;
    rig.finish().await;
}

/// 中途换件：在检期间序号变了（没收到工件结束），本件按中断放下、不回结果，新件照常登记。
#[tokio::test]
async fn sequence_change_while_inspecting_loses_part_and_new_part_is_taken() {
    let mut rig = Rig::new("lost-part").await;
    rig.arrive(1, "SN-1").await;
    rig.admit().await;
    rig.plc.arrive(2, "SN-2");
    rig.until("中途换件", |event| {
        matches!(event, SessionEvent::Lost(_)).then_some(())
    })
    .await;
    rig.session.settle(&rig.engine, "中途换件").await;
    let request = rig.started().await;
    assert_eq!(request.request_seq, 2);
    assert_eq!(rig.plc.get(tag::RESULT_SEQ), 0, "被换掉的件不回结果");
    rig.finish().await;
}

/// 等不到工件结束而放下的件：还在检时不再重新接它，也不回结果；它结束后的下一件照常接，
/// 即使 PLC 重置过序号、新件与它同号。
#[tokio::test]
async fn settled_part_is_not_retaken_while_inspecting_but_same_sequence_after_end_is_a_new_part() {
    let mut rig = Rig::new("settled-part").await;
    rig.arrive(3, "SN-SLOW").await;
    rig.admit().await;
    rig.session.settle(&rig.engine, "工件结束超时").await;
    rig.quiet().await;
    rig.plc.done();
    rig.quiet().await;
    assert_eq!(rig.plc.result(), None);
    rig.arrive(3, "SN-AFTER-RESET").await;
    rig.finish().await;
}

/// 旧结果不被误取：PLC 重置序号后新件的序号撞上 DB2 里留着的 resultSeq，PC 在它在检期间把 resultSeq 清零，
/// 工件结束时 PLC 取不到上一件的结果。
#[tokio::test]
async fn stale_result_with_the_new_parts_sequence_is_cleared_while_it_is_inspecting() {
    let mut rig = Rig::new("stale-result").await;
    rig.part(1, "SN-OLD").await;
    rig.session.report(&rig.engine, 11, 0).await.unwrap();
    rig.arrive(1, "SN-NEW").await;
    assert_eq!(rig.plc.get(tag::RESULT_SEQ), 0);
    rig.admit().await;
    rig.end().await;
    assert_eq!(rig.plc.result(), None);
    rig.session.report(&rig.engine, 1, 0).await.unwrap();
    assert_eq!(rig.plc.result(), Some((1, 0)));
    let events = rig.part_events(1, "SN-NEW", "resultWritten").await;
    let received = events
        .iter()
        .find(|e| e["event"] == "received" && e["sn"] == "SN-NEW")
        .unwrap();
    let observed_at = received["observedAt"].as_i64().unwrap();
    let sn_changes = rig
        .engine
        .logbook()
        .query(&ly_plc::LogQuery {
            categories: vec!["value".into()],
            point_id: Some("p_sn".into()),
            context: Some("partSeq=1".into()),
            ..Default::default()
        })
        .unwrap();
    let sn_timestamp = |sn: &str| {
        sn_changes
            .items
            .iter()
            .find(|r| r.new_value.as_deref() == Some(sn))
            .unwrap()
            .ts
    };
    assert!(sn_timestamp("SN-OLD") < observed_at);
    assert!(observed_at <= sn_timestamp("SN-NEW"));
    rig.finish().await;
}

/// PLC 不等 PC：上一件结果还没出、下一件已到达时，上一件按中断放下、不回结果，新件马上登记（不等上一件收尾，
/// 否则新件的帧计数基线建晚了会丢帧）；PLC 重置序号后与上一件同号的新件同样处理。
#[tokio::test]
async fn next_part_arriving_before_result_loses_previous_part_and_is_taken_at_once() {
    for (name, next) in [("next-part-new-seq", 2), ("next-part-same-seq", 1)] {
        let mut rig = Rig::new(name).await;
        rig.part(1, "SN-1").await;
        rig.plc.arrive(next, "SN-NEXT");
        rig.until("上一件被换下", |event| {
            matches!(event, SessionEvent::Lost(_)).then_some(())
        })
        .await;
        rig.session.settle(&rig.engine, "收尾期间换件").await;
        let request = rig.started().await;
        assert_eq!(
            (request.request_seq, request.sn.as_str()),
            (next, "SN-NEXT")
        );
        assert_eq!(rig.plc.get(tag::RESULT_SEQ), 0, "{name}");
        rig.finish().await;
    }
}

/// 结果写出前的最后一次核对：PLC 已切到下一件时本件结果不再回写（交付失败），下一件照常登记。
#[tokio::test]
async fn result_arriving_after_plc_moved_on_is_not_written() {
    let mut rig = Rig::new("late-result").await;
    rig.part(1, "SN-1").await;
    rig.plc.arrive(2, "SN-2");
    rig.fresh().await;
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert_eq!(rig.plc.get(tag::RESULT_SEQ), 0);
    assert_eq!(rig.started().await.request_seq, 2);
    rig.finish().await;
}

/// PC 只在工件结束后写结果（清除旧结果的规则依赖这一点）：还在检的件要求回写时拒绝并放下，不写 PLC。
#[tokio::test]
async fn result_before_part_end_is_refused_and_writes_nothing() {
    let mut rig = Rig::new("early-result").await;
    rig.arrive(1, "SN-1").await;
    rig.admit().await;
    assert!(rig.session.report(&rig.engine, 90, 98).await.is_err());
    assert_eq!(rig.plc.get(tag::RESULT_SEQ), 0);
    rig.quiet().await;
    rig.finish().await;
}

/// PLC 来件后不等 PC：接件登记还没完成工件就已结束，登记照常完成，下一次轮询即报工件结束。
#[tokio::test]
async fn part_end_before_admit_is_reported_after_admit() {
    let mut rig = Rig::new("end-before-admit").await;
    rig.arrive(1, "SN-FAST").await;
    rig.plc.done();
    rig.quiet().await;
    rig.admit().await;
    rig.fresh().await;
    let event = rig.session.poll(&rig.engine).await;
    assert!(matches!(event, SessionEvent::End(_)), "{event:?}");
    rig.finish().await;
}

/// 通讯条件不满足即未就绪，恢复后不需要任何复位自动就绪；中断时在检的那一件不重新接。
/// 两种失去条件的方式：PLC 心跳停、连接断开后重连。
#[tokio::test]
async fn lost_communication_makes_session_not_ready_and_recovery_needs_no_reset() {
    for (name, lose, recover) in [
        (
            "heartbeat-stop",
            Fault::HeartbeatStop,
            Some(Fault::HeartbeatResume),
        ),
        ("reconnect", Fault::Disconnect, None),
    ] {
        let mut rig = Rig::new(name).await;
        rig.arrive(1, "SN-1").await;
        rig.admit().await;
        rig.plc.fault(lose);
        rig.not_ready().await;
        assert!(rig.session.view().message.is_some());
        rig.session.settle(&rig.engine, "通讯异常").await;
        if let Some(recover) = recover {
            rig.plc.fault(recover);
        }
        rig.ready().await;
        rig.quiet().await;
        rig.plc.done();
        rig.quiet().await;
        assert_eq!(rig.plc.result(), None, "{name}");
        rig.arrive(2, "SN-2").await;
        if name == "heartbeat-stop" {
            rig.part_events(2, "SN-2", "received").await;
            let link = rig
                .engine
                .logbook()
                .query(&ly_plc::LogQuery {
                    categories: vec!["connection".into()],
                    ..Default::default()
                })
                .unwrap();
            let warnings: Vec<_> = link
                .items
                .iter()
                .filter(|row| row.level == "warn")
                .collect();
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].context.is_none());
            assert!(link.items.iter().any(|row| row.id > warnings[0].id
                && row.level == "info"
                && row.context.is_none()));
        }
        rig.finish().await;
    }
}

/// 写结果时连接断开、PC 确认不了写入：本件按交付失败结束，不重写；通讯恢复后自动就绪，下一件照常。
#[tokio::test]
async fn disconnect_while_writing_result_fails_delivery_and_session_recovers() {
    let mut rig = Rig::new("disconnect-on-result").await;
    rig.part(1, "SN-1").await;
    rig.plc
        .fault(Fault::DisconnectAfterWriteTo(RESULT_SEQ_DWORD));
    assert!(rig.session.report(&rig.engine, 1, 0).await.is_err());
    assert!(!rig.session.pending());
    let events = rig.part_events(1, "SN-1", "resultFailed").await;
    assert!(events.iter().all(|event| event["event"] != "resultWritten"));
    assert_eq!(events.last().unwrap()["sn"], "SN-1");
    rig.ready().await;
    rig.arrive(2, "SN-2").await;
    rig.finish().await;
}

/// SN 不合法的件没有可登记的身份：不登记、不回结果，通讯仍然就绪，下一件照常接。
#[tokio::test]
async fn part_with_invalid_sn_is_skipped_without_losing_readiness() {
    let mut rig = Rig::new("invalid-sn").await;
    rig.plc.arrive(1, "");
    rig.quiet().await;
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    assert!(rig.session.notice().is_some());
    rig.plc.done();
    rig.quiet().await;
    rig.arrive(2, "SN-OK").await;
    rig.finish().await;
}

/// 维护模式在接件时锁存：维护件按 pcMode=1、结果码 0 回写，期间切回正常模式不影响本件；
/// 写完后空闲时 pcMode 回到当前模式。正常件不能回结果码 0（0 只属于维护模式）。
#[tokio::test]
async fn maintenance_mode_is_latched_at_arrival_and_result_code_0_belongs_to_it_only() {
    let mut rig = Rig::new("maintenance").await;
    rig.session.set_maintenance(true);
    rig.part(1, "SN-M").await;
    assert!(rig.session.part_maintenance());
    rig.session.set_maintenance(false);
    rig.session.report(&rig.engine, 0, 0).await.unwrap();
    assert_eq!(
        (rig.plc.result(), rig.plc.get(tag::PC_MODE)),
        (Some((0, 0)), 1)
    );
    rig.quiet().await;
    assert_eq!(rig.plc.get(tag::PC_MODE), 0);
    rig.part(2, "SN-N").await;
    assert!(rig.session.report(&rig.engine, 0, 0).await.is_err());
    assert_eq!(rig.plc.get(tag::RESULT_SEQ), 1);
    rig.finish().await;
}

/// PC 输出保持：PLC 重启或 DB 重新初始化把 pcVersion 清零，PC 空闲时重写，不需要人工处理。
#[tokio::test]
async fn overwritten_pc_version_is_rewritten_while_idle() {
    let mut rig = Rig::new("output-repair").await;
    rig.plc.set(&[(tag::PC_VERSION, 0)]);
    rig.fresh().await;
    rig.session.poll(&rig.engine).await;
    assert_eq!(
        rig.plc.get(tag::PC_VERSION),
        handshake::PROTOCOL_VERSION as u32
    );
    assert_eq!(rig.session.phase(), SessionPhase::Idle);
    rig.finish().await;
}

/// 握手日志只为补中断记录：登记后的在途件（身份、场景）重启后读得到；结果写完或按中断放下后，重启不再有在途件。
#[tokio::test]
async fn in_flight_part_survives_restart_until_it_is_reported() {
    let mut rig = Rig::new("journal-restart").await;
    let cycle_id = "0123456789abcdef0123456789abcdef";
    let site = PartSite {
        source: RecordSource::Real,
        record: RecordMode::Off,
    };
    rig.arrive(7, "SN-J").await;
    rig.session.bind_cycle_id(cycle_id, site).unwrap();
    rig.session.admit(Some("R-7")).await;
    let reopened = PlcSession::open(rig.session.path.clone());
    assert_eq!(
        (
            reopened.cycle_id(),
            reopened.request().map(|r| r.sn.as_str()),
            reopened.site(),
            reopened.recipe_id(),
        ),
        (Some(cycle_id), Some("SN-J"), Some(site), Some("R-7"))
    );
    rig.end().await;
    rig.session.report(&rig.engine, 1, 0).await.unwrap();
    assert!(!PlcSession::open(rig.session.path.clone()).pending());
    rig.finish().await;
}

/// 握手日志不可用都不挡通讯：损坏的日志、未提交的临时文件、协议 8 留下的在途件各自改名保留，会话以空日志启动并自动就绪。
#[tokio::test]
async fn unusable_journal_files_are_set_aside_and_session_becomes_ready() {
    let previous = r#"{"version":1,"lastRequestSeq":4,"pending":{"request":{"protocolVersion":8,"requestSeq":4,"sn":"SN-8","productCode":1},"result":null,"phase":"acquiring"}}"#;
    for (name, journal, kept) in [
        ("journal-corrupt", "{ 不是 JSON", "session.corrupt-"),
        ("journal-protocol-8", previous, "session.v8.bak"),
    ] {
        let mut rig = Rig::with(
            name,
            |dir| {
                std::fs::write(dir.join("session.json"), journal).unwrap();
                std::fs::write(dir.join("session.pending.tmp"), "{").unwrap();
            },
            |_| {},
        )
        .await;
        assert!(rig.session.archived().is_some() && !rig.session.pending());
        let names: Vec<String> = std::fs::read_dir(rig.session.path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names
                .iter()
                .any(|n| n == "session.pending.tmp" || n == "session.json"),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.starts_with(kept)), "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("session.pending-")),
            "{names:?}"
        );
        rig.arrive(1, "SN-J1").await;
        rig.finish().await;
    }
}

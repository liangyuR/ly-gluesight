//! PLC 开始请求不合法时的处置：不布防、不生成结果，进入视觉故障，等 PLC 释放后明确复位。
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ly_plc::{DataType, EventSink, Logbook, PlcConfig, PlcEngine};

use super::line::Layout;
use super::s7_server::{S7Server, SharedMemory};
use super::*;
use crate::handshake::{Contract, PROTOCOL_VERSION};
use crate::inspection::tag;
use crate::plc_session::{PlcSession, SessionEvent, SessionPhase};

struct Plc {
    config: PlcConfig,
    memory: SharedMemory,
}

impl Plc {
    fn location(&self, field: &str) -> (u16, u32, Option<u8>, u32, DataType) {
        let contract = Contract::validate(&self.config).unwrap();
        let id = contract.point_id(field).unwrap();
        let point = self.config.points.iter().find(|p| p.id == id).unwrap();
        let location = ly_plc::resolve(&self.config.connection, &point.address, point.data_type).unwrap();
        (location.area.index, location.start, location.bit, location.len, point.data_type)
    }

    fn set(&self, field: &str, value: u32) {
        let (db, byte, bit, len, _) = self.location(field);
        let mut memory = self.memory.lock().unwrap();
        match bit {
            Some(bit) => memory.set_bit(db, byte, bit, value != 0).unwrap(),
            None => memory.write(db, byte, &value.to_be_bytes()[4 - len as usize..]).unwrap(),
        }
    }

    fn set_text(&self, field: &str, value: &str) {
        let (db, byte, _, len, data_type) = self.location(field);
        let header = match data_type { DataType::String => 2, other => panic!("{field} 不是 S7 STRING 点位：{other:?}") };
        let mut bytes = vec![0u8; len as usize];
        if header == 2 { bytes[0] = (len - 2) as u8; }
        bytes[header - 1] = value.len() as u8;
        bytes[header..header + value.len()].copy_from_slice(value.as_bytes());
        self.memory.lock().unwrap().write(db, byte, &bytes).unwrap();
    }

    fn get(&self, field: &str) -> u32 {
        let (db, byte, bit, len, _) = self.location(field);
        let memory = self.memory.lock().unwrap();
        match bit {
            Some(bit) => memory.bit(db, byte, bit).unwrap() as u32,
            None => memory.read(db, byte, len as usize).unwrap().iter().fold(0, |value, byte| (value << 8) | *byte as u32),
        }
    }
}

async fn poll_until(session: &mut PlcSession, engine: &PlcEngine, what: &str, mut accept: impl FnMut(&SessionEvent) -> bool) -> SessionEvent {
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        let event = session.poll(engine, false, true).await;
        if accept(&event) { return event; }
        assert!(Instant::now() < until, "{what}：{:?}", session.view());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 复位后的空闲会话；复位时 PLC 的 requestSeq 为 5，即本会话已用过的最大事务序号。
async fn idle_session(name: &str) -> (S7Server, PlcEngine, PlcSession, Plc) {
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
    let engine = PlcEngine::new(config.clone(), Arc::new(Logbook::open(&directory.join("plc.db"), sink.clone(), 1).unwrap()), sink);
    engine.connect().await;
    let plc = Plc { config, memory };
    plc.set(tag::REQUEST_SEQ, 5);
    let mut session = PlcSession::open(directory.join("session.json"));
    let settle = Instant::now() + Duration::from_millis(400);
    while Instant::now() < settle {
        session.poll(&engine, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    plc.set(tag::FAULT_RESET, 1);
    poll_until(&mut session, &engine, "复位未完成", |event| matches!(event, SessionEvent::Ready)).await;
    plc.set(tag::FAULT_RESET, 0);
    (server, engine, session, plc)
}

#[tokio::test]
async fn invalid_start_request_faults_without_arming_or_result_while_a_valid_one_starts() {
    for (case, request_seq, plan_reserved, accepted) in [
        ("requestSeq 与上次相同", 5, 0, false),
        ("requestSeq 回退", 3, 0, false),
        ("开始校验失败（planReserved 非零）", 6, 1, false),
        ("合法请求", 6, 0, true),
    ] {
        let (_server, engine, mut session, plc) = idle_session("start-request").await;
        plc.set_text(tag::PART_SN, "SN-CORE-1");
        for (field, value) in [(tag::PROTOCOL_VERSION, PROTOCOL_VERSION as u32), (tag::REQUEST_SEQ, request_seq), (tag::PRODUCT_CODE, 1),
            (tag::SHOT_COUNT, 4), (tag::PLAN_VERSION, 7), (tag::PLAN_RESERVED, plan_reserved), (tag::DEVICE_SHOTS, 4),
            (tag::DEVICE_TRIGGERS, 0), (tag::PART_START, 1)] {
            plc.set(field, value);
        }
        let event = poll_until(&mut session, &engine, case, |event| matches!(event, SessionEvent::Start(_) | SessionEvent::Fault(_))).await;
        assert_eq!(matches!(event, SessionEvent::Start(_)), accepted, "{case}：{event:?}");
        if !accepted {
            assert_eq!(session.phase(), SessionPhase::Fault, "{case}");
            assert!(!session.pending(), "{case}：拒收的请求不应留下待处理事务");
            let outputs: Vec<_> = [tag::VISION_FAULT, tag::VISION_READY, tag::ARMED, tag::BUSY, tag::DONE, tag::RESULT_CODE, tag::ACCEPTED_SEQ]
                .into_iter().map(|field| plc.get(field)).collect();
            assert_eq!(outputs, [1, 0, 0, 0, 0, 0, 0], "{case}：visionFault/visionReady/armed/busy/done/resultCode/acceptedSeq");
        }
        engine.disconnect().await;
    }
}

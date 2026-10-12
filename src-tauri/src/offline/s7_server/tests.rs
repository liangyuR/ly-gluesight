use super::*;
use ly_plc::{Access, ConnectionConfig, DataType, EventSink, LinkState, Logbook, PlcConfig, PlcEngine, PlcPoint, PlcValue, ProtocolKind};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn point(id: &str, address: &str, data_type: DataType, access: Access) -> PlcPoint {
    PlcPoint { id: id.into(), name: id.into(), address: address.into(), data_type, access, ..PlcPoint::default() }
}

async fn eventually<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = probe() { return value; }
        assert!(Instant::now() < until, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn ly_plc_s7_client_reads_and_writes_bit_word_dword_and_recovers_after_disconnect() {
    let memory: SharedMemory = Arc::default();
    memory.lock().unwrap().create(5, 64);
    let server = S7Server::start(memory.clone(), None).await.unwrap();
    let config = PlcConfig {
        connection: ConnectionConfig { protocol: ProtocolKind::S7, host: "127.0.0.1".into(), port: server.port(),
            timeout_ms: 250, poll_interval_ms: 20, reconnect_interval_ms: 100, ..ConnectionConfig::default() },
        points: vec![
            point("bit", "DB5.DBX1.3", DataType::Bool, Access::ReadWrite),
            point("word", "DB5.DBW2", DataType::U16, Access::ReadWrite),
            point("dword", "DB5.DBD4", DataType::U32, Access::ReadWrite),
            point("input", "DB5.DBD8", DataType::U32, Access::Read),
        ],
        ..PlcConfig::default()
    };
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/offline-tests")
        .join(format!("s7-server-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&directory).unwrap();
    let sink: EventSink = Arc::new(|_| {});
    let engine = PlcEngine::new(config, Arc::new(Logbook::open(&directory.join("plc.db"), sink.clone(), 1).unwrap()), sink);
    engine.connect().await;
    let first = eventually("S7 客户端未连上进程内服务", || {
        let status = engine.status();
        (status.state == LinkState::Connected && status.poll_count >= 2).then_some(status.since)
    }).await;

    engine.write_point("bit", &serde_json::json!(true)).await.unwrap();
    engine.write_point("word", &serde_json::json!(0xBEEF)).await.unwrap();
    engine.write_point("dword", &serde_json::json!(0x1234_5678u32)).await.unwrap();
    assert_eq!(memory.lock().unwrap().read(5, 0, 8).unwrap(), [0, 0b1000, 0xBE, 0xEF, 0x12, 0x34, 0x56, 0x78]);
    memory.lock().unwrap().write(5, 8, &[0, 0, 0x30, 0x39]).unwrap();
    eventually("客户端未读回 DB 输入", || engine.values().get("input").and_then(|v| v.value.clone()).filter(|v| *v == PlcValue::Int(12345))).await;
    eventually("客户端未读回位与字", || {
        let values = engine.values();
        (values["bit"].value == Some(PlcValue::Bool(true)) && values["word"].value == Some(PlcValue::Int(0xBEEF))).then_some(())
    }).await;

    server.inject(Fault::Disconnect);
    eventually("断线后客户端未重新连接", || {
        let status = engine.status();
        (status.state == LinkState::Connected && status.since != first && status.poll_count >= 2).then_some(())
    }).await;
    engine.write_point("word", &serde_json::json!(7)).await.unwrap();
    assert_eq!(memory.lock().unwrap().read(5, 2, 2).unwrap(), [0, 7]);
    engine.disconnect().await;
}

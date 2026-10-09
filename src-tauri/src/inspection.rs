use std::collections::HashMap;
use std::sync::RwLock;

use ly_plc::{Access, ConnectionConfig, DataType, EdgeMode, HeartbeatConfig, PlcConfig, PlcEngine, PlcPoint, PlcValue, ProtocolKind};
use serde_json::Value;

pub mod tag {
    pub const PART_START: &str = "partStart";
    pub const PART_END: &str = "partEnd";
    pub const RESULT_ACK: &str = "resultAck";
    pub const FAULT_RESET: &str = "faultReset";
    pub const PART_SN: &str = "partSn";
    pub const PRODUCT_CODE: &str = "productCode";
    pub const SHOT_COUNT: &str = "shotCount";
    pub const VISION_READY: &str = "visionReady";
    pub const ARMED: &str = "armed";
    pub const BUSY: &str = "busy";
    pub const DONE: &str = "done";
    pub const RESULT_CODE: &str = "resultCode";
    pub const FAULT_CODE: &str = "faultCode";
    pub const RESULT_SN: &str = "resultSn";
    pub const PROTOCOL_VERSION: &str = "protocolVersion";
    pub const REQUEST_SEQ: &str = "requestSeq";
    pub const PLAN_VERSION: &str = "planVersion";
    pub const PLAN_HASH: &str = "planHash";
    pub const CAMERA_SHOTS: [&str; 3] = ["camera1Shots", "camera2Shots", "camera3Shots"];
    pub const CAMERA_TRIGGERS: [&str; 3] = ["camera1Triggers", "camera2Triggers", "camera3Triggers"];
    pub const ACK_SEQ: &str = "ackSeq";
    pub const PLC_HEARTBEAT: &str = "plcHeartbeat";
    pub const PC_HEARTBEAT: &str = "pcHeartbeat";
    pub const PC_PROTOCOL_VERSION: &str = "pcProtocolVersion";
    pub const ACCEPTED_SEQ: &str = "acceptedSeq";
    pub const ACCEPTED_PLAN_HASH: &str = "acceptedPlanHash";
    pub const RESULT_SEQ: &str = "resultSeq";
    pub const VISION_FAULT: &str = "visionFault";
}

/// 标签 → 点位 id。节拍频繁按标签读写，不能每次都把整个地址表克隆一遍；地址表保存后清空重建。
static TAG_IDS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

pub fn invalidate_tags() {
    *TAG_IDS.write().unwrap() = None;
}

pub fn point_id(engine: &PlcEngine, tag: &str) -> Option<String> {
    if let Some(map) = TAG_IDS.read().unwrap().as_ref() {
        return map.get(tag).cloned();
    }
    let mut map = HashMap::new();
    for p in engine.config().points {
        for t in p.tags {
            // 同一标签挂在多个点位上时取第一个，与原来的查找顺序一致
            map.entry(t).or_insert_with(|| p.id.clone());
        }
    }
    let id = map.get(tag).cloned();
    *TAG_IDS.write().unwrap() = Some(map);
    id
}

pub fn read_tag(engine: &PlcEngine, tag: &str) -> Option<PlcValue> {
    let id = point_id(engine, tag)?;
    engine.values().get(&id).and_then(|v| v.value.clone())
}

pub fn tag_is_on(engine: &PlcEngine, tag: &str) -> bool {
    read_tag(engine, tag).is_some_and(|v| v.is_truthy())
}

pub fn read_tag_u32(engine: &PlcEngine, tag: &str) -> Option<u32> {
    match read_tag(engine, tag)? {
        PlcValue::Int(i) => u32::try_from(i).ok(),
        PlcValue::Float(f) => Some(f as u32),
        PlcValue::Bool(b) => Some(b as u32),
    }
}

pub async fn write_tag(engine: &PlcEngine, tag: &str, value: Value) -> Result<(), String> {
    let id = point_id(engine, tag).ok_or_else(|| format!("地址表中没有标签为 {tag} 的点位"))?;
    engine.write_point(&id, &value).await
}

pub fn default_plc_config() -> PlcConfig {
    s7_phase1_config(100).expect("DB100 is a valid S7 data block")
}

pub fn s7_phase1_config(db_number: u16) -> Result<PlcConfig, String> {
    if db_number == 0 {
        return Err("S7 DB 号必须在 1–65535 之间".into());
    }
    use DataType::{Bool, U16, U32};
    use EdgeMode::{None as NoEdge, Rising};
    let input = Access::Read;
    let output = Access::ReadWrite;
    let definitions = [
        (tag::PART_START, "工件开始", "DBX0.0", Bool, input, Rising),
        (tag::PART_END, "触发计划完成", "DBX0.1", Bool, input, Rising),
        (tag::RESULT_ACK, "结果确认", "DBX0.2", Bool, input, Rising),
        (tag::FAULT_RESET, "故障复位", "DBX0.3", Bool, input, Rising),
        (tag::PLC_HEARTBEAT, "PLC 心跳", "DBX0.4", Bool, input, NoEdge),
        (tag::PROTOCOL_VERSION, "握手协议版本", "DBW2", U16, input, NoEdge),
        (tag::REQUEST_SEQ, "请求事务序号", "DBD4", U32, input, NoEdge),
        (tag::PART_SN, "工件 SN", "DBD8", U32, input, NoEdge),
        (tag::PRODUCT_CODE, "产品代码", "DBW12", U16, input, NoEdge),
        (tag::SHOT_COUNT, "计划拍照总数", "DBW14", U16, input, NoEdge),
        (tag::PLAN_VERSION, "拍照计划版本", "DBD16", U32, input, NoEdge),
        (tag::PLAN_HASH, "拍照计划摘要", "DBD20", U32, input, NoEdge),
        (tag::CAMERA_SHOTS[0], "相机槽 1 计划数", "DBW24", U16, input, NoEdge),
        (tag::CAMERA_SHOTS[1], "相机槽 2 计划数", "DBW26", U16, input, NoEdge),
        (tag::CAMERA_SHOTS[2], "相机槽 3 计划数", "DBW28", U16, input, NoEdge),
        (tag::ACK_SEQ, "确认事务序号", "DBD32", U32, input, NoEdge),
        (tag::CAMERA_TRIGGERS[0], "相机槽 1 已发触发数", "DBW36", U16, input, NoEdge),
        (tag::CAMERA_TRIGGERS[1], "相机槽 2 已发触发数", "DBW38", U16, input, NoEdge),
        (tag::CAMERA_TRIGGERS[2], "相机槽 3 已发触发数", "DBW40", U16, input, NoEdge),
        (tag::VISION_READY, "视觉就绪", "DBX64.0", Bool, output, NoEdge),
        (tag::ARMED, "已布防", "DBX64.1", Bool, output, NoEdge),
        (tag::BUSY, "检测中", "DBX64.2", Bool, output, NoEdge),
        (tag::DONE, "结果有效", "DBX64.3", Bool, output, NoEdge),
        (tag::VISION_FAULT, "视觉故障", "DBX64.4", Bool, output, NoEdge),
        (tag::PC_HEARTBEAT, "上位机心跳", "DBX64.5", Bool, output, NoEdge),
        (tag::PC_PROTOCOL_VERSION, "上位机协议版本", "DBW66", U16, output, NoEdge),
        (tag::RESULT_SEQ, "结果事务序号", "DBD68", U32, output, NoEdge),
        (tag::RESULT_SN, "结果 SN", "DBD72", U32, output, NoEdge),
        (tag::RESULT_CODE, "结果码", "DBW76", U16, output, NoEdge),
        (tag::FAULT_CODE, "异常码", "DBW78", U16, output, NoEdge),
        (tag::ACCEPTED_SEQ, "布防事务序号", "DBD80", U32, output, NoEdge),
        (tag::ACCEPTED_PLAN_HASH, "布防计划摘要", "DBD84", U32, output, NoEdge),
    ];
    let points = definitions.into_iter().map(|(tag, name, address, data_type, access, edge)| PlcPoint {
        id: format!("p_{tag}"), name: name.into(), address: format!("DB{db_number}.{address}"),
        data_type, access, edge, tags: vec![tag.into()],
        log_changes: !matches!(tag, tag::PLC_HEARTBEAT | tag::PC_HEARTBEAT),
        description: if access == input { "一期 S7 · PLC 写入，上位机只读" } else { "一期 S7 · 上位机握手专用，禁止手动写入" }.into(),
        ..PlcPoint::default()
    }).collect();
    let config = PlcConfig {
        connection: ConnectionConfig { protocol: ProtocolKind::S7, port: 102, poll_interval_ms: 50, ..ConnectionConfig::default() },
        points,
        heartbeat: HeartbeatConfig { point_id: Some(format!("p_{}", tag::PC_HEARTBEAT)), interval_ms: 500 },
        auto_connect: false,
        ..PlcConfig::default()
    };
    config.validate()?;
    Ok(config)
}

pub fn reserved_s7_point(config: &PlcConfig, id: &str) -> bool {
    if config.connection.protocol != ProtocolKind::S7 { return false; }
    if config.heartbeat.point_id.as_deref() == Some(id) { return true; }
    let Some(point) = config.points.iter().find(|p| p.id == id) else { return false; };
    s7_phase1_config(100).is_ok_and(|template| template.points.iter().any(|preset|
        preset.tags.iter().any(|tag| point.tags.contains(tag))))
}

#[cfg(test)]
pub fn legacy_simulator_config() -> PlcConfig {
    let point = |id: &str, name: &str, address: &str, data_type, edge, tag: &str| PlcPoint {
        id: id.into(),
        name: name.into(),
        address: address.into(),
        data_type,
        access: Access::ReadWrite,
        edge,
        log_changes: true,
        tags: if tag.is_empty() { Vec::new() } else { vec![tag.to_string()] },
        ..PlcPoint::default()
    };
    use DataType::{Bool, U16, U32};
    use EdgeMode::{None as NoEdge, Rising};
    PlcConfig {
        connection: ConnectionConfig { poll_interval_ms: 50, ..ConnectionConfig::default() },
        points: vec![
            point("p_part_start", "工件开始", "C10", Bool, Rising, tag::PART_START),
            point("p_part_end", "运动结束", "C11", Bool, Rising, tag::PART_END),
            point("p_result_ack", "结果确认", "C12", Bool, Rising, tag::RESULT_ACK),
            point("p_fault_reset", "故障复位", "C13", Bool, Rising, tag::FAULT_RESET),
            point("p_part_sn", "工件序列号", "HR100", U32, NoEdge, tag::PART_SN),
            point("p_product_code", "产品代码", "HR102", U16, NoEdge, tag::PRODUCT_CODE),
            point("p_shot_count", "计划拍照点数", "HR103", U16, NoEdge, tag::SHOT_COUNT),
            point("p_vision_ready", "视觉就绪", "C20", Bool, NoEdge, tag::VISION_READY),
            point("p_armed", "已布防", "C21", Bool, NoEdge, tag::ARMED),
            point("p_busy", "检测中", "C22", Bool, NoEdge, tag::BUSY),
            point("p_done", "结果有效", "C23", Bool, NoEdge, tag::DONE),
            point("p_result_code", "结果码", "HR110", U16, NoEdge, tag::RESULT_CODE),
            point("p_fault_code", "异常码", "HR111", U16, NoEdge, tag::FAULT_CODE),
            point("p_result_sn", "结果 SN", "HR112", U32, NoEdge, tag::RESULT_SN),
            PlcPoint { log_changes: false, ..point("p_heartbeat", "上位机心跳", "C0", Bool, NoEdge, "") },
        ],
        heartbeat: HeartbeatConfig { point_id: Some("p_heartbeat".into()), interval_ms: 1000 },
        auto_connect: true,
        ..PlcConfig::default()
    }
}

use std::collections::HashMap;
use std::sync::RwLock;

use ly_plc::{Access, ConnectionConfig, DataType, EdgeMode, HeartbeatConfig, PlcConfig, PlcEngine, PlcPoint, PlcValue, ProtocolKind, S7Options};

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
    pub const PLAN_RESERVED: &str = "planReserved";
    pub const DEVICE_SHOTS: &str = "deviceShots";
    pub const DEVICE_TRIGGERS: &str = "deviceTriggers";
    pub const ACK_SEQ: &str = "ackSeq";
    pub const PLC_HEARTBEAT: &str = "plcHeartbeat";
    pub const PC_HEARTBEAT: &str = "pcHeartbeat";
    pub const PC_PROTOCOL_VERSION: &str = "pcProtocolVersion";
    pub const ACCEPTED_SEQ: &str = "acceptedSeq";
    pub const ACCEPTED_PLAN_RESERVED: &str = "acceptedPlanReserved";
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

pub fn read_tag_u32(engine: &PlcEngine, tag: &str) -> Option<u32> {
    match read_tag(engine, tag)? {
        PlcValue::Int(i) => u32::try_from(i).ok(),
        PlcValue::Float(f) => Some(f as u32),
        PlcValue::Bool(b) => Some(b as u32),
        PlcValue::Text(_) | PlcValue::Bytes(_) => None,
    }
}

/// 涂胶检测通信交互表 V1.0：S7-200 SMART，V 区即 DB1。
pub const DEFAULT_PLC_HOST: &str = "192.168.2.10";
pub const DEFAULT_DB: u16 = 1;
pub const DEFAULT_SN_TYPE: DataType = DataType::SmartString;

pub fn default_plc_config() -> PlcConfig {
    s7_phase1_config(DEFAULT_DB, DEFAULT_SN_TYPE).expect("DB1 is a valid S7 data block")
}

pub fn s7_phase1_config(db_number: u16, sn_type: DataType) -> Result<PlcConfig, String> {
    if db_number == 0 {
        return Err("S7 DB 号必须在 1–65535 之间".into());
    }
    if !matches!(sn_type, DataType::SmartString | DataType::String) {
        return Err("工件 SN 只能用 SmartString（200 SMART 字符串）或 String（S7 STRING）".into());
    }
    use DataType::{Bool, I32, U16};
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
        (tag::REQUEST_SEQ, "请求事务序号", "DBD4", I32, input, NoEdge),
        (tag::PRODUCT_CODE, "产品代码", "DBW12", U16, input, NoEdge),
        (tag::SHOT_COUNT, "计划拍照总数", "DBW14", U16, input, NoEdge),
        (tag::PLAN_VERSION, "拍照计划版本", "DBD16", I32, input, NoEdge),
        (tag::PLAN_RESERVED, "拍照计划保留位", "DBD20", I32, input, NoEdge),
        (tag::DEVICE_SHOTS, "设备计划触发数", "DBW24", U16, input, NoEdge),
        (tag::ACK_SEQ, "确认事务序号", "DBD32", I32, input, NoEdge),
        (tag::DEVICE_TRIGGERS, "设备已发触发数", "DBW36", U16, input, NoEdge),
        (tag::PART_SN, "工件 SN", "DBB160.50", sn_type, input, NoEdge),
        (tag::VISION_READY, "视觉就绪", "DBX64.0", Bool, output, NoEdge),
        (tag::ARMED, "已布防", "DBX64.1", Bool, output, NoEdge),
        (tag::BUSY, "检测中", "DBX64.2", Bool, output, NoEdge),
        (tag::DONE, "结果有效", "DBX64.3", Bool, output, NoEdge),
        (tag::VISION_FAULT, "视觉故障", "DBX64.4", Bool, output, NoEdge),
        (tag::PC_HEARTBEAT, "上位机心跳", "DBX64.5", Bool, output, NoEdge),
        (tag::PC_PROTOCOL_VERSION, "上位机协议版本", "DBW66", U16, output, NoEdge),
        (tag::RESULT_SEQ, "结果事务序号", "DBD68", I32, output, NoEdge),
        (tag::RESULT_CODE, "结果码", "DBW76", U16, output, NoEdge),
        (tag::FAULT_CODE, "异常码", "DBW78", U16, output, NoEdge),
        (tag::ACCEPTED_SEQ, "布防事务序号", "DBD80", I32, output, NoEdge),
        (tag::ACCEPTED_PLAN_RESERVED, "布防计划保留位", "DBD84", I32, output, NoEdge),
        (tag::RESULT_SN, "结果 SN", "DBB100.50", sn_type, output, NoEdge),
    ];
    let points = definitions.into_iter().map(|(tag, name, address, data_type, access, edge)| PlcPoint {
        id: format!("p_{tag}"), name: name.into(), address: format!("DB{db_number}.{address}"),
        data_type, access, edge, tags: vec![tag.into()],
        log_changes: !matches!(tag, tag::PLC_HEARTBEAT | tag::PC_HEARTBEAT),
        description: if access == input { "一期 S7 · PLC 写入，上位机只读" } else { "一期 S7 · 上位机握手专用，禁止手动写入" }.into(),
        ..PlcPoint::default()
    }).collect();
    let config = PlcConfig {
        connection: ConnectionConfig {
            protocol: ProtocolKind::S7, host: DEFAULT_PLC_HOST.into(), port: 102, poll_interval_ms: 50,
            s7: S7Options { rack: 0, slot: 1, local_tsap: Some(0x0102), remote_tsap: Some(0x0201), pdu_size: 240, ..S7Options::default() },
            ..ConnectionConfig::default()
        },
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
    s7_phase1_config(DEFAULT_DB, DEFAULT_SN_TYPE).is_ok_and(|template| template.points.iter().any(|preset|
        preset.tags.iter().any(|tag| point.tags.contains(tag))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use DataType::{Bool, SmartString, I32, U16};

    #[test]
    fn default_template_matches_plc_interaction_table_v1_0() {
        let config = default_plc_config();
        assert_eq!((config.connection.host.as_str(), config.connection.port), ("192.168.2.10", 102));
        let table: Vec<_> = config.points.iter().map(|p| (p.tags[0].as_str(), p.address.as_str(), p.data_type, p.access)).collect();
        let (r, w) = (Access::Read, Access::ReadWrite);
        assert_eq!(table, [
            ("partStart", "DB1.DBX0.0", Bool, r), ("partEnd", "DB1.DBX0.1", Bool, r), ("resultAck", "DB1.DBX0.2", Bool, r),
            ("faultReset", "DB1.DBX0.3", Bool, r), ("plcHeartbeat", "DB1.DBX0.4", Bool, r), ("protocolVersion", "DB1.DBW2", U16, r),
            ("requestSeq", "DB1.DBD4", I32, r), ("productCode", "DB1.DBW12", U16, r), ("shotCount", "DB1.DBW14", U16, r),
            ("planVersion", "DB1.DBD16", I32, r), ("planReserved", "DB1.DBD20", I32, r), ("deviceShots", "DB1.DBW24", U16, r),
            ("ackSeq", "DB1.DBD32", I32, r), ("deviceTriggers", "DB1.DBW36", U16, r), ("partSn", "DB1.DBB160.50", SmartString, r),
            ("visionReady", "DB1.DBX64.0", Bool, w), ("armed", "DB1.DBX64.1", Bool, w), ("busy", "DB1.DBX64.2", Bool, w),
            ("done", "DB1.DBX64.3", Bool, w), ("visionFault", "DB1.DBX64.4", Bool, w), ("pcHeartbeat", "DB1.DBX64.5", Bool, w),
            ("pcProtocolVersion", "DB1.DBW66", U16, w), ("resultSeq", "DB1.DBD68", I32, w), ("resultCode", "DB1.DBW76", U16, w),
            ("faultCode", "DB1.DBW78", U16, w), ("acceptedSeq", "DB1.DBD80", I32, w), ("acceptedPlanReserved", "DB1.DBD84", I32, w),
            ("resultSn", "DB1.DBB100.50", SmartString, w),
        ]);
        assert!(crate::plc::validate_config(&config).is_ok());
    }

    #[test]
    fn default_template_never_logs_value_changes_of_the_two_heartbeats_but_logs_every_other_signal() {
        let quiet: Vec<_> = default_plc_config().points.iter().filter(|p| !p.log_changes).map(|p| p.tags[0].clone()).collect();
        assert_eq!(quiet, [tag::PLC_HEARTBEAT, tag::PC_HEARTBEAT]);
    }
}

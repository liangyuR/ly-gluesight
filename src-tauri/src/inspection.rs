use ly_plc::{Access, ConnectionConfig, DataType, EdgeMode, HeartbeatConfig, PlcConfig, PlcPoint, ProtocolKind, S7Options};

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

/// 涂胶检测通信交互表 V1.1（握手协议 6，点表布局与协议 5 相同）：S7-1200，PLC 写 DB4、PC 写 DB2，两个 DB 都从字节 0 起。
pub const DEFAULT_PLC_HOST: &str = "192.168.2.10";
pub const DEFAULT_PLC_DB: u16 = 4;
pub const DEFAULT_PC_DB: u16 = 2;

pub fn default_plc_config() -> PlcConfig {
    s7_phase1_config(DEFAULT_PLC_DB, DEFAULT_PC_DB).expect("DB4 / DB2 are valid S7 data blocks")
}

pub fn s7_phase1_config(plc_db: u16, pc_db: u16) -> Result<PlcConfig, String> {
    if plc_db == 0 || pc_db == 0 {
        return Err("S7 DB 号必须在 1–65535 之间".into());
    }
    if plc_db == pc_db {
        return Err("PLC 区与 PC 区必须使用两个不同的 DB".into());
    }
    use DataType::{Bool, I16, I32, String as Text};
    use EdgeMode::{None as NoEdge, Rising};
    let input = Access::Read;
    let output = Access::ReadWrite;
    let definitions = [
        (tag::PART_START, "工件开始", "DBX0.0", Bool, input, Rising),
        (tag::PART_END, "触发计划完成", "DBX0.1", Bool, input, Rising),
        (tag::RESULT_ACK, "结果确认", "DBX0.2", Bool, input, Rising),
        (tag::FAULT_RESET, "故障复位", "DBX0.3", Bool, input, Rising),
        (tag::PLC_HEARTBEAT, "PLC 心跳", "DBX0.4", Bool, input, NoEdge),
        (tag::PROTOCOL_VERSION, "握手协议版本", "DBW2", I16, input, NoEdge),
        (tag::REQUEST_SEQ, "请求事务序号", "DBD4", I32, input, NoEdge),
        (tag::PRODUCT_CODE, "产品代码", "DBW8", I16, input, NoEdge),
        (tag::SHOT_COUNT, "计划拍照总数", "DBW10", I16, input, NoEdge),
        (tag::PLAN_VERSION, "拍照计划版本", "DBD12", I32, input, NoEdge),
        (tag::PLAN_RESERVED, "拍照计划保留位", "DBD16", I32, input, NoEdge),
        (tag::DEVICE_SHOTS, "设备计划触发数", "DBW20", I16, input, NoEdge),
        (tag::ACK_SEQ, "确认事务序号", "DBD28", I32, input, NoEdge),
        (tag::DEVICE_TRIGGERS, "采集预留字段（固定 0）", "DBW32", I16, input, NoEdge),
        (tag::PART_SN, "工件 SN", "DBB38.50", Text, input, NoEdge),
        (tag::VISION_READY, "视觉就绪", "DBX0.0", Bool, output, NoEdge),
        (tag::ARMED, "已布防", "DBX0.1", Bool, output, NoEdge),
        (tag::BUSY, "检测中", "DBX0.2", Bool, output, NoEdge),
        (tag::DONE, "结果有效", "DBX0.3", Bool, output, NoEdge),
        (tag::VISION_FAULT, "视觉故障", "DBX0.4", Bool, output, NoEdge),
        (tag::PC_HEARTBEAT, "上位机心跳", "DBX0.5", Bool, output, NoEdge),
        (tag::PC_PROTOCOL_VERSION, "上位机协议版本", "DBW2", I16, output, NoEdge),
        (tag::RESULT_SEQ, "结果事务序号", "DBD4", I32, output, NoEdge),
        (tag::RESULT_CODE, "结果码", "DBW8", I16, output, NoEdge),
        (tag::FAULT_CODE, "异常码", "DBW10", I16, output, NoEdge),
        (tag::ACCEPTED_SEQ, "布防事务序号", "DBD12", I32, output, NoEdge),
        (tag::ACCEPTED_PLAN_RESERVED, "布防计划保留位", "DBD16", I32, output, NoEdge),
        (tag::RESULT_SN, "结果 SN", "DBB20.50", Text, output, NoEdge),
    ];
    let mut points: Vec<_> = definitions.into_iter().map(|(tag, name, address, data_type, access, edge)| PlcPoint {
        id: format!("p_{tag}"), name: name.into(), address: format!("DB{}.{address}", if access == input { plc_db } else { pc_db }),
        data_type, access, edge, tags: vec![tag.into()],
        log_changes: !matches!(tag, tag::PLC_HEARTBEAT | tag::PC_HEARTBEAT),
        ..PlcPoint::default()
    }).collect();
    if ![plc_db, pc_db].iter().any(|db| matches!(db, 6 | 8)) { points.extend(teaching_points()); }
    let config = PlcConfig {
        connection: ConnectionConfig {
            protocol: ProtocolKind::S7, host: DEFAULT_PLC_HOST.into(), port: 102, poll_interval_ms: 50,
            s7: S7Options { rack: 0, slot: 1, local_tsap: None, remote_tsap: None, pdu_size: 480, ..S7Options::default() },
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

pub fn teaching_points() -> Vec<PlcPoint> {
    let mut points: Vec<_> = [(6, Access::ReadWrite, ["teachPcVersion", "teachPcState", "teachRoundId", "teachPcReason", "teachPcReserved"]),
        (12, Access::Read, ["teachPlcVersion", "teachPlcState", "teachRoundIdEcho", "teachPlcReason", "teachPlcReserved"])]
        .into_iter().flat_map(|(db, access, tags)| tags.into_iter().enumerate().map(move |(offset, tag)| PlcPoint {
            id: format!("p_{tag}"), name: tag.into(), address: format!("DB{db}.DBW{}", offset * 2),
            data_type: DataType::I16, access, tags: vec![tag.into()], poll: false,
            ..PlcPoint::default()
        })).collect();
    points.push(PlcPoint { id: "p_teachPlcSnapshot".into(), name: "teachPlcSnapshot".into(), address: "DB12.DBB0.10".into(),
        data_type: DataType::Bytes, access: Access::Read, tags: vec!["teachPlcSnapshot".into()], poll: false, ..PlcPoint::default() });
    points
}

pub fn add_teaching_points(config: &mut PlcConfig) {
    if config.connection.protocol != ProtocolKind::S7 { return; }
    let defaults = teaching_points();
    for point in &mut config.points {
        if let Some(expected) = defaults.iter().find(|p| p.address.starts_with("DB12.") && p.tags.iter().any(|t| point.tags.contains(t))) {
            if point.address.eq_ignore_ascii_case(&expected.address.replacen("DB12.", "DB8.", 1)) {
                point.address = expected.address.clone();
            }
        }
    }
    if config.points.iter().any(|p|
        p.tags.iter().any(|t| t.starts_with("teach")) || p.address.to_ascii_uppercase().starts_with("DB6.") || p.address.to_ascii_uppercase().starts_with("DB12.")) { return; }
    config.points.extend(defaults);
}

pub fn reserved_s7_point(config: &PlcConfig, id: &str) -> bool {
    if config.connection.protocol != ProtocolKind::S7 { return false; }
    if config.heartbeat.point_id.as_deref() == Some(id) { return true; }
    let Some(point) = config.points.iter().find(|p| p.id == id) else { return false; };
    s7_phase1_config(DEFAULT_PLC_DB, DEFAULT_PC_DB).is_ok_and(|template| template.points.iter().any(|preset|
        preset.tags.iter().any(|tag| point.tags.contains(tag))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use DataType::{Bool, I16, I32, String as Text};

    #[test]
    fn saved_teaching_db8_points_move_to_db12_without_relocating_other_fields() {
        let mut config = default_plc_config();
        for point in &mut config.points {
            point.address = point.address.replacen("DB12.", "DB8.", 1);
        }
        let mut unrelated = config.points.iter().find(|p| p.tags.contains(&"teachPlcState".into())).unwrap().clone();
        unrelated.id = "occupied_db8".into();
        unrelated.tags = vec!["siteOther".into()];
        config.points.push(unrelated.clone());
        add_teaching_points(&mut config);
        assert_eq!(config.points.iter().find(|p| p.id == unrelated.id).unwrap().address, "DB8.DBW2");
        for expected in teaching_points() {
            assert_eq!(config.points.iter().find(|p| p.id == expected.id).unwrap().address, expected.address);
        }
        let once = serde_json::to_value(&config).unwrap();
        add_teaching_points(&mut config);
        assert_eq!(serde_json::to_value(&config).unwrap(), once);
    }

    #[test]
    fn default_template_matches_plc_interaction_table_v1_1() {
        let config = default_plc_config();
        assert_eq!((config.connection.host.as_str(), config.connection.port), ("192.168.2.10", 102));
        let table: Vec<_> = config.points.iter().filter(|p| !p.tags[0].starts_with("teach")).map(|p| (p.tags[0].as_str(), p.address.as_str(), p.data_type, p.access)).collect();
        let (r, w) = (Access::Read, Access::ReadWrite);
        assert_eq!(table, [
            ("partStart", "DB4.DBX0.0", Bool, r), ("partEnd", "DB4.DBX0.1", Bool, r), ("resultAck", "DB4.DBX0.2", Bool, r),
            ("faultReset", "DB4.DBX0.3", Bool, r), ("plcHeartbeat", "DB4.DBX0.4", Bool, r), ("protocolVersion", "DB4.DBW2", I16, r),
            ("requestSeq", "DB4.DBD4", I32, r), ("productCode", "DB4.DBW8", I16, r), ("shotCount", "DB4.DBW10", I16, r),
            ("planVersion", "DB4.DBD12", I32, r), ("planReserved", "DB4.DBD16", I32, r), ("deviceShots", "DB4.DBW20", I16, r),
            ("ackSeq", "DB4.DBD28", I32, r), ("deviceTriggers", "DB4.DBW32", I16, r), ("partSn", "DB4.DBB38.50", Text, r),
            ("visionReady", "DB2.DBX0.0", Bool, w), ("armed", "DB2.DBX0.1", Bool, w), ("busy", "DB2.DBX0.2", Bool, w),
            ("done", "DB2.DBX0.3", Bool, w), ("visionFault", "DB2.DBX0.4", Bool, w), ("pcHeartbeat", "DB2.DBX0.5", Bool, w),
            ("pcProtocolVersion", "DB2.DBW2", I16, w), ("resultSeq", "DB2.DBD4", I32, w), ("resultCode", "DB2.DBW8", I16, w),
            ("faultCode", "DB2.DBW10", I16, w), ("acceptedSeq", "DB2.DBD12", I32, w), ("acceptedPlanReserved", "DB2.DBD16", I32, w),
            ("resultSn", "DB2.DBB20.50", Text, w),
        ]);
        assert!(crate::plc::validate_config(&config).is_ok());
    }

    #[test]
    fn default_template_never_logs_value_changes_of_the_two_heartbeats_but_logs_every_other_signal() {
        let quiet: Vec<_> = default_plc_config().points.iter().filter(|p| !p.log_changes).map(|p| p.tags[0].clone()).collect();
        assert_eq!(quiet, [tag::PLC_HEARTBEAT, tag::PC_HEARTBEAT]);
    }
}

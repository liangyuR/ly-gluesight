use ly_plc::{Access, ConnectionConfig, DataType, EdgeMode, HeartbeatConfig, PlcConfig, PlcPoint, ProtocolKind, S7Options};

pub mod tag {
    pub const PLC_VERSION: &str = "plcVersion";
    pub const PLC_HEARTBEAT: &str = "plcHeartbeat";
    pub const PLC_STATE: &str = "plcState";
    pub const PLC_REASON: &str = "plcReason";
    pub const REQUEST_SEQ: &str = "requestSeq";
    pub const PRODUCT_CODE: &str = "productCode";
    pub const SN: &str = "sn";
    pub const PC_VERSION: &str = "pcVersion";
    pub const PC_HEARTBEAT: &str = "pcHeartbeat";
    pub const PC_STATE: &str = "pcState";
    pub const PC_MODE: &str = "pcMode";
    pub const SEQ_ECHO: &str = "seqEcho";
    pub const RESULT_CODE: &str = "resultCode";
    pub const FAULT_CODE: &str = "faultCode";
    pub const SN_ECHO: &str = "snEcho";
}

/// 协议 7 双方状态字的取值。
pub mod state {
    pub const PLC_IDLE: u16 = 0;
    pub const PLC_REQUEST: u16 = 10;
    pub const PLC_MOTION_END: u16 = 20;
    pub const PLC_CONFIRMED: u16 = 30;
    pub const PLC_FAULT: u16 = 90;
    pub const PLC_RESET: u16 = 99;
    pub const PC_NOT_READY: u16 = 0;
    pub const PC_READY: u16 = 10;
    pub const PC_ARMED: u16 = 20;
    pub const PC_RESULT: u16 = 30;
    pub const PC_FAULT: u16 = 90;
    pub const PC_RESET: u16 = 99;
}

/// 生产通讯协议 7：S7-1200，PLC 写 DB4、PC 写 DB2，两个 DB 都从字节 0 起、各 68 字节。
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
    use DataType::{String as Text, I16, I32};
    let input = Access::Read;
    let output = Access::ReadWrite;
    let definitions = [
        (tag::PLC_VERSION, "PLC 协议版本", "DBW0", I16, input),
        (tag::PLC_HEARTBEAT, "PLC 心跳", "DBW2", I16, input),
        (tag::PLC_STATE, "PLC 状态", "DBW4", I16, input),
        (tag::PLC_REASON, "PLC 原因码", "DBW6", I16, input),
        (tag::REQUEST_SEQ, "请求事务序号", "DBD8", I32, input),
        (tag::PRODUCT_CODE, "型号", "DBW12", I16, input),
        (tag::SN, "工件 SN", "DBB16.50", Text, input),
        (tag::PC_VERSION, "上位机协议版本", "DBW0", I16, output),
        (tag::PC_HEARTBEAT, "上位机心跳", "DBW2", I16, output),
        (tag::PC_STATE, "上位机状态", "DBW4", I16, output),
        (tag::PC_MODE, "上位机模式", "DBW6", I16, output),
        (tag::SEQ_ECHO, "回显事务序号", "DBD8", I32, output),
        (tag::RESULT_CODE, "结果码", "DBW12", I16, output),
        (tag::FAULT_CODE, "异常码", "DBW14", I16, output),
        (tag::SN_ECHO, "回显工件 SN", "DBB16.50", Text, output),
    ];
    let points: Vec<_> = definitions
        .into_iter()
        .map(|(tag, name, address, data_type, access)| PlcPoint {
            id: format!("p_{tag}"),
            name: name.into(),
            address: format!(
                "DB{}.{address}",
                if access == input { plc_db } else { pc_db }
            ),
            data_type,
            access,
            edge: EdgeMode::None,
            tags: vec![tag.into()],
            log_changes: !matches!(tag, tag::PLC_HEARTBEAT | tag::PC_HEARTBEAT),
            ..PlcPoint::default()
        })
        .collect();
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

/// 已删除的示教握手（DB6 / DB12）点位的标签。
const RETIRED_TEACHING_TAGS: [&str; 11] = [
    "teachPcVersion",
    "teachPcState",
    "teachRoundId",
    "teachPcReason",
    "teachPcReserved",
    "teachPlcVersion",
    "teachPlcState",
    "teachRoundIdEcho",
    "teachPlcReason",
    "teachPlcReserved",
    "teachPlcSnapshot",
];

/// 去掉旧 plc.json 里示教握手的点位，返回去掉的个数；其余点位原样保留。
pub fn drop_teaching_points(config: &mut PlcConfig) -> usize {
    let before = config.points.len();
    config.points.retain(|p| {
        !p.tags
            .iter()
            .any(|t| RETIRED_TEACHING_TAGS.contains(&t.as_str()))
    });
    before - config.points.len()
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

    #[test]
    fn saved_teaching_points_are_dropped_and_production_points_stay_unchanged() {
        let production = default_plc_config();
        let mut config = production.clone();
        let mut extra = |id: &str, address: &str, tag: &str| {
            config.points.push(PlcPoint {
                id: id.into(),
                name: tag.into(),
                address: address.into(),
                data_type: DataType::I16,
                access: Access::ReadWrite,
                tags: vec![tag.into()],
                ..PlcPoint::default()
            })
        };
        extra("p_teachPcState", "DB6.DBW2", "teachPcState");
        extra("p_teachRoundIdEcho", "DB12.DBW4", "teachRoundIdEcho");
        extra("p_teachPlcSnapshot", "DB12.DBB0.10", "teachPlcSnapshot");
        extra("site_db6", "DB6.DBW20", "siteOther");
        assert_eq!(drop_teaching_points(&mut config), 3);
        let ids: Vec<&str> = config.points.iter().map(|p| p.id.as_str()).collect();
        let mut expected: Vec<&str> = production.points.iter().map(|p| p.id.as_str()).collect();
        expected.push("site_db6");
        assert_eq!(ids, expected);
        assert_eq!(
            serde_json::to_value(&config.points[..production.points.len()]).unwrap(),
            serde_json::to_value(&production.points).unwrap()
        );
    }
}

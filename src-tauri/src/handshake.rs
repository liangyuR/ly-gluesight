use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::{Duration, Instant};

use ly_plc::{Access, DataType, EdgeMode, LinkState, PlcConfig, PlcEngine, PlcStatus, PlcValue, PointValue, ProtocolKind, WordOrder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Input,
    Output,
}

#[derive(Clone, Copy)]
struct Field {
    tag: &'static str,
    data_type: DataType,
    direction: Direction,
    edge: bool,
}

const FIELDS: &[Field] = &[
    input("protocolVersion", DataType::U16, false),
    input("requestSeq", DataType::U32, false),
    input("partSn", DataType::U32, false),
    input("productCode", DataType::U16, false),
    input("shotCount", DataType::U16, false),
    input("planVersion", DataType::U32, false),
    input("planReserved", DataType::U32, false),
    input("camera1Shots", DataType::U16, false),
    input("camera2Shots", DataType::U16, false),
    input("camera3Shots", DataType::U16, false),
    input("camera1Triggers", DataType::U16, false),
    input("camera2Triggers", DataType::U16, false),
    input("camera3Triggers", DataType::U16, false),
    input("ackSeq", DataType::U32, false),
    input("partStart", DataType::Bool, true),
    input("partEnd", DataType::Bool, true),
    input("resultAck", DataType::Bool, true),
    input("faultReset", DataType::Bool, true),
    input("plcHeartbeat", DataType::Bool, false),
    output("visionReady", DataType::Bool),
    output("armed", DataType::Bool),
    output("busy", DataType::Bool),
    output("done", DataType::Bool),
    output("visionFault", DataType::Bool),
    output("pcHeartbeat", DataType::Bool),
    output("pcProtocolVersion", DataType::U16),
    output("acceptedSeq", DataType::U32),
    output("acceptedPlanReserved", DataType::U32),
    output("resultSeq", DataType::U32),
    output("resultSn", DataType::U32),
    output("resultCode", DataType::U16),
    output("faultCode", DataType::U16),
];

const fn input(tag: &'static str, data_type: DataType, edge: bool) -> Field {
    Field { tag, data_type, direction: Direction::Input, edge }
}

const fn output(tag: &'static str, data_type: DataType) -> Field {
    Field { tag, data_type, direction: Direction::Output, edge: false }
}

#[derive(Clone, Debug)]
pub(crate) struct Contract {
    config: PlcConfig,
    ids: HashMap<&'static str, String>,
}

impl Contract {
    pub(crate) fn validate(config: &PlcConfig) -> Result<Self, String> {
        if config.connection.protocol != ProtocolKind::S7 {
            return Err("一期严格握手仅适用于 S7 协议".into());
        }
        config.validate()?;
        let mut ids = HashMap::new();
        let mut locations = Vec::new();
        for field in FIELDS {
            let matches: Vec<_> = config.points.iter().filter(|p| p.tags.iter().any(|t| t == field.tag)).collect();
            let [point] = matches.as_slice() else {
                return Err(format!("握手标签 {} 必须且只能配置一个点位", field.tag));
            };
            if point.tags.iter().filter(|t| t.as_str() == field.tag).count() != 1 {
                return Err(format!("握手标签 {} 重复", field.tag));
            }
            if point.data_type != field.data_type {
                return Err(format!("{} 必须使用 {:?} 类型", field.tag, field.data_type));
            }
            let access = if field.direction == Direction::Input { Access::Read } else { Access::ReadWrite };
            if point.access != access {
                return Err(format!("{} 的访问方向必须为 {:?}", field.tag, access));
            }
            if point.word_order.is_some_and(|o| o != WordOrder::Abcd) {
                return Err(format!("{} 必须使用 S7 ABCD 字节序", field.tag));
            }
            if field.edge && !matches!(point.edge, EdgeMode::Rising | EdgeMode::Both) {
                return Err(format!("{} 必须启用上升沿检测", field.tag));
            }
            let location = ly_plc::resolve(&config.connection, &point.address, point.data_type)?;
            if location.area.code != 0x84 || !point.address.trim().to_ascii_uppercase().starts_with("DB") {
                return Err(format!("{} 必须使用标准 S7 DB 地址", field.tag));
            }
            if field.direction == Direction::Output && !location.writable {
                return Err(format!("{} 所在地址不可写", field.tag));
            }
            for (other, other_location) in &locations {
                let other: &Field = other;
                let other_location: &ly_plc::Location = other_location;
                if location.area != other_location.area {
                    continue;
                }
                let byte_overlap = location.start < other_location.start.saturating_add(other_location.len)
                    && other_location.start < location.start.saturating_add(location.len);
                let different_bits = location.start == other_location.start
                    && location.bit.zip(other_location.bit).is_some_and(|(a, b)| a != b);
                if byte_overlap && (field.direction != other.direction || !different_bits) {
                    return Err(format!("{} 与 {} 地址重叠，PLC 和 PC 不能共用输出字节", field.tag, other.tag));
                }
            }
            locations.push((*field, location));
            ids.insert(field.tag, point.id.clone());
        }
        for point in config.points.iter().filter(|p| !ids.values().any(|id| id == &p.id)) {
            let location = ly_plc::resolve(&config.connection, &point.address, point.data_type)?;
            let direction = if point.access == Access::ReadWrite { Direction::Output } else { Direction::Input };
            for (field, owned) in &locations {
                let byte_overlap = location.area == owned.area
                    && location.start < owned.start.saturating_add(owned.len)
                    && owned.start < location.start.saturating_add(location.len);
                let different_bits = location.start == owned.start
                    && location.bit.zip(owned.bit).is_some_and(|(a, b)| a != b);
                if byte_overlap && (direction != field.direction || !different_bits) {
                    return Err(format!("额外点位 {} 与握手字段 {} 共用地址或跨方向字节", point.name, field.tag));
                }
            }
        }
        if config.heartbeat.point_id.as_deref() != ids.get("pcHeartbeat").map(String::as_str) {
            return Err("通讯心跳必须绑定 pcHeartbeat 点位".into());
        }
        if !(100..=1000).contains(&config.heartbeat.interval_ms) {
            return Err("PC 心跳周期必须在 100–1000 ms 之间".into());
        }
        Ok(Self { config: config.clone(), ids })
    }

    pub(crate) fn point_id(&self, tag: &str) -> Result<&str, String> {
        self.ids.get(tag).map(String::as_str).ok_or_else(|| format!("不是握手契约标签：{tag}"))
    }

    fn check_config(&self, engine: &PlcEngine) -> Result<(), String> {
        if engine.config() != self.config {
            return Err("PLC 配置已变化，必须重新校验握手并复位".into());
        }
        Ok(())
    }

    fn value<'a>(&self, values: &'a HashMap<String, PointValue>, tag: &str) -> Result<&'a PlcValue, String> {
        let point = values.get(self.point_id(tag)?).ok_or_else(|| format!("{tag} 尚未读到"))?;
        if let Some(error) = &point.error {
            return Err(format!("{tag} 读取失败：{error}"));
        }
        let value = point.value.as_ref().ok_or_else(|| format!("{tag} 没有有效值"))?;
        let field = FIELDS.iter().find(|f| f.tag == tag).ok_or_else(|| format!("未知握手标签：{tag}"))?;
        let valid = match (field.data_type, value) {
            (DataType::Bool, PlcValue::Bool(_)) => true,
            (DataType::U16, PlcValue::Int(n)) => u16::try_from(*n).is_ok(),
            (DataType::U32, PlcValue::Int(n)) => u32::try_from(*n).is_ok(),
            _ => false,
        };
        if !valid {
            return Err(format!("{tag} 的值与 {:?} 类型不符", field.data_type));
        }
        Ok(value)
    }

    fn validate_writes(&self, writes: &[WriteOp]) -> Result<(), String> {
        let mut tags = HashSet::new();
        for op in writes {
            let field = FIELDS.iter().find(|f| f.tag == op.tag).ok_or_else(|| format!("未知握手标签：{}", op.tag))?;
            if field.direction != Direction::Output {
                return Err(format!("PC 不得写入 PLC 所有的 {}", op.tag));
            }
            if !tags.insert(op.tag) {
                return Err(format!("写入计划重复包含 {}", op.tag));
            }
            let valid = match field.data_type {
                DataType::Bool => op.value.as_bool().is_some(),
                DataType::U16 => op.value.as_u64().is_some_and(|v| u16::try_from(v).is_ok()),
                DataType::U32 => op.value.as_u64().is_some_and(|v| u32::try_from(v).is_ok()),
                _ => false,
            };
            if !valid {
                return Err(format!("{} 的写入值与 {:?} 不符", op.tag, field.data_type));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub(crate) connection_since: i64,
    pub(crate) poll_count: u64,
    pub(crate) last_poll: i64,
    values: HashMap<&'static str, PlcValue>,
}

fn check_freshness(status: &PlcStatus, now: i64, max_age_ms: u64) -> Result<i64, String> {
    if status.state != LinkState::Connected {
        return Err("PLC 未连接，握手状态不可使用".into());
    }
    let polled = status.last_poll.filter(|_| status.poll_count > 0).ok_or("当前连接尚未完成轮询")?;
    if polled < status.since {
        return Err("当前连接尚未产生新的轮询结果".into());
    }
    let age = now.checked_sub(polled).filter(|v| *v >= 0).ok_or("PLC 轮询时钟异常")? as u64;
    if max_age_ms == 0 || age > max_age_ms {
        return Err(format!("PLC 轮询结果已过期：{age} ms"));
    }
    Ok(polled)
}

impl Snapshot {
    pub(crate) fn from_values(
        contract: &Contract,
        status: &PlcStatus,
        values: &HashMap<String, PointValue>,
        now: i64,
        max_age_ms: u64,
    ) -> Result<Self, String> {
        let last_poll = check_freshness(status, now, max_age_ms)?;
        let values = FIELDS.iter().map(|f| Ok((f.tag, contract.value(values, f.tag)?.clone()))).collect::<Result<_, String>>()?;
        Ok(Self { connection_since: status.since, poll_count: status.poll_count, last_poll, values })
    }

    pub(crate) fn capture(engine: &PlcEngine, contract: &Contract, max_age_ms: u64) -> Result<Self, String> {
        contract.check_config(engine)?;
        for _ in 0..3 {
            let before = engine.status();
            let values = engine.values();
            let after = engine.status();
            if before.state == after.state && before.since == after.since && before.poll_count == after.poll_count {
                contract.check_config(engine)?;
                return Self::from_values(contract, &after, &values, ly_plc::now_ms(), max_age_ms);
            }
        }
        Err("PLC 轮询快照更新中，请重新读取".into())
    }

    pub(crate) fn read_bool(&self, tag: &str) -> Result<bool, String> {
        match self.values.get(tag) {
            Some(PlcValue::Bool(v)) => Ok(*v),
            _ => Err(format!("{tag} 不是有效 Bool")),
        }
    }

    pub(crate) fn read_u32(&self, tag: &str) -> Result<u32, String> {
        match self.values.get(tag) {
            Some(PlcValue::Int(v)) => u32::try_from(*v).map_err(|_| format!("{tag} 超出 U32 范围")),
            _ => Err(format!("{tag} 不是有效无符号整数")),
        }
    }

    pub(crate) fn request(&self) -> Result<Request, String> {
        let request = Request {
            protocol_version: self.read_u32("protocolVersion")? as u16,
            request_seq: self.read_u32("requestSeq")?,
            sn: self.read_u32("partSn")?,
            product_code: self.read_u32("productCode")? as u16,
            shot_count: self.read_u32("shotCount")? as u16,
            plan_version: self.read_u32("planVersion")?,
            plan_hash: self.read_u32("planReserved")?,
            camera_shots: [self.read_u32("camera1Shots")? as u16, self.read_u32("camera2Shots")? as u16, self.read_u32("camera3Shots")? as u16],
        };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn trigger_counts(&self) -> Result<[u16; 3], String> {
        Ok([self.read_u32("camera1Triggers")? as u16, self.read_u32("camera2Triggers")? as u16, self.read_u32("camera3Triggers")? as u16])
    }

    fn start_request(&self) -> Result<Request, String> {
        for (tag, expected) in [
            ("partStart", true), ("partEnd", false), ("resultAck", false), ("visionReady", true),
            ("armed", false), ("busy", false), ("done", false), ("visionFault", false),
        ] {
            if self.read_bool(tag)? != expected {
                return Err(format!("开始握手要求 {tag}={expected}"));
            }
        }
        if self.read_u32("pcProtocolVersion")? != PROTOCOL_VERSION as u32 {
            return Err("PC 协议版本尚未确认，需复位".into());
        }
        if self.trigger_counts()? != [0; 3] {
            return Err("布防前各相机触发计数必须清零".into());
        }
        self.request()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    pub(crate) protocol_version: u16,
    pub(crate) request_seq: u32,
    pub(crate) sn: u32,
    pub(crate) product_code: u16,
    pub(crate) shot_count: u16,
    pub(crate) plan_version: u32,
    pub(crate) plan_hash: u32,
    pub(crate) camera_shots: [u16; 3],
}

impl Request {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(format!("PLC 协议版本 {} 与 PC {} 不一致", self.protocol_version, PROTOCOL_VERSION));
        }
        if self.request_seq == 0 {
            return Err("requestSeq 不能为零".into());
        }
        if self.shot_count == 0 || self.camera_shots.iter().map(|v| *v as u32).sum::<u32>() != self.shot_count as u32 {
            return Err("各相机计划拍照数之和必须等于非零 shotCount".into());
        }
        Ok(())
    }
}

pub(crate) fn confirm_start(first: &Snapshot, second: &Snapshot) -> Result<Request, String> {
    if first.connection_since != second.connection_since {
        return Err("开始校验期间 PLC 已重新连接，必须复位".into());
    }
    // ly-plc 先发布值再递增 poll_count，相隔两次可排除同一次轮询被读两遍。
    if second.poll_count < first.poll_count.saturating_add(2) {
        return Err("开始快照必须跨两个后续完成的轮询".into());
    }
    let request = first.start_request()?;
    if second.start_request()? != request {
        return Err("开始快照不一致，PLC 必须保持请求字段直到释放".into());
    }
    Ok(request)
}

pub(crate) async fn stable_start(engine: &PlcEngine, contract: &Contract, timeout: Duration, max_age_ms: u64) -> Result<Request, String> {
    let first = Snapshot::capture(engine, contract, max_age_ms)?;
    first.start_request()?;
    let started = Instant::now();
    while started.elapsed() < timeout {
        tokio::time::sleep(poll_wait(contract)).await;
        let second = Snapshot::capture(engine, contract, max_age_ms)?;
        if second.connection_since != first.connection_since {
            return Err("开始校验期间 PLC 已重新连接，必须复位".into());
        }
        if second.poll_count >= first.poll_count.saturating_add(2) {
            return confirm_start(&first, &second);
        }
    }
    Err("等待开始快照双读确认超时".into())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResultEnvelope {
    pub(crate) request_seq: u32,
    pub(crate) sn: u32,
    pub(crate) result_code: u16,
    pub(crate) fault_code: u16,
}

pub(crate) fn matching_ack(snapshot: &Snapshot, result: &ResultEnvelope) -> Result<bool, String> {
    if !snapshot.read_bool("done")? {
        return Err("待确认结果的 done 已被清除，必须故障复位".into());
    }
    for (tag, expected) in [
        ("requestSeq", result.request_seq), ("resultSeq", result.request_seq), ("resultSn", result.sn),
        ("resultCode", result.result_code as u32), ("faultCode", result.fault_code as u32),
    ] {
        if snapshot.read_u32(tag)? != expected {
            return Err(format!("待确认结果的 {tag} 与本次结果不一致"));
        }
    }
    if !snapshot.read_bool("resultAck")? {
        return Ok(false);
    }
    if snapshot.read_u32("ackSeq")? != result.request_seq {
        return Err("ackSeq 与本次 resultSeq 不匹配，保持 done".into());
    }
    Ok(true)
}

pub(crate) fn verify_part_end(snapshot: &Snapshot, request: &Request) -> Result<(), String> {
    if !snapshot.read_bool("partEnd")? || !snapshot.read_bool("partStart")? {
        return Err("运动结束要求 partEnd 和 partStart 保持有效".into());
    }
    if snapshot.request()? != *request {
        return Err("运动结束时请求身份或拍照计划已改变".into());
    }
    if snapshot.trigger_counts()? != request.camera_shots {
        return Err(format!("PLC 实际触发数 {:?} 与计划 {:?} 不符", snapshot.trigger_counts()?, request.camera_shots));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResetBaseline {
    pub(crate) connection_since: i64,
    pub(crate) request_seq: u32,
    pub(crate) ack_seq: u32,
}

impl ResetBaseline {
    pub(crate) fn capture(snapshot: &Snapshot) -> Result<Self, String> {
        if snapshot.read_u32("protocolVersion")? != PROTOCOL_VERSION as u32 {
            return Err("PLC 协议版本不一致，不能完成复位".into());
        }
        for tag in ["partStart", "partEnd", "resultAck"] {
            if snapshot.read_bool(tag)? {
                return Err(format!("复位前 PLC 必须释放 {tag}"));
            }
        }
        Ok(Self { connection_since: snapshot.connection_since, request_seq: snapshot.read_u32("requestSeq")?, ack_seq: snapshot.read_u32("ackSeq")? })
    }

    pub(crate) fn verify(&self, snapshot: &Snapshot) -> Result<(), String> {
        if Self::capture(snapshot)? != *self {
            return Err("复位期间 PLC 会话或请求基线变化，不能置 visionReady".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WriteOp {
    pub(crate) tag: &'static str,
    pub(crate) value: Value,
}

impl WriteOp {
    pub(crate) fn new(tag: &'static str, value: Value) -> Self {
        Self { tag, value }
    }
}

pub(crate) fn arm_plan(request: &Request) -> Vec<WriteOp> {
    vec![
        WriteOp::new("visionReady", json!(false)),
        WriteOp::new("acceptedSeq", json!(request.request_seq)),
        WriteOp::new("acceptedPlanReserved", json!(request.plan_hash)),
        WriteOp::new("busy", json!(true)),
        WriteOp::new("armed", json!(true)),
    ]
}

pub(crate) fn report_plan(result: &ResultEnvelope) -> Vec<WriteOp> {
    vec![
        WriteOp::new("visionReady", json!(false)),
        WriteOp::new("armed", json!(false)),
        WriteOp::new("busy", json!(true)),
        WriteOp::new("resultCode", json!(result.result_code)),
        WriteOp::new("faultCode", json!(result.fault_code)),
        WriteOp::new("resultSn", json!(result.sn)),
        WriteOp::new("resultSeq", json!(result.request_seq)),
        WriteOp::new("done", json!(true)),
    ]
}

pub(crate) fn release_plan() -> Vec<WriteOp> {
    vec![WriteOp::new("done", json!(false)), WriteOp::new("busy", json!(false))]
}

pub(crate) fn fault_plan() -> Vec<WriteOp> {
    vec![WriteOp::new("visionReady", json!(false)), WriteOp::new("armed", json!(false)), WriteOp::new("visionFault", json!(true))]
}

pub(crate) fn reset_plan() -> Vec<WriteOp> {
    vec![
        WriteOp::new("visionReady", json!(false)),
        WriteOp::new("armed", json!(false)),
        WriteOp::new("done", json!(false)),
        WriteOp::new("busy", json!(false)),
        WriteOp::new("visionFault", json!(false)),
        WriteOp::new("pcProtocolVersion", json!(PROTOCOL_VERSION)),
    ]
}

pub(crate) async fn write_all_with<F, Fut>(writes: &[WriteOp], mut write: F) -> Result<(), String>
where
    F: FnMut(WriteOp) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    for (i, op) in writes.iter().enumerate() {
        write(op.clone()).await.map_err(|e| format!("握手第 {} 步 {} 写入失败：{e}", i + 1, op.tag))?;
    }
    Ok(())
}

pub(crate) fn verify_writes(contract: &Contract, values: &HashMap<String, PointValue>, writes: &[WriteOp]) -> Result<bool, String> {
    contract.validate_writes(writes)?;
    for op in writes {
        let value = contract.value(values, op.tag)?;
        let matches = match value {
            PlcValue::Bool(v) => op.value.as_bool() == Some(*v),
            PlcValue::Int(v) => op.value.as_i64() == Some(*v),
            PlcValue::Float(_) => false,
        };
        if !matches {
            return Ok(false);
        }
    }
    Ok(true)
}

fn poll_wait(contract: &Contract) -> Duration {
    Duration::from_millis((contract.config.connection.poll_interval_ms / 4).clamp(5, 25))
}

fn connection_stamp(engine: &PlcEngine, contract: &Contract, max_age_ms: u64) -> Result<PlcStatus, String> {
    contract.check_config(engine)?;
    let status = engine.status();
    check_freshness(&status, ly_plc::now_ms(), max_age_ms)?;
    Ok(status)
}

pub(crate) async fn write_confirmed(
    engine: &PlcEngine,
    contract: &Contract,
    writes: &[WriteOp],
    timeout: Duration,
    max_age_ms: u64,
) -> Result<(), String> {
    contract.validate_writes(writes)?;
    if writes.is_empty() {
        return Ok(());
    }
    let initial = connection_stamp(engine, contract, max_age_ms)?;
    let started = Instant::now();
    write_all_with(writes, |op| async move {
        if started.elapsed() >= timeout {
            return Err("握手写入事务超时".into());
        }
        if connection_stamp(engine, contract, max_age_ms)?.since != initial.since {
            return Err("PLC 连接已变化，必须复位".into());
        }
        engine.write_point(contract.point_id(op.tag)?, &op.value).await
    }).await?;
    let written = connection_stamp(engine, contract, max_age_ms)?;
    if written.since != initial.since {
        return Err("PLC 写入期间已重新连接，必须复位".into());
    }
    while started.elapsed() < timeout {
        tokio::time::sleep(poll_wait(contract)).await;
        let before = connection_stamp(engine, contract, max_age_ms)?;
        if before.since != initial.since {
            return Err("PLC 读回确认期间已重新连接，必须复位".into());
        }
        if before.poll_count < written.poll_count.saturating_add(2) {
            continue;
        }
        let values = engine.values();
        let after = connection_stamp(engine, contract, max_age_ms)?;
        if after.since != initial.since {
            return Err("PLC 读回确认期间已重新连接，必须复位".into());
        }
        if before.poll_count != after.poll_count {
            continue;
        }
        if verify_writes(contract, &values, writes)? {
            return Ok(());
        }
    }
    Err("PLC 写入后的新轮询读回未确认，保持当前握手状态".into())
}

#[derive(Clone, Debug)]
pub(crate) struct HeartbeatWatch {
    timeout_ms: u64,
    first_seen: Option<i64>,
    last_seen: Option<i64>,
    level: Option<bool>,
    last_change: Option<i64>,
}

impl HeartbeatWatch {
    pub(crate) fn new(timeout_ms: u64) -> Result<Self, String> {
        if timeout_ms < 3000 {
            return Err("PLC 心跳超时不能小于 3000 ms".into());
        }
        Ok(Self { timeout_ms, first_seen: None, last_seen: None, level: None, last_change: None })
    }

    pub(crate) fn reset(&mut self) {
        self.first_seen = None;
        self.last_seen = None;
        self.level = None;
        self.last_change = None;
    }

    pub(crate) fn observe(&mut self, level: bool, now: i64) -> Result<(), String> {
        if self.first_seen.is_some() {
            self.check(now)?;
        }
        if self.last_seen.is_some_and(|last| now < last) {
            return Err("PLC 心跳时钟倒退，必须重新同步".into());
        }
        if self.level.is_some_and(|old| old != level) {
            self.last_change = Some(now);
        }
        self.first_seen.get_or_insert(now);
        self.last_seen = Some(now);
        self.level = Some(level);
        Ok(())
    }

    pub(crate) fn check(&self, now: i64) -> Result<(), String> {
        let baseline = self.last_change.or(self.first_seen).ok_or("尚未读取 PLC 心跳")?;
        let age = now.checked_sub(baseline).filter(|v| *v >= 0).ok_or("PLC 心跳时钟倒退")? as u64;
        if age > self.timeout_ms {
            return Err(format!("PLC 心跳超过 {} ms 未翻转", self.timeout_ms));
        }
        Ok(())
    }

    pub(crate) fn is_live(&self, now: i64) -> bool {
        self.last_change.is_some() && self.check(now).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ly_plc::{ConnectionConfig, HeartbeatConfig, PlcPoint};

    fn config() -> PlcConfig {
        let mut offsets = [0u32; 2];
        let points = FIELDS.iter().map(|field| {
            let index = usize::from(field.direction == Direction::Output);
            let offset = offsets[index];
            offsets[index] += field.data_type.byte_size().max(1);
            let address = match field.data_type {
                DataType::Bool => format!("DB{}.DBX{offset}.0", index + 1),
                DataType::U16 => format!("DB{}.DBW{offset}", index + 1),
                _ => format!("DB{}.DBD{offset}", index + 1),
            };
            PlcPoint {
                id: field.tag.into(), name: field.tag.into(), address, data_type: field.data_type,
                access: if field.direction == Direction::Input { Access::Read } else { Access::ReadWrite },
                edge: if field.edge { EdgeMode::Rising } else { EdgeMode::None },
                tags: vec![field.tag.into()], ..PlcPoint::default()
            }
        }).collect();
        PlcConfig {
            connection: ConnectionConfig { protocol: ProtocolKind::S7, host: "127.0.0.1".into(), port: 102, poll_interval_ms: 50, ..ConnectionConfig::default() },
            points, heartbeat: HeartbeatConfig { point_id: Some("pcHeartbeat".into()), interval_ms: 1000 }, ..PlcConfig::default()
        }
    }

    fn data() -> (Contract, PlcStatus, HashMap<String, PointValue>) {
        let contract = Contract::validate(&config()).unwrap();
        let status = PlcStatus { state: LinkState::Connected, message: String::new(), since: 100, last_poll: Some(1000), cycle_ms: Some(2), poll_count: 10, error_count: 0 };
        let mut values: HashMap<_, _> = FIELDS.iter().map(|f| (f.tag.into(), PointValue {
            value: Some(if f.data_type == DataType::Bool { PlcValue::Bool(false) } else { PlcValue::Int(0) }), error: None, ts: 1,
        })).collect();
        for (tag, value) in [
            ("protocolVersion", 1), ("pcProtocolVersion", 1), ("requestSeq", 7), ("partSn", 42),
            ("productCode", 3), ("shotCount", 4), ("planVersion", 2), ("planReserved", 99),
            ("camera1Shots", 2), ("camera2Shots", 1), ("camera3Shots", 1),
        ] {
            values.get_mut(tag).unwrap().value = Some(PlcValue::Int(value));
        }
        for tag in ["partStart", "visionReady"] {
            values.get_mut(tag).unwrap().value = Some(PlcValue::Bool(true));
        }
        (contract, status, values)
    }

    fn snapshot() -> Snapshot {
        let (contract, status, values) = data();
        Snapshot::from_values(&contract, &status, &values, 1010, 1000).unwrap()
    }

    fn result() -> ResultEnvelope {
        ResultEnvelope { request_seq: 7, sn: 42, result_code: 1, fault_code: 0 }
    }

    #[test]
    fn contract_rejects_missing_duplicate_wrong_type_direction_and_protocol() {
        for case in 0..7 {
            let mut cfg = config();
            match case {
                0 => { cfg.points.retain(|p| p.id != "requestSeq"); }
                1 => { let mut p = cfg.points[0].clone(); p.id = "duplicate".into(); cfg.points.push(p); }
                2 => cfg.points.iter_mut().find(|p| p.id == "requestSeq").unwrap().data_type = DataType::F32,
                3 => cfg.points[0].access = Access::ReadWrite,
                4 => cfg.connection.protocol = ProtocolKind::Simulator,
                5 => cfg.points.iter_mut().find(|p| p.id == "partStart").unwrap().edge = EdgeMode::None,
                _ => cfg.points[0].word_order = Some(WordOrder::Dcba),
            }
            assert!(Contract::validate(&cfg).is_err(), "case {case}");
        }
    }

    #[test]
    fn contract_rejects_cross_owner_bytes_even_when_bits_differ() {
        let mut cfg = config();
        cfg.points.iter_mut().find(|p| p.id == "partStart").unwrap().address = "DB10.DBX0.0".into();
        cfg.points.iter_mut().find(|p| p.id == "done").unwrap().address = "DB10.DBX0.1".into();
        assert!(Contract::validate(&cfg).is_err());
        cfg.points.iter_mut().find(|p| p.id == "done").unwrap().address = "DB10.DBX1.0".into();
        cfg.points.iter_mut().find(|p| p.id == "busy").unwrap().address = "DB10.DBX1.1".into();
        assert!(Contract::validate(&cfg).is_ok());
        cfg.points.iter_mut().find(|p| p.id == "busy").unwrap().address = "DB10.DBX1.0".into();
        assert!(Contract::validate(&cfg).is_err());
    }

    #[test]
    fn untagged_alias_cannot_write_plc_owned_byte() {
        let mut cfg = config();
        cfg.points.iter_mut().find(|p| p.id == "partStart").unwrap().address = "DB10.DBX0.0".into();
        cfg.points.push(PlcPoint {
            id: "alias".into(), name: "额外点位".into(), address: "DB10.DBX0.1".into(),
            data_type: DataType::Bool, access: Access::ReadWrite, ..PlcPoint::default()
        });
        assert!(Contract::validate(&cfg).is_err());
        cfg.points.last_mut().unwrap().access = Access::Read;
        assert!(Contract::validate(&cfg).is_ok());
        cfg.points.last_mut().unwrap().address = "DB10.DBX0.0".into();
        assert!(Contract::validate(&cfg).is_err());
    }

    #[test]
    fn constant_point_timestamp_is_not_mistaken_for_stale_poll() {
        let (contract, mut status, mut values) = data();
        assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_ok());
        assert!(Snapshot::from_values(&contract, &status, &values, 1600, 500).is_err());
        status.since = 1001;
        assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_err());
        status.since = 100;
        values.get_mut("partSn").unwrap().error = Some("DB 不可访问".into());
        assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_err());
    }

    #[test]
    fn snapshot_rejects_missing_invalid_and_fractional_values() {
        for case in 0..4 {
            let (contract, status, mut values) = data();
            match case {
                0 => { values.remove("partSn"); }
                1 => values.get_mut("partSn").unwrap().value = None,
                2 => values.get_mut("partSn").unwrap().value = Some(PlcValue::Float(42.0)),
                _ => values.get_mut("shotCount").unwrap().value = Some(PlcValue::Int(65536)),
            }
            assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_err());
        }
    }

    #[test]
    fn start_requires_distinct_completed_polls_same_identity_and_connection() {
        let first = snapshot();
        let mut second = first.clone();
        second.poll_count += 1;
        assert!(confirm_start(&first, &second).is_err());
        second.poll_count += 1;
        assert_eq!(confirm_start(&first, &second).unwrap().camera_shots, [2, 1, 1]);
        second.values.insert("planReserved", PlcValue::Int(100));
        assert!(confirm_start(&first, &second).is_err());
        second = first.clone();
        second.poll_count += 2;
        second.connection_since += 1;
        assert!(confirm_start(&first, &second).is_err());
    }

    #[test]
    fn malformed_or_unreleased_request_cannot_arm() {
        for (tag, value) in [
            ("requestSeq", PlcValue::Int(0)), ("protocolVersion", PlcValue::Int(2)),
            ("camera2Shots", PlcValue::Int(3)), ("resultAck", PlcValue::Bool(true)),
            ("camera1Triggers", PlcValue::Int(1)), ("done", PlcValue::Bool(true)),
        ] {
            let mut s = snapshot();
            s.values.insert(tag, value);
            assert!(s.start_request().is_err(), "{tag}");
        }
    }

    #[test]
    fn ack_requires_matching_result_identity_and_retained_done() {
        let result = result();
        let mut s = snapshot();
        for op in report_plan(&result) {
            s.values.insert(op.tag, serde_json::from_value(op.value).unwrap());
        }
        assert!(!matching_ack(&s, &result).unwrap());
        s.values.insert("resultAck", PlcValue::Bool(true));
        s.values.insert("ackSeq", PlcValue::Int(6));
        assert!(matching_ack(&s, &result).is_err());
        s.values.insert("ackSeq", PlcValue::Int(7));
        assert!(matching_ack(&s, &result).unwrap());
        s.values.insert("done", PlcValue::Bool(false));
        assert!(matching_ack(&s, &result).is_err());
    }

    #[test]
    fn part_end_compares_each_camera_and_frozen_request() {
        let mut s = snapshot();
        let request = s.request().unwrap();
        s.values.insert("partEnd", PlcValue::Bool(true));
        for (tag, value) in [("camera1Triggers", 2), ("camera2Triggers", 1), ("camera3Triggers", 1)] {
            s.values.insert(tag, PlcValue::Int(value));
        }
        assert!(verify_part_end(&s, &request).is_ok());
        s.values.insert("camera2Triggers", PlcValue::Int(0));
        assert!(verify_part_end(&s, &request).is_err());
        s.values.insert("camera2Triggers", PlcValue::Int(1));
        s.values.insert("requestSeq", PlcValue::Int(8));
        assert!(verify_part_end(&s, &request).is_err());
    }

    #[test]
    fn part_end_rejects_extra_count_in_a_used_camera_slot() {
        let mut s = snapshot();
        let request = s.request().unwrap();
        assert_eq!(request.camera_shots, [2, 1, 1]);
        s.values.insert("partEnd", PlcValue::Bool(true));
        for (tag, value) in [("camera1Triggers", 3), ("camera2Triggers", 1), ("camera3Triggers", 1)] {
            s.values.insert(tag, PlcValue::Int(value));
        }
        let error = verify_part_end(&s, &request).unwrap_err();
        assert!(error.contains("[3, 1, 1]") && error.contains("[2, 1, 1]"), "{error}");
        assert_eq!(s.request().unwrap(), request);
        s.values.insert("camera1Triggers", PlcValue::Int(2));
        assert!(verify_part_end(&s, &request).is_ok());
    }

    #[test]
    fn part_end_rejects_each_unused_slot_even_when_total_count_matches() {
        let mut s = snapshot();
        for (tag, value) in [("camera1Shots", 4), ("camera2Shots", 0), ("camera3Shots", 0)] {
            s.values.insert(tag, PlcValue::Int(value));
        }
        let request = s.request().unwrap();
        request.validate().unwrap();
        assert_eq!(request.camera_shots, [4, 0, 0]);
        s.values.insert("partEnd", PlcValue::Bool(true));
        for counts in [[4, 1, 0], [4, 0, 1], [3, 1, 0], [3, 0, 1], [4, 0, 0]] {
            for (tag, value) in ["camera1Triggers", "camera2Triggers", "camera3Triggers"].into_iter().zip(counts) {
                s.values.insert(tag, PlcValue::Int(value));
            }
            let result = verify_part_end(&s, &request);
            if counts == [4, 0, 0] {
                assert!(result.is_ok());
            } else {
                let error = result.unwrap_err();
                assert!(error.contains(&format!("{counts:?}")) && error.contains("[4, 0, 0]"), "{error}");
            }
            assert_eq!(s.request().unwrap(), request);
        }
    }

    #[test]
    fn write_plans_commit_last_and_fault_preserves_unacknowledged_result() {
        let arm = arm_plan(&snapshot().request().unwrap());
        assert_eq!(arm.last().unwrap(), &WriteOp::new("armed", json!(true)));
        assert_eq!(arm[arm.len() - 2].tag, "busy");
        assert_eq!(report_plan(&result()).last().unwrap(), &WriteOp::new("done", json!(true)));
        assert!(fault_plan().iter().all(|op| !["done", "busy", "resultSeq", "resultSn", "resultCode", "faultCode"].contains(&op.tag)));
        assert!(!reset_plan().contains(&WriteOp::new("visionReady", json!(true))));
        assert_eq!(release_plan(), vec![WriteOp::new("done", json!(false)), WriteOp::new("busy", json!(false))]);
    }

    #[tokio::test]
    async fn rejected_write_stops_before_done_commit() {
        let mut seen = Vec::new();
        let r = write_all_with(&report_plan(&result()), |op| {
            seen.push(op.tag);
            std::future::ready(if op.tag == "resultSeq" { Err("S7 拒绝写入".into()) } else { Ok(()) })
        }).await;
        assert!(r.is_err());
        assert!(!seen.contains(&"done"));
        assert_eq!(seen.last(), Some(&"resultSeq"));
    }

    #[test]
    fn write_confirmation_does_not_accept_cached_errors_or_input_writes() {
        let (contract, _, mut values) = data();
        let writes = [WriteOp::new("busy", json!(true))];
        assert!(!verify_writes(&contract, &values, &writes).unwrap());
        values.get_mut("busy").unwrap().value = Some(PlcValue::Bool(true));
        assert!(verify_writes(&contract, &values, &writes).unwrap());
        values.get_mut("busy").unwrap().error = Some("读取被拒绝".into());
        assert!(verify_writes(&contract, &values, &writes).is_err());
        assert!(verify_writes(&contract, &values, &[WriteOp::new("partStart", json!(false))]).is_err());
    }

    #[test]
    fn reset_requires_released_signals_and_unchanged_baseline() {
        let mut s = snapshot();
        assert!(ResetBaseline::capture(&s).is_err());
        s.values.insert("partStart", PlcValue::Bool(false));
        s.values.insert("requestSeq", PlcValue::Int(0));
        let baseline = ResetBaseline::capture(&s).unwrap();
        assert!(baseline.verify(&s).is_ok());
        s.values.insert("requestSeq", PlcValue::Int(1));
        assert!(baseline.verify(&s).is_err());
    }

    #[test]
    fn heartbeat_requires_a_transition_and_constant_reads_do_not_extend_deadline() {
        assert!(HeartbeatWatch::new(2999).is_err());
        let mut watch = HeartbeatWatch::new(3000).unwrap();
        watch.observe(false, 100).unwrap();
        assert!(!watch.is_live(100));
        assert!(watch.check(1000).is_ok());
        watch.observe(true, 1100).unwrap();
        assert!(watch.is_live(1100));
        watch.observe(true, 4000).unwrap();
        assert!(watch.check(4101).is_err());
        assert!(watch.observe(false, 4101).is_err());
        watch.reset();
        watch.observe(false, 5000).unwrap();
        assert!(!watch.is_live(5000));
        assert!(watch.check(8001).is_err());
    }
}

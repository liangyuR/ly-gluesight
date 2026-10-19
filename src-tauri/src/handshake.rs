use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::{Duration, Instant};

use ly_plc::{Access, DataType, EdgeMode, LinkState, PlcConfig, PlcEngine, PlcError, PlcStatus, PlcValue, PointValue, ProtocolKind, WordOrder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const PROTOCOL_VERSION: u16 = 4;
pub(crate) const SN_MAX_LEN: usize = 50;
const SN_HEADER_ERRORS: [&str; 2] = ["PLC 字符串声明长度", "PLC 字符串实际长度"];

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
    input("requestSeq", DataType::I32, false),
    input("partSn", DataType::String, false),
    input("productCode", DataType::U16, false),
    input("shotCount", DataType::U16, false),
    input("planVersion", DataType::I32, false),
    input("planReserved", DataType::I32, false),
    input("deviceShots", DataType::U16, false),
    input("deviceTriggers", DataType::U16, false),
    input("ackSeq", DataType::I32, false),
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
    output("acceptedSeq", DataType::I32),
    output("acceptedPlanReserved", DataType::I32),
    output("resultSeq", DataType::I32),
    output("resultSn", DataType::String),
    output("resultCode", DataType::U16),
    output("faultCode", DataType::U16),
];

/// 已改名的握手标签（D-0 不兼容旧配置）：旧点表直接拒绝，并说清新名字。
const RETIRED_TAGS: &[(&str, &str)] = &[
    ("planHash", "请使用 planReserved"), ("acceptedPlanHash", "请使用 acceptedPlanReserved"),
    ("camera1Shots", "请使用 deviceShots"), ("camera2Shots", "已删除，不再配置设备通道 2"),
    ("camera3Shots", "已删除，不再配置设备通道 3"), ("camera1Triggers", "请使用 deviceTriggers"),
    ("camera2Triggers", "已删除，不再配置设备通道 2"), ("camera3Triggers", "已删除，不再配置设备通道 3"),
];

const fn input(tag: &'static str, data_type: DataType, edge: bool) -> Field {
    Field { tag, data_type, direction: Direction::Input, edge }
}

const fn output(tag: &'static str, data_type: DataType) -> Field {
    Field { tag, data_type, direction: Direction::Output, edge: false }
}

/// SN 点位的 S7 字符串头字节数：200 SMART 字符串只有实际长度，S7 STRING 另有声明长度。
fn sn_header(data_type: DataType) -> Option<usize> {
    match data_type {
        DataType::SmartString => Some(1),
        DataType::String => Some(2),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Contract {
    config: PlcConfig,
    ids: HashMap<&'static str, String>,
    sn_type: DataType,
}

impl Contract {
    pub(crate) fn validate(config: &PlcConfig) -> Result<Self, String> {
        if config.connection.protocol != ProtocolKind::S7 {
            return Err("一期严格握手仅适用于 S7 协议".into());
        }
        config.validate()?;
        for point in &config.points {
            if let Some((old, new)) = RETIRED_TAGS.iter().find(|(old, _)| point.tags.iter().any(|t| t == old)) {
                return Err(format!("点位 {} 使用已停用的握手标签 {old}（{new}），请按单设备 S7 协议 {PROTOCOL_VERSION} 模板重新生成点表", point.name));
            }
        }
        let mut ids = HashMap::new();
        let mut locations = Vec::new();
        let mut sn_types = Vec::new();
        for field in FIELDS {
            let matches: Vec<_> = config.points.iter().filter(|p| p.tags.iter().any(|t| t == field.tag)).collect();
            let [point] = matches.as_slice() else {
                return Err(format!("握手标签 {} 必须且只能配置一个点位", field.tag));
            };
            if point.tags.iter().filter(|t| t.as_str() == field.tag).count() != 1 {
                return Err(format!("握手标签 {} 重复", field.tag));
            }
            if !point.poll {
                return Err(format!("{} 必须参与轮询，握手依赖轮询读回确认", field.tag));
            }
            let text = field.data_type == DataType::String;
            if text != sn_header(point.data_type).is_some() || (!text && point.data_type != field.data_type) {
                let expected = if text { "SmartString 或 String".to_owned() } else { format!("{:?}", field.data_type) };
                return Err(format!("{} 必须使用 {expected} 类型，请按单设备 S7 协议 {PROTOCOL_VERSION} 模板重新生成点表", field.tag));
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
            if let Some(header) = sn_header(point.data_type) {
                if location.len as usize != SN_MAX_LEN + header {
                    return Err(format!("{} 必须为最大 {SN_MAX_LEN} 字符的 S7 字符串，地址写成 DB{{n}}.DBB起始字节.{SN_MAX_LEN}", field.tag));
                }
                sn_types.push(point.data_type);
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
        let [sn_type, result_sn_type] = sn_types[..] else { return Err("partSn 与 resultSn 必须各配置一个字符串点位".into()) };
        if sn_type != result_sn_type {
            return Err("partSn 与 resultSn 必须使用同一种字符串格式（SmartString 或 String），PLC 才能逐字节比较".into());
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
        Ok(Self { config: config.clone(), ids, sn_type })
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
            (DataType::U16, PlcValue::Int(n)) if u16::try_from(*n).is_err() => return Err(out_of_range(tag, *n, WORD)),
            (DataType::I32, PlcValue::Int(n)) if !(0..=i32::MAX as i64).contains(n) => return Err(out_of_range(tag, *n, DINT)),
            (DataType::U16 | DataType::I32, PlcValue::Int(_)) => true,
            (DataType::String, PlcValue::Text(text)) if self.smart_length_reserved(text) => return Err(format!(
                "{tag} 的 200 SMART 字符串长度字节为 {SN_MAX_LEN}，与 S7 STRING 的声明长度相同：PLC 可能按 S7 STRING 写入，核对两端 SN 格式（200 SMART 字符串 SN 最多 {} 个字符）",
                SN_MAX_LEN - 1,
            )),
            (DataType::String, PlcValue::Text(_)) => true,
            _ => false,
        };
        if !valid {
            return Err(format!("{tag} 的值与 {:?} 类型不符", field.data_type));
        }
        Ok(value)
    }

    /// 200 SMART 字符串的长度字节 50 与 S7 STRING 的声明长度相同，保留用于识别两端格式不一致。
    fn smart_length_reserved(&self, text: &str) -> bool {
        self.sn_type == DataType::SmartString && text.chars().count() == SN_MAX_LEN
    }

    fn sn_header_broken(&self, values: &HashMap<String, PointValue>, tag: &str) -> bool {
        let Some(point) = self.point_id(tag).ok().and_then(|id| values.get(id)) else { return false };
        match (&point.error, &point.value) {
            (Some(PlcError::Decode(m)), _) => SN_HEADER_ERRORS.iter().any(|prefix| m.starts_with(prefix)),
            (None, Some(PlcValue::Text(text))) => self.smart_length_reserved(text),
            _ => false,
        }
    }

    fn validate_writes(&self, writes: &[WriteOp]) -> Result<(), String> {
        let publishes = writes.iter().any(|op| op.tag == "done" && op.value == json!(true));
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
                DataType::I32 => op.value.as_u64().is_some_and(|v| v <= i32::MAX as u64),
                DataType::String => op.value.as_str().is_some_and(|v| check_sn(v).is_ok() || (v.is_empty() && !publishes)),
                _ => false,
            };
            if !valid {
                return Err(format!("{} 的写入值与 {:?} 不符", op.tag, field.data_type));
            }
        }
        Ok(())
    }
}

pub(crate) fn check_sn(sn: &str) -> Result<(), String> {
    let count = sn.chars().count();
    if !(1..=SN_MAX_LEN).contains(&count) {
        return Err(format!("partSn 长度 {count} 不在 1–{SN_MAX_LEN} 个字符之间"));
    }
    if let Some(c) = sn.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(format!("partSn 含非 ASCII 可打印字符 U+{:04X}", c as u32));
    }
    if sn.trim().is_empty() {
        return Err("partSn 不能全为空白".into());
    }
    Ok(())
}

const WORD: &str = "WORD（0–65535）";
const DINT: &str = "非负 DInt（0–2147483647）";

fn out_of_range(tag: &str, value: impl std::fmt::Display, kind: &str) -> String {
    format!("PLC 字段 {tag}={value} 超出 {kind} 范围，按协议不能截断使用；核对 PLC 程序与点表类型")
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub(crate) connection_since: i64,
    pub(crate) poll_count: u64,
    pub(crate) last_poll: i64,
    /// plcHeartbeat 在 ly-plc 轮询缓存里的时间戳：值或读错误变化时更新，读错误恢复为原电平也会前进。
    pub(crate) heartbeat_changed_at: i64,
    values: HashMap<&'static str, PlcValue>,
    broken_outputs: Vec<&'static str>,
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
        Self::read(contract, status, values, now, max_age_ms, false)
    }

    fn read(
        contract: &Contract,
        status: &PlcStatus,
        raw: &HashMap<String, PointValue>,
        now: i64,
        max_age_ms: u64,
        tolerate_output_sn_header: bool,
    ) -> Result<Self, String> {
        let last_poll = check_freshness(status, now, max_age_ms)?;
        let mut values = HashMap::new();
        let mut broken_outputs = Vec::new();
        for field in FIELDS {
            if tolerate_output_sn_header && field.direction == Direction::Output && field.data_type == DataType::String
                && contract.sn_header_broken(raw, field.tag) {
                broken_outputs.push(field.tag);
                continue;
            }
            values.insert(field.tag, contract.value(raw, field.tag)?.clone());
        }
        let heartbeat_changed_at = raw.get(contract.point_id("plcHeartbeat")?).map_or(last_poll, |point| point.ts);
        Ok(Self { connection_since: status.since, poll_count: status.poll_count, last_poll, heartbeat_changed_at, values, broken_outputs })
    }

    pub(crate) fn capture(engine: &PlcEngine, contract: &Contract, max_age_ms: u64) -> Result<Self, String> {
        Self::capture_with(engine, contract, max_age_ms, false)
    }

    pub(crate) fn capture_for_reset(engine: &PlcEngine, contract: &Contract, max_age_ms: u64) -> Result<Self, String> {
        Self::capture_with(engine, contract, max_age_ms, true)
    }

    fn capture_with(engine: &PlcEngine, contract: &Contract, max_age_ms: u64, tolerate_output_sn_header: bool) -> Result<Self, String> {
        contract.check_config(engine)?;
        for _ in 0..3 {
            let before = engine.status();
            let values = engine.values();
            let after = engine.status();
            if before.state == after.state && before.since == after.since && before.poll_count == after.poll_count {
                contract.check_config(engine)?;
                return Self::read(contract, &after, &values, ly_plc::now_ms(), max_age_ms, tolerate_output_sn_header);
            }
        }
        Err("PLC 轮询快照更新中，请重新读取".into())
    }

    pub(crate) fn sn_repair_plan(&self) -> Vec<WriteOp> {
        self.broken_outputs.iter().map(|&tag| WriteOp::new(tag, json!(""))).collect()
    }

    pub(crate) fn read_bool(&self, tag: &str) -> Result<bool, String> {
        match self.values.get(tag) {
            Some(PlcValue::Bool(v)) => Ok(*v),
            _ => Err(format!("{tag} 不是有效 Bool")),
        }
    }

    pub(crate) fn read_text(&self, tag: &str) -> Result<&str, String> {
        match self.values.get(tag) {
            Some(PlcValue::Text(v)) => Ok(v),
            _ => Err(format!("{tag} 不是有效字符串")),
        }
    }

    pub(crate) fn read_u32(&self, tag: &str) -> Result<u32, String> {
        match self.values.get(tag) {
            Some(PlcValue::Int(v)) => u32::try_from(*v).map_err(|_| out_of_range(tag, v, DINT)),
            _ => Err(format!("{tag} 不是有效非负整数")),
        }
    }

    /// WORD 字段：超出 0–65535 报错，不截断。
    pub(crate) fn read_u16(&self, tag: &str) -> Result<u16, String> {
        let value = self.read_u32(tag)?;
        u16::try_from(value).map_err(|_| out_of_range(tag, value, WORD))
    }

    pub(crate) fn request(&self) -> Result<Request, String> {
        if self.read_u32("planReserved")? != 0 { return Err("PLC 计划保留位必须为零".into()); }
        let request = Request {
            protocol_version: self.read_u16("protocolVersion")?,
            request_seq: self.read_u32("requestSeq")?,
            sn: self.read_text("partSn")?.to_owned(),
            product_code: self.read_u16("productCode")?,
            shot_count: self.read_u16("shotCount")?,
            plan_version: self.read_u32("planVersion")?,
            device_shots: self.read_u16("deviceShots")?,
        };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn trigger_count(&self) -> Result<u16, String> {
        self.read_u16("deviceTriggers")
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
        if self.trigger_count()? != 0 {
            return Err("布防前设备触发计数必须清零".into());
        }
        self.request()
    }

    /// 空闲时仍为 1 的 PLC 所有握手位：partEnd / resultAck 应在 PLC 释放时与 partStart 一起清零。
    pub(crate) fn idle_plc_held(&self) -> Result<Vec<&'static str>, String> {
        let mut held = Vec::new();
        for tag in ["partEnd", "resultAck"] {
            if self.read_bool(tag)? { held.push(tag); }
        }
        Ok(held)
    }

    /// 偏离空闲状态（[`idle_plan`]）的视觉所有输出，形如 `visionReady=0`。
    pub(crate) fn idle_drift(&self) -> Result<Vec<String>, String> {
        let mut drifted = Vec::new();
        for op in idle_plan() {
            match op.value.as_bool() {
                Some(expected) => if self.read_bool(op.tag)? != expected { drifted.push(format!("{}={}", op.tag, u8::from(!expected))); },
                None => {
                    let actual = self.read_u32(op.tag)?;
                    if Some(actual as u64) != op.value.as_u64() { drifted.push(format!("{}={actual}", op.tag)); }
                }
            }
        }
        Ok(drifted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) protocol_version: u16,
    pub(crate) request_seq: u32,
    pub(crate) sn: String,
    pub(crate) product_code: u16,
    pub(crate) shot_count: u16,
    pub(crate) plan_version: u32,
    pub(crate) device_shots: u16,
}

impl Request {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(format!("PLC 协议版本 {} 与 PC {} 不一致", self.protocol_version, PROTOCOL_VERSION));
        }
        if self.request_seq == 0 {
            return Err("requestSeq 不能为零".into());
        }
        check_sn(&self.sn)?;
        if self.shot_count == 0 || self.device_shots != self.shot_count {
            return Err("设备计划触发数 deviceShots 必须等于非零 shotCount".into());
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
    pub(crate) sn: String,
    pub(crate) result_code: u16,
    pub(crate) fault_code: u16,
}

pub(crate) fn matching_ack(snapshot: &Snapshot, result: &ResultEnvelope) -> Result<bool, String> {
    if !snapshot.read_bool("done")? {
        return Err("待确认结果的 done 已被清除，必须故障复位".into());
    }
    for (tag, expected) in [
        ("requestSeq", result.request_seq), ("resultSeq", result.request_seq),
        ("resultCode", result.result_code as u32), ("faultCode", result.fault_code as u32),
    ] {
        if snapshot.read_u32(tag)? != expected {
            return Err(format!("待确认结果的 {tag} 与本次结果不一致"));
        }
    }
    if snapshot.read_text("resultSn")? != result.sn {
        return Err("待确认结果的 resultSn 与本次结果不一致".into());
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
    if snapshot.trigger_count()? != request.device_shots {
        return Err(format!("PLC 设备实际触发数 {} 与计划 {} 不符", snapshot.trigger_count()?, request.device_shots));
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
        WriteOp::new("acceptedPlanReserved", json!(0u32)),
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
        WriteOp::new("resultSn", json!(result.sn.as_str())),
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

/// 空闲（无在途事务）时视觉应保持的输出：其余握手位清零、写当前协议版本，就绪位最后置位。
pub(crate) fn idle_plan() -> Vec<WriteOp> {
    vec![
        WriteOp::new("armed", json!(false)),
        WriteOp::new("done", json!(false)),
        WriteOp::new("busy", json!(false)),
        WriteOp::new("visionFault", json!(false)),
        WriteOp::new("pcProtocolVersion", json!(PROTOCOL_VERSION)),
        WriteOp::new("visionReady", json!(true)),
    ]
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
            PlcValue::Text(v) => op.value.as_str() == Some(v.as_str()),
            PlcValue::Float(_) | PlcValue::Bytes(_) => false,
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
        Ok(engine.write_point(contract.point_id(op.tag)?, &op.value).await?)
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
    changed_at: Option<i64>,
    last_change: Option<i64>,
}

impl HeartbeatWatch {
    pub(crate) fn new(timeout_ms: u64) -> Result<Self, String> {
        if timeout_ms < 3000 {
            return Err("PLC 心跳超时不能小于 3000 ms".into());
        }
        Ok(Self { timeout_ms, first_seen: None, last_seen: None, level: None, changed_at: None, last_change: None })
    }

    pub(crate) fn reset(&mut self) {
        *self = Self { timeout_ms: self.timeout_ms, first_seen: None, last_seen: None, level: None, changed_at: None, last_change: None };
    }

    /// `changed_at` 取轮询缓存里心跳点位的时间戳（[`Snapshot::heartbeat_changed_at`]）。
    ///
    /// 连续观察时先按上次翻转判超时，再记这次读数：翻转间隔超过阈值即判失活，此后的翻转不能抵消。
    /// 观察出现空档（节拍被长操作阻塞超过 1/3 阈值）时，空档里 PLC 是否翻转只能看轮询缓存：
    /// 时间戳前进或电平变了就认其最近一次变化的时刻，再判超时，不把 PC 自己没来观察当成 PLC 心跳停止。
    /// 缓存不区分值变化与读错误恢复：空档里心跳所在读块出错又恢复也算一次翻转，PLC 心跳若恰在空档里停止，空档结束后最多再过一个超时周期才判失活。
    pub(crate) fn observe(&mut self, level: bool, changed_at: i64, now: i64) -> Result<(), String> {
        if self.last_seen.is_some_and(|last| now < last) {
            return Err("PLC 心跳时钟倒退，必须重新同步".into());
        }
        let blind = self.last_seen.is_some_and(|last| (now - last) as u64 > self.timeout_ms / 3);
        if self.first_seen.is_some() && !blind {
            self.check(now)?;
        }
        if self.level.is_some_and(|old| old != level) || self.changed_at.is_some_and(|old| changed_at > old) {
            self.last_change = Some(changed_at.min(now));
        }
        self.first_seen.get_or_insert(now);
        self.last_seen = Some(now);
        self.level = Some(level);
        self.changed_at = Some(changed_at);
        if blind { self.check(now) } else { Ok(()) }
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
            offsets[index] += if field.data_type == DataType::String { SN_MAX_LEN as u32 + 2 } else { field.data_type.byte_size().max(1) };
            let address = match field.data_type {
                DataType::Bool => format!("DB{}.DBX{offset}.0", index + 1),
                DataType::U16 => format!("DB{}.DBW{offset}", index + 1),
                DataType::String => format!("DB{}.DBB{offset}.{SN_MAX_LEN}", index + 1),
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
        let status = PlcStatus { state: LinkState::Connected, message: String::new(), since: 100, last_poll: Some(1000), cycle_ms: Some(2), poll_count: 10, error_count: 0,
            dials: 1, outage_dials: 1, last_dial_error: None, last_error: None };
        let mut values: HashMap<_, _> = FIELDS.iter().map(|f| (f.tag.into(), PointValue {
            value: Some(match f.data_type {
                DataType::Bool => PlcValue::Bool(false),
                DataType::String => PlcValue::Text(String::new()),
                _ => PlcValue::Int(0),
            }), error: None, ts: 1,
        })).collect();
        values.get_mut("partSn").unwrap().value = Some(PlcValue::Text("SN-42".into()));
        for (tag, value) in [
            ("protocolVersion", PROTOCOL_VERSION as i64), ("pcProtocolVersion", PROTOCOL_VERSION as i64), ("requestSeq", 7),
            ("productCode", 3), ("shotCount", 4), ("planVersion", 2), ("planReserved", 0),
            ("deviceShots", 4),
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
        ResultEnvelope { request_seq: 7, sn: "SN-42".into(), result_code: 1, fault_code: 0 }
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
    fn unpolled_handshake_point_is_rejected_by_contract() {
        let mut cfg = config();
        assert!(Contract::validate(&cfg).is_ok());
        cfg.points.iter_mut().find(|p| p.id == "resultSn").unwrap().poll = false;
        assert!(Contract::validate(&cfg).unwrap_err().contains("resultSn"));
    }

    #[test]
    fn constant_point_timestamp_is_not_mistaken_for_stale_poll() {
        let (contract, mut status, mut values) = data();
        assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_ok());
        assert!(Snapshot::from_values(&contract, &status, &values, 1600, 500).is_err());
        status.since = 1001;
        assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_err());
        status.since = 100;
        values.get_mut("partSn").unwrap().error = Some(PlcError::Rejected("DB 不可访问".into()));
        assert!(Snapshot::from_values(&contract, &status, &values, 1010, 500).is_err());
    }

    #[test]
    fn start_requires_distinct_completed_polls_same_identity_and_connection() {
        let first = snapshot();
        let mut second = first.clone();
        second.poll_count += 1;
        assert!(confirm_start(&first, &second).is_err());
        second.poll_count += 1;
        assert_eq!(confirm_start(&first, &second).unwrap().device_shots, 4);
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
            ("deviceShots", PlcValue::Int(3)), ("resultAck", PlcValue::Bool(true)),
            ("deviceTriggers", PlcValue::Int(1)), ("done", PlcValue::Bool(true)),
        ] {
            let mut s = snapshot();
            s.values.insert(tag, value);
            assert!(s.start_request().is_err(), "{tag}");
        }
    }

    #[test]
    fn part_sn_outside_1_to_50_printable_ascii_cannot_arm_but_valid_sn_is_kept_verbatim() {
        for sn in ["", "   ", "A\u{7f}B", "A\tB", "SN-\u{e9}", &"X".repeat(51)] {
            let mut s = snapshot();
            s.values.insert("partSn", PlcValue::Text(sn.into()));
            assert!(s.start_request().is_err(), "{sn:?}");
        }
        for sn in [" a-01 ~".to_owned(), "X".repeat(50)] {
            let mut s = snapshot();
            s.values.insert("partSn", PlcValue::Text(sn.clone()));
            assert_eq!(s.start_request().unwrap().sn, sn);
        }
    }

    #[test]
    fn contract_accepts_either_50_char_sn_format_only_when_both_sides_match() {
        let set = |cfg: &mut PlcConfig, tag: &str, data_type: DataType, address: &str| {
            let point = cfg.points.iter_mut().find(|p| p.id == tag).unwrap();
            point.data_type = data_type;
            point.address = address.into();
        };
        let mut cfg = config();
        set(&mut cfg, "partSn", DataType::SmartString, "DB1.DBB160.50");
        set(&mut cfg, "resultSn", DataType::SmartString, "DB2.DBB100.50");
        assert!(Contract::validate(&cfg).is_ok());
        for (tag, data_type, address) in [
            ("partSn", DataType::I32, "DB1.DBD160"), ("resultSn", DataType::U32, "DB2.DBD100"),
            ("partSn", DataType::SmartString, "DB1.DBB160.24"), ("resultSn", DataType::SmartString, "DB2.DBB100.49"),
            ("partSn", DataType::String, "DB1.DBB160.50"),
        ] {
            let mut broken = cfg.clone();
            set(&mut broken, tag, data_type, address);
            assert!(Contract::validate(&broken).is_err(), "{tag} {data_type:?} {address}");
        }
    }

    #[test]
    fn smart_string_length_byte_50_from_s7_string_header_is_read_error_not_shifted_sn() {
        let mut cfg = config();
        for point in cfg.points.iter_mut().filter(|p| p.data_type == DataType::String) {
            point.data_type = DataType::SmartString;
        }
        let contract = Contract::validate(&cfg).unwrap();
        let (_, status, mut values) = data();
        let sn = "9".repeat(SN_MAX_LEN - 1);
        values.get_mut("partSn").unwrap().value = Some(PlcValue::Text(format!("1{sn}")));
        let error = Snapshot::from_values(&contract, &status, &values, 1010, 1000).unwrap_err();
        assert!(error.contains("partSn") && error.contains("S7 STRING"), "{error}");
        values.get_mut("partSn").unwrap().value = Some(PlcValue::Text(sn.clone()));
        assert_eq!(Snapshot::from_values(&contract, &status, &values, 1010, 1000).unwrap().request().unwrap().sn, sn);
    }

    #[test]
    fn negative_dint_from_plc_is_rejected_instead_of_reinterpreted_as_unsigned() {
        let (contract, status, mut values) = data();
        values.get_mut("requestSeq").unwrap().value = Some(PlcValue::Int(i32::MAX as i64));
        assert_eq!(Snapshot::from_values(&contract, &status, &values, 1010, 1000).unwrap().request().unwrap().request_seq, i32::MAX as u32);
        for tag in ["requestSeq", "planVersion", "ackSeq"] {
            let (contract, status, mut values) = data();
            values.get_mut(tag).unwrap().value = Some(PlcValue::Int(-1));
            let error = Snapshot::from_values(&contract, &status, &values, 1010, 1000).unwrap_err();
            assert!(error.contains(tag), "{error}");
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
        s.values.insert("resultSn", PlcValue::Text("SN-42 ".into()));
        assert!(matching_ack(&s, &result).is_err());
        s.values.insert("resultSn", PlcValue::Text("SN-42".into()));
        s.values.insert("done", PlcValue::Bool(false));
        assert!(matching_ack(&s, &result).is_err());
    }

    #[test]
    fn part_end_compares_device_count_and_frozen_request() {
        let mut s = snapshot();
        let request = s.request().unwrap();
        s.values.insert("partEnd", PlcValue::Bool(true));
        s.values.insert("deviceTriggers", PlcValue::Int(4));
        assert!(verify_part_end(&s, &request).is_ok());
        s.values.insert("deviceTriggers", PlcValue::Int(12));
        assert!(verify_part_end(&s, &request).is_err());
        s.values.insert("deviceTriggers", PlcValue::Int(4));
        s.values.insert("requestSeq", PlcValue::Int(8));
        assert!(verify_part_end(&s, &request).is_err());
    }

    #[test]
    fn contract_rejects_retired_multichannel_tags_even_as_extra_points() {
        for old in ["camera1Shots", "camera2Shots", "camera3Shots", "camera1Triggers", "camera2Triggers", "camera3Triggers"] {
            let mut cfg = config();
            cfg.points.push(PlcPoint {
                id: format!("old_{old}"), name: old.into(), address: "DB3.DBW0".into(),
                data_type: DataType::U16, access: Access::Read, tags: vec![old.into()], ..PlcPoint::default()
            });
            let error = Contract::validate(&cfg).unwrap_err();
            assert!(error.contains(old) && error.contains("已停用"), "{error}");
        }
    }

    #[test]
    fn request_deserialization_rejects_retired_channel_payloads() {
        let request = snapshot().request().unwrap();
        let mut value = serde_json::to_value(&request).unwrap();
        value["cameraShots"] = json!([4, 0, 0]);
        assert!(serde_json::from_value::<Request>(value).is_err());
        let mut legacy = serde_json::to_value(&request).unwrap();
        legacy.as_object_mut().unwrap().remove("deviceShots");
        legacy["cameraShots"] = json!([4, 0, 0]);
        assert!(serde_json::from_value::<Request>(legacy).is_err());
        assert_eq!(serde_json::from_value::<Request>(serde_json::to_value(&request).unwrap()).unwrap(), request);
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

    fn idle_snapshot() -> Snapshot {
        let mut s = snapshot();
        s.values.insert("partStart", PlcValue::Bool(false));
        s
    }

    #[test]
    fn idle_checks_split_vision_outputs_from_plc_inputs() {
        // 自动重写只碰 PC 区输出；等待清除的只有 PLC 区输入（协议文档第 2 节的所有者划分）
        let direction = |tag: &str| FIELDS.iter().find(|f| f.tag == tag).unwrap().direction;
        assert!(idle_plan().iter().all(|op| direction(op.tag) == Direction::Output));
        assert_eq!(idle_plan().last().unwrap(), &WriteOp::new("visionReady", json!(true)));
        let s = idle_snapshot();
        assert!(s.idle_drift().unwrap().is_empty());
        assert!(s.idle_plc_held().unwrap().is_empty());
        assert!(s.idle_plc_held().unwrap().iter().all(|tag| direction(tag) == Direction::Input));

        // PLC 重启 / DB 重新初始化：PC 区读回全 0
        let mut s = idle_snapshot();
        for op in idle_plan() {
            s.values.insert(op.tag, if op.value.is_boolean() { PlcValue::Bool(false) } else { PlcValue::Int(0) });
        }
        assert_eq!(s.idle_drift().unwrap(), ["pcProtocolVersion=0", "visionReady=0"]);
        let mut s = idle_snapshot();
        for tag in ["armed", "busy", "done", "visionFault"] { s.values.insert(tag, PlcValue::Bool(true)); }
        assert_eq!(s.idle_drift().unwrap(), ["armed=1", "done=1", "busy=1", "visionFault=1"]);

        let mut s = idle_snapshot();
        s.values.insert("partEnd", PlcValue::Bool(true));
        s.values.insert("resultAck", PlcValue::Bool(true));
        assert_eq!(s.idle_plc_held().unwrap(), ["partEnd", "resultAck"]);
        assert!(s.idle_drift().unwrap().is_empty());
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
        values.get_mut("busy").unwrap().error = Some(PlcError::Rejected("读取被拒绝".into()));
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
        watch.observe(false, 100, 100).unwrap();
        assert!(!watch.is_live(100));
        assert!(watch.check(1000).is_ok());
        watch.observe(true, 1100, 1100).unwrap();
        assert!(watch.is_live(1100));
        watch.observe(true, 1100, 4000).unwrap();
        assert!(watch.check(4101).is_err());
        assert!(watch.observe(false, 4101, 4101).is_err());
        watch.reset();
        watch.observe(false, 4101, 5000).unwrap();
        assert!(!watch.is_live(5000));
        assert!(watch.check(8001).is_err());
    }

    #[test]
    fn blocked_tick_is_judged_by_the_last_polled_change_not_mistaken_for_heartbeat_loss() {
        let mut watch = HeartbeatWatch::new(3000).unwrap();
        watch.observe(false, 0, 0).unwrap();
        watch.observe(true, 500, 500).unwrap();
        // 节拍被长操作阻塞 5 s，其间 PLC 照常翻转：轮询缓存里最近一次变化在 5400
        assert!(watch.observe(true, 5400, 5500).is_ok());
        assert!(watch.is_live(5500));
        // 心跳真的停了：空档后缓存里的变化时刻不再前进，仍判失活
        assert!(watch.observe(true, 5400, 9000).is_err());
    }
}

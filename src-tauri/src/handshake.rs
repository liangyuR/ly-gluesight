use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::{Duration, Instant};

use ly_plc::{
    Access, DataType, LinkState, PlcConfig, PlcEngine, PlcError, PlcStatus, PlcValue, PointValue,
    ProtocolKind, WordOrder,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::inspection::{state, tag};

/// 生产通讯协议 7：双方各一个状态字握手，PC 回显序号与 SN；PLC 只报型号、序号与 SN，不回传计划。
pub(crate) const PROTOCOL_VERSION: u16 = 7;
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
}

const FIELDS: &[Field] = &[
    input(tag::PLC_VERSION, DataType::I16),
    input(tag::PLC_HEARTBEAT, DataType::I16),
    input(tag::PLC_STATE, DataType::I16),
    input(tag::PLC_REASON, DataType::I16),
    input(tag::REQUEST_SEQ, DataType::I32),
    input(tag::PRODUCT_CODE, DataType::I16),
    input(tag::SN, DataType::String),
    output(tag::PC_VERSION, DataType::I16),
    output(tag::PC_HEARTBEAT, DataType::I16),
    output(tag::PC_STATE, DataType::I16),
    output(tag::PC_MODE, DataType::I16),
    output(tag::SEQ_ECHO, DataType::I32),
    output(tag::RESULT_CODE, DataType::I16),
    output(tag::FAULT_CODE, DataType::I16),
    output(tag::SN_ECHO, DataType::String),
];

const PROTOCOL_6: &str = "协议 6 的握手标签已停用";

/// 已停用的握手标签（D-0 不兼容旧配置）：旧点表直接拒绝，并说清原因。
const RETIRED_TAGS: &[(&str, &str)] = &[
    ("planHash", PROTOCOL_6),
    ("acceptedPlanHash", PROTOCOL_6),
    ("camera1Shots", PROTOCOL_6),
    ("camera2Shots", PROTOCOL_6),
    ("camera3Shots", PROTOCOL_6),
    ("camera1Triggers", PROTOCOL_6),
    ("camera2Triggers", PROTOCOL_6),
    ("camera3Triggers", PROTOCOL_6),
    ("partStart", PROTOCOL_6),
    ("partEnd", PROTOCOL_6),
    ("resultAck", PROTOCOL_6),
    ("faultReset", PROTOCOL_6),
    ("visionReady", PROTOCOL_6),
    ("armed", PROTOCOL_6),
    ("busy", PROTOCOL_6),
    ("done", PROTOCOL_6),
    ("visionFault", PROTOCOL_6),
    ("acceptedSeq", PROTOCOL_6),
    ("resultSeq", PROTOCOL_6),
    ("ackSeq", PROTOCOL_6),
    ("shotCount", PROTOCOL_6),
    ("planVersion", PROTOCOL_6),
    ("planReserved", PROTOCOL_6),
    ("deviceShots", PROTOCOL_6),
    ("deviceTriggers", PROTOCOL_6),
    ("acceptedPlanReserved", PROTOCOL_6),
    ("partSn", PROTOCOL_6),
    ("resultSn", PROTOCOL_6),
    ("protocolVersion", PROTOCOL_6),
    ("pcProtocolVersion", PROTOCOL_6),
];

const fn input(tag: &'static str, data_type: DataType) -> Field {
    Field {
        tag,
        data_type,
        direction: Direction::Input,
    }
}

const fn output(tag: &'static str, data_type: DataType) -> Field {
    Field {
        tag,
        data_type,
        direction: Direction::Output,
    }
}

/// SN 点位为 S7 STRING（TIA String[n]）：声明长度、实际长度两个头字节加字符区。
const SN_HEADER_LEN: usize = 2;

/// 与 PLC 页地址表“类型”列同一套叫法，报错能直接对上表格。
fn type_label(data_type: DataType) -> String {
    match data_type {
        DataType::Bool => "Bool".into(),
        DataType::U16 => "UInt16 (Word)".into(),
        DataType::I16 => "Int16 (Int)".into(),
        DataType::U32 => "UInt32 (DWord)".into(),
        DataType::I32 => "Int32 (DInt)".into(),
        DataType::String => "String (S7 STRING)".into(),
        DataType::SmartString => "String (200 SMART)".into(),
        other => format!("{other:?}"),
    }
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
        for point in &config.points {
            if let Some((old, new)) = RETIRED_TAGS.iter().find(|(old, _)| point.tags.iter().any(|t| t == old)) {
                return Err(format!("点位 {} 使用已停用的握手标签 {old}（{new}），请在 PLC 页载入协议 {PROTOCOL_VERSION} 点表", point.name));
            }
        }
        let mut ids = HashMap::new();
        let mut locations = Vec::new();
        for field in FIELDS {
            let matches: Vec<_> = config.points.iter().filter(|p| p.tags.iter().any(|t| t == field.tag)).collect();
            let [point] = matches.as_slice() else {
                return Err(format!("握手标签 {} 必须且只能配置一个点位", field.tag));
            };
            // 报错用地址表里看得到的名称和地址定位点位，标签放括号里备查
            let label = format!("{}（{} · {}）", point.name, field.tag, point.address);
            if point.tags.iter().filter(|t| t.as_str() == field.tag).count() != 1 {
                return Err(format!("{label} 的握手标签重复"));
            }
            if !point.poll {
                return Err(format!("{label} 必须参与轮询，握手依赖轮询读回确认"));
            }
            if point.data_type != field.data_type {
                return Err(format!(
                    "{label} 的类型为 {}，协议 {PROTOCOL_VERSION} 要求 {}；请在 PLC 页“载入一期 S7 点表”重新生成点表",
                    type_label(point.data_type), type_label(field.data_type),
                ));
            }
            let access = if field.direction == Direction::Input { Access::Read } else { Access::ReadWrite };
            if point.access != access {
                return Err(format!("{label} 的访问方向必须为 {:?}", access));
            }
            if point.word_order.is_some_and(|o| o != WordOrder::Abcd) {
                return Err(format!("{label} 必须使用 S7 ABCD 字节序"));
            }
            let location = ly_plc::resolve(&config.connection, &point.address, point.data_type)?;
            if location.area.code != 0x84 || !point.address.trim().to_ascii_uppercase().starts_with("DB") {
                return Err(format!("{label} 必须使用标准 S7 DB 地址"));
            }
            if field.data_type == DataType::String && location.len as usize != SN_MAX_LEN + SN_HEADER_LEN {
                return Err(format!("{label} 必须为最大 {SN_MAX_LEN} 字符的 S7 字符串，地址写成 DB{{n}}.DBB起始字节.{SN_MAX_LEN}"));
            }
            if field.direction == Direction::Output && !location.writable {
                return Err(format!("{label} 所在地址不可写"));
            }
            for (other, other_location, other_label) in &locations {
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
                    return Err(format!("{label} 与 {other_label} 地址重叠，PLC 和 PC 不能共用输出字节"));
                }
            }
            locations.push((*field, location, label));
            ids.insert(field.tag, point.id.clone());
        }
        for point in config.points.iter().filter(|p| !ids.values().any(|id| id == &p.id)) {
            let location = ly_plc::resolve(&config.connection, &point.address, point.data_type)?;
            let direction = if point.access == Access::ReadWrite { Direction::Output } else { Direction::Input };
            for (field, owned, owned_label) in &locations {
                let byte_overlap = location.area == owned.area
                    && location.start < owned.start.saturating_add(owned.len)
                    && owned.start < location.start.saturating_add(location.len);
                let different_bits = location.start == owned.start
                    && location.bit.zip(owned.bit).is_some_and(|(a, b)| a != b);
                if byte_overlap && (direction != field.direction || !different_bits) {
                    return Err(format!("额外点位 {}（{}）与握手字段 {owned_label} 共用地址或跨方向字节", point.name, point.address));
                }
            }
        }
        if config.heartbeat.point_id.as_deref() != ids.get(tag::PC_HEARTBEAT).map(String::as_str) {
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
            // SN 头字节错误多半是 PLC 侧数据块声明或写法不对，直接告诉现场该改哪里
            let hint = match error {
                PlcError::Decode(m) if m.starts_with(SN_HEADER_ERRORS[0]) => format!(
                    "；PLC 数据块中 {tag} 须声明为 String[{SN_MAX_LEN}]（TIA 里不写长度的 String 默认为 String[254]），改后重新初始化该 DB，再在在线检测页复位"
                ),
                PlcError::Decode(m) if m.starts_with(SN_HEADER_ERRORS[1]) => format!(
                    "；PLC 写入的 {tag} 超过 {SN_MAX_LEN} 个字符，或头字节被块操作覆盖，核对 PLC 程序后再复位"
                ),
                _ => String::new(),
            };
            return Err(format!("{tag} 读取失败：{error}{hint}"));
        }
        let value = point.value.as_ref().ok_or_else(|| format!("{tag} 没有有效值"))?;
        let field = FIELDS.iter().find(|f| f.tag == tag).ok_or_else(|| format!("未知握手标签：{tag}"))?;
        let valid = match (field.data_type, value) {
            (DataType::Bool, PlcValue::Bool(_)) => true,
            (DataType::I16, PlcValue::Int(_)) if tag == tag::PLC_HEARTBEAT => true,
            (DataType::I16, PlcValue::Int(n)) if !(0..=i16::MAX as i64).contains(n) => return Err(out_of_range(tag, *n, INT)),
            (DataType::I32, PlcValue::Int(n)) if !(0..=i32::MAX as i64).contains(n) => return Err(out_of_range(tag, *n, DINT)),
            (DataType::I16 | DataType::I32, PlcValue::Int(_)) => true,
            (DataType::String, PlcValue::Text(_)) => true,
            _ => false,
        };
        if !valid {
            return Err(format!("{tag} 的值与 {:?} 类型不符", field.data_type));
        }
        Ok(value)
    }

    fn sn_header_broken(&self, values: &HashMap<String, PointValue>, tag: &str) -> bool {
        let Some(point) = self.point_id(tag).ok().and_then(|id| values.get(id)) else { return false };
        matches!(&point.error, Some(PlcError::Decode(m)) if SN_HEADER_ERRORS.iter().any(|prefix| m.starts_with(prefix)))
    }

    fn validate_writes(&self, writes: &[WriteOp]) -> Result<(), String> {
        let publishes = writes
            .iter()
            .any(|op| op.tag == tag::PC_STATE && op.value == json!(state::PC_RESULT));
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
                DataType::I16 => op.value.as_u64().is_some_and(|v| v <= i16::MAX as u64),
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
        return Err(format!("sn 长度 {count} 不在 1–{SN_MAX_LEN} 个字符之间"));
    }
    if let Some(c) = sn.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(format!("sn 含非 ASCII 可打印字符 U+{:04X}", c as u32));
    }
    if sn.trim().is_empty() {
        return Err("sn 不能全为空白".into());
    }
    Ok(())
}

const INT: &str = "非负 Int（0–32767）";
const DINT: &str = "非负 DInt（0–2147483647）";

fn out_of_range(tag: &str, value: impl std::fmt::Display, kind: &str) -> String {
    format!("PLC 字段 {tag}={value} 超出 {kind} 范围，按协议不能截断使用；核对 PLC 程序与点表类型")
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub(crate) connection_since: i64,
    pub(crate) poll_count: u64,
    /// plcHeartbeat 在 ly-plc 轮询缓存里的时间戳：值或读错误变化时更新，读错误恢复为原值也会前进。
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
    #[cfg(test)]
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
        let heartbeat_changed_at = raw
            .get(contract.point_id(tag::PLC_HEARTBEAT)?)
            .map_or(last_poll, |point| point.ts);
        Ok(Self {
            connection_since: status.since,
            poll_count: status.poll_count,
            heartbeat_changed_at,
            values,
            broken_outputs,
        })
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

    /// Int 字段：超出 0–32767 报错，不截断。
    pub(crate) fn read_u16(&self, tag: &str) -> Result<u16, String> {
        let value = self.read_u32(tag)?;
        u16::try_from(value).ok().filter(|v| *v <= i16::MAX as u16).ok_or_else(|| out_of_range(tag, value, INT))
    }

    pub(crate) fn plc_state(&self) -> Result<u16, String> {
        self.read_u16(tag::PLC_STATE)
    }

    pub(crate) fn pc_state(&self) -> Result<u16, String> {
        self.read_u16(tag::PC_STATE)
    }

    /// PLC 心跳可加 1 自然回绕，允许负值。
    pub(crate) fn plc_heartbeat(&self) -> Result<i64, String> {
        match self.values.get(tag::PLC_HEARTBEAT) {
            Some(PlcValue::Int(v)) => Ok(*v),
            _ => Err("plcHeartbeat 不是有效整数".into()),
        }
    }

    pub(crate) fn plc_reason(&self) -> Result<String, String> {
        let code = self.read_u16(tag::PLC_REASON)?;
        Ok(format!("plcReason={code} {}", plc_reason_text(code)))
    }

    pub(crate) fn request(&self) -> Result<Request, String> {
        let request = Request {
            protocol_version: self.read_u16(tag::PLC_VERSION)?,
            request_seq: self.read_u32(tag::REQUEST_SEQ)?,
            sn: self.read_text(tag::SN)?.to_owned(),
            product_code: self.read_u16(tag::PRODUCT_CODE)?,
        };
        request.validate()?;
        Ok(request)
    }

    fn start_request(&self) -> Result<Request, String> {
        if self.plc_state()? != state::PLC_REQUEST {
            return Err("开始握手要求 plcState=10".into());
        }
        if self.pc_state()? != state::PC_READY {
            return Err("开始握手要求 pcState=10".into());
        }
        if self.read_u16(tag::PC_VERSION)? != PROTOCOL_VERSION {
            return Err("PC 协议版本尚未确认，需复位".into());
        }
        self.request()
    }

    /// 偏离空闲状态（[`idle_plan`]）的视觉输出，形如 `pcState=0`。
    pub(crate) fn idle_drift(&self, mode: u16) -> Result<Vec<String>, String> {
        let mut drifted = Vec::new();
        for op in idle_plan(mode) {
            let actual = self.read_u32(op.tag)?;
            if Some(u64::from(actual)) != op.value.as_u64() {
                drifted.push(format!("{}={actual}", op.tag));
            }
        }
        Ok(drifted)
    }
}

/// PC 置故障（pcState=90）时写入的 faultCode：PC 握手故障，与检测错误 91–100 区分。
pub(crate) const SESSION_FAULT_CODE: u16 = 101;

pub(crate) fn plc_reason_text(code: u16) -> &'static str {
    match code {
        0 => "无",
        4 => "现场互锁不满足",
        5 => "协议或状态非法",
        6 => "PC 心跳失活",
        7 => "运动超时",
        8 => "等待 PC 结果超时",
        9 => "现场执行故障",
        10 => "等待 PC 回就绪超时",
        11 => "等待 PC 布防超时",
        12 => "序号耗尽",
        13 => "复位条件不满足，拒绝软件复位",
        14 => "在途中 PC 故障或未就绪",
        _ => "未定义的原因码",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) protocol_version: u16,
    pub(crate) request_seq: u32,
    pub(crate) sn: String,
    pub(crate) product_code: u16,
}

impl Request {
    /// 请求层只校验协议版本、序号与 SN；型号由接件按配方核对。
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(format!("PLC 协议版本 {} 与 PC {} 不一致", self.protocol_version, PROTOCOL_VERSION));
        }
        if self.request_seq == 0 {
            return Err("requestSeq 不能为零".into());
        }
        check_sn(&self.sn)?;
        Ok(())
    }
}

pub(crate) fn confirm_start(first: &Snapshot, second: &Snapshot) -> Result<Request, String> {
    if first.connection_since != second.connection_since {
        return Err("开始校验期间 PLC 已重新连接，必须复位".into());
    }
    // 相隔两次后续轮询：请求字段须在约两个轮询周期内保持不变，才认作 PLC 已写完整的一件。
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

/// PLC 置 30 表示已锁存结果；结果身份与 PC 输出必须保持发布时的值。
pub(crate) fn matching_ack(snapshot: &Snapshot, result: &ResultEnvelope) -> Result<bool, String> {
    if snapshot.pc_state()? != state::PC_RESULT {
        return Err("待确认结果的 pcState 已不是 30，必须故障复位".into());
    }
    for (tag, expected) in [
        (tag::REQUEST_SEQ, result.request_seq),
        (tag::SEQ_ECHO, result.request_seq),
        (tag::RESULT_CODE, result.result_code as u32),
        (tag::FAULT_CODE, result.fault_code as u32),
    ] {
        if snapshot.read_u32(tag)? != expected {
            return Err(format!("待确认结果的 {tag} 与本次结果不一致"));
        }
    }
    if snapshot.read_text(tag::SN_ECHO)? != result.sn {
        return Err("待确认结果的 snEcho 与本次结果不一致".into());
    }
    match snapshot.plc_state()? {
        state::PLC_CONFIRMED => Ok(true),
        state::PLC_REQUEST | state::PLC_MOTION_END => Ok(false),
        other => Err(format!("等待 PLC 确认结果时 plcState={other}")),
    }
}

/// 运动结束只核对 PLC 状态与请求身份，返回预留计数 0。
pub(crate) fn verify_part_end(snapshot: &Snapshot, request: &Request) -> Result<u16, String> {
    if snapshot.plc_state()? != state::PLC_MOTION_END {
        return Err("运动结束要求 plcState=20".into());
    }
    if snapshot.request()? != *request {
        return Err("运动结束时请求身份已改变".into());
    }
    Ok(0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResetBaseline {
    pub(crate) connection_since: i64,
    pub(crate) request_seq: u32,
}

impl ResetBaseline {
    pub(crate) fn capture(snapshot: &Snapshot) -> Result<Self, String> {
        if snapshot.read_u16(tag::PLC_VERSION)? != PROTOCOL_VERSION {
            return Err("PLC 协议版本不一致，不能完成复位".into());
        }
        let plc = snapshot.plc_state()?;
        if !matches!(plc, state::PLC_IDLE | state::PLC_RESET) {
            return Err(format!(
                "复位要求 PLC 处于空闲（0）或复位请求（99），当前 plcState={plc}"
            ));
        }
        Ok(Self {
            connection_since: snapshot.connection_since,
            request_seq: snapshot.read_u32(tag::REQUEST_SEQ)?,
        })
    }

    pub(crate) fn verify(&self, snapshot: &Snapshot) -> Result<(), String> {
        if Self::capture(snapshot)? != *self {
            return Err("复位期间 PLC 会话或请求序号变化，不能置就绪".into());
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

/// 布防：先写回显与模式，最后单独写状态。
pub(crate) fn arm_plan(request: &Request, mode: u16) -> Vec<WriteOp> {
    vec![
        WriteOp::new(tag::SEQ_ECHO, json!(request.request_seq)),
        WriteOp::new(tag::PC_MODE, json!(mode)),
        WriteOp::new(tag::PC_STATE, json!(state::PC_ARMED)),
    ]
}

pub(crate) fn report_plan(result: &ResultEnvelope) -> Vec<WriteOp> {
    vec![
        WriteOp::new(tag::SEQ_ECHO, json!(result.request_seq)),
        WriteOp::new(tag::RESULT_CODE, json!(result.result_code)),
        WriteOp::new(tag::FAULT_CODE, json!(result.fault_code)),
        WriteOp::new(tag::SN_ECHO, json!(result.sn.as_str())),
        WriteOp::new(tag::PC_STATE, json!(state::PC_RESULT)),
    ]
}

/// 故障：原因写在 faultCode，最后置 90。
pub(crate) fn fault_plan(fault_code: u16) -> Vec<WriteOp> {
    vec![
        WriteOp::new(tag::FAULT_CODE, json!(fault_code)),
        WriteOp::new(tag::PC_STATE, json!(state::PC_FAULT)),
    ]
}

pub(crate) fn not_ready_plan() -> Vec<WriteOp> {
    vec![WriteOp::new(tag::PC_STATE, json!(state::PC_NOT_READY))]
}

pub(crate) fn reset_request_plan() -> Vec<WriteOp> {
    vec![WriteOp::new(tag::PC_STATE, json!(state::PC_RESET))]
}

/// 复位完成前清掉上一件留下的结果字段。
pub(crate) fn reset_clear_plan() -> Vec<WriteOp> {
    vec![
        WriteOp::new(tag::RESULT_CODE, json!(0)),
        WriteOp::new(tag::FAULT_CODE, json!(0)),
        WriteOp::new(tag::SN_ECHO, json!("")),
    ]
}

/// 空闲时视觉应保持的输出：版本、模式，最后置就绪。
pub(crate) fn idle_plan(mode: u16) -> Vec<WriteOp> {
    vec![
        WriteOp::new(tag::PC_VERSION, json!(PROTOCOL_VERSION)),
        WriteOp::new(tag::PC_MODE, json!(mode)),
        WriteOp::new(tag::PC_STATE, json!(state::PC_READY)),
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

/// 一次写后确认的耗时：逐点写入（含等驱动锁）与读回确认，以及确认用了几次轮询。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WriteTiming {
    pub(crate) write: Duration,
    pub(crate) confirm: Duration,
    pub(crate) polls: u64,
}

pub(crate) async fn write_confirmed(
    engine: &PlcEngine,
    contract: &Contract,
    writes: &[WriteOp],
    timeout: Duration,
    max_age_ms: u64,
) -> Result<WriteTiming, String> {
    contract.validate_writes(writes)?;
    if writes.is_empty() {
        return Ok(WriteTiming::default());
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
    let write = started.elapsed();
    // ly-plc 在持有驱动锁期间发布轮询值并计数：written 之后计入的轮询一定读于写入之后，立即补一次轮询即可核对。
    engine.poll_now().await.map_err(|error| format!("PLC 写入后立即读回失败：{error}"))?;
    let mut first = true;
    while started.elapsed() < timeout {
        if !std::mem::take(&mut first) { tokio::time::sleep(poll_wait(contract)).await; }
        let before = connection_stamp(engine, contract, max_age_ms)?;
        if before.since != initial.since {
            return Err("PLC 读回确认期间已重新连接，必须复位".into());
        }
        if before.poll_count <= written.poll_count {
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
            return Ok(WriteTiming { write, confirm: started.elapsed() - write, polls: after.poll_count - written.poll_count });
        }
    }
    Err("PLC 写入后的新轮询读回未确认，保持当前握手状态".into())
}

#[derive(Clone, Debug)]
pub(crate) struct HeartbeatWatch {
    timeout_ms: u64,
    first_seen: Option<i64>,
    last_seen: Option<i64>,
    level: Option<i64>,
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
    /// 连续观察时先按上次变化判超时，再记这次读数：变化间隔超过阈值即判失活，此后的变化不能抵消。
    /// 观察出现空档（节拍被长操作阻塞超过 1/3 阈值）时，空档里 PLC 是否变化只能看轮询缓存：
    /// 时间戳前进或值变了就认其最近一次变化的时刻，再判超时，不把 PC 自己没来观察当成 PLC 心跳停止。
    /// 缓存不区分值变化与读错误恢复：空档里心跳所在读块出错又恢复也算一次变化，PLC 心跳若恰在空档里停止，空档结束后最多再过一个超时周期才判失活。
    pub(crate) fn observe(&mut self, level: i64, changed_at: i64, now: i64) -> Result<(), String> {
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
            return Err(format!("PLC 心跳超过 {} ms 未变化", self.timeout_ms));
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
                DataType::I16 => format!("DB{}.DBW{offset}", index + 1),
                DataType::String => format!("DB{}.DBB{offset}.{SN_MAX_LEN}", index + 1),
                _ => format!("DB{}.DBD{offset}", index + 1),
            };
            PlcPoint {
                id: field.tag.into(), name: field.tag.into(), address, data_type: field.data_type,
                access: if field.direction == Direction::Input { Access::Read } else { Access::ReadWrite },
                tags: vec![field.tag.into()], ..PlcPoint::default()
            }
        }).collect();
        PlcConfig {
            connection: ConnectionConfig {
                protocol: ProtocolKind::S7,
                host: "127.0.0.1".into(),
                port: 102,
                poll_interval_ms: 50,
                ..ConnectionConfig::default()
            },
            points,
            heartbeat: HeartbeatConfig {
                point_id: Some(tag::PC_HEARTBEAT.into()),
                interval_ms: 1000,
            },
            ..PlcConfig::default()
        }
    }

    fn data() -> (Contract, PlcStatus, HashMap<String, PointValue>) {
        let contract = Contract::validate(&config()).unwrap();
        let status = PlcStatus { state: LinkState::Connected, message: String::new(), since: 100, last_poll: Some(1000), cycle_ms: Some(2), poll_count: 10, error_count: 0,
            dials: 1, outage_dials: 1, last_dial_error: None, last_error: None };
        let mut values: HashMap<_, _> = FIELDS.iter().map(|f| (f.tag.into(), PointValue {
            value: Some(match f.data_type {
                DataType::String => PlcValue::Text(String::new()),
                _ => PlcValue::Int(0),
            }), error: None, ts: 1,
        })).collect();
        values.get_mut(tag::SN).unwrap().value = Some(PlcValue::Text("SN-42".into()));
        for (tag, value) in [
            (tag::PLC_VERSION, PROTOCOL_VERSION as i64),
            (tag::PC_VERSION, PROTOCOL_VERSION as i64),
            (tag::REQUEST_SEQ, 7),
            (tag::PRODUCT_CODE, 3),
            (tag::PLC_STATE, state::PLC_REQUEST as i64),
            (tag::PC_STATE, state::PC_READY as i64),
        ] {
            values.get_mut(tag).unwrap().value = Some(PlcValue::Int(value));
        }
        (contract, status, values)
    }

    fn snapshot() -> Snapshot {
        let (contract, status, values) = data();
        Snapshot::from_values(&contract, &status, &values, 1010, 1000).unwrap()
    }

    #[test]
    fn contract_rejects_plc_and_pc_fields_sharing_bytes() {
        let mut cfg = config();
        cfg.points
            .iter_mut()
            .find(|p| p.id == tag::PLC_STATE)
            .unwrap()
            .address = "DB10.DBW0".into();
        cfg.points
            .iter_mut()
            .find(|p| p.id == tag::PC_STATE)
            .unwrap()
            .address = "DB10.DBW1".into();
        assert!(Contract::validate(&cfg).is_err());
        cfg.points
            .iter_mut()
            .find(|p| p.id == tag::PC_STATE)
            .unwrap()
            .address = "DB10.DBW2".into();
        assert!(Contract::validate(&cfg).is_ok());
    }

    #[test]
    fn start_requires_distinct_completed_polls_same_identity_and_connection() {
        let first = snapshot();
        let mut second = first.clone();
        second.poll_count += 1;
        assert!(confirm_start(&first, &second).is_err());
        second.poll_count += 1;
        assert_eq!(confirm_start(&first, &second).unwrap().product_code, 3);
        second.values.insert(tag::PRODUCT_CODE, PlcValue::Int(4));
        assert!(confirm_start(&first, &second).is_err());
        second = first.clone();
        second.poll_count += 2;
        second.connection_since += 1;
        assert!(confirm_start(&first, &second).is_err());
    }

    /// 只接受 PC 当前的协议版本：PLC 仍写上一版（协议 6）不能开始；参数表由 PROTOCOL_VERSION 派生，升版时随之移动。
    #[test]
    fn plc_writing_previous_protocol_cannot_start() {
        let current = i64::from(PROTOCOL_VERSION);
        for (version, accepted) in [(current - 1, false), (current, true)] {
            let mut s = snapshot();
            s.values.insert(tag::PLC_VERSION, PlcValue::Int(version));
            assert_eq!(s.start_request().is_ok(), accepted, "plcVersion={version}");
        }
    }

    #[test]
    fn heartbeat_requires_a_change_and_constant_reads_do_not_extend_deadline() {
        assert!(HeartbeatWatch::new(2999).is_err());
        let mut watch = HeartbeatWatch::new(3000).unwrap();
        watch.observe(0, 100, 100).unwrap();
        assert!(!watch.is_live(100));
        assert!(watch.check(1000).is_ok());
        watch.observe(1, 1100, 1100).unwrap();
        assert!(watch.is_live(1100));
        watch.observe(1, 1100, 4000).unwrap();
        assert!(watch.check(4101).is_err());
        assert!(watch.observe(2, 4101, 4101).is_err());
        watch.reset();
        watch.observe(2, 4101, 5000).unwrap();
        assert!(!watch.is_live(5000));
        assert!(watch.check(8001).is_err());
    }
}

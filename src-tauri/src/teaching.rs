use std::path::PathBuf;
use std::time::{Duration, Instant};

use ly_plc::{Access, DataType, LinkState, PlcEngine, PlcValue, VerifyOptions};
use serde::{Deserialize, Serialize};
use serde_json::json;

const PC_TAGS: [&str; 5] = [
    "teachPcVersion",
    "teachPcState",
    "teachRoundId",
    "teachPcReason",
    "teachPcReserved",
];
const PLC_TAGS: [&str; 5] = [
    "teachPlcVersion",
    "teachPlcState",
    "teachRoundIdEcho",
    "teachPlcReason",
    "teachPlcReserved",
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachingView {
    pub available: bool,
    pub busy: bool,
    pub round_id: Option<u16>,
    pub phase: String,
    pub message: String,
    pub monitor: TeachingMonitor,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachingMonitor {
    pub pc: Option<TeachingRegisters>,
    pub plc: Option<TeachingRegisters>,
    pub fresh: bool,
    pub violation: Option<TeachingViolation>,
}

/// PLC 示教寄存器不合规的那一项，供现场定位 PLC 程序。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachingViolation {
    pub field: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachingRegisters {
    pub version: u16,
    pub state: u16,
    pub round_id: u16,
    pub reason: u16,
    pub reserved: u16,
}

impl From<TeachingReply> for TeachingRegisters {
    fn from(reply: TeachingReply) -> Self {
        Self {
            version: reply.version,
            state: reply.state,
            round_id: reply.id,
            reason: reply.reason,
            reserved: reply.reserved,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum TeachEvent {
    None,
    Accepted,
    Ended,
    Rejected(String),
    Released,
    Fault(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    high_water: u16,
    pending: Option<Pending>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    id: u16,
    capture_round_id: String,
    #[serde(default)]
    offline: bool,
    pc_state: u16,
    reason: u16,
    accepted: bool,
    ended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeachingReply {
    pub version: u16,
    pub state: u16,
    pub id: u16,
    pub reason: u16,
    pub reserved: u16,
}

impl TeachingReply {
    fn check(self) -> Result<(), TeachingViolation> {
        let (state, reason) = (self.state, self.reason);
        let violation = |field, detail: String| Err(TeachingViolation { field, detail });
        if self.version != 1 {
            return violation("version", format!("协议版本为 {}，应为 1", self.version));
        }
        if self.reserved != 0 {
            return violation("reserved", format!("保留字段为 {}，应为 0", self.reserved));
        }
        if !matches!(state, 0 | 10 | 20 | 30 | 40 | 50) {
            return violation("state", format!("状态 {state} 不是协议状态（0/10/20/30/40/50）"));
        }
        if self.id > 32767 || (state != 0 && state != 50 && self.id == 0) {
            return violation("roundId", format!("状态 {state} 时握手编号回显为 {}：PLC 须把 PC 的 teachRoundId 写回 teachRoundIdEcho", self.id));
        }
        match state {
            0 if reason != 0 => violation("reason", format!("回空闲后原因码 {reason} 未清 0")),
            10 | 20 | 30 if reason != 0 => violation("reason", format!("状态 {state} 时原因码应为 0，实际 {reason}")),
            40 if !(1..=5).contains(&reason) => violation("reason", format!("拒绝（状态 40）原因码 {reason} 不在 1–5")),
            50 if !(4..=10).contains(&reason) => violation("reason", format!("故障（状态 50）原因码 {reason} 不在 4–10")),
            _ => Ok(()),
        }
    }

    fn validate(self) -> Result<(), String> {
        self.check().map_err(|v| format!(
            "示教 PLC 寄存器不合规：{}（DB12 版本 {} 状态 {} 编号 {} 原因 {} 保留 {}）",
            v.detail, self.version, self.state, self.id, self.reason, self.reserved
        ))
    }
}

fn plc_state_name(state: u16) -> &'static str {
    match state {
        0 => "空闲",
        10 => "已接受",
        20 => "采集结束",
        30 => "已确认",
        40 => "已拒绝",
        _ => "故障",
    }
}

pub struct Teaching {
    path: PathBuf,
    offline: bool,
    journal: Journal,
    load_error: Option<String>,
    ready: bool,
    resetting: bool,
    configured: bool,
    faulted: bool,
    message: String,
    previous: Option<TeachingReply>,
    last_reply: Option<TeachingReply>,
    monitor: TeachingMonitor,
    monitor_observed_at: Option<Instant>,
    monitor_retry_at: Option<Instant>,
    connection: Option<(i64, u64)>,
    since: Instant,
}

impl Teaching {
    pub fn open(path: PathBuf) -> Self {
        let loaded = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Journal>(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(e) => Err(e.to_string()),
        }
        .and_then(|j| {
            if j.high_water > 32767
                || j.pending.as_ref().is_some_and(|p| {
                    p.id == 0
                        || p.id > j.high_water
                        || p.capture_round_id.is_empty()
                        || !matches!(p.pc_state, 0 | 10 | 20 | 30 | 40)
                })
            {
                Err("示教持久记录非法".into())
            } else {
                Ok(j)
            }
        });
        let (journal, load_error) = match loaded {
            Ok(j) => (j, None),
            Err(e) => (Journal::default(), Some(e)),
        };
        Self {
            path,
            offline: false,
            journal,
            load_error,
            ready: false,
            resetting: false,
            configured: false,
            faulted: false,
            message: "示教接口等待连接和显式复位".into(),
            previous: None,
            last_reply: None,
            monitor: TeachingMonitor::default(),
            monitor_observed_at: None,
            monitor_retry_at: None,
            connection: None,
            since: Instant::now(),
        }
    }

    pub fn set_origin(&mut self, offline: bool) {
        if self.offline != offline {
            self.offline = offline;
            self.ready = false;
            self.resetting = false;
            self.previous = None;
            self.last_reply = None;
            self.monitor = TeachingMonitor::default();
            self.monitor_retry_at = None;
            self.connection = None;
            self.message = "示教来源已切换，需要显式复位".into();
        }
    }

    fn check_origin(&self) -> Result<(), String> {
        if self
            .journal
            .pending
            .as_ref()
            .is_some_and(|p| p.offline != self.offline)
        {
            return Err("示教未结事务属于另一数据来源，请回原真机或离线环境处置".into());
        }
        Ok(())
    }

    pub fn blocks_site_switch(&self) -> bool {
        self.load_error.is_some()
            || self.resetting
            || self
                .journal
                .pending
                .as_ref()
                .is_some_and(|p| !p.offline || self.offline)
    }

    pub fn reset_pending(&self) -> bool {
        self.resetting
    }

    pub fn busy(&self) -> bool {
        self.journal.pending.is_some() || self.load_error.is_some() || self.resetting
    }
    pub fn bound_round_id(&self) -> Option<&str> {
        self.journal
            .pending
            .as_ref()
            .map(|p| p.capture_round_id.as_str())
    }
    pub fn view(&self) -> TeachingView {
        let phase = if !self.configured {
            "unavailable"
        } else if self.faulted {
            "fault"
        } else if !self.ready {
            "resetRequired"
        } else {
            match self.journal.pending.as_ref() {
                None => "idle",
                Some(p) if p.pc_state == 0 => "releasing",
                Some(p) if p.pc_state == 40 => "cancelling",
                Some(p) if p.pc_state == 20 || p.pc_state == 30 => "reporting",
                Some(p) if p.ended => "draining",
                Some(p) if p.accepted => "accepted",
                Some(_) => "requested",
            }
        };
        let mut monitor = self.monitor.clone();
        monitor.fresh &= self
            .monitor_observed_at
            .is_some_and(|at| at.elapsed() <= Duration::from_secs(3));
        TeachingView {
            available: self.configured && self.ready && !self.faulted && self.load_error.is_none(),
            busy: self.busy(),
            round_id: self.journal.pending.as_ref().map(|p| p.id),
            phase: phase.into(),
            message: self.message.clone(),
            monitor,
        }
    }

    pub fn monitor_view(&self, engine: &PlcEngine) -> TeachingView {
        let mut view = self.view();
        let status = engine.status();
        view.monitor.fresh &= status.state == LinkState::Connected
            && self.connection == Some((status.since, status.dials));
        view
    }

    fn save(&mut self) -> Result<(), String> {
        let result = serde_json::to_string_pretty(&self.journal)
            .map_err(|e| e.to_string())
            .and_then(|text| crate::fsio::write_atomic(&self.path, &text));
        if let Err(error) = &result {
            self.load_error = Some(error.clone());
            self.ready = false;
            self.faulted = true;
            self.message = format!("示教持久化失败：{error}");
        }
        result
    }

    pub fn force_fault(&mut self, reason: String) -> TeachEvent {
        self.fail(reason)
    }

    fn fail(&mut self, reason: String) -> TeachEvent {
        self.ready = false;
        self.resetting = false;
        self.faulted = true;
        self.message = reason.clone();
        TeachEvent::Fault(reason)
    }

    pub fn initialize_after_reset(&mut self, reply: TeachingReply) -> Result<(), String> {
        self.check_origin()?;
        if let Some(error) = &self.load_error {
            return Err(format!("示教记录损坏，须人工恢复编号后重启：{error}"));
        }
        reply.validate()?;
        if reply.state != 0 {
            return Err("PLC 示教尚未回空闲".into());
        }
        self.journal.high_water = self.journal.high_water.max(reply.id);
        self.journal.pending = None;
        self.save()?;
        self.ready = true;
        self.faulted = false;
        self.resetting = false;
        self.last_reply = Some(reply);
        self.message = "示教接口已就绪".into();
        Ok(())
    }

    pub fn prepare_request(&mut self, capture_round_id: &str) -> Result<u16, String> {
        if !self.ready || self.faulted || self.busy() {
            return Err("示教接口未就绪或仍有未结轮次".into());
        }
        if capture_round_id.is_empty() {
            return Err("采集轮次不能为空".into());
        }
        let reply = self.last_reply.ok_or("尚未读取 PLC 示教状态")?;
        if reply.state != 0 {
            return Err("PLC 示教尚未空闲".into());
        }
        let id = self
            .journal
            .high_water
            .max(reply.id)
            .checked_add(1)
            .filter(|id| *id <= 32767)
            .ok_or("示教编号已到 32767，须维护处置，不能自动回绕")?;
        self.journal.high_water = id;
        self.journal.pending = Some(Pending {
            id,
            capture_round_id: capture_round_id.into(),
            offline: self.offline,
            pc_state: 10,
            reason: 0,
            accepted: false,
            ended: false,
        });
        self.save()?;
        self.since = Instant::now();
        self.message = "已准备图像接收，正在请求 PLC 示教采集".into();
        Ok(id)
    }

    pub fn observe_reply(&mut self, mut reply: TeachingReply) -> Result<TeachEvent, String> {
        if let Some(last) = self
            .last_reply
            .filter(|last| last.state == reply.state && (last.id == reply.id || reply.state == 0))
        {
            reply.reason = last.reason;
        }
        reply.validate()?;
        self.last_reply = Some(reply);
        let Some(p) = self.journal.pending.as_mut() else {
            if reply.state != 0 {
                return Err("PLC 留有未结示教状态，请核对并复位".into());
            }
            self.journal.high_water = self.journal.high_water.max(reply.id);
            return Ok(TeachEvent::None);
        };
        if reply.state == 50 {
            return Err(format!("PLC 示教故障，原因码 {}", reply.reason));
        }
        if reply.state == 0 && matches!(p.pc_state, 10 | 30 | 40) && !p.accepted && reply.id <= p.id
        {
            return Ok(TeachEvent::None);
        }
        if reply.id != p.id {
            return Err("PLC 示教回显编号与当前轮次不一致".into());
        }
        let before = serde_json::to_string(&self.journal).map_err(|e| e.to_string())?;
        let p = self.journal.pending.as_mut().unwrap();
        let event = match reply.state {
            0 if p.pc_state == 0 => {
                self.journal.pending = None;
                self.message = "示教轮次已确认并释放".into();
                TeachEvent::Released
            }
            10 if matches!(p.pc_state, 10 | 30 | 40) && !p.ended => {
                if p.accepted {
                    TeachEvent::None
                } else {
                    p.accepted = true;
                    self.message = "PLC 已接受示教，运动和采集由 PLC 控制".into();
                    if p.pc_state == 10 {
                        TeachEvent::Accepted
                    } else {
                        TeachEvent::None
                    }
                }
            }
            20 if matches!(p.pc_state, 10 | 20 | 30 | 40) => {
                if p.ended || p.pc_state != 10 {
                    TeachEvent::None
                } else {
                    p.accepted = true;
                    p.ended = true;
                    self.message = "PLC 已结束采集，等待尾帧和保存".into();
                    TeachEvent::Ended
                }
            }
            30 if matches!(p.pc_state, 20 | 30 | 40 | 0) => {
                if p.pc_state != 0 {
                    p.pc_state = 0;
                    p.reason = 0;
                    self.message = "PLC 已确认，等待双方释放".into();
                    self.since = Instant::now();
                }
                TeachEvent::None
            }
            40 if matches!(p.pc_state, 10 | 30 | 40 | 0) && !p.accepted => {
                if p.pc_state == 0 {
                    TeachEvent::None
                } else {
                    p.pc_state = 0;
                    p.reason = 0;
                    self.message = format!("PLC 拒绝示教，原因码 {}", reply.reason);
                    self.since = Instant::now();
                    TeachEvent::Rejected(self.message.clone())
                }
            }
            _ => return Err("PLC 示教状态迁移异常，禁止继续采集".into()),
        };
        if event == TeachEvent::Accepted || event == TeachEvent::Ended {
            self.since = Instant::now();
        }
        if serde_json::to_string(&self.journal).map_err(|e| e.to_string())? != before {
            self.save()?;
        }
        Ok(event)
    }

    fn points(
        engine: &PlcEngine,
        tags: &[&str; 5],
        db: u16,
        access: Access,
    ) -> Result<[String; 5], String> {
        let config = engine.config();
        let mut ids = Vec::new();
        for (i, tag) in tags.iter().enumerate() {
            let points: Vec<_> = config
                .points
                .iter()
                .filter(|p| p.tags.iter().any(|t| t == tag))
                .collect();
            let p = match points.as_slice() {
                [p] => *p,
                _ => return Err(format!("示教点位 {tag} 缺失或重复，请应用示教 V1 点表")),
            };
            if p.data_type != DataType::I16
                || p.access != access
                || !p
                    .address
                    .eq_ignore_ascii_case(&format!("DB{db}.DBW{}", i * 2))
            {
                return Err(format!(
                    "示教点位 {tag} 必须为 DB{db}.DBW{} / Int / {access:?}",
                    i * 2
                ));
            }
            ids.push(p.id.clone());
        }
        ids.try_into().map_err(|_| "示教点表不完整".into())
    }

    async fn read(engine: &PlcEngine, _ids: &[String; 5]) -> Result<TeachingReply, String> {
        let config = engine.config();
        let points: Vec<_> = config
            .points
            .iter()
            .filter(|p| p.tags.iter().any(|t| t == "teachPlcSnapshot"))
            .collect();
        let point = match points.as_slice() {
            [point] => *point,
            _ => return Err("缺少或重复的示教完整快照点位".into()),
        };
        if point.data_type != DataType::Bytes
            || point.access != Access::Read
            || !point.address.eq_ignore_ascii_case("DB12.DBB0.10")
        {
            return Err("示教快照必须为 DB12.DBB0.10 / Bytes / Read".into());
        }
        let bytes = match engine
            .read_now(&point.id)
            .await
            .map_err(|e| e.to_string())?
        {
            PlcValue::Bytes(bytes) if bytes.len() == 10 => bytes,
            _ => return Err("示教快照必须完整读取 10 字节".into()),
        };
        let mut values = [0u16; 5];
        for (i, word) in bytes.chunks_exact(2).enumerate() {
            values[i] = u16::from_be_bytes([word[0], word[1]]);
            if values[i] > 32767 {
                return Err("示教快照包含负数 Int".into());
            }
        }
        Ok(TeachingReply {
            version: values[0],
            state: values[1],
            id: values[2],
            reason: values[3],
            reserved: values[4],
        })
    }

    async fn publish(&mut self, engine: &PlcEngine) -> Result<(), String> {
        self.monitor.pc = None;
        let ids = Self::points(engine, &PC_TAGS, 6, Access::ReadWrite)?;
        let (id, state, reason) = self
            .journal
            .pending
            .as_ref()
            .map_or((self.journal.high_water, 0, 0), |p| {
                (p.id, p.pc_state, p.reason)
            });
        let values = [1, state, id, reason, 0];
        for i in [0, 2, 3, 4, 1] {
            engine
                .write_point_verified(
                    &ids[i],
                    &json!(values[i]),
                    VerifyOptions {
                        attempts: 1,
                        delay: Duration::ZERO,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        self.monitor.pc = Some(TeachingRegisters {
            version: 1,
            state,
            round_id: id,
            reason,
            reserved: 0,
        });
        Ok(())
    }

    pub async fn commit_request(&mut self, engine: &PlcEngine) -> Result<(), String> {
        if !self.ready
            || self.faulted
            || !self
                .journal
                .pending
                .as_ref()
                .is_some_and(|p| p.pc_state == 10 && !p.accepted)
        {
            return Err("没有已准备的示教请求".into());
        }
        if let Err(error) = self.publish(engine).await {
            self.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub fn prepare_finish(&mut self, success: bool, reason: u16) -> Result<(), String> {
        if !self.ready || self.faulted {
            return Err("示教会话需要复位".into());
        }
        let p = self.journal.pending.as_mut().ok_or("无示教事务")?;
        if p.pc_state != 10 {
            return Ok(());
        }
        if success && !p.ended {
            return Err("未收到 PLC 采集结束，不能报告完成".into());
        }
        if !success && !(3..=7).contains(&reason) {
            return Err("示教失败原因必须为 3–7".into());
        }
        p.pc_state = if success { 20 } else { 30 };
        p.reason = if success { 0 } else { reason };
        self.save()?;
        self.since = Instant::now();
        self.message = "已报告采集结果，等待 PLC 确认".into();
        Ok(())
    }

    pub async fn finish(
        &mut self,
        engine: &PlcEngine,
        success: bool,
        reason: u16,
    ) -> Result<(), String> {
        self.prepare_finish(success, reason)?;
        if let Err(error) = self.publish(engine).await {
            self.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub fn prepare_cancel(&mut self, reason: u16) -> Result<(), String> {
        if !self.ready || self.faulted {
            return Err("示教会话需要复位".into());
        }
        if !(1..=2).contains(&reason) {
            return Err("示教取消原因必须为 1–2".into());
        }
        let p = self.journal.pending.as_mut().ok_or("无示教事务")?;
        if p.pc_state != 10 {
            return Ok(());
        }
        p.pc_state = 40;
        p.reason = reason;
        self.save()?;
        self.since = Instant::now();
        self.message = "正在取消，等待 PLC 停止运动并确认".into();
        Ok(())
    }

    pub async fn cancel(&mut self, engine: &PlcEngine, reason: u16) -> Result<(), String> {
        self.prepare_cancel(reason)?;
        if let Err(error) = self.publish(engine).await {
            self.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub async fn poll(
        &mut self,
        engine: &PlcEngine,
        reset: bool,
        base_ready: bool,
    ) -> Result<TeachEvent, String> {
        let ids = match Self::points(engine, &PLC_TAGS, 12, Access::Read)
            .and_then(|ids| Self::points(engine, &PC_TAGS, 6, Access::ReadWrite).map(|_| ids))
        {
            Ok(ids) => {
                self.configured = true;
                ids
            }
            Err(error) => {
                self.configured = false;
                self.monitor = TeachingMonitor::default();
                self.previous = None;
                self.ready = false;
                self.message = error.clone();
                return if self.busy() {
                    Ok(self.fail(error))
                } else {
                    Ok(TeachEvent::None)
                };
            }
        };
        let status = engine.status();
        if status.state != LinkState::Connected {
            self.previous = None;
            self.connection = None;
            self.monitor = TeachingMonitor::default();
            return Ok(self.fail("PLC 连接中断，示教须核对并复位".into()));
        }
        let key = (status.since, status.dials);
        if self.connection != Some(key) {
            self.connection = Some(key);
            self.previous = None;
            self.monitor = TeachingMonitor::default();
            self.monitor_retry_at = None;
            self.ready = false;
            self.resetting = false;
            self.message = "PLC 示教新连接，等待显式复位".into();
        }
        if reset {
            self.check_origin()?;
            if let Some(error) = &self.load_error {
                return Err(format!("示教记录损坏：{error}"));
            }
            if let Some(p) = self.journal.pending.as_mut() {
                p.pc_state = 0;
                p.reason = 0;
            }
            self.save()?;
            self.ready = false;
            self.resetting = true;
            self.faulted = false;
            if let Err(error) = self.publish(engine).await {
                return Ok(self.fail(error));
            }
            self.since = Instant::now();
        }
        if !self.ready
            && !self.resetting
            && self.monitor_retry_at.is_some_and(|at| Instant::now() < at)
        {
            return Ok(TeachEvent::None);
        }
        let reply = match Self::read(engine, &ids).await {
            Ok(reply) => {
                self.monitor_retry_at = None;
                reply
            }
            Err(error) => {
                self.monitor.fresh = false;
                self.previous = None;
                self.monitor_retry_at = Some(Instant::now() + Duration::from_secs(1));
                if !self.ready && !self.resetting {
                    if !self.faulted {
                        self.message = format!("示教状态读取失败：{error}；请核对连接并显式复位");
                    }
                    return Ok(TeachEvent::None);
                }
                return Ok(self.fail(error));
            }
        };
        let stable = self.previous == Some(reply);
        self.previous = Some(reply);
        if !stable {
            return Ok(TeachEvent::None);
        }
        self.monitor.plc = Some(reply.into());
        let mut reply = reply;
        if let Some(last) = self
            .last_reply
            .filter(|last| last.state == reply.state && (last.id == reply.id || reply.state == 0))
        {
            reply.reason = last.reason;
        }
        self.monitor.violation = reply.check().err();
        self.monitor.fresh = true;
        self.monitor_observed_at = Some(Instant::now());
        if !self.ready && !self.resetting {
            if !self.faulted && self.message.starts_with("示教状态读取失败：") {
                self.message = "示教通讯已恢复，等待显式复位".into();
            }
            return Ok(TeachEvent::None);
        }
        if self.resetting {
            if base_ready && reply.state == 0 && self.monitor.violation.is_none() {
                match self.initialize_after_reset(reply) {
                    Ok(()) => return Ok(TeachEvent::Released),
                    Err(e) => return Ok(self.fail(e)),
                }
            }
            let waiting = match &self.monitor.violation {
                Some(v) => format!("PLC 寄存器不合规：{}", v.detail),
                None if reply.state != 0 => format!("PLC 示教状态 {}（{}）", reply.state, plc_state_name(reply.state)),
                None => "等待基础握手复位完成".into(),
            };
            if self.since.elapsed() > Duration::from_secs(15) {
                return Ok(self.fail(format!("示教复位等待 PLC 空闲超时：{waiting}")));
            }
            self.message = format!("复位等待 PLC 示教回到空闲：{waiting}");
            return Ok(TeachEvent::None);
        }
        if let Err(error) = reply.validate() {
            return Ok(self.fail(error));
        }
        if !self.ready || self.faulted {
            return Ok(TeachEvent::None);
        }
        let before = self.journal.pending.as_ref().map(|p| p.pc_state);
        let event = match self.observe_reply(reply) {
            Ok(event) => event,
            Err(error) => return Ok(self.fail(error)),
        };
        if before != self.journal.pending.as_ref().map(|p| p.pc_state)
            && self.journal.pending.is_some()
        {
            if let Err(error) = self.publish(engine).await {
                return Ok(self.fail(error));
            }
        }
        if let Some(p) = self.journal.pending.as_ref() {
            let elapsed = self.since.elapsed();
            if p.pc_state == 10 && !p.accepted && elapsed > Duration::from_secs(10) {
                self.cancel(engine, 2).await?;
            } else if p.pc_state == 10 && !p.ended && elapsed > Duration::from_secs(130) {
                self.finish(engine, false, 7).await?;
            } else if matches!(p.pc_state, 0 | 20 | 30 | 40) && elapsed > Duration::from_secs(10) {
                return Ok(self.fail("示教等待结果确认或释放超时，请处置并复位".into()));
            }
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gluesight-teaching-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn session(&self) -> Teaching {
            Teaching::open(self.0.join("teaching.json"))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn reply(id: u16, state: u16, reason: u16) -> TeachingReply {
        TeachingReply {
            version: 1,
            id,
            state,
            reason,
            reserved: 0,
        }
    }

    /// 现场 bug：PLC 已接受（状态 10）却没回显轮次号，PC 只报笼统的"组合非法"；协议规定的原因码边界要指到具体字段。
    #[test]
    fn plc_register_violation_names_the_offending_field() {
        for (case, reply, field) in [
            ("已接受未回显轮次", reply(0, 10, 0), Some("roundId")),
            ("回空闲未清原因", reply(5, 0, 3), Some("reason")),
            ("拒绝原因 0", reply(5, 40, 0), Some("reason")),
            ("拒绝原因 5", reply(5, 40, 5), None),
            ("故障原因 3", reply(0, 50, 3), Some("reason")),
            ("故障不回显轮次", reply(0, 50, 5), None),
        ] {
            assert_eq!(reply.check().err().map(|v| v.field), field, "{case}");
        }
    }

    #[test]
    fn plc_end_without_observed_acceptance_begins_drain_and_blocks_another_round() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(4, 0, 0)).unwrap();
        let id = session.prepare_request("capture-a").unwrap();
        assert_eq!(
            session.observe_reply(reply(id, 20, 0)).unwrap(),
            TeachEvent::Ended
        );
        assert!(session.busy());
        assert!(session.prepare_request("capture-b").is_err());
        assert_eq!(
            session.observe_reply(reply(id, 20, 0)).unwrap(),
            TeachEvent::None
        );
        assert!(session.observe_reply(reply(id, 0, 0)).is_err());
    }

    #[test]
    fn rejected_round_requires_matching_release_and_its_number_is_not_reused() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(0, 0, 0)).unwrap();
        let id = session.prepare_request("capture-a").unwrap();
        assert!(matches!(
            session.observe_reply(reply(id, 40, 2)).unwrap(),
            TeachEvent::Rejected(_)
        ));
        assert!(session.busy());
        assert!(session.prepare_request("capture-b").is_err());
        assert!(session.observe_reply(reply(id + 1, 0, 0)).is_err());
        assert_eq!(
            session.observe_reply(reply(id, 0, 0)).unwrap(),
            TeachEvent::Released
        );
        assert_eq!(session.prepare_request("capture-b").unwrap(), id + 1);
    }

    #[test]
    fn restart_preserves_unfinished_round_and_requires_explicit_reset_before_new_number() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(5, 0, 0)).unwrap();
        let id = session.prepare_request("capture-a").unwrap();
        drop(session);
        let mut restored = temp.session();
        assert!(restored.busy());
        assert_eq!(restored.bound_round_id(), Some("capture-a"));
        assert!(restored.prepare_request("capture-b").is_err());
        restored.initialize_after_reset(reply(id, 0, 0)).unwrap();
        assert_eq!(restored.prepare_request("capture-b").unwrap(), id + 1);
    }

    #[test]
    fn cancel_before_acceptance_requires_plc_ack_and_idle_before_next_request() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(0, 0, 0)).unwrap();
        let id = session.prepare_request("capture-a").unwrap();
        session.prepare_cancel(1).unwrap();
        assert_eq!(
            session.observe_reply(reply(id, 30, 0)).unwrap(),
            TeachEvent::None
        );
        assert!(session.busy());
        assert!(session.prepare_request("capture-b").is_err());
        assert_eq!(
            session.observe_reply(reply(id, 0, 0)).unwrap(),
            TeachEvent::Released
        );
        assert_eq!(session.prepare_request("capture-b").unwrap(), id + 1);
    }

    #[test]
    fn payload_reason_written_before_new_state_is_only_consumed_when_state_changes() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(0, 0, 0)).unwrap();
        let id = session.prepare_request("capture-a").unwrap();
        assert_eq!(
            session.observe_reply(reply(id, 10, 0)).unwrap(),
            TeachEvent::Accepted
        );
        assert_eq!(
            session.observe_reply(reply(id, 10, 4)).unwrap(),
            TeachEvent::None
        );
        assert!(session.observe_reply(reply(id, 50, 4)).is_err());
    }

    #[test]
    fn real_pending_round_cannot_be_cleared_by_offline_reset() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(0, 0, 0)).unwrap();
        let id = session.prepare_request("real-capture").unwrap();
        drop(session);
        let mut restored = temp.session();
        restored.set_origin(true);
        assert!(restored.initialize_after_reset(reply(id, 0, 0)).is_err());
        assert_eq!(restored.bound_round_id(), Some("real-capture"));
        assert!(restored.blocks_site_switch());
        restored.set_origin(false);
        restored.initialize_after_reset(reply(id, 0, 0)).unwrap();
        assert!(!restored.busy());
        restored.set_origin(true);
        restored.initialize_after_reset(reply(id, 0, 0)).unwrap();
        let offline_id = restored.prepare_request("offline-capture").unwrap();
        assert!(restored.blocks_site_switch());
        drop(restored);
        let mut restarted = temp.session();
        assert!(restarted.busy());
        assert!(!restarted.blocks_site_switch());
        assert!(restarted
            .initialize_after_reset(reply(offline_id, 0, 0))
            .is_err());
        restarted.set_origin(true);
        restarted
            .initialize_after_reset(reply(offline_id, 0, 0))
            .unwrap();
        assert!(!restarted.busy());
    }

    #[test]
    fn unpublished_idle_payload_does_not_accept_or_fault_request_or_early_cancel() {
        for cancel in [false, true] {
            let temp = Temp::new();
            let mut session = temp.session();
            session.initialize_after_reset(reply(3, 0, 0)).unwrap();
            let id = session.prepare_request("capture").unwrap();
            if cancel {
                session.prepare_cancel(1).unwrap();
            }
            assert_eq!(
                session.observe_reply(reply(id, 0, 2)).unwrap(),
                TeachEvent::None
            );
            assert!(session.busy());
            if cancel {
                assert_eq!(
                    session.observe_reply(reply(id, 30, 0)).unwrap(),
                    TeachEvent::None
                );
            } else {
                assert!(matches!(
                    session.observe_reply(reply(id, 40, 2)).unwrap(),
                    TeachEvent::Rejected(_)
                ));
            }
        }
    }

    #[test]
    fn int_round_number_exhaustion_or_corrupt_journal_never_resets_sequence_silently() {
        let temp = Temp::new();
        let mut session = temp.session();
        session.initialize_after_reset(reply(32767, 0, 0)).unwrap();
        assert!(session.prepare_request("capture-a").is_err());
        std::fs::write(temp.0.join("teaching.json"), "broken").unwrap();
        let mut broken = temp.session();
        assert!(broken.busy());
        assert!(broken.initialize_after_reset(reply(0, 0, 0)).is_err());
        assert!(broken.prepare_request("capture-b").is_err());
    }
}

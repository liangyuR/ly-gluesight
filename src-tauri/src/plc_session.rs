use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ly_plc::{now_ms, LinkState, LogContext, PlcEngine};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::applog::{self, codes, Fields};
use crate::handshake::{self, Contract, HeartbeatWatch, Request, ResultEnvelope, Snapshot, WriteOp};
use crate::inspection::{state, tag};
use crate::plc_plan::PlcPlan;
use crate::settings::RecordMode;
use crate::store::RecordSource;

mod recovery;
pub use recovery::AckRecovery;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionPhase { ResetRequired, Idle, Validating, Acquiring, Draining, AwaitAck, Releasing, Fault }

/// 事务来自哪台 PLC：真机，或离线会话的进程内虚拟 PLC。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin { Real, Offline }

/// 未结事务对切换站点模式的影响：离线来源的事务切换后由新连上的 PLC 复位，真机或来源不明的事务必须先处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingKind { None, Offline, Blocking }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartSite {
    pub source: RecordSource,
    pub record: RecordMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    /// 旧版本日志没有此字段：来源不明按真机事务处理，只能在真机 PLC 上复位（D-0，不推断来源）。
    #[serde(default)]
    origin: Option<Origin>,
    request: Request,
    result: Option<ResultEnvelope>,
    phase: SessionPhase,
    #[serde(default)]
    cycle_id: Option<String>,
    #[serde(default)]
    capture_id: Option<String>,
    #[serde(default)]
    acknowledged: bool,
    #[serde(default)]
    started_at: i64,
    #[serde(default)]
    site: Option<PartSite>,
    /// 接件时锁存的 pcMode（0 正常，1 维护）：布防与回写结果都按它，换模式从下一件生效
    #[serde(default)]
    mode: u16,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    version: u8,
    last_request_seq: u32,
    pending: Option<Pending>,
    last_resolution: Option<String>,
}

fn valid_cycle_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_pending(pending: &Pending, last_request_seq: u32) -> Result<(), String> {
    pending.request.validate()?;
    if pending.capture_id.is_some() && pending.cycle_id.is_some() { return Err("S7事务不能同时绑定生产与示教采集身份".into()); }
    if pending.cycle_id.as_deref().is_some_and(|id| !valid_cycle_id(id)) {
        return Err("S7 握手 cycleId 必须是 32 位十六进制持久身份".into());
    }
    if pending.request.request_seq != last_request_seq
        || matches!(pending.phase, SessionPhase::Idle | SessionPhase::ResetRequired)
        || (matches!(pending.phase, SessionPhase::AwaitAck | SessionPhase::Releasing) && pending.result.is_none())
        || (pending.acknowledged && (pending.result.is_none()
            || !matches!(pending.phase, SessionPhase::AwaitAck | SessionPhase::Releasing | SessionPhase::Fault)))
        || pending.result.as_ref().is_some_and(|result|
            result.request_seq != pending.request.request_seq || result.sn != pending.request.sn
            || !result_allowed(pending.mode, result.result_code, result.fault_code)) {
        return Err("握手事务日志的身份或阶段不一致，禁止自动恢复".into());
    }
    Ok(())
}

/// 协议 7 第七节：维护模式（pcMode=1）结果码固定 0；正常模式不得为 0，除 90 外 faultCode 为 0。
fn result_allowed(mode: u16, result_code: u16, fault_code: u16) -> bool {
    match mode {
        MAINTENANCE => result_code == 0 && fault_code == 0,
        _ => matches!(result_code, 1 | 2 | 11 | 12 | 13 | 14 | 90) && (result_code == 90 || fault_code == 0),
    }
}

/// 握手日志里在途事务的协议版本、序号与 SN；与当前协议一致或没有在途事务时为 None。
fn foreign_request(text: &str) -> Option<(u64, u64, String)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let request = value.get("pending")?.get("request")?;
    let version = request.get("protocolVersion")?.as_u64()?;
    (version != u64::from(handshake::PROTOCOL_VERSION)).then(|| {
        (
            version,
            request
                .get("requestSeq")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            request
                .get("sn")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )
    })
}

/// 握手日志里的在途事务来自别的协议版本（升级前的协议 6）：按 D-0 不迁移、不续做，原文件改名为
/// `<名>.v{n}.bak` 保留证据，会话以空事务启动，照常要求明确复位；不这样做的话旧请求过不了当前协议的校验，复位会一直被挡住。
fn archive_foreign_protocol(
    path: &std::path::Path,
    (version, seq, sn): &(u64, u64, String),
) -> Result<String, String> {
    let backup = path.with_extension(format!("v{version}.bak"));
    std::fs::rename(path, &backup).map_err(|error| format!("握手日志中的在途事务来自协议 {version}，归档为 {} 失败：{error}；保留现场信号并处理日志后重启",
        backup.display()))?;
    Ok(format!("握手日志中的在途事务来自协议 {version}（requestSeq={seq} SN={sn}），不能在协议 {} 下续做，已归档为 {}；核对该件后明确复位",
        handshake::PROTOCOL_VERSION,
        backup.file_name().map_or_else(|| backup.display().to_string(), |name| name.to_string_lossy().into_owned())))
}

/// 布防回写的耗时构成：逐点写入（含等驱动锁）、读回确认（及用了几次轮询）、握手日志落盘（不计入布防预算）。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmTiming {
    pub write_ms: f64,
    pub confirm_ms: f64,
    pub confirm_polls: u64,
    pub journal_ms: Option<f64>,
}

fn millis(duration: Duration) -> f64 { duration.as_secs_f64() * 1000.0 }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub phase: SessionPhase,
    pub request_seq: Option<u32>,
    pub result_seq: Option<u32>,
    pub message: Option<String>,
    /// 会话已核对的 PLC 连接（连接时间戳）
    pub connection: Option<i64>,
    /// 当前复位前置检查的首个阻塞原因；通过检查仍需显式发起复位。
    pub reset_blocked_reason: Option<String>,
}

/// 空闲提示（等待 PLC 释放输入、设备未就绪）的前缀：不是故障，不需要复位，现状恢复后自动消失。
pub const NOTICE: &str = "S7 空闲：";
/// 空闲时视觉输出被外部改写，60 s 内自动重写超过 2 次就判故障（有写入方在持续覆盖 PC 区）。
const RESTORE_WINDOW: Duration = Duration::from_secs(60);
const RESTORE_LIMIT: usize = 2;
/// pcMode：维护模式下 PLC 按未检放行。
const MAINTENANCE: u16 = 1;
/// 软件发起复位（pcState=99）后等待 PLC 回空闲的时限，超时 PC 回未就绪。
const RESET_RESPONSE: Duration = Duration::from_secs(10);
/// PC 主动断开后多久自动重连：PLC 按 3 s 未翻转判 PC 心跳失活，再留一个 PC 心跳周期（500 ms）余量，保证 PLC 先判失活停线。
const RECONNECT_AFTER_STOP: Duration = Duration::from_millis(3500);
const RECONNECT_MANUAL: &str = "自动重连后仍无法确认撤下 pcState，已断开通讯并停止自动重连；检查 PLC 能否写入（写保护、PC 区被覆盖）后手动连接，再明确复位，未结事务保留";

/// PC 为停止心跳主动断开通讯后的去向
#[derive(Clone, Copy)]
enum AfterStop {
    Reconnect(Instant),
    /// 自动重连过一次仍确认不了置故障：不再自动重连，等人工连接
    Manual,
}

#[derive(Default)]
struct Reconnect {
    /// 断开后的连接时间戳与去向；期间被人工重连过或改了配置就作废
    stopped: Option<(i64, AfterStop)>,
    /// 已自动重连，还没在新连接上确认撤销布防或置故障
    spent: bool,
}

#[derive(Debug)]
pub enum SessionEvent {
    None, Ready, Start(Request),
    /// 运动结束（plcState=20，已核对请求身份）
    End(Instant),
    Fault(String),
    /// PLC 已锁存结果，PC 已回就绪
    Released,
    /// 空闲时视觉输出被改写，已重写空闲输出并继续（附原因）
    Restored(String),
}

pub struct PlcSession {
    path: PathBuf,
    journal: Journal,
    load_error: Option<String>,
    /// 启动时归档了旧协议在途事务的提示
    archived: Option<String>,
    phase: SessionPhase,
    contract: Option<Contract>,
    arm_timing: Option<ArmTiming>,
    /// 布防输出已读回确认、尚未落盘采集阶段
    outputs_armed: bool,
    config_key: String,
    connection: Option<i64>,
    /// 当前连接的是真机还是离线虚拟 PLC：新事务按此记录来源
    origin: Origin,
    /// 软件发起复位已写 pcState=99 的时刻，等待 PLC 回空闲
    reset_exchange: Option<Instant>,
    heartbeat: Mutex<HeartbeatWatch>,
    fault_written: bool,
    fault_attempted: Option<Instant>,
    /// 本次进入复位或故障的原因类别，写故障输出时记入运行日志
    fault_cause: &'static str,
    message: Option<String>,
    reset_blocked_reason: Option<String>,
    /// 设备未就绪或暂停接件时本侧把 pcState 置 0（不是外部改写）
    withdrawn: bool,
    /// 空闲时外部改写视觉输出后自动重写的时刻
    restores: Vec<Instant>,
    /// 当前 pcMode，下一件接件时锁存进事务
    mode: u16,
    /// 最近一次写出空闲输出时的 pcMode：切换维护模式后的重写不算外部改写
    mode_written: u16,
    /// 本侧为停止 PC 心跳主动断开通讯：到点后自动重连一次，重连后仍确认不了置故障就停在断开等人工处理
    reconnect: Mutex<Reconnect>,
}

impl PlcSession {
    pub fn open(path: PathBuf) -> Self {
        let unfinished = path.with_extension("pending.tmp").exists();
        let mut archived = None;
        let loaded = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let text = text.trim_start_matches('\u{feff}');
                match foreign_request(text).filter(|_| !unfinished) {
                    Some(foreign) => archive_foreign_protocol(&path, &foreign).map(|notice| {
                        archived = Some(notice);
                        Journal {
                            version: 1,
                            ..Journal::default()
                        }
                    }),
                    None => serde_json::from_str::<Journal>(text)
                        .map_err(|e| format!("S7 握手日志损坏，保留现场信号并修复日志后重启：{e}")),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !unfinished => {
                Ok(Journal {
                    version: 1,
                    ..Journal::default()
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err("发现未提交的握手事务文件，保留现场信号并核对日志后重启".into())
            }
            Err(error) => Err(format!("S7 握手日志无法读取：{error}")),
        };
        let (journal, mut load_error) = match loaded {
            Ok(journal) if journal.version == 1 => {
                let error = journal.pending.as_ref().and_then(|pending| validate_pending(pending, journal.last_request_seq).err());
                (journal, error)
            }
            Ok(journal) => (journal, Some("不支持的 S7 握手日志版本".into())),
            Err(error) => (Journal::default(), Some(error)),
        };
        if unfinished {
            load_error = Some("发现未提交的握手事务文件，保留现场信号并核对日志后重启".into());
        }
        let message = archived
            .clone()
            .unwrap_or_else(|| "S7 启动后需复位握手".into());
        Self {
            path,
            journal,
            load_error,
            archived,
            phase: SessionPhase::ResetRequired,
            contract: None,
            arm_timing: None,
            outputs_armed: false,
            config_key: String::new(),
            connection: None,
            reset_exchange: None,
            heartbeat: Mutex::new(HeartbeatWatch::new(3000).expect("valid heartbeat timeout")),
            origin: Origin::Real,
            fault_written: false,
            fault_attempted: None,
            fault_cause: "startup",
            message: Some(message),
            reset_blocked_reason: Some("正在核对复位条件".into()),
            withdrawn: false,
            restores: Vec::new(),
            mode: 0,
            mode_written: 0,
            reconnect: Mutex::default(),
        }
    }

    /// 启动时把旧协议的在途事务归档（见 [`archive_foreign_protocol`]）后的提示；没有归档为空。
    pub fn archived(&self) -> Option<&str> { self.archived.as_deref() }

    /// 维护模式开关：从下一件起按 pcMode=1 布防并回写结果码 0；空闲时随即重写 pcMode。
    pub fn set_maintenance(&mut self, on: bool) {
        self.mode = if on { MAINTENANCE } else { 0 };
    }

    /// 在途事务是否按维护模式接件（结果码须为 0）。
    pub fn part_maintenance(&self) -> bool {
        self.journal.pending.as_ref().is_some_and(|pending| pending.mode == MAINTENANCE)
    }

    /// 空闲时的等待提示（以 [`NOTICE`] 开头）；不是故障。
    pub fn notice(&self) -> Option<&str> {
        self.message.as_deref().filter(|message| self.phase == SessionPhase::Idle && message.starts_with(NOTICE))
    }

    pub fn pending(&self) -> bool { self.journal.pending.is_some() || self.load_error.is_some() }

    /// 之后开始的事务记为该来源；由节拍按当前站点模式设置。
    pub fn set_origin(&mut self, origin: Origin) { self.origin = origin; }

    pub fn pending_kind(&self) -> PendingKind {
        match (&self.journal.pending, &self.load_error) {
            (None, None) => PendingKind::None,
            (Some(pending), None) if pending.origin == Some(Origin::Offline) => PendingKind::Offline,
            _ => PendingKind::Blocking,
        }
    }

    /// 未结事务的身份与来源，用于提示处置路径。
    pub fn pending_summary(&self) -> Option<String> {
        if let Some(error) = &self.load_error { return Some(format!("握手日志不可用（{error}）")); }
        let pending = self.journal.pending.as_ref()?;
        let origin = match pending.origin { Some(Origin::Offline) => "离线", Some(Origin::Real) => "真机", None => "来源未记录" };
        Some(format!("{origin}事务 requestSeq={} SN={}", pending.request.request_seq, pending.request.sn))
    }

    pub fn acknowledged(&self) -> bool { self.journal.pending.as_ref().is_some_and(|pending| pending.acknowledged) }

    pub fn bind_cycle_id(&mut self, id: &str, site: PartSite) -> Result<(), String> {
        if !valid_cycle_id(id) { return Err("S7 握手 cycleId 必须是 32 位十六进制持久身份".into()); }
        if let Some(error) = &self.load_error { return Err(error.clone()); }
        let pending = self.journal.pending.as_mut().ok_or("缺少可关联的 S7 请求事务")?;
        if pending.capture_id.is_some() { return Err("S7事务已绑定示教采集，不能用于生产".into()); }
        match pending.cycle_id.as_deref() {
            Some(bound) if bound != id => Err("S7 事务已绑定其他 cycleId，禁止更换工件身份".into()),
            Some(_) => Ok(()),
            None => { pending.cycle_id = Some(id.into()); pending.site = Some(site); Ok(()) }
        }
    }

    pub fn bind_capture_id(&mut self, id: &str) -> Result<(), String> {
        let pending = self.journal.pending.as_mut().ok_or("缺少采集 S7 请求事务")?;
        if pending.cycle_id.is_some() || pending.capture_id.as_deref().is_some_and(|bound| bound != id) {
            return Err("S7 事务已绑定其他生产或采集身份".into());
        }
        pending.capture_id = Some(id.to_owned());
        Ok(())
    }

    pub fn capture_id(&self) -> Option<&str> { self.journal.pending.as_ref().and_then(|p| p.capture_id.as_deref()) }

    pub fn cycle_id(&self) -> Option<&str> { self.journal.pending.as_ref().and_then(|pending| pending.cycle_id.as_deref()) }

    pub fn site(&self) -> Option<PartSite> { self.journal.pending.as_ref().and_then(|pending| pending.site) }

    pub fn recover_acknowledgements(&self, unresolved: &std::collections::BTreeSet<String>) -> AckRecovery { recovery::read(self, unresolved) }

    pub fn request(&self) -> Option<&Request> { self.journal.pending.as_ref().map(|p| &p.request) }

    pub fn result(&self) -> Option<&ResultEnvelope> { self.journal.pending.as_ref().and_then(|p| p.result.as_ref()) }

    pub fn started_at(&self) -> Option<i64> { self.journal.pending.as_ref().map(|p| p.started_at).filter(|ts| *ts > 0) }

    pub fn phase(&self) -> SessionPhase { self.phase }

    #[cfg(test)]
    pub(crate) fn heartbeat_live(&self) -> bool { self.heartbeat.lock().unwrap().is_live(now_ms()) }

    pub fn view(&self) -> SessionView {
        SessionView { phase: self.phase, request_seq: self.request().map(|r| r.request_seq),
            result_seq: self.result().map(|r| r.request_seq), message: self.message.clone(), connection: self.connection,
            reset_blocked_reason: self.reset_blocked_reason.clone() }
    }

    async fn blocking_io<T: Send + 'static>(&self, engine: &PlcEngine,
        operation: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        self.blocking_io_with_monitor(engine, true, operation).await
    }

    async fn blocking_io_with_monitor<T: Send + 'static>(&self, engine: &PlcEngine, monitor: bool,
        operation: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        let mut task = tokio::task::spawn_blocking(operation);
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut observation_error = None;
        loop {
            tokio::select! {
                result = &mut task => {
                    let value = result.map_err(|error| format!("握手日志保存任务失败：{error}"))??;
                    return observation_error.map_or(Ok(value), Err);
                }
                _ = ticker.tick(), if monitor && observation_error.is_none() => {
                    let observe = || {
                        let contract = self.contract.as_ref().ok_or("S7 点表未校验")?;
                        let snapshot = Snapshot::capture(engine, contract, Self::max_age(engine))?;
                        if Some(snapshot.connection_since) != self.connection { return Err("握手日志保存期间 PLC 已重连，需明确复位".into()); }
                        self.heartbeat.lock().unwrap().observe(snapshot.plc_heartbeat()?, snapshot.heartbeat_changed_at, now_ms())
                    };
                    observation_error = observe().err();
                    if let Some(error) = &observation_error {
                        let result = self.stop_outputs(engine).await;
                        log_fault_output("observation_failed", Some(error), self.phase, &result);
                    }
                }
            }
        }
    }

    async fn persist(&self, engine: &PlcEngine) -> Result<(), String> {
        self.persist_journal(engine, &self.journal).await
    }

    async fn persist_journal(&self, engine: &PlcEngine, journal: &Journal) -> Result<(), String> {
        if let Some(error) = &self.load_error { return Err(error.clone()); }
        let path = self.path.clone();
        let text = serde_json::to_vec_pretty(journal).map_err(|e| e.to_string())?;
        let monitor = !(self.phase == SessionPhase::Fault && journal.pending.is_some());
        self.blocking_io_with_monitor(engine, monitor, move || {
            if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
            let temporary = path.with_extension("pending.tmp");
            let mut file = std::fs::File::create(&temporary).map_err(|e| format!("无法保存握手事务：{e}"))?;
            file.write_all(&text).and_then(|_| file.sync_all()).map_err(|e| format!("握手事务写入失败：{e}"))?;
            drop(file);
            std::fs::rename(&temporary, &path).map_err(|e| format!("握手事务提交失败：{e}"))
        }).await
    }

    async fn audit(&self, engine: &PlcEngine, action: &str, reason: &str) -> Result<(), String> {
        if let Some(error) = &self.load_error { return Err(error.clone()); }
        let path = self.path.with_extension("audit.jsonl");
        let mut record = serde_json::to_vec(&json!({
            "version": 1, "ts": now_ms(), "action": action, "reason": reason,
            "connectionSince": self.connection, "lastRequestSeq": self.journal.last_request_seq,
            "pending": self.journal.pending,
        })).map_err(|e| e.to_string())?;
        record.push(b'\n');
        self.blocking_io(engine, move || crate::handshake_audit::append(&path, &record)).await
    }

    async fn set_pending_phase(&mut self, engine: &PlcEngine, phase: SessionPhase) -> Result<(), String> {
        let mut next = self.journal.clone();
        next.pending.as_mut().ok_or("缺少未结 S7 事务")?.phase = phase;
        self.persist_journal(engine, &next).await?;
        self.journal = next;
        self.phase = phase;
        Ok(())
    }

    fn timeout(engine: &PlcEngine) -> Duration {
        let config = engine.config().connection;
        Duration::from_millis((config.timeout_ms.saturating_mul(3) + config.poll_interval_ms.saturating_mul(6)).max(1500))
    }

    fn max_age(engine: &PlcEngine) -> u64 {
        let config = engine.config().connection;
        (config.timeout_ms.saturating_mul(2) + config.poll_interval_ms.saturating_mul(5)).max(1000)
    }

    fn snapshot(&self, engine: &PlcEngine) -> Result<Snapshot, String> {
        Snapshot::capture(
            engine,
            self.contract.as_ref().ok_or("S7 握手点表尚未校验")?,
            Self::max_age(engine),
        )
    }

    async fn write(&self, engine: &PlcEngine, operations: Vec<WriteOp>) -> Result<(), String> {
        self.write_timed(engine, operations).await.map(|_| ())
    }

    async fn write_timed(&self, engine: &PlcEngine, operations: Vec<WriteOp>) -> Result<handshake::WriteTiming, String> {
        let contract = self.contract.as_ref().ok_or("S7 握手点表尚未校验")?;
        if self.connection != Some(engine.status().since) {
            return Err("PLC 连接已更换，禁止提交握手输出，需明确复位".into());
        }
        handshake::write_confirmed(engine, contract, &operations, Self::timeout(engine), Self::max_age(engine)).await
    }

    pub async fn fault(&mut self, engine: &PlcEngine, reason: String) {
        self.reset_exchange = None;
        self.phase = SessionPhase::Fault;
        self.fault_cause = "fault";
        self.reset_blocked_reason = Some(reason.clone());
        self.message = Some(reason);
        let _ = self.ensure_fault_output(engine).await;
        if self.journal.pending.as_ref().is_some_and(|pending| pending.phase != SessionPhase::Fault) {
            if let Some(pending) = self.journal.pending.as_mut() { pending.phase = SessionPhase::Fault; }
            if let Err(error) = self.persist(engine).await { self.message = Some(error); }
        }
    }

    async fn ensure_fault_output(&mut self, engine: &PlcEngine) -> Result<(), String> {
        if self.fault_written || self.fault_attempted.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            return Ok(());
        }
        if engine.status().state != LinkState::Connected || self.contract.is_none() { return Ok(()); }
        let result = self.stop_outputs(engine).await;
        log_fault_output(self.fault_cause, self.message.as_ref(), self.phase, &result);
        self.fault_attempted = Some(Instant::now());
        self.fault_written = result.is_ok();
        result
    }

    async fn stop_outputs(&self, engine: &PlcEngine) -> Result<(), String> {
        let mut errors = Vec::new();
        let mut stop_confirmed = false;
        // 复位前只撤到未就绪（0），其余情形置故障（90）；两者都让 PLC 拉低 Line0
        let plan = if self.phase == SessionPhase::ResetRequired {
            handshake::not_ready_plan()
        } else {
            handshake::fault_plan(handshake::SESSION_FAULT_CODE)
        };
        for operation in plan {
            let tag = operation.tag;
            let result = match self.contract.as_ref() {
                Some(contract) => handshake::write_confirmed(engine, contract, &[operation], Self::timeout(engine), Self::max_age(engine)).await.map(|_| ()),
                None => Err("S7 握手点表尚未校验".into()),
            };
            match result {
                Ok(()) => {
                    if tag == tag::PC_STATE {
                        stop_confirmed = true;
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        if stop_confirmed {
            self.reconnect.lock().unwrap().spent = false;
        } else {
            let error = errors.join("；");
            applog::emit_event(
                &codes::S7_RECONNECT_FORCED,
                &error,
                Fields::default().with_data(json!({"error": error})),
            );
            engine.disconnect().await;
            let since = engine.status().since;
            let mut reconnect = self.reconnect.lock().unwrap();
            if reconnect.spent {
                reconnect.stopped = Some((since, AfterStop::Manual));
                errors.push(RECONNECT_MANUAL.into());
            } else {
                reconnect.stopped = Some((since, AfterStop::Reconnect(Instant::now() + RECONNECT_AFTER_STOP)));
                errors.push(format!("无法确认撤下 pcState，已断开通讯以停止 PC 心跳；{:.1} s 后自动重连，重连后需明确复位", RECONNECT_AFTER_STOP.as_secs_f32()));
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("；")) }
    }

    #[cfg(test)]
    pub async fn poll(&mut self, engine: &PlcEngine, reset_requested: bool, devices_ready: bool) -> SessionEvent {
        self.poll_with_audit(engine, reset_requested, devices_ready, true).await
    }

    #[cfg(test)]
    pub async fn poll_with_audit(&mut self, engine: &PlcEngine, reset_requested: bool, devices_ready: bool, audit_ready: bool) -> SessionEvent {
        self.poll_gated(engine, reset_requested, devices_ready, audit_ready, true).await
    }

    /// `intake_ready` 只决定空闲时是否接下一件（如资料盘空间不足）：在途件照常释放、复位照常完成，之后空闲时把 pcState 撤到 0。
    #[cfg(test)]
    pub async fn poll_gated(&mut self, engine: &PlcEngine, reset_requested: bool, devices_ready: bool, audit_ready: bool, intake_ready: bool) -> SessionEvent {
        let devices = if devices_ready { Ok(()) } else { Err("检测设备未就绪".to_string()) };
        self.poll_with_devices(engine, reset_requested, &devices, audit_ready, intake_ready).await
    }

    /// 设备未就绪时带上具体原因：复位被拒时提示操作员缺什么。
    pub async fn poll_with_devices(&mut self, engine: &PlcEngine, reset_requested: bool, devices: &Result<(), String>, audit_ready: bool, intake_ready: bool) -> SessionEvent {
        let event = match self.poll_inner(engine, reset_requested, devices, audit_ready, intake_ready).await {
            Ok(event) => event,
            Err(error) => {
                let changed = self.phase != SessionPhase::Fault || self.message.as_ref() != Some(&error);
                self.fault(engine, error.clone()).await;
                if changed { SessionEvent::Fault(error) } else { SessionEvent::None }
            }
        };
        self.reset_blocked_reason = self.check_reset_ready(engine, devices, audit_ready).err();
        event
    }

    /// 只读检查：与实际复位共用校验，不改输出、不清事务。
    fn check_reset_ready(
        &self,
        engine: &PlcEngine,
        devices: &Result<(), String>,
        audit_ready: bool,
    ) -> Result<(), String> {
        if !matches!(
            self.phase,
            SessionPhase::ResetRequired | SessionPhase::Fault | SessionPhase::Idle
        ) {
            return Err("在途工件不能直接复位，请先处理当前工件".into());
        }
        if self.reset_exchange.is_some() {
            return Err("已发起软件复位，等待 PLC 回空闲".into());
        }
        if engine.status().state != LinkState::Connected {
            return Err("S7 PLC 未连接，请先连接 PLC".into());
        }
        if self.connection != Some(engine.status().since) {
            return Err("正在核对新的 PLC 连接会话".into());
        }
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        let contract = self.contract.as_ref().ok_or("S7 握手点表尚未校验")?;
        let snapshot = Snapshot::capture_for_reset(engine, contract, Self::max_age(engine))?;
        if self.phase == SessionPhase::Idle && snapshot.plc_state()? != state::PLC_FAULT {
            return Err("会话空闲，无需复位".into());
        }
        if self.phase != SessionPhase::Idle && !self.fault_written {
            return Err(self
                .message
                .clone()
                .unwrap_or_else(|| "等待故障输出写入并读回确认".into()));
        }
        self.reset_devices_ready(devices)?;
        if !audit_ready { return Err("审计与录制尚未完成持久化，请等待追溯就绪".into()); }
        self.reset_allowed(&snapshot)
    }

    fn reset_devices_ready(&self, devices: &Result<(), String>) -> Result<(), String> {
        if let Err(reason) = devices {
            return Err(format!("设备尚未就绪，不能复位并置视觉就绪：{reason}"));
        }
        if !self.heartbeat.lock().unwrap().is_live(now_ms()) {
            return Err("尚未观察到 PLC 心跳变化，不能复位".into());
        }
        Ok(())
    }

    fn reset_allowed(&self, snapshot: &Snapshot) -> Result<(), String> {
        if self.origin == Origin::Offline && self.journal.pending.as_ref().is_some_and(|pending| pending.origin != Some(Origin::Offline)) {
            return Err(format!("{}不能由离线虚拟 PLC 复位：退出离线，连接真机 PLC 后复位", self.pending_summary().unwrap_or_default()));
        }
        let version = snapshot.read_u16(tag::PLC_VERSION)?;
        if version != handshake::PROTOCOL_VERSION {
            return Err(format!(
                "PLC 协议版本 {version} 与 PC {} 不一致，不能复位",
                handshake::PROTOCOL_VERSION
            ));
        }
        Ok(())
    }

    async fn poll_inner(&mut self, engine: &PlcEngine, reset_requested: bool, devices: &Result<(), String>, audit_ready: bool, intake_ready: bool) -> Result<SessionEvent, String> {
        let devices_ready = devices.is_ok();
        let config = engine.config();
        let key = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        if key != self.config_key {
            let contract = Contract::validate(&config)?;
            // plc.db 每条日志带上当时的 requestSeq，按件追溯握手信号变化
            engine.set_log_context(Some(LogContext { point_id: contract.point_id(tag::REQUEST_SEQ)?.to_owned(), label: tag::REQUEST_SEQ.into() }));
            self.contract = Some(contract);
            self.config_key = key;
            self.connection = None;
            *self.reconnect.lock().unwrap() = Reconnect::default();
        }
        let status = engine.status();
        if status.state != LinkState::Connected {
            self.connection = None;
            let stopped = self.reconnect.lock().unwrap().stopped;
            match stopped {
                Some((since, _)) if status.state != LinkState::Disconnected || status.since != since => {
                    self.reconnect.lock().unwrap().stopped = None;
                }
                Some((_, AfterStop::Reconnect(at))) if Instant::now() >= at => {
                    *self.reconnect.lock().unwrap() = Reconnect { stopped: None, spent: true };
                    engine.connect().await;
                    return Err("PC 主动断开超过 PLC 心跳失活阈值，已自动重连；连上后需明确复位，未结事务保留".into());
                }
                Some((_, AfterStop::Reconnect(_))) => {
                    return Err(format!("已断开通讯以停止 PC 心跳，{:.1} s 后自动重连；重连后需明确复位，未结事务保留", RECONNECT_AFTER_STOP.as_secs_f32()));
                }
                Some((_, AfterStop::Manual)) => return Err(RECONNECT_MANUAL.into()),
                None => {}
            }
            return Err("S7 PLC 未连接；保留未结事务，重连后需复位".into());
        }
        if self.connection != Some(status.since) {
            self.connection = Some(status.since);
            self.phase = SessionPhase::ResetRequired;
            self.reset_exchange = None;
            self.heartbeat.lock().unwrap().reset();
            self.fault_written = false;
            self.fault_attempted = None;
            self.fault_cause = "new_connection";
            self.message = Some("S7 连接会话已更新，需复位握手；未结事务保留".into());
        }
        if status.poll_count == 0 || status.last_poll.is_none_or(|polled| polled < status.since) {
            return Ok(SessionEvent::None);
        }
        let contract = self.contract.as_ref().unwrap();
        let snapshot = if matches!(self.phase, SessionPhase::ResetRequired | SessionPhase::Fault) {
            Snapshot::capture_for_reset(engine, contract, Self::max_age(engine))?
        } else {
            Snapshot::capture(engine, contract, Self::max_age(engine))?
        };
        let now = now_ms();
        let heartbeat = snapshot.plc_heartbeat()?;
        let observation = self.heartbeat.lock().unwrap().observe(heartbeat, snapshot.heartbeat_changed_at, now);
        if let Err(error) = observation {
            let mut watch = self.heartbeat.lock().unwrap();
            watch.reset();
            watch.observe(heartbeat, snapshot.heartbeat_changed_at, now)?;
            return Err(error);
        }
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        if reset_requested {
            let data = json!({"source": "operator", "phase": format!("{:?}", self.phase)});
            applog::emit_event(
                &codes::S7_RESET_REQUEST,
                "收到复位（operator）",
                Fields::default().with_data(data),
            );
            match self.phase {
                SessionPhase::Idle => {
                    self.phase = SessionPhase::ResetRequired;
                    self.fault_written = false;
                    self.fault_attempted = None;
                    self.fault_cause = "reset_operator";
                }
                SessionPhase::ResetRequired | SessionPhase::Fault => {}
                _ => {
                    return Err(
                        "在途工件不能直接复位；保留事务进入故障，处理该件后再次明确复位".into(),
                    )
                }
            }
        }
        if matches!(
            self.phase,
            SessionPhase::ResetRequired | SessionPhase::Fault
        ) {
            return self
                .reset_step(engine, &snapshot, reset_requested, devices, audit_ready)
                .await;
        }
        if !self.heartbeat.lock().unwrap().is_live(now) { return Err("PLC 心跳活性尚未确认".into()); }
        // 空闲时设备未就绪只把 pcState 撤到 0（见 idle）；已提交的请求照常接下，由节拍按设备丢失判 ERR
        if !devices_ready && !matches!(self.phase, SessionPhase::Idle | SessionPhase::AwaitAck | SessionPhase::Releasing) {
            return Err("检测设备未就绪".into());
        }
        let plc = snapshot.plc_state()?;
        match self.phase {
            SessionPhase::Idle
                if plc == state::PLC_REQUEST && snapshot.pc_state()? == state::PC_READY =>
            {
                let request = handshake::stable_start(
                    engine,
                    self.contract.as_ref().unwrap(),
                    Self::timeout(engine),
                    Self::max_age(engine),
                )
                .await?;
                if request.request_seq <= self.journal.last_request_seq {
                    return Err("请求事务序号必须递增；PLC 清零或回绕需先执行复位".into());
                }
                self.journal.pending = Some(Pending { origin: Some(self.origin), capture_id: None, request: request.clone(), result: None, phase: SessionPhase::Validating,
                    cycle_id: None, acknowledged: false, started_at: now_ms(), site: None, mode: self.mode });
                self.journal.last_request_seq = request.request_seq;
                self.persist(engine).await?;
                self.phase = SessionPhase::Validating;
                return Ok(SessionEvent::Start(request));
            }
            SessionPhase::Idle => {
                return self
                    .idle(
                        engine,
                        &snapshot,
                        devices_ready && audit_ready && intake_ready,
                        devices_ready,
                    )
                    .await
            }
            SessionPhase::Validating
            | SessionPhase::Acquiring
            | SessionPhase::Draining
            | SessionPhase::AwaitAck => {
                let request = self.request().ok_or("当前 S7 事务丢失")?.clone();
                if plc == state::PLC_FAULT {
                    return Err(format!(
                        "PLC 在途中进入故障（{}），本件中断",
                        snapshot.plc_reason()?
                    ));
                }
                if snapshot.request()? != request {
                    return Err("在途工件的请求被修改，禁止继续或切换下一件".into());
                }
                let pc = snapshot.pc_state()?;
                let legal = match self.phase {
                    SessionPhase::Validating => plc == state::PLC_REQUEST && pc == state::PC_READY,
                    SessionPhase::Acquiring => {
                        matches!(plc, state::PLC_REQUEST | state::PLC_MOTION_END)
                            && pc == state::PC_ARMED
                    }
                    SessionPhase::Draining => plc == state::PLC_MOTION_END && pc == state::PC_ARMED,
                    _ => true,
                };
                if !legal {
                    return Err(format!(
                        "在途握手状态非法：plcState={plc} pcState={pc}（{:?}）",
                        self.phase
                    ));
                }
                if matches!(self.phase, SessionPhase::Acquiring | SessionPhase::Draining)
                    && snapshot.read_u32(tag::SEQ_ECHO)? != request.request_seq
                {
                    return Err("已布防事务的 seqEcho 被修改".into());
                }
                if self.phase == SessionPhase::Acquiring && plc == state::PLC_MOTION_END {
                    handshake::verify_part_end(&snapshot, &request)?;
                    let ended_at = Instant::now();
                    self.set_pending_phase(engine, SessionPhase::Draining).await?;
                    return Ok(SessionEvent::End(ended_at));
                }
                if self.phase == SessionPhase::AwaitAck {
                    let result = self.result().ok_or("S7 结果尚未生成")?;
                    if handshake::matching_ack(&snapshot, result)? {
                        let data = json!({"requestSeq": result.request_seq, "resultCode": result.result_code, "faultCode": result.fault_code});
                        applog::emit_event(
                            &codes::S7_ACK,
                            &format!("PLC 确认结果，请求序号 {}", result.request_seq),
                            Fields::default().with_data(data),
                        );
                        let mut next = self.journal.clone();
                        let pending = next.pending.as_mut().unwrap();
                        pending.acknowledged = true;
                        pending.phase = SessionPhase::Releasing;
                        self.persist_journal(engine, &next).await?;
                        self.journal = next;
                        self.phase = SessionPhase::Releasing;
                    }
                }
            }
            SessionPhase::Releasing => {
                let result = self.result().ok_or("S7 结果丢失")?;
                if !handshake::matching_ack(&snapshot, result)? {
                    return Err("PLC 确认结果后撤回了确认".into());
                }
                if !audit_ready {
                    return Ok(SessionEvent::None);
                }
                self.audit(engine, "acknowledged", "结果身份已确认，PLC 已锁存结果")
                    .await?;
                let mut next = self.journal.clone();
                next.pending = None;
                next.last_resolution = Some("结果已匹配确认并释放；原事务已保存至审计日志".into());
                self.persist_journal(engine, &next).await?;
                self.journal = next;
                // PLC 在 30 只等 pcState=10；设备未就绪或暂停接件时由空闲检查在 PLC 回 0 后撤到 0
                self.write_idle(engine).await?;
                self.phase = SessionPhase::Idle;
                return Ok(SessionEvent::Released);
            }
            _ => {}
        }
        Ok(SessionEvent::None)
    }

    /// 复位：PLC 置 99（现场复位）或 PLC 已空闲时直接完成；否则 PC 置 99 请 PLC 清未结事务，等 PLC 回 0 再完成。
    async fn reset_step(
        &mut self,
        engine: &PlcEngine,
        snapshot: &Snapshot,
        reset_requested: bool,
        devices: &Result<(), String>,
        audit_ready: bool,
    ) -> Result<SessionEvent, String> {
        if self.reset_exchange.is_none() {
            self.ensure_fault_output(engine).await?;
        }
        let plc = snapshot.plc_state()?;
        let plc_reset = plc == state::PLC_RESET;
        if !(reset_requested || plc_reset || self.reset_exchange.is_some()) {
            return Ok(SessionEvent::None);
        }
        if let Err(error) = self.reset_devices_ready(devices) {
            if reset_requested || self.reset_exchange.is_some() {
                return Err(error);
            }
            self.message = Some(format!("PLC 请求复位，{error}"));
            return Ok(SessionEvent::None);
        }
        if !audit_ready {
            return Ok(SessionEvent::None);
        }
        self.reset_allowed(snapshot)?;
        if let Some(started) = self.reset_exchange {
            if plc != state::PLC_IDLE {
                if started.elapsed() <= RESET_RESPONSE {
                    return Ok(SessionEvent::None);
                }
                self.reset_exchange = None;
                self.phase = SessionPhase::ResetRequired;
                self.fault_written = false;
                self.fault_attempted = None;
                self.fault_cause = "reset_timeout";
                self.message = Some(format!(
                    "PLC {} s 内未响应软件复位（plcState={plc}），已回未就绪，可重新发起复位",
                    RESET_RESPONSE.as_secs()
                ));
                self.ensure_fault_output(engine).await?;
                return Ok(SessionEvent::None);
            }
        } else if !matches!(plc, state::PLC_IDLE | state::PLC_RESET) {
            self.write(engine, handshake::reset_request_plan()).await?;
            self.reset_exchange = Some(Instant::now());
            self.message =
                Some("已发起软件复位（pcState=99），等待 PLC 清除未结事务并回空闲".into());
            return Ok(SessionEvent::None);
        }
        if plc_reset && !reset_requested {
            let data = json!({"source": "plc", "phase": format!("{:?}", self.phase)});
            applog::emit_event(
                &codes::S7_RESET_REQUEST,
                "收到复位（plc）",
                Fields::default().with_data(data),
            );
        }
        self.complete_reset(
            engine,
            snapshot,
            if plc_reset {
                "PLC 现场复位"
            } else {
                "软件复位"
            },
        )
        .await
    }

    async fn complete_reset(
        &mut self,
        engine: &PlcEngine,
        snapshot: &Snapshot,
        source: &str,
    ) -> Result<SessionEvent, String> {
        let baseline = handshake::ResetBaseline::capture(snapshot)?;
        let repair = snapshot.sn_repair_plan();
        let reason = if repair.is_empty() {
            format!("明确复位（{source}）；尚未清除未结事务")
        } else {
            let tags = repair
                .iter()
                .map(|op| op.tag)
                .collect::<Vec<_>>()
                .join("、");
            self.write(engine, repair).await?;
            baseline.verify(&self.snapshot(engine)?)?;
            format!("明确复位（{source}）；已把头字节异常的 {tags} 重写为空串；尚未清除未结事务")
        };
        self.audit(engine, "resetRequested", &reason).await?;
        self.write(engine, handshake::reset_clear_plan()).await?;
        let after = self.snapshot(engine)?;
        baseline.verify(&after)?;
        self.audit(
            engine,
            "resetCompleted",
            "明确复位已清除结果输出并确认 PLC 空闲，原事务保留在本审计记录",
        )
        .await?;
        let mut next = self.journal.clone();
        next.last_request_seq = after.read_u32(tag::REQUEST_SEQ)?;
        next.last_resolution = Some("明确复位结束；原请求和结果已保存至审计日志".into());
        next.pending = None;
        self.persist_journal(engine, &next).await?;
        self.journal = next;
        baseline.verify(&self.snapshot(engine)?)?;
        self.write_idle(engine).await?;
        self.phase = SessionPhase::Idle;
        self.reset_exchange = None;
        self.message = None;
        self.fault_written = false;
        self.fault_attempted = None;
        self.withdrawn = false;
        self.restores.clear();
        Ok(SessionEvent::Ready)
    }

    fn withdrawn_notice() -> String {
        format!("{NOTICE}检测设备未就绪，已置 pcState=0 暂停接件；设备恢复后自动就绪，无需复位")
    }

    /// 空闲（无在途事务）时核对握手状态，能恢复的不判故障：
    /// - PLC 不在空闲（未检放行中、尚未回 0、故障）：只提示等待，不写输出；
    /// - 设备未就绪或暂停接件：pcState 撤到 0，恢复后重新置 10；
    /// - 视觉输出偏离空闲状态（PLC 重启或 DB 重新初始化读回 0、调试表误写）：记审计后重写空闲输出，
    ///   60 s 内反复被改写才判故障。
    async fn idle(
        &mut self,
        engine: &PlcEngine,
        snapshot: &Snapshot,
        accepting: bool,
        devices_ready: bool,
    ) -> Result<SessionEvent, String> {
        if self.journal.pending.is_some() {
            return Err("空闲阶段仍留有未结 S7 事务，状态矛盾，需明确复位".into());
        }
        let plc = snapshot.plc_state()?;
        let waiting = match plc {
            state::PLC_IDLE | state::PLC_RESET => None,
            state::PLC_REQUEST => {
                Some("PLC 在视觉未就绪时提交了请求，等待 PLC 按未检放行后回空闲".to_owned())
            }
            state::PLC_MOTION_END | state::PLC_CONFIRMED => {
                Some(format!("PLC 仍处于 plcState={plc}，等待 PLC 回空闲"))
            }
            state::PLC_FAULT => Some(format!(
                "PLC 故障（{}），在 PLC 现场复位或在软件发起复位",
                snapshot.plc_reason()?
            )),
            other => return Err(format!("PLC 状态 plcState={other} 未定义")),
        };
        if let Some(text) = waiting {
            self.message = Some(format!("{NOTICE}{text}"));
            return Ok(SessionEvent::None);
        }
        if !accepting {
            if !self.withdrawn || snapshot.pc_state()? != state::PC_NOT_READY {
                self.write(engine, handshake::not_ready_plan()).await?;
                self.withdrawn = true;
            }
            self.message = if devices_ready {
                None
            } else {
                Some(Self::withdrawn_notice())
            };
            return Ok(SessionEvent::None);
        }
        let drifted = snapshot.idle_drift(self.mode)?;
        if drifted.is_empty() {
            self.withdrawn = false;
            if self.notice().is_some() { self.message = None; }
            return Ok(SessionEvent::None);
        }
        let mode_changed = self.mode != self.mode_written;
        let own = drifted.iter().all(|d| (self.withdrawn && d == "pcState=0") || (mode_changed && d.starts_with("pcMode=")));
        let reason = if own { String::new() } else {
            let now = Instant::now();
            self.restores.retain(|at| now.saturating_duration_since(*at) < RESTORE_WINDOW);
            if self.restores.len() >= RESTORE_LIMIT {
                return Err(format!("空闲时视觉输出 {} s 内第 {} 次被改写（{}），疑似 PLC 程序或其他写入方覆盖 PC 区，排查后明确复位",
                    RESTORE_WINDOW.as_secs(), self.restores.len() + 1, drifted.join("、")));
            }
            self.restores.push(now);
            let reason = format!("空闲时视觉输出 {} 与就绪状态不符（PLC 重启、DB 重新初始化或被其他写入方改写），已重写空闲输出，无需复位",
                drifted.join("、"));
            self.audit(engine, "idleOutputsRestored", &reason).await?;
            reason
        };
        self.write_idle(engine).await?;
        self.withdrawn = false;
        if self.notice().is_some() { self.message = None; }
        Ok(if own { SessionEvent::None } else { SessionEvent::Restored(reason) })
    }

    async fn write_idle(&mut self, engine: &PlcEngine) -> Result<(), String> {
        self.write(engine, handshake::idle_plan(self.mode)).await?;
        self.mode_written = self.mode;
        Ok(())
    }

    /// 布防前核对软件计划。示教采集计划（[`PlcPlan::is_teaching`]）只能用于已绑定采集轮次的事务；协议 7 的请求不带计划，不与 PLC 比对。
    pub fn validate_plan(&self, plan: &PlcPlan) -> Result<(), String> {
        if self.phase != SessionPhase::Validating { return Err("当前 S7 事务不能布防".into()); }
        if plan.is_teaching() && self.capture_id().is_none() { return Err("示教采集计划不能用于生产事务".into()); }
        let rebuilt = plan.recompiled()?;
        if &rebuilt != plan { return Err("软件拍照计划内容与计数不一致".into()); }
        Ok(())
    }

    /// 最近一次布防的耗时构成；失败时只含已完成的部分。
    pub fn arm_timing(&self) -> Option<ArmTiming> { self.arm_timing }

    /// 布防并把事务推进到采集阶段：[`arm_outputs`](Self::arm_outputs) + [`commit_arm`](Self::commit_arm)。
    pub async fn arm(&mut self, engine: &PlcEngine, plan: &PlcPlan) -> Result<(), String> {
        self.arm_outputs(engine, Some(plan)).await?;
        self.commit_arm(engine).await
    }

    /// 写布防输出并读回确认（布防预算只计到这里）；之后必须 [`commit_arm`](Self::commit_arm) 落盘才进入采集阶段。
    /// 没有拍照计划只允许维护模式的生产事务：PLC 照常运动，PC 只采集或不采集，结果码 0。
    pub async fn arm_outputs(&mut self, engine: &PlcEngine, plan: Option<&PlcPlan>) -> Result<(), String> {
        self.arm_timing = None;
        self.outputs_armed = false;
        match plan {
            Some(plan) => self.validate_plan(plan)?,
            None if self.phase == SessionPhase::Validating && self.part_maintenance() && self.capture_id().is_none() => {}
            None => return Err("没有拍照计划只能在维护模式下布防".into()),
        }
        let mode = self.journal.pending.as_ref().map_or(0, |pending| pending.mode);
        let attempt: Result<(), String> = async {
            let snapshot = self.snapshot(engine)?;
            let request = self.request().ok_or("缺少 S7 请求")?.clone();
            if snapshot.request()? != request
                || snapshot.plc_state()? != state::PLC_REQUEST
                || snapshot.pc_state()? != state::PC_READY
            {
                return Err("布防前请求或握手状态已改变".into());
            }
            let written = self
                .write_timed(engine, handshake::arm_plan(&request, mode))
                .await?;
            self.arm_timing = Some(ArmTiming {
                write_ms: millis(written.write),
                confirm_ms: millis(written.confirm),
                confirm_polls: written.polls,
                journal_ms: None,
            });
            let after = self.snapshot(engine)?;
            if after.request()? != request
                || !matches!(
                    after.plc_state()?,
                    state::PLC_REQUEST | state::PLC_MOTION_END
                )
            {
                return Err("布防提交期间请求身份或握手状态已改变".into());
            }
            Ok(())
        }.await;
        match &attempt {
            Ok(()) => self.outputs_armed = true,
            Err(error) => self.fault(engine, error.clone()).await,
        }
        attempt
    }

    /// 布防输出已确认后把事务阶段落盘为采集中。
    pub async fn commit_arm(&mut self, engine: &PlcEngine) -> Result<(), String> {
        if self.phase != SessionPhase::Validating || !std::mem::take(&mut self.outputs_armed) {
            return Err("布防输出尚未确认，不能进入采集阶段".into());
        }
        let started = Instant::now();
        let persisted = self.set_pending_phase(engine, SessionPhase::Acquiring).await;
        if let Some(timing) = self.arm_timing.as_mut() { timing.journal_ms = Some(millis(started.elapsed())); }
        if let Err(error) = &persisted { self.fault(engine, error.clone()).await; }
        persisted
    }

    pub async fn report(&mut self, engine: &PlcEngine, result_code: u16, fault_code: u16) -> Result<(), String> {
        if !matches!(self.phase, SessionPhase::Validating | SessionPhase::Acquiring | SessionPhase::Draining) {
            return Err("当前 S7 事务不能发布新结果".into());
        }
        let mode = self.journal.pending.as_ref().map_or(0, |pending| pending.mode);
        if !result_allowed(mode, result_code, fault_code) {
            return Err(if mode == MAINTENANCE { "维护模式下结果码须为 0".into() } else { "结果码或异常码与协议 7 不一致".into() });
        }
        if self.phase != SessionPhase::Draining && mode != MAINTENANCE && result_code != 90 {
            return Err("运动结束及触发校验完成前只能提交检测异常结果".into());
        }
        let request = self.request().ok_or("缺少 S7 请求")?;
        let envelope = ResultEnvelope { request_seq: request.request_seq, sn: request.sn.clone(), result_code, fault_code };
        self.journal.pending.as_mut().unwrap().result = Some(envelope.clone());
        let attempt = async {
            self.persist(engine).await?;
            let snapshot = self.snapshot(engine)?;
            if snapshot.request()? != *self.request().ok_or("缺少 S7 请求")?
                || !matches!(
                    snapshot.plc_state()?,
                    state::PLC_REQUEST | state::PLC_MOTION_END
                )
                || snapshot.pc_state()? == state::PC_RESULT
            {
                return Err("结果发布前请求或握手状态已改变".into());
            }
            self.write(engine, handshake::report_plan(&envelope)).await?;
            self.set_pending_phase(engine, SessionPhase::AwaitAck).await
        }.await;
        if let Err(error) = &attempt { self.fault(engine, error.clone()).await; }
        attempt
    }
}

#[cfg(test)]
mod tests;

/// 运行日志 s7.fault_output：PLC 可见的故障输出（或断开通讯）都经过这里，带触发原因。
fn log_fault_output(
    cause: &str,
    reason: Option<&String>,
    phase: SessionPhase,
    result: &Result<(), String>,
) {
    let data = json!({"cause": cause, "reason": reason, "phase": format!("{phase:?}"), "ok": result.is_ok(), "error": result.as_ref().err()});
    let msg = format!(
        "写出故障输出（{cause}）{}",
        reason.map(|r| format!("：{r}")).unwrap_or_default()
    );
    applog::emit_event(
        &codes::S7_FAULT_OUTPUT,
        &msg,
        Fields::default().with_data(data),
    );
}

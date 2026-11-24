use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ly_plc::{now_ms, LinkState, LogContext, PlcEngine};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::applog::{self, codes, Fields};
use crate::handshake::{
    self, Contract, HeartbeatWatch, Request, ResultEnvelope, Snapshot, WriteOp,
};
use crate::inspection::tag;
use crate::plc_plan::PlcPlan;
use crate::settings::RecordMode;
use crate::store::RecordSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionPhase {
    NotReady,
    Idle,
    Validating,
    Acquiring,
    Draining,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartSite {
    pub source: RecordSource,
    pub record: RecordMode,
}

/// 在途件：只用于进程中途退出后，下次启动补一条中断记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    request: Request,
    #[serde(default)]
    cycle_id: Option<String>,
    #[serde(default)]
    started_at: i64,
    #[serde(default)]
    site: Option<PartSite>,
    /// 接件时锁存的 pcMode（0 正常，1 维护）：回写结果按它，换模式从下一件生效
    #[serde(default)]
    mode: u16,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    version: u8,
    pending: Option<Pending>,
}

const JOURNAL_VERSION: u8 = 2;

fn valid_cycle_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 第六节结果码：维护模式（pcMode=1）固定 0；正常模式不得为 0，除 90 外 faultCode 为 0。
fn result_allowed(mode: u16, result_code: u16, fault_code: u16) -> bool {
    match mode {
        MAINTENANCE => result_code == 0 && fault_code == 0,
        _ => {
            matches!(result_code, 1 | 2 | 11 | 12 | 13 | 14 | 90)
                && (result_code == 90 || fault_code == 0)
        }
    }
}

/// 握手日志里在途件的协议版本、序号与 SN；与当前协议一致或没有在途件时为 None。
fn foreign_request(value: &serde_json::Value) -> Option<(u64, u64, String)> {
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

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// 把不能用的日志文件 `from` 改名到握手日志旁（`<名>.<suffix>`）保留证据，返回运行日志提示。
fn set_aside(journal: &Path, from: &Path, suffix: &str, reason: &str, then: &str) -> String {
    let backup = journal.with_extension(suffix);
    match std::fs::rename(from, &backup) {
        Ok(()) => format!("{reason}，已移至 {}，{then}", file_name(&backup)),
        Err(error) => format!(
            "{reason}，移至 {} 失败（{error}），{then}，原文件将被覆盖",
            file_name(&backup)
        ),
    }
}

/// 读出握手日志里的在途件。别的协议版本留下的在途件（D-0 不迁移、不续做）与损坏的日志改名保留，返回提示；
/// 上一版格式里没有在途件的日志不带可用内容，按空日志处理。
fn read_pending(path: &Path) -> Result<Option<Pending>, String> {
    let aside =
        |suffix: String, reason: String| set_aside(path, path, &suffix, &reason, "以空日志启动");
    let corrupt = |error: String| {
        aside(
            format!("corrupt-{}.json", now_ms()),
            format!("S7 握手日志不可用（{error}）"),
        )
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(corrupt(error.to_string())),
    };
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| corrupt(error.to_string()))?;
    if let Some((version, seq, sn)) = foreign_request(&value) {
        return Err(aside(
            format!("v{version}.bak"),
            format!(
                "握手日志中的在途件来自协议 {version}（序号 {seq} SN={sn}），不能在协议 {} 下续做",
                handshake::PROTOCOL_VERSION
            ),
        ));
    }
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => Ok(None),
        Some(version) if version == u64::from(JOURNAL_VERSION) => {
            let journal: Journal =
                serde_json::from_value(value).map_err(|error| corrupt(error.to_string()))?;
            if let Some(pending) = &journal.pending {
                pending.request.validate().map_err(&corrupt)?;
                if pending
                    .cycle_id
                    .as_deref()
                    .is_some_and(|id| !valid_cycle_id(id))
                {
                    return Err(corrupt("cycleId 必须是 32 位十六进制持久身份".into()));
                }
            }
            Ok(journal.pending)
        }
        other => Err(corrupt(format!("不支持的版本 {other:?}"))),
    }
}

fn write_journal(path: &Path, text: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension("pending.tmp");
    let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(text)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub phase: SessionPhase,
    /// 在途件的件序号（partSeq）
    pub request_seq: Option<u32>,
    pub message: Option<String>,
    /// 会话已核对的 PLC 连接（连接时间戳）
    pub connection: Option<i64>,
}

/// 空闲提示的前缀：不是故障，现状恢复后自动消失。
pub const NOTICE: &str = "S7 空闲：";
/// pcMode：维护模式下结果码固定 0，PLC 按未检放行。
const MAINTENANCE: u16 = 1;
/// PC 输出被外部改写后重写的日志间隔：有写入方持续覆盖 PC 区时不每拍写一条。
const REPAIR_LOG_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum SessionEvent {
    None,
    /// 通讯就绪（PLC 已连接、协议版本一致、PLC 心跳在变化）
    Ready,
    /// 新件到达
    Start(Request),
    /// 工件结束（inspecting 回 0）
    End(Instant),
    /// 在途件没等到工件结束就被换件；节拍记中断后调用 [`PlcSession::settle`]
    Lost(String),
}

pub struct PlcSession {
    path: PathBuf,
    pending: Option<Pending>,
    /// 启动时归档旧协议在途件、移开损坏日志或未提交临时文件的提示
    archived: Option<String>,
    phase: SessionPhase,
    contract: Option<Contract>,
    config_key: String,
    connection: Option<i64>,
    heartbeat: HeartbeatWatch,
    message: Option<String>,
    /// PC 不接的在检件序号：已按中断记录、到达时 PC 未就绪或身份不合法的件，等它结束
    settled: Option<u32>,
    /// 当前 pcMode，下一件接件时锁存
    mode: u16,
    /// 最近一次写出的 pcMode：切换维护模式后的重写不算外部改写
    mode_written: u16,
    repair_logged: Option<Instant>,
}

impl PlcSession {
    /// 打开握手日志，任何日志问题都不挡通讯：未提交的临时文件、损坏或别的协议版本的日志改名到旁边保留，
    /// 会话以空日志启动，提示经 [`Self::archived`] 记入运行日志。
    pub fn open(path: PathBuf) -> Self {
        let mut notices = Vec::new();
        let temporary = path.with_extension("pending.tmp");
        if temporary.exists() {
            notices.push(set_aside(
                &path,
                &temporary,
                &format!("pending-{}.tmp", now_ms()),
                "发现未提交的握手日志（上次保存未完成）",
                "按已提交的握手日志启动",
            ));
        }
        let pending = read_pending(&path).unwrap_or_else(|notice| {
            notices.push(notice);
            None
        });
        Self {
            path,
            pending,
            archived: (!notices.is_empty()).then(|| notices.join("；")),
            phase: SessionPhase::NotReady,
            contract: None,
            config_key: String::new(),
            connection: None,
            heartbeat: HeartbeatWatch::new(3000).expect("valid heartbeat timeout"),
            message: Some("PLC 未连接".into()),
            settled: None,
            mode: 0,
            mode_written: 0,
            repair_logged: None,
        }
    }

    /// 启动时归档旧协议在途件、移开损坏日志或未提交临时文件后的提示；都没有时为空。
    pub fn archived(&self) -> Option<&str> {
        self.archived.as_deref()
    }

    /// 维护模式开关：从下一件起结果码回 0；空闲时随即重写 pcMode。
    pub fn set_maintenance(&mut self, on: bool) {
        self.mode = if on { MAINTENANCE } else { 0 };
    }

    /// 在途件是否按维护模式接件（结果码须为 0）。
    pub fn part_maintenance(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.mode == MAINTENANCE)
    }

    /// 空闲时的提示（以 [`NOTICE`] 开头）；不是故障。
    pub fn notice(&self) -> Option<&str> {
        self.message
            .as_deref()
            .filter(|message| self.phase == SessionPhase::Idle && message.starts_with(NOTICE))
    }

    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// 上次运行留下的在途件已由启动流程补了中断记录，不属于本次运行。
    pub fn forget_previous(&mut self) {
        self.pending = None;
    }

    pub fn bind_cycle_id(&mut self, id: &str, site: PartSite) -> Result<(), String> {
        if !valid_cycle_id(id) {
            return Err("cycleId 必须是 32 位十六进制持久身份".into());
        }
        let pending = self.pending.as_mut().ok_or("缺少可关联的在途件")?;
        match pending.cycle_id.as_deref() {
            Some(bound) if bound != id => Err("在途件已绑定其他 cycleId，禁止更换工件身份".into()),
            Some(_) => Ok(()),
            None => {
                pending.cycle_id = Some(id.into());
                pending.site = Some(site);
                Ok(())
            }
        }
    }

    pub fn cycle_id(&self) -> Option<&str> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.cycle_id.as_deref())
    }

    pub fn site(&self) -> Option<PartSite> {
        self.pending.as_ref().and_then(|pending| pending.site)
    }

    pub fn request(&self) -> Option<&Request> {
        self.pending.as_ref().map(|pending| &pending.request)
    }

    pub fn started_at(&self) -> Option<i64> {
        self.pending
            .as_ref()
            .map(|pending| pending.started_at)
            .filter(|ts| *ts > 0)
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn view(&self) -> SessionView {
        SessionView {
            phase: self.phase,
            request_seq: self.request().map(|request| request.request_seq),
            message: self.message.clone(),
            connection: self.connection,
        }
    }

    /// 握手日志只为补中断记录，保存失败不挡检测。
    async fn persist(&self) {
        let journal = Journal {
            version: JOURNAL_VERSION,
            pending: self.pending.clone(),
        };
        let path = self.path.clone();
        let saved = match serde_json::to_vec_pretty(&journal) {
            Ok(text) => tokio::task::spawn_blocking(move || write_journal(&path, &text))
                .await
                .map_err(|error| error.to_string())
                .and_then(|saved| saved),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = saved {
            applog::write(
                "warn",
                "S7 握手日志",
                &format!("保存失败，进程中途退出时本件补不出中断记录：{error}"),
            );
        }
    }

    fn timeout(engine: &PlcEngine) -> Duration {
        let config = engine.config().connection;
        Duration::from_millis(
            (config.timeout_ms.saturating_mul(3) + config.poll_interval_ms.saturating_mul(6))
                .max(1500),
        )
    }

    fn max_age(engine: &PlcEngine) -> u64 {
        let config = engine.config().connection;
        (config.timeout_ms.saturating_mul(2) + config.poll_interval_ms.saturating_mul(5)).max(1000)
    }

    fn snapshot(&self, engine: &PlcEngine) -> Result<Snapshot, String> {
        Snapshot::capture(
            engine,
            self.contract.as_ref().ok_or("S7 点表尚未校验")?,
            Self::max_age(engine),
        )
    }

    async fn write(&self, engine: &PlcEngine, operations: Vec<WriteOp>) -> Result<(), String> {
        let contract = self.contract.as_ref().ok_or("S7 点表尚未校验")?;
        if self.connection != Some(engine.status().since) {
            return Err("PLC 连接已更换".into());
        }
        handshake::write_confirmed(
            engine,
            contract,
            &operations,
            Self::timeout(engine),
            Self::max_age(engine),
        )
        .await
    }

    /// 按协议读 PLC 的当前件、维持 PC 输出。通讯条件不满足时回到未就绪并给出原因，条件恢复后自动就绪；
    /// 相机、配方、追溯与磁盘问题由节拍按本件检测错误处理，不影响通讯就绪。
    pub async fn poll(&mut self, engine: &PlcEngine) -> SessionEvent {
        match self.poll_inner(engine).await {
            Ok(event) => event,
            Err(reason) => {
                self.phase = SessionPhase::NotReady;
                self.message = Some(reason);
                SessionEvent::None
            }
        }
    }

    async fn poll_inner(&mut self, engine: &PlcEngine) -> Result<SessionEvent, String> {
        let config = engine.config();
        let key = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        if key != self.config_key {
            self.contract = None;
            let contract = Contract::validate(&config)?;
            // plc.db 每条日志带上当时的 partSeq，按件追溯信号变化
            engine.set_log_context(Some(LogContext {
                point_id: contract.point_id(tag::PART_SEQ)?.to_owned(),
                label: tag::PART_SEQ.into(),
            }));
            self.contract = Some(contract);
            self.config_key = key;
            self.connection = None;
        }
        let status = engine.status();
        if status.state != LinkState::Connected {
            self.connection = None;
            return Err("PLC 未连接".into());
        }
        if self.connection != Some(status.since) {
            self.connection = Some(status.since);
            self.heartbeat.reset();
            if self.phase != SessionPhase::NotReady {
                return Err("PLC 已重新连接，正在核对新连接".into());
            }
        }
        let snapshot = self.snapshot(engine)?;
        let version = snapshot.read_u16(tag::PLC_VERSION)?;
        if version != handshake::PROTOCOL_VERSION {
            return Err(format!(
                "PLC 协议版本 {version} 与 PC {} 不一致",
                handshake::PROTOCOL_VERSION
            ));
        }
        let now = now_ms();
        let heartbeat = snapshot.plc_heartbeat()?;
        if let Err(error) = self
            .heartbeat
            .observe(heartbeat, snapshot.heartbeat_changed_at, now)
        {
            self.heartbeat.reset();
            self.heartbeat
                .observe(heartbeat, snapshot.heartbeat_changed_at, now)?;
            return Err(error);
        }
        if !self.heartbeat.is_live(now) {
            return Err("等待 PLC 心跳变化".into());
        }
        let inspecting = snapshot.inspecting()?;
        let seq = snapshot.part_seq()?;
        let result_seq = snapshot.read_u32(tag::RESULT_SEQ)?;
        if self.phase == SessionPhase::NotReady {
            self.write(engine, snapshot.output_repairs(self.mode))
                .await?;
            self.mode_written = self.mode;
            self.phase = SessionPhase::Idle;
            self.message = None;
            self.settled = None;
            if inspecting {
                self.skip(seq, &snapshot, "到达时 PC 未就绪");
            }
            return Ok(SessionEvent::Ready);
        }
        // PC 只在工件结束后写结果：在检件的序号等于 resultSeq 时那是别的件留下的旧结果，清掉后 PLC 不会误取
        if inspecting && seq != 0 && result_seq == seq {
            self.write(engine, handshake::clear_result_plan()).await?;
            applog::emit_event(
                &codes::S7_RESULT_CLEARED,
                &format!("在检件的序号 {seq} 与旧结果的序号相同，已把 resultSeq 清零"),
                Fields::default().with_data(json!({"partSeq": seq})),
            );
        }
        match self.phase {
            SessionPhase::Idle => self.idle(engine, &snapshot, inspecting, seq).await,
            SessionPhase::Validating | SessionPhase::Acquiring => {
                let current = self.request().ok_or("在途件丢失")?.request_seq;
                if seq != current {
                    return Ok(SessionEvent::Lost(format!(
                        "在检期间件序号由 {current} 变为 {seq}，没有收到工件结束"
                    )));
                }
                if !inspecting && self.phase == SessionPhase::Acquiring {
                    self.phase = SessionPhase::Draining;
                    return Ok(SessionEvent::End(Instant::now()));
                }
                Ok(SessionEvent::None)
            }
            SessionPhase::Draining | SessionPhase::NotReady => Ok(SessionEvent::None),
        }
    }

    /// 空闲：`inspecting` 由 0 变 1，或在检的序号不是 PC 已放下的那一件，就是新件；连续两次轮询内容一致才登记。
    async fn idle(
        &mut self,
        engine: &PlcEngine,
        snapshot: &Snapshot,
        inspecting: bool,
        seq: u32,
    ) -> Result<SessionEvent, String> {
        if !inspecting {
            self.settled = None;
            if self.notice().is_some() {
                self.message = None;
            }
            return self.keep_outputs(engine, snapshot).await;
        }
        if self.settled == Some(seq) {
            return Ok(SessionEvent::None);
        }
        let contract = self.contract.as_ref().ok_or("S7 点表尚未校验")?;
        let Some(arrival) = handshake::stable_arrival(
            engine,
            contract,
            snapshot,
            Self::timeout(engine),
            Self::max_age(engine),
        )
        .await?
        else {
            return Ok(SessionEvent::None);
        };
        match arrival.request() {
            Ok(request) => {
                self.pending = Some(Pending {
                    request: request.clone(),
                    cycle_id: None,
                    started_at: now_ms(),
                    site: None,
                    mode: self.mode,
                });
                self.settled = None;
                self.message = None;
                self.phase = SessionPhase::Validating;
                Ok(SessionEvent::Start(request))
            }
            Err(reason) => {
                self.skip(seq, &arrival, &format!("身份不合法（{reason}）"));
                Ok(SessionEvent::None)
            }
        }
    }

    /// 不接这一件：不登记、不回结果，PLC 等不到结果记未检。
    fn skip(&mut self, seq: u32, snapshot: &Snapshot, reason: &str) {
        self.settled = Some(seq);
        let text = format!("件序号 {seq} {reason}，本件不检测、不回结果，等待下一件");
        applog::emit_event(
            &codes::S7_PART_SKIPPED,
            &text,
            Fields::default().with_data(
                json!({"partSeq": seq, "sn": snapshot.read_text(tag::SN).ok(), "reason": reason}),
            ),
        );
        self.message = Some(format!("{NOTICE}{text}"));
    }

    /// 没有在检件时保持 PC 输出：版本、模式被改写（PLC 重启、DB 重新初始化、调试表误写）就重写。
    async fn keep_outputs(
        &mut self,
        engine: &PlcEngine,
        snapshot: &Snapshot,
    ) -> Result<SessionEvent, String> {
        let repairs = snapshot.output_repairs(self.mode);
        if repairs.is_empty() {
            return Ok(SessionEvent::None);
        }
        let own = self.mode != self.mode_written && repairs.iter().all(|op| op.tag == tag::PC_MODE);
        let tags = repairs
            .iter()
            .map(|op| op.tag)
            .collect::<Vec<_>>()
            .join("、");
        self.write(engine, repairs).await?;
        self.mode_written = self.mode;
        if !own
            && self
                .repair_logged
                .is_none_or(|at| at.elapsed() >= REPAIR_LOG_INTERVAL)
        {
            self.repair_logged = Some(Instant::now());
            applog::write(
                "warn",
                "S7 输出恢复",
                &format!("PC 输出 {tags} 被改写（PLC 重启、DB 重新初始化或其他写入方），已重写"),
            );
        }
        Ok(SessionEvent::None)
    }

    /// 接件时核对软件计划；PLC 的请求不带计划，不与 PLC 比对。
    pub fn validate_plan(&self, plan: &PlcPlan) -> Result<(), String> {
        if self.phase != SessionPhase::Validating {
            return Err("当前工件不在接件阶段".into());
        }
        let rebuilt = plan.recompiled()?;
        if &rebuilt != plan {
            return Err("软件拍照计划内容与计数不一致".into());
        }
        Ok(())
    }

    /// 接件登记完成：进入采集，在途件落盘；不写 PLC。
    pub async fn admit(&mut self) {
        if self.phase == SessionPhase::Validating {
            self.phase = SessionPhase::Acquiring;
        }
        self.persist().await;
    }

    /// 在途件已由节拍按中断记录（通讯未就绪、中途换件、等不到工件结束）：放下它，不回结果，PLC 换下一件后照常接件。
    pub async fn settle(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.settled = Some(pending.request.request_seq);
            self.persist().await;
        }
        if self.phase != SessionPhase::NotReady {
            self.phase = SessionPhase::Idle;
        }
    }

    /// 回写结果：最后写 resultSeq，写完本件即结束，不等 PLC。PLC 已切到下一件时不写。
    pub async fn report(
        &mut self,
        engine: &PlcEngine,
        result_code: u16,
        fault_code: u16,
    ) -> Result<(), String> {
        let draining = self.phase == SessionPhase::Draining;
        let pending = self.pending.take().ok_or("缺少在途件")?;
        if self.phase != SessionPhase::NotReady {
            self.phase = SessionPhase::Idle;
        }
        self.persist().await;
        if !draining {
            self.settled = Some(pending.request.request_seq);
            return Err("工件尚未结束（inspecting 仍为 1），不能回写结果".into());
        }
        if !result_allowed(pending.mode, result_code, fault_code) {
            return Err(if pending.mode == MAINTENANCE {
                "维护模式下结果码须为 0".into()
            } else {
                format!(
                    "结果码或异常码与协议 {} 不一致",
                    handshake::PROTOCOL_VERSION
                )
            });
        }
        let envelope = ResultEnvelope {
            request_seq: pending.request.request_seq,
            sn: pending.request.sn,
            result_code,
            fault_code,
        };
        let snapshot = self.snapshot(engine)?;
        if snapshot.inspecting()? || snapshot.part_seq()? != envelope.request_seq {
            return Err("PLC 已切到下一件，本件结果不再回写".into());
        }
        self.write(engine, handshake::report_plan(&envelope, pending.mode))
            .await?;
        self.mode_written = pending.mode;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

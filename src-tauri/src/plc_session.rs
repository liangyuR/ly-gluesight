use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ly_plc::{now_ms, LinkState, PlcEngine};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::handshake::{self, Contract, HeartbeatWatch, Request, ResultEnvelope, Snapshot, WriteOp};
use crate::inspection::tag;
use crate::plc_plan::PlcPlan;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionPhase { ResetRequired, Idle, Validating, Acquiring, Draining, AwaitAck, Releasing, Fault }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    request: Request,
    result: Option<ResultEnvelope>,
    phase: SessionPhase,
    #[serde(default)]
    acknowledged: bool,
    #[serde(default)]
    started_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    version: u8,
    last_request_seq: u32,
    pending: Option<Pending>,
    last_resolution: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub phase: SessionPhase,
    pub request_seq: Option<u32>,
    pub result_seq: Option<u32>,
    pub message: Option<String>,
}

#[derive(Debug)]
pub enum SessionEvent { None, Ready, Start(Request), End(Instant), Released, Fault(String) }

pub struct PlcSession {
    path: PathBuf,
    journal: Journal,
    load_error: Option<String>,
    phase: SessionPhase,
    contract: Option<Contract>,
    config_key: String,
    connection: Option<i64>,
    reset_low_seen: bool,
    reset_level: bool,
    heartbeat: Mutex<HeartbeatWatch>,
    fault_written: bool,
    fault_attempted: Option<Instant>,
    message: Option<String>,
}

impl PlcSession {
    pub fn open(path: PathBuf) -> Self {
        let loaded = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Journal>(text.trim_start_matches('\u{feff}')).map_err(|e| format!("S7 握手日志损坏，保留现场信号并修复日志后重启：{e}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !path.with_extension("pending.tmp").exists() => Ok(Journal { version: 1, ..Journal::default() }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err("发现未提交的握手事务文件，保留现场信号并核对日志后重启".into()),
            Err(error) => Err(format!("S7 握手日志无法读取：{error}")),
        };
        let (journal, mut load_error) = match loaded {
            Ok(journal) if journal.version == 1 => {
                let error = journal.pending.as_ref().and_then(|pending| {
                    pending.request.validate().err().or_else(|| {
                        (pending.request.request_seq != journal.last_request_seq
                            || matches!(pending.phase, SessionPhase::Idle | SessionPhase::ResetRequired)
                            || (matches!(pending.phase, SessionPhase::AwaitAck | SessionPhase::Releasing) && pending.result.is_none())
                            || pending.result.as_ref().is_some_and(|result|
                                result.request_seq != pending.request.request_seq || result.sn != pending.request.sn))
                            .then(|| "握手事务日志的身份或阶段不一致，禁止自动恢复".into())
                    })
                });
                (journal, error)
            }
            Ok(journal) => (journal, Some("不支持的 S7 握手日志版本".into())),
            Err(error) => (Journal::default(), Some(error)),
        };
        if path.with_extension("pending.tmp").exists() {
            load_error = Some("发现未提交的握手事务文件，保留现场信号并核对日志后重启".into());
        }
        Self { path, journal, load_error, phase: SessionPhase::ResetRequired, contract: None,
            config_key: String::new(), connection: None, reset_low_seen: false,
            reset_level: false, heartbeat: Mutex::new(HeartbeatWatch::new(3000).expect("valid heartbeat timeout")),
            fault_written: false, fault_attempted: None, message: Some("S7 启动后需确认空闲输入并复位握手".into()) }
    }

    pub fn pending(&self) -> bool { self.journal.pending.is_some() || self.load_error.is_some() }

    pub fn request(&self) -> Option<&Request> { self.journal.pending.as_ref().map(|p| &p.request) }

    pub fn result(&self) -> Option<&ResultEnvelope> { self.journal.pending.as_ref().and_then(|p| p.result.as_ref()) }

    pub fn phase(&self) -> SessionPhase { self.phase }

    pub fn view(&self) -> SessionView {
        SessionView { phase: self.phase, request_seq: self.request().map(|r| r.request_seq),
            result_seq: self.result().map(|r| r.request_seq), message: self.message.clone() }
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
                        self.heartbeat.lock().unwrap().observe(snapshot.read_bool(tag::PLC_HEARTBEAT)?, now_ms())
                    };
                    observation_error = observe().err();
                    if observation_error.is_some() { let _ = self.stop_outputs(engine).await; }
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
        self.blocking_io(engine, move || {
            if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)
                .map_err(|e| format!("无法保存握手审计：{e}"))?;
            file.write_all(&record).and_then(|_| file.sync_all()).map_err(|e| format!("握手审计写入失败：{e}"))
        }).await
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

    async fn write(&self, engine: &PlcEngine, operations: Vec<WriteOp>) -> Result<(), String> {
        let contract = self.contract.as_ref().ok_or("S7 握手点表尚未校验")?;
        if self.connection != Some(engine.status().since) {
            return Err("PLC 连接已更换，禁止提交握手输出，需明确复位".into());
        }
        handshake::write_confirmed(engine, contract, &operations, Self::timeout(engine), Self::max_age(engine)).await
    }

    pub async fn fault(&mut self, engine: &PlcEngine, reason: String) {
        self.phase = SessionPhase::Fault;
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
        self.fault_attempted = Some(Instant::now());
        self.fault_written = result.is_ok();
        result
    }

    async fn stop_outputs(&self, engine: &PlcEngine) -> Result<(), String> {
        let mut errors = Vec::new();
        let mut stop_confirmed = false;
        for operation in handshake::fault_plan() {
            let tag = operation.tag;
            let result = match self.contract.as_ref() {
                Some(contract) => handshake::write_confirmed(engine, contract, &[operation], Self::timeout(engine), Self::max_age(engine)).await,
                None => Err("S7 握手点表尚未校验".into()),
            };
            match result {
                Ok(()) => { if tag == tag::ARMED || tag == tag::VISION_FAULT { stop_confirmed = true; } }
                Err(error) => errors.push(error),
            }
        }
        if !stop_confirmed {
            engine.disconnect().await;
            errors.push("无法确认撤销布防或置故障，已断开通讯以停止 PC 心跳；重新连接后需明确复位".into());
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("；")) }
    }

    pub async fn poll(&mut self, engine: &PlcEngine, reset_requested: bool, devices_ready: bool) -> SessionEvent {
        match self.poll_inner(engine, reset_requested, devices_ready).await {
            Ok(event) => event,
            Err(error) => {
                let changed = self.phase != SessionPhase::Fault || self.message.as_ref() != Some(&error);
                self.fault(engine, error.clone()).await;
                if changed { SessionEvent::Fault(error) } else { SessionEvent::None }
            }
        }
    }

    async fn poll_inner(&mut self, engine: &PlcEngine, reset_requested: bool, devices_ready: bool) -> Result<SessionEvent, String> {
        let config = engine.config();
        let key = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        if key != self.config_key {
            self.contract = Some(Contract::validate(&config)?);
            self.config_key = key;
            self.connection = None;
        }
        let status = engine.status();
        if status.state != LinkState::Connected {
            self.connection = None;
            return Err("S7 PLC 未连接；保留未结事务，重连后需复位".into());
        }
        if self.connection != Some(status.since) {
            self.connection = Some(status.since);
            self.phase = SessionPhase::ResetRequired;
            self.reset_low_seen = false;
            self.reset_level = false;
            self.heartbeat.lock().unwrap().reset();
            self.fault_written = false;
            self.fault_attempted = None;
            self.message = Some("S7 连接会话已更新，需确认空闲输入后复位；未结事务保留".into());
        }
        let snapshot = Snapshot::capture(engine, self.contract.as_ref().unwrap(), Self::max_age(engine))?;
        let now = now_ms();
        let heartbeat = snapshot.read_bool(tag::PLC_HEARTBEAT)?;
        let observation = self.heartbeat.lock().unwrap().observe(heartbeat, now);
        if let Err(error) = observation {
            let mut watch = self.heartbeat.lock().unwrap();
            watch.reset();
            watch.observe(heartbeat, now)?;
            return Err(error);
        }
        let reset_level = snapshot.read_bool(tag::FAULT_RESET)?;
        if !reset_level { self.reset_low_seen = true; }
        let reset_edge = reset_level && !self.reset_level && self.reset_low_seen;
        self.reset_level = reset_level;
        if let Some(error) = &self.load_error { return Err(error.clone()); }
        if (reset_requested || reset_edge) && self.phase == SessionPhase::Idle {
            self.phase = SessionPhase::ResetRequired;
            self.fault_written = false;
            self.fault_attempted = None;
        } else if (reset_requested || reset_edge) && !matches!(self.phase, SessionPhase::ResetRequired | SessionPhase::Fault) {
            return Err("在途工件不能直接复位；保留事务进入故障，处理该件并释放 PLC 输入后再次明确复位".into());
        }
        if matches!(self.phase, SessionPhase::ResetRequired | SessionPhase::Fault) {
            self.ensure_fault_output(engine).await?;
            if !(reset_requested || reset_edge) { return Ok(SessionEvent::None); }
            if !devices_ready { return Err("设备尚未就绪，不能复位并置视觉就绪".into()); }
            if !self.heartbeat.lock().unwrap().is_live(now) { return Err("尚未观察到 PLC 心跳翻转，不能复位".into()); }
            let baseline = handshake::ResetBaseline::capture(&snapshot)?;
            self.audit(engine, "resetRequested", "明确复位请求；尚未清除未结事务").await?;
            self.write(engine, handshake::reset_plan()).await?;
            let after = Snapshot::capture(engine, self.contract.as_ref().unwrap(), Self::max_age(engine))?;
            baseline.verify(&after)?;
            self.audit(engine, "resetCompleted", "明确复位已清除输出并确认中性输入，原事务保留在本审计记录").await?;
            let mut next = self.journal.clone();
            next.last_request_seq = after.read_u32(tag::REQUEST_SEQ)?;
            next.last_resolution = Some("明确复位结束；原请求和结果已保存至审计日志".into());
            next.pending = None;
            self.persist_journal(engine, &next).await?;
            self.journal = next;
            let before_ready = Snapshot::capture(engine, self.contract.as_ref().unwrap(), Self::max_age(engine))?;
            baseline.verify(&before_ready)?;
            self.write(engine, vec![WriteOp { tag: tag::VISION_READY, value: json!(true) }]).await?;
            self.phase = SessionPhase::Idle;
            self.message = None;
            self.fault_written = false;
            self.fault_attempted = None;
            return Ok(SessionEvent::Ready);
        }
        if !self.heartbeat.lock().unwrap().is_live(now) { return Err("PLC 心跳活性尚未确认".into()); }
        if !devices_ready && !matches!(self.phase, SessionPhase::AwaitAck | SessionPhase::Releasing) {
            return Err("检测设备未就绪".into());
        }
        match self.phase {
            SessionPhase::Idle if snapshot.read_bool(tag::PART_START)? => {
                let request = handshake::stable_start(engine, self.contract.as_ref().unwrap(), Self::timeout(engine), Self::max_age(engine)).await?;
                if request.request_seq == 0 || request.request_seq <= self.journal.last_request_seq {
                    return Err("请求事务序号必须非零且递增；PLC 清零或回绕需先执行空闲复位".into());
                }
                self.journal.pending = Some(Pending { request: request.clone(), result: None, phase: SessionPhase::Validating,
                    acknowledged: false, started_at: now_ms() });
                self.journal.last_request_seq = request.request_seq;
                self.persist(engine).await?;
                self.phase = SessionPhase::Validating;
                return Ok(SessionEvent::Start(request));
            }
            SessionPhase::Validating | SessionPhase::Acquiring | SessionPhase::Draining | SessionPhase::AwaitAck => {
                let request = self.request().ok_or("当前 S7 事务丢失")?;
                if snapshot.request()? != *request || !snapshot.read_bool(tag::PART_START)? {
                    return Err("在途工件的请求或计划被修改，禁止继续或切换下一件".into());
                }
                if self.phase != SessionPhase::AwaitAck && snapshot.read_bool(tag::RESULT_ACK)? {
                    return Err("结果发布前收到 ACK，禁止把提前确认用于本次结果".into());
                }
                if self.phase == SessionPhase::Validating && snapshot.read_bool(tag::PART_END)? {
                    return Err("布防前已收到 partEnd，触发计划越过握手边界".into());
                }
                if matches!(self.phase, SessionPhase::Acquiring | SessionPhase::Draining)
                    && (!snapshot.read_bool(tag::BUSY)? || !snapshot.read_bool(tag::ARMED)?
                        || snapshot.read_u32(tag::ACCEPTED_SEQ)? != request.request_seq
                        || snapshot.read_u32(tag::ACCEPTED_PLAN_HASH)? != request.plan_hash) {
                    return Err("已布防事务的 busy、armed 或接受的计划身份被修改".into());
                }
                if self.phase == SessionPhase::Acquiring && snapshot.read_bool(tag::PART_END)? {
                    handshake::verify_part_end(&snapshot, request)?;
                    let ended_at = Instant::now();
                    self.set_pending_phase(engine, SessionPhase::Draining).await?;
                    return Ok(SessionEvent::End(ended_at));
                }
                if self.phase == SessionPhase::AwaitAck {
                    let result = self.result().ok_or("S7 结果尚未生成")?;
                    if handshake::matching_ack(&snapshot, result)? {
                        let mut next = self.journal.clone();
                        next.pending.as_mut().unwrap().acknowledged = true;
                        self.persist_journal(engine, &next).await?;
                        self.journal = next;
                        self.write(engine, handshake::release_plan()).await?;
                        self.set_pending_phase(engine, SessionPhase::Releasing).await?;
                    }
                }
            }
            SessionPhase::Releasing => {
                if [tag::PART_START, tag::PART_END, tag::RESULT_ACK].into_iter().all(|tag| snapshot.read_bool(tag) == Ok(false)) {
                    if !devices_ready { return Err("结果已确认，但设备未就绪，不能释放下一件".into()); }
                    if snapshot.read_bool(tag::DONE)? || snapshot.read_bool(tag::BUSY)? || snapshot.read_bool(tag::ARMED)? {
                        return Err("释放输出未保持清除状态，禁止开始下一件".into());
                    }
                    self.audit(engine, "acknowledged", "结果身份已确认且双方握手已释放").await?;
                    let mut next = self.journal.clone();
                    next.pending = None;
                    next.last_resolution = Some("结果已匹配确认并释放；原事务已保存至审计日志".into());
                    self.persist_journal(engine, &next).await?;
                    self.journal = next;
                    let before_ready = Snapshot::capture(engine, self.contract.as_ref().unwrap(), Self::max_age(engine))?;
                    if [tag::PART_START, tag::PART_END, tag::RESULT_ACK, tag::DONE, tag::BUSY, tag::ARMED, tag::VISION_FAULT]
                        .into_iter().any(|tag| before_ready.read_bool(tag) != Ok(false)) {
                        return Err("释放落盘期间握手信号已改变，禁止提前发出下一件就绪".into());
                    }
                    self.write(engine, vec![WriteOp { tag: tag::VISION_READY, value: json!(true) }]).await?;
                    self.phase = SessionPhase::Idle;
                    return Ok(SessionEvent::Released);
                }
            }
            SessionPhase::Idle => {
                if snapshot.read_bool(tag::DONE)? || snapshot.read_bool(tag::BUSY)? || snapshot.read_bool(tag::ARMED)?
                    || snapshot.read_bool(tag::VISION_FAULT)? || !snapshot.read_bool(tag::VISION_READY)?
                    || snapshot.read_bool(tag::PART_END)? || snapshot.read_bool(tag::RESULT_ACK)? {
                    return Err("空闲握手信号不一致，需明确复位".into());
                }
            }
            _ => {}
        }
        Ok(SessionEvent::None)
    }

    pub fn validate_plan(&self, plan: &PlcPlan) -> Result<(), String> {
        if self.phase != SessionPhase::Validating { return Err("当前 S7 事务不能布防".into()); }
        let rebuilt = PlcPlan::compile(plan.recipe_id.clone(), plan.plan_version, plan.camera_slots.clone(), plan.shots.clone())?;
        if &rebuilt != plan { return Err("软件拍照计划内容、计数与摘要不一致".into()); }
        let request = self.request().ok_or("缺少 S7 请求")?;
        if request.protocol_version != plan.protocol_version || request.plan_version != plan.plan_version || request.plan_hash != plan.plan_hash
            || request.shot_count != plan.shot_count || request.camera_shots != plan.camera_shots {
            return Err(format!("拍照计划不一致：需 version={} hash={} shots={} cameraShots={:?}",
                plan.plan_version, plan.plan_hash, plan.shot_count, plan.camera_shots));
        }
        Ok(())
    }

    pub async fn arm(&mut self, engine: &PlcEngine, plan: &PlcPlan) -> Result<(), String> {
        self.validate_plan(plan)?;
        let attempt = async {
            let snapshot = Snapshot::capture(engine, self.contract.as_ref().ok_or("S7 点表未校验")?, Self::max_age(engine))?;
            let request = self.request().ok_or("缺少 S7 请求")?;
            if snapshot.request()? != *request || !snapshot.read_bool(tag::PART_START)?
                || snapshot.read_bool(tag::PART_END)? || snapshot.read_bool(tag::RESULT_ACK)? || snapshot.trigger_counts()? != [0; 3] {
                return Err("布防前请求或触发边界已改变".into());
            }
            self.write(engine, handshake::arm_plan(request)).await?;
            let after = Snapshot::capture(engine, self.contract.as_ref().unwrap(), Self::max_age(engine))?;
            if after.request()? != *request || !after.read_bool(tag::PART_START)? || after.read_bool(tag::RESULT_ACK)? {
                return Err("布防提交期间请求身份或确认边界已改变".into());
            }
            self.set_pending_phase(engine, SessionPhase::Acquiring).await
        }.await;
        if let Err(error) = &attempt { self.fault(engine, error.clone()).await; }
        attempt
    }

    pub async fn report(&mut self, engine: &PlcEngine, result_code: u16, fault_code: u16) -> Result<(), String> {
        if !matches!(self.phase, SessionPhase::Validating | SessionPhase::Acquiring | SessionPhase::Draining) {
            return Err("当前 S7 事务不能发布新结果".into());
        }
        if self.phase != SessionPhase::Draining && result_code != 90 {
            return Err("运动结束及触发校验完成前只能提交检测异常结果".into());
        }
        if !matches!(result_code, 1 | 2 | 11 | 12 | 13 | 14 | 90) || (result_code != 90 && fault_code != 0) {
            return Err("结果码或异常码与一期协议不一致".into());
        }
        let request = self.request().ok_or("缺少 S7 请求")?;
        let envelope = ResultEnvelope { request_seq: request.request_seq, sn: request.sn, result_code, fault_code };
        self.journal.pending.as_mut().unwrap().result = Some(envelope.clone());
        let attempt = async {
            self.persist(engine).await?;
            let snapshot = Snapshot::capture(engine, self.contract.as_ref().ok_or("S7 点表未校验")?, Self::max_age(engine))?;
            if snapshot.request()? != *self.request().ok_or("缺少 S7 请求")? || !snapshot.read_bool(tag::PART_START)?
                || snapshot.read_bool(tag::RESULT_ACK)? || snapshot.read_bool(tag::DONE)? {
                return Err("结果发布前请求或确认边界已改变".into());
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

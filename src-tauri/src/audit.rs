use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

mod spool;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::applog;
use crate::cycle::{self, FrameView};
use crate::judge::{Judgement, PointState};
use crate::offline::sample_set::SampleRef;
use crate::recipe::Recipe;
use crate::recorder::{RecordedRawFile, RecordingOutcome, RecordingState as RecorderState};
use crate::store::{
    BurstRecord, FrameKey, FrameRecord, PartDetail, PartMode, PartRecord, PartShot, PlcDelivery,
    PlcDeliveryState, RecordSource, RecordingEvidence, RecordingState, Store,
};

const PENDING_TTL_MS: i64 = 30 * 60 * 1000;
const RETRY_BASE_MS: i64 = 1000;
const RETRY_MAX_MS: i64 = 30_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedPart {
    pub ts: i64,
    pub sn: String,
    #[serde(with = "part_recipe")]
    pub recipe: Option<Arc<Recipe>>,
    pub judgement: Judgement,
    pub drain_ms: Option<u64>,
    pub frames: Vec<FrameView>,
    pub frames_expected: usize,
    pub frames_received: usize,
    pub triggers: u64,
    #[serde(with = "part_table")]
    pub table: Option<Vec<PointState>>,
    pub software_version: String,
    pub cycle_id: String,
    pub bundle_id: Option<String>,
    pub delivery: PlcDelivery,
    pub shots: Vec<PartShot>,
    pub source: RecordSource,
    pub sample: Option<SampleRef>,
    /// 段配方的帧与区域记录；没有原配方的记录为 None
    #[serde(default)]
    pub burst: Option<BurstRecord>,
    #[serde(default)]
    pub mode: PartMode,
}

impl RecordedPart {
    fn borrowed(&self) -> PartRecord<'_> {
        PartRecord {
            mode: self.mode,
            ts: self.ts,
            sn: &self.sn,
            recipe: self.recipe.as_deref(),
            judgement: &self.judgement,
            drain_ms: self.drain_ms,
            frames: &self.frames,
            frames_expected: self.frames_expected,
            frames_received: self.frames_received,
            triggers: self.triggers,
            table: self.table.as_deref(),
            software_version: &self.software_version,
            cycle_id: Some(&self.cycle_id),
            bundle_id: self.bundle_id.as_deref(),
            delivery: &self.delivery,
            shots: &self.shots,
            source: self.source,
            sample: self.sample.as_ref(),
            burst: self.burst.as_ref(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
enum Event {
    Insert(Box<RecordedPart>),
    Delivery(String, PlcDelivery),
    Submission(String, PlcDelivery, Option<u64>),
    Recording(RecordingOutcome),
}

impl Event {
    fn cycle_id(&self) -> &str {
        match self {
            Self::Insert(part) => &part.cycle_id,
            Self::Delivery(cycle, _) | Self::Submission(cycle, _, _) => cycle,
            Self::Recording(outcome) => &outcome.cycle_id,
        }
    }
}

mod part_recipe {
    use super::*;
    pub fn serialize<S: serde::Serializer>(recipe: &Option<Arc<Recipe>>, serializer: S) -> Result<S::Ok, S::Error> {
        recipe.as_deref().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Arc<Recipe>>, D::Error> {
        Option::<serde_json::Value>::deserialize(deserializer)?
            .map(|value| Recipe::from_snapshot(value).map(Arc::new).map_err(serde::de::Error::custom)).transpose()
    }
}

mod part_table {
    use super::*;
    /// 数值按位保存，NaN、负零原样往返。d_bits 为空即有胶、无位置基准；格式变动由 spool 文件的 format 拦截旧文件。
    #[derive(Serialize, Deserialize)]
    enum Point { Pending, Measured { d_bits: Option<u32>, w_bits: u32 }, OutOfRange { w_bits: Option<u32> }, Gap, Invalid }
    pub fn serialize<S: serde::Serializer>(table: &Option<Vec<PointState>>, serializer: S) -> Result<S::Ok, S::Error> {
        table.as_ref().map(|table| table.iter().map(|point| match point {
            PointState::Pending => Point::Pending,
            PointState::Measured { d, w } => Point::Measured { d_bits: d.map(f32::to_bits), w_bits: w.to_bits() },
            PointState::OutOfRange { w } => Point::OutOfRange { w_bits: w.map(f32::to_bits) },
            PointState::Gap => Point::Gap,
            PointState::Invalid => Point::Invalid,
        }).collect::<Vec<_>>()).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<PointState>>, D::Error> {
        Option::<Vec<Point>>::deserialize(deserializer).map(|table| table.map(|table| table.into_iter().map(|point| match point {
            Point::Pending => PointState::Pending,
            Point::Measured { d_bits, w_bits } => PointState::Measured { d: d_bits.map(f32::from_bits), w: f32::from_bits(w_bits) },
            Point::OutOfRange { w_bits } => PointState::OutOfRange { w: w_bits.map(f32::from_bits) },
            Point::Gap => PointState::Gap,
            Point::Invalid => PointState::Invalid,
        }).collect()))
    }
}

/// 落盘失败时暂存在内存里的事件上限，满了丢最早的一条并记错误。
const BACKLOG_LIMIT: usize = 1024;
/// 一件连续这么多次没能入库，就移到未恢复目录，不再占着 spool。
const QUARANTINE_ATTEMPTS: u32 = 20;

type Backlog = Mutex<VecDeque<Event>>;

struct Problem {
    cycle: String,
    message: String,
    /// 落盘失败这类持续状态，要显示为追溯当前问题；冲突、无效事件只记日志。
    current: bool,
}

#[derive(Default)]
struct RecordingBarrier {
    inserted: bool,
    terminal: Option<RecordingOutcome>,
}

type RecordingBarriers = Mutex<BTreeMap<String, RecordingBarrier>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuditReadiness { Ready, WaitingRecording, WaitingStore }

impl AuditReadiness {
    fn require_ready(self) -> Result<(), String> {
        match self {
            Self::Ready => Ok(()),
            Self::WaitingRecording => Err("原图录制尚未完成耐久接受".into()),
            Self::WaitingStore => Err("持久追溯仍有待入库事件".into()),
        }
    }
}

fn check_health(failure: &Mutex<Option<String>>, running: &AtomicBool) -> Result<(), String> {
    if !running.load(Ordering::SeqCst) {
        return Err("持久追溯线程已停止".into());
    }
    if let Some(error) = failure.lock().map_err(|_| "追溯状态锁损坏")?.as_ref() {
        return Err(error.clone());
    }
    Ok(())
}

fn readiness(
    spool: &spool::Spool,
    backlog: &Backlog,
    failure: &Mutex<Option<String>>,
    running: &AtomicBool,
    pending_recordings: &RecordingBarriers,
) -> Result<AuditReadiness, String> {
    check_health(failure, running)?;
    if !pending_recordings
        .lock()
        .map_err(|_| "录制收尾状态锁损坏")?
        .is_empty()
    {
        return Ok(AuditReadiness::WaitingRecording);
    }
    let backlog = backlog.lock().map_err(|_| "追溯暂存锁损坏")?;
    Ok(if spool.empty() && backlog.is_empty() {
        AuditReadiness::Ready
    } else {
        AuditReadiness::WaitingStore
    })
}

fn with_ready_spool<T>(
    spool: &Mutex<spool::Spool>,
    backlog: &Backlog,
    failure: &Mutex<Option<String>>,
    running: &AtomicBool,
    pending_recordings: &RecordingBarriers,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let spool = spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
    readiness(&spool, backlog, failure, running, pending_recordings)?.require_ready()?;
    let result = action();
    drop(spool);
    result
}

/// 落盘；落不了或内存里已有排队的事件时排到队尾（保持顺序），由追溯线程重试。
fn enqueue(
    spool: &mut spool::Spool,
    backlog: &mut VecDeque<Event>,
    event: Event,
    problems: &mut Vec<Problem>,
) {
    if backlog.is_empty() {
        match spool.append(&event) {
            Ok(()) => return,
            Err(error) => problems.push(Problem {
                cycle: event.cycle_id().into(),
                message: format!("{error}；事件暂存内存，后台重试落盘"),
                current: true,
            }),
        }
    }
    if backlog.len() >= BACKLOG_LIMIT {
        if let Some(dropped) = backlog.pop_front() {
            problems.push(Problem {
                cycle: dropped.cycle_id().into(),
                message: format!("内存暂存已满 {BACKLOG_LIMIT} 条，丢弃最早一条未落盘的追溯事件"),
                current: true,
            });
        }
    }
    backlog.push_back(event);
}

fn persist_event(
    spool: &Mutex<spool::Spool>,
    backlog: &Backlog,
    pending_recordings: &RecordingBarriers,
    event: Event,
) -> Result<Vec<Problem>, String> {
    let mut spool = spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
    let mut backlog = backlog.lock().map_err(|_| "追溯暂存锁损坏")?;
    spool::validate(&event)?;
    let mut pending = pending_recordings
        .lock()
        .map_err(|_| "录制收尾状态锁损坏")?;
    let mut problems = Vec::new();
    if let Event::Recording(outcome) = &event {
        if let Some(barrier) = pending
            .get_mut(&outcome.cycle_id)
            .filter(|barrier| !barrier.inserted)
        {
            if barrier
                .terminal
                .as_ref()
                .is_some_and(|previous| previous != outcome)
            {
                problems.push(Problem {
                    cycle: outcome.cycle_id.clone(),
                    message: "主记录接受前出现另一份录制结果，保留首次结果".into(),
                    current: false,
                });
            } else {
                barrier.terminal = Some(outcome.clone());
            }
            return Ok(problems);
        }
    }
    let cycle = event.cycle_id().to_string();
    let inserted = matches!(event, Event::Insert(_));
    let recorded = matches!(event, Event::Recording(_));
    enqueue(&mut spool, &mut backlog, event, &mut problems);
    if inserted {
        if let Some(barrier) = pending.get_mut(&cycle) {
            barrier.inserted = true;
            if let Some(outcome) = barrier.terminal.take() {
                enqueue(
                    &mut spool,
                    &mut backlog,
                    Event::Recording(outcome),
                    &mut problems,
                );
                pending.remove(&cycle);
            }
        }
    } else if recorded {
        pending.remove(&cycle);
    }
    Ok(problems)
}

fn drain(spool: &mut spool::Spool, backlog: &Backlog) -> Result<(), String> {
    let mut backlog = backlog.lock().map_err(|_| "追溯暂存锁损坏")?;
    while let Some(event) = backlog.front() {
        spool
            .append(event)
            .map_err(|error| format!("内存暂存的追溯事件仍未落盘：{error}"))?;
        backlog.pop_front();
    }
    Ok(())
}

#[derive(Clone)]
pub struct Audit {
    tx: SyncSender<()>,
    app: AppHandle,
    spool: Arc<Mutex<spool::Spool>>,
    backlog: Arc<Backlog>,
    /// 当前问题（界面与告警用）：健康检查通过且内存暂存全部落盘后清除。
    failure: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
    pending_recordings: Arc<RecordingBarriers>,
}

impl Audit {
    pub fn new(app: &AppHandle, path: PathBuf) -> Result<Self, String> {
        let (mut spool, notes) = spool::Spool::open(path)?;
        for note in notes {
            app_log(app, "err", "持久追溯未恢复", None, note);
        }
        let notices = recover_all(&mut spool, &mut AppSink(app), ly_plc::now_ms());
        publish(app, notices);
        let failure = spool.probe().err();
        if let Some(error) = &failure {
            app_log(app, "err", "持久追溯故障", None, error.clone());
        }
        let (tx, rx) = sync_channel(1);
        let audit = Self {
            tx,
            app: app.clone(),
            spool: Arc::new(Mutex::new(spool)),
            backlog: Arc::new(Mutex::new(VecDeque::new())),
            failure: Arc::new(Mutex::new(failure)),
            running: Arc::new(AtomicBool::new(true)),
            pending_recordings: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let worker = audit.clone();
        std::thread::Builder::new()
            .name("inspection-audit".into())
            .spawn(move || writer(worker, rx))
            .map_err(|error| format!("追溯线程启动失败：{error}"))?;
        Ok(audit)
    }

    /// Err 是追溯当前问题，只用于界面显示和历史清理、项目切换这类非生产操作的守卫。
    pub(crate) fn readiness(&self) -> Result<AuditReadiness, String> {
        let spool = self.spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
        readiness(
            &spool,
            &self.backlog,
            &self.failure,
            &self.running,
            &self.pending_recordings,
        )
    }

    pub fn with_ready<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        with_ready_spool(
            &self.spool,
            &self.backlog,
            &self.failure,
            &self.running,
            &self.pending_recordings,
            action,
        )
    }

    pub fn begin_recording(&self, cycle_id: &str) {
        let result = self
            .pending_recordings
            .lock()
            .map_err(|_| "录制收尾状态锁损坏".to_string())
            .map(|mut pending| {
                if pending.contains_key(cycle_id) {
                    false
                } else {
                    pending.insert(cycle_id.into(), RecordingBarrier::default());
                    true
                }
            });
        match result {
            Ok(true) => (),
            Ok(false) => app_log(
                &self.app,
                "warn",
                "录制追溯屏障",
                Some(cycle_id),
                format!("cycleId={cycle_id} 的录制仍未收尾又再次开始，沿用首次屏障"),
            ),
            Err(error) => app_log(&self.app, "err", "录制追溯屏障", Some(cycle_id), error),
        }
    }

    pub fn protected_recording_directories(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        let spool = self.spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
        let backlog = self.backlog.lock().map_err(|_| "追溯暂存锁损坏")?;
        spool.protected_directories(root, &backlog)
    }

    pub fn protected_cycle_ids(&self) -> Result<BTreeSet<String>, String> {
        let spool = self.spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
        let mut cycles = spool.cycles();
        cycles.extend(
            self.backlog
                .lock()
                .map_err(|_| "追溯暂存锁损坏")?
                .iter()
                .map(|event| event.cycle_id().to_string()),
        );
        Ok(cycles)
    }

    /// 记为当前问题；与当前问题相同的不重复写日志。
    fn problem(&self, cycle: Option<&str>, error: String) {
        let repeated = self
            .failure
            .lock()
            .map(|mut failure| failure.replace(error.clone()).as_ref() == Some(&error))
            .unwrap_or(false);
        if !repeated {
            app_log(&self.app, "err", "持久追溯故障", cycle, error);
        }
    }

    /// 健康检查并把内存暂存落盘；两者都成功才清除当前问题。
    fn maintain(&self) {
        let Ok(mut spool) = self.spool.lock() else {
            return self.problem(None, "追溯 spool 锁损坏".into());
        };
        match spool
            .probe()
            .and_then(|()| drain(&mut spool, &self.backlog))
        {
            Ok(()) => {
                if let Ok(Some(previous)) = self.failure.lock().map(|mut failure| failure.take()) {
                    app_log(
                        &self.app,
                        "info",
                        "持久追溯恢复",
                        None,
                        format!("此前问题已恢复：{previous}"),
                    );
                }
            }
            Err(error) => self.problem(None, error),
        }
    }

    fn persist(&self, event: Event) {
        let cycle = event.cycle_id().to_string();
        match persist_event(&self.spool, &self.backlog, &self.pending_recordings, event) {
            Ok(problems) => {
                for problem in problems {
                    let message = format!(
                        "cycleId={}：{}；原检测结论不变",
                        problem.cycle, problem.message
                    );
                    if problem.current {
                        self.problem(Some(&problem.cycle), message);
                    } else {
                        app_log(
                            &self.app,
                            "err",
                            "持久追溯故障",
                            Some(&problem.cycle),
                            message,
                        );
                    }
                }
            }
            Err(error) => app_log(
                &self.app,
                "err",
                "持久追溯故障",
                Some(&cycle),
                format!("cycleId={cycle}：{error}；本条事件未保存，原检测结论不变"),
            ),
        }
        if matches!(self.tx.try_send(()), Err(TrySendError::Disconnected(()))) {
            self.problem(
                Some(&cycle),
                "追溯线程已停止，已落盘的事件在重启后入库".into(),
            );
        }
    }

    async fn persist_async(&self, event: Event, timeout: Duration) {
        let cycle = event.cycle_id().to_string();
        let audit = self.clone();
        let task = tauri::async_runtime::spawn_blocking(move || audit.persist(event));
        if timeout.is_zero() {
            app_log(
                &self.app,
                "warn",
                "持久追溯接收超时",
                Some(&cycle),
                format!("cycleId={cycle}：已无剩余处理预算，后台继续落盘"),
            );
            return;
        }
        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(())) => (),
            Ok(Err(error)) => self.problem(
                Some(&cycle),
                format!("cycleId={cycle}：持久追溯接收线程异常：{error}"),
            ),
            Err(_) => app_log(
                &self.app,
                "warn",
                "持久追溯接收超时",
                Some(&cycle),
                format!(
                    "cycleId={cycle}：超过 {} ms 未完成，后台继续落盘",
                    timeout.as_millis()
                ),
            ),
        }
    }

    pub async fn insert_durable(&self, part: RecordedPart, timeout: Duration) {
        self.persist_async(Event::Insert(Box::new(part)), timeout)
            .await
    }

    pub async fn delivery_durable(&self, cycle_id: &str, delivery: PlcDelivery) {
        self.persist_async(
            Event::Delivery(cycle_id.into(), delivery),
            Duration::from_secs(5),
        )
        .await
    }

    pub async fn submission_durable(
        &self,
        cycle_id: &str,
        delivery: PlcDelivery,
        drain_ms: Option<u64>,
    ) {
        self.persist_async(
            Event::Submission(cycle_id.into(), delivery, drain_ms),
            Duration::from_secs(5),
        )
        .await
    }

    pub fn insert(&self, part: RecordedPart) {
        self.persist(Event::Insert(Box::new(part)))
    }

    pub fn delivery(&self, cycle_id: &str, delivery: PlcDelivery) {
        self.persist(Event::Delivery(cycle_id.into(), delivery))
    }

    pub fn recording(&self, outcome: RecordingOutcome) {
        log_recording(&self.app, &outcome);
        self.persist(Event::Recording(outcome))
    }
}

struct RecordingRecord {
    level: Option<&'static str>,
    event: &'static str,
    message: String,
    fields: applog::Fields,
}

/// 录制结果常在下一件开工后才到，按本件 cycleId 显式记一条带结构化状态的结果，迟到也进本件时间线。
fn log_recording(app: &AppHandle, outcome: &RecordingOutcome) {
    for record in recording_records(outcome) {
        app_record(app, record.level, record.event, record.message, record.fields);
    }
}

fn recording_records(outcome: &RecordingOutcome) -> Vec<RecordingRecord> {
    let mut records = Vec::new();
    let own = || applog::Fields { cycle_id: Some(outcome.cycle_id.clone()), ..applog::Fields::default() };
    if !outcome.retention_errors.is_empty() {
        records.push(RecordingRecord { level: Some("warn"), event: "录制保留清理失败", message: format!("cycleId={}：{}", outcome.cycle_id, outcome.retention_errors.join("；")), fields: own() });
    }
    if outcome.state == RecorderState::Off { return records; }
    let failed = !outcome.errors.is_empty() || matches!(outcome.state, RecorderState::Failed | RecorderState::Incomplete)
        || (outcome.available && outcome.files.is_empty());
    let directory = outcome.directory.as_ref().map(|path| path.display().to_string());
    let fields = applog::Fields {
        stage: Some("recording".into()),
        data: Some(serde_json::json!({
            "state": outcome.state, "available": outcome.available, "files": outcome.files.len(),
            "directory": directory, "errors": outcome.errors,
        })),
        ..own()
    };
    records.push(if failed {
        let message = format!("cycleId={}：状态 {:?}，目录 {}，实际原图 {} 张，可用={}，{}；原检测结论不变", outcome.cycle_id, outcome.state,
            directory.as_deref().unwrap_or("无目录"), outcome.files.len(), outcome.available, outcome.errors.join("；"));
        RecordingRecord { level: Some("err"), event: "原图录制未完成", message, fields }
    } else {
        let message = match outcome.state {
            RecorderState::NotRetained => format!("cycleId={}：按录制策略不保留本件原图", outcome.cycle_id),
            _ => format!("cycleId={}：{} 张原图已核验落盘", outcome.cycle_id, outcome.files.len()),
        };
        RecordingRecord { level: None, event: "原图录制完成", message, fields }
    });
    records
}

enum InsertResult {
    Inserted(i64),
    Existing(Box<PartDetail>),
}

trait Sink {
    fn find(&mut self, cycle: &str) -> Result<Option<PartDetail>, String>;
    fn insert(&mut self, part: &RecordedPart) -> Result<InsertResult, String>;
    fn matches_record(&mut self, part: &RecordedPart) -> Result<bool, String>;
    fn delivery(&mut self, cycle: &str, delivery: &PlcDelivery) -> Result<bool, String>;
    fn submission_timing(&mut self, cycle: &str, drain_ms: u64) -> Result<bool, String>;
    /// 原图引用，按 (camera, seq) 一批写入
    fn frame_files(&mut self, cycle: &str, files: &BTreeMap<FrameKey, String>) -> Result<bool, String>;
    fn recording(&mut self, cycle: &str, evidence: &RecordingEvidence) -> Result<bool, String>;
}

fn insert_store(store: &Store, part: &RecordedPart) -> Result<InsertResult, String> {
    match store.insert(&part.borrowed()) {
        Ok(id) => Ok(InsertResult::Inserted(id)),
        Err(error) => match store.detail_by_cycle(&part.cycle_id) {
            Ok(Some(existing)) => Ok(InsertResult::Existing(Box::new(existing))),
            Ok(None) => Err(error),
            Err(lookup) => Err(format!("{error}；按 cycleId 核对原记录失败：{lookup}")),
        },
    }
}

struct AppSink<'a>(&'a AppHandle);

impl Sink for AppSink<'_> {
    fn find(&mut self, cycle: &str) -> Result<Option<PartDetail>, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.detail_by_cycle(cycle)
    }

    fn insert(&mut self, part: &RecordedPart) -> Result<InsertResult, String> {
        let store = self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?;
        insert_store(&store, part)
    }

    fn delivery(&mut self, cycle: &str, delivery: &PlcDelivery) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.update_delivery(cycle, delivery)
    }

    fn matches_record(&mut self, part: &RecordedPart) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.matches_record(&part.borrowed())
    }

    fn submission_timing(&mut self, cycle: &str, drain_ms: u64) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.update_submission_timing(cycle, drain_ms)
    }

    fn frame_files(&mut self, cycle: &str, files: &BTreeMap<FrameKey, String>) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.update_frame_raw_files(cycle, files)
    }

    fn recording(&mut self, cycle: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.update_recording(cycle, evidence)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Inserted(i64),
    Updated(String),
    Log { level: &'static str, event: &'static str, cycle: String, message: String },
}

fn log(notices: &mut Vec<Notice>, level: &'static str, event: &'static str, cycle: &str, message: impl AsRef<str>) {
    notices.push(Notice::Log { level, event, cycle: cycle.into(), message: format!("cycleId={cycle}：{}；原检测结论不变", message.as_ref()) });
}

#[derive(Default)]
struct CycleEntry {
    inserted: bool,
    record: Option<Box<RecordedPart>>,
    delivery: Option<PlcDelivery>,
    saved_delivery: Option<PlcDelivery>,
    drain_ms: Option<u64>,
    saved_drain_ms: Option<u64>,
    /// 每帧原图引用（录制根内相对路径）
    frame_files: BTreeMap<FrameKey, String>,
    saved_frame_files: BTreeMap<FrameKey, String>,
    frame_keys: Option<BTreeSet<FrameKey>>,
    last_recording: Option<RecordingOutcome>,
    recording: Option<RecordingEvidence>,
    saved_recording: Option<RecordingEvidence>,
    touched_at: i64,
    failures: u32,
    retry_at: i64,
}

impl CycleEntry {
    fn pending(&self) -> bool {
        self.record.is_some()
            || self.delivery != self.saved_delivery
            || self.drain_ms != self.saved_drain_ms
            || self.frame_files != self.saved_frame_files
            || self.recording != self.saved_recording
    }

    fn defer_retry(&mut self, now: i64) {
        self.failures = self.failures.saturating_add(1);
        let delay = (RETRY_BASE_MS << self.failures.saturating_sub(1).min(5)).min(RETRY_MAX_MS);
        self.retry_at = now.saturating_add(delay);
    }
}

struct Coordinator {
    entries: HashMap<String, CycleEntry>,
    order: VecDeque<String>,
    pending_limit: usize,
    cache_limit: usize,
}

impl Coordinator {
    fn new(pending_limit: usize, cache_limit: usize) -> Self {
        assert!(pending_limit > 0 && cache_limit >= pending_limit);
        Self { entries: HashMap::new(), order: VecDeque::new(), pending_limit, cache_limit }
    }

    fn handle(&mut self, event: Event, sink: &mut impl Sink, now: i64) -> Vec<Notice> {
        let cycle = event.cycle_id().to_string();
        let mut notices = Vec::new();
        if cycle.is_empty() || cycle.len() > 128 {
            log(&mut notices, "err", "追溯身份无效", &cycle, "cycleId 长度必须为 1–128");
            return notices;
        }
        let mut entry = match self.entries.remove(&cycle) {
            Some(entry) => entry,
            None => {
                self.order.push_back(cycle.clone());
                let mut entry = CycleEntry::default();
                match sink.find(&cycle) {
                    Ok(Some(existing)) => restore_existing(&cycle, &mut entry, existing, &mut notices),
                    Ok(None) => (),
                    Err(error) => log(&mut notices, "err", "追溯原记录读取失败", &cycle, error),
                }
                entry
            }
        };
        entry.touched_at = now;
        let changed = match event {
            Event::Insert(part) => {
                if !entry.inserted {
                    if entry.record.is_none() {
                        entry.record = Some(part);
                    }
                    true
                } else {
                    false
                }
            }
            Event::Delivery(_, delivery) => merge_delivery(&cycle, &mut entry.delivery, delivery, &mut notices),
            Event::Submission(_, delivery, drain_ms) => {
                let delivery_changed = merge_delivery(&cycle, &mut entry.delivery, delivery, &mut notices);
                merge_timing(&cycle, &mut entry.drain_ms, drain_ms, &mut notices) || delivery_changed
            },
            Event::Recording(outcome) => merge_recording(&cycle, &mut entry, outcome, &mut notices),
        };
        if changed {
            flush(&cycle, &mut entry, sink, now, &mut notices);
        }
        self.entries.insert(cycle, entry);
        self.trim(now, &mut notices);
        notices
    }

    fn trim(&mut self, now: i64, notices: &mut Vec<Notice>) {
        let expired = self
            .order
            .iter()
            .filter(|cycle| {
                let entry = &self.entries[*cycle];
                entry.pending() && now.saturating_sub(entry.touched_at) >= PENDING_TTL_MS
            })
            .cloned()
            .collect::<Vec<_>>();
        for cycle in expired {
            self.evict(&cycle, "等待检测记录或磁盘恢复已超过 30 分钟", notices);
        }
        while self.entries.values().filter(|entry| entry.pending()).count() > self.pending_limit {
            let cycle = self.order.iter().find(|cycle| self.entries[*cycle].pending()).unwrap().clone();
            self.evict(&cycle, &format!("待写 cycle 数量达到上限 {}", self.pending_limit), notices);
        }
        while self.entries.len() > self.cache_limit {
            let cycle = self.order.iter().find(|cycle| !self.entries[*cycle].pending()).or_else(|| self.order.front()).unwrap().clone();
            self.evict(&cycle, "追溯缓存达到上限", notices);
        }
    }

    fn evict(&mut self, cycle: &str, reason: &str, notices: &mut Vec<Notice>) {
        let entry = self.entries.remove(cycle).unwrap();
        self.order.retain(|key| key != cycle);
        if entry.pending() {
            let verdict =
                entry.record.as_ref().map(|part| format!("，待入库原判定 {:?}：{}", part.judgement.verdict, part.judgement.reason)).unwrap_or_default();
            log(
                notices,
                "err",
                "追溯待写数据淘汰",
                cycle,
                format!(
                    "{reason}，交付待写={}，录制状态待写={}，帧原图数={}，已停止自动重试{verdict}",
                    entry.delivery != entry.saved_delivery,
                    entry.recording != entry.saved_recording,
                    entry.frame_files.len()
                ),
            );
        }
    }
}

fn merge_delivery(cycle: &str, current: &mut Option<PlcDelivery>, mut incoming: PlcDelivery, notices: &mut Vec<Notice>) -> bool {
    if let Some(previous) = current.as_ref() {
        if previous.state == PlcDeliveryState::Acknowledged && incoming.state != PlcDeliveryState::Acknowledged {
            return false;
        }
        if (previous.state == PlcDeliveryState::NotRequired) != (incoming.state == PlcDeliveryState::NotRequired) {
            log(notices, "err", "PLC 交付身份冲突", cycle, "无需交付与已有 PLC 事务不能相互替换");
            return false;
        }
        if matches!(previous.state, PlcDeliveryState::Submitted | PlcDeliveryState::Failed) && incoming.state == PlcDeliveryState::Pending {
            return false;
        }
        if incoming.state == PlcDeliveryState::Acknowledged && previous.state != PlcDeliveryState::Acknowledged {
            incoming.updated_at = incoming.updated_at.max(previous.updated_at);
        } else if incoming.updated_at < previous.updated_at {
            return false;
        }
        if previous == &incoming {
            return false;
        }
    }
    *current = Some(incoming);
    true
}

fn merge_timing(cycle: &str, current: &mut Option<u64>, incoming: Option<u64>, notices: &mut Vec<Notice>) -> bool {
    let Some(incoming) = incoming else { return false };
    match current {
        Some(previous) if *previous != incoming => {
            log(notices, "err", "PLC 提交耗时身份冲突", cycle, "已有不同的实际提交耗时，保留首次观测");
            false
        },
        Some(_) => false,
        None => { *current = Some(incoming); true },
    }
}

/// 原图引用的身份校验：按 (camera, seq) 归属，seq 从 1 起。
fn raw_file(file: RecordedRawFile) -> Result<(FrameKey, String), String> {
    let key = file.frame;
    if key.seq == 0 || file.width == 0 || file.height == 0 {
        return Err(format!(
            "原图 相机 {} 第 {} 帧的帧号或尺寸无效",
            key.camera, key.seq
        ));
    }
    if file.file.is_empty() || file.file.contains(['\\', ':', '\0']) || file.file.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err("原图文件不是录制根目录内的相对路径".into());
    }
    Ok((key, file.file))
}

/// 一帧的原图引用：只增不换，原记录没有这一帧时不归属。
fn merge_frame_file(cycle: &str, entry: &mut CycleEntry, key: FrameKey, incoming: String, notices: &mut Vec<Notice>) -> bool {
    if entry.frame_keys.as_ref().is_some_and(|keys| !keys.contains(&key)) {
        log(notices, "err", "原图归属失败", cycle, format!("原记录没有相机 {} 第 {} 帧", key.camera, key.seq));
        return false;
    }
    match entry.frame_files.get(&key) {
        Some(previous) if *previous == incoming => false,
        Some(_) => {
            log(notices, "err", "原图身份冲突", cycle, format!("相机 {} 第 {} 帧已有关联文件，保留首次原图证据", key.camera, key.seq));
            false
        }
        None => {
            entry.frame_files.insert(key, incoming);
            true
        }
    }
}

fn merge_recording(cycle: &str, entry: &mut CycleEntry, outcome: RecordingOutcome, notices: &mut Vec<Notice>) -> bool {
    if entry.last_recording.as_ref() == Some(&outcome) {
        return false;
    }
    entry.last_recording = Some(outcome.clone());
    let mut changed = false;
    let mut errors = outcome.errors;
    let had_files = !outcome.files.is_empty();
    for file in outcome.files {
        match raw_file(file) {
            Ok((key, file)) => changed |= merge_frame_file(cycle, entry, key, file, notices),
            Err(error) => {
                log(notices, "err", "原图引用无效", cycle, &error);
                errors.push(error);
            }
        }
    }
    let mut state = match outcome.state {
        RecorderState::Off => RecordingState::Off,
        RecorderState::NotRetained => RecordingState::NotRetained,
        RecorderState::Complete => RecordingState::Complete,
        RecorderState::Incomplete => RecordingState::Incomplete,
        RecorderState::Failed => RecordingState::Failed,
    };
    let available = state == RecordingState::Complete && outcome.available && had_files && errors.is_empty() && outcome.directory.is_some();
    if state == RecordingState::Complete && !available {
        state = if had_files { RecordingState::Incomplete } else { RecordingState::Failed };
        errors.push("录制结果缺少完整落盘证据，原图不可用".into());
    }
    let evidence = RecordingEvidence { state, available, directory: outcome.directory.map(|path| path.to_string_lossy().into_owned()), errors };
    changed |= merge_evidence(cycle, &mut entry.recording, evidence, notices);
    changed
}

fn merge_evidence(cycle: &str, current: &mut Option<RecordingEvidence>, evidence: RecordingEvidence, notices: &mut Vec<Notice>) -> bool {
    match current.as_ref() {
        Some(previous) if previous == &evidence => false,
        Some(previous) if previous.state != RecordingState::Pending => {
            log(notices, "err", "录制收尾证据冲突", cycle, "保留首次收尾状态、可用性和错误，不替换为另一份结果");
            false
        }
        _ => {
            *current = Some(evidence);
            true
        }
    }
}

fn restore_existing(cycle: &str, entry: &mut CycleEntry, existing: PartDetail, notices: &mut Vec<Notice>) {
    let frames = existing.burst.map(|burst| burst.frames).unwrap_or_default();
    restore(
        cycle,
        entry,
        existing.summary.delivery,
        frames,
        existing.recording,
        existing.summary.drain_ms,
        notices,
    );
}

fn restore(
    cycle: &str,
    entry: &mut CycleEntry,
    delivery: PlcDelivery,
    frames: Vec<FrameRecord>,
    recording: RecordingEvidence,
    drain_ms: Option<u64>,
    notices: &mut Vec<Notice>,
) {
    entry.inserted = true;
    let pending_timing = entry.drain_ms.take();
    entry.drain_ms = drain_ms;
    entry.saved_drain_ms = drain_ms;
    merge_timing(cycle, &mut entry.drain_ms, pending_timing, notices);
    entry.record = None;
    entry.frame_keys = Some(frames.iter().map(|frame| FrameKey { camera: frame.camera, seq: frame.seq }).collect());
    let pending_delivery = entry.delivery.take();
    entry.saved_delivery = Some(delivery.clone());
    entry.delivery = Some(delivery);
    if let Some(delivery) = pending_delivery {
        merge_delivery(cycle, &mut entry.delivery, delivery, notices);
    }
    let pending_recording = entry.recording.take();
    entry.saved_recording = Some(recording.clone());
    entry.recording = Some(recording);
    if let Some(recording) = pending_recording {
        merge_evidence(cycle, &mut entry.recording, recording, notices);
    }
    let pending = std::mem::take(&mut entry.frame_files);
    entry.saved_frame_files = frames.into_iter()
        .filter_map(|frame| frame.raw_file.map(|file| (FrameKey { camera: frame.camera, seq: frame.seq }, file)))
        .collect();
    entry.frame_files = entry.saved_frame_files.clone();
    for (key, file) in pending {
        merge_frame_file(cycle, entry, key, file, notices);
    }
}

fn flush(cycle: &str, entry: &mut CycleEntry, sink: &mut impl Sink, now: i64, notices: &mut Vec<Notice>) {
    if now < entry.retry_at {
        return;
    }
    if let Some(part) = entry.record.as_ref() {
        match sink.insert(part) {
            Ok(InsertResult::Inserted(id)) => {
                let part = entry.record.take().unwrap();
                let frames = part.burst.map(|burst| burst.frames).unwrap_or_default();
                restore(
                    cycle,
                    entry,
                    part.delivery,
                    frames,
                    RecordingEvidence::default(),
                    part.drain_ms,
                    notices,
                );
                notices.push(Notice::Inserted(id));
            }
            Ok(InsertResult::Existing(existing)) => {
                restore_existing(cycle, entry, *existing, notices);
                log(notices, "info", "重复追溯入库", cycle, "数据库已有相同 cycleId，保留原记录及原图证据");
            }
            Err(error) => {
                let verdict = part.judgement.verdict;
                entry.defer_retry(now);
                log(
                    notices,
                    "err",
                    "追溯入库失败",
                    cycle,
                    format!("{error}；保留待写原判定 {verdict:?}，写入失败 {} 次，将退避重试", entry.failures),
                );
                return;
            }
        }
    }
    let mut changed = false;
    let mut failed = false;
    if entry.delivery != entry.saved_delivery {
        if let Some(delivery) = entry.delivery.as_ref() {
            match sink.delivery(cycle, delivery) {
                Ok(true) => {
                    entry.saved_delivery = Some(delivery.clone());
                    entry.inserted = true;
                    changed = true;
                }
                Ok(false) if entry.inserted => {
                    failed = true;
                    log(notices, "err", "PLC 交付追溯未写入", cycle, "原记录不存在或数据库已有更新的交付状态，保留待写交付事件");
                }
                Ok(false) => (),
                Err(error) => {
                    failed = true;
                    log(notices, "err", "PLC 交付追溯失败", cycle, error);
                }
            }
        }
    }
    if entry.drain_ms != entry.saved_drain_ms {
        if let Some(drain_ms) = entry.drain_ms {
            match sink.submission_timing(cycle, drain_ms) {
                Ok(true) => { entry.saved_drain_ms = Some(drain_ms); changed = true; },
                Ok(false) if entry.inserted => { failed = true; log(notices, "err", "PLC 提交耗时未写入", cycle, "原记录不存在，保留实际耗时事件"); },
                Ok(false) => (),
                Err(error) => { failed = true; log(notices, "err", "PLC 提交耗时写入失败", cycle, error); },
            }
        }
    }
    let frame_files: BTreeMap<FrameKey, String> = entry.frame_files.iter()
        .filter(|(key, file)| entry.saved_frame_files.get(*key) != Some(*file))
        .map(|(key, file)| (*key, file.clone()))
        .collect();
    if !frame_files.is_empty() {
        match sink.frame_files(cycle, &frame_files) {
            Ok(true) => {
                entry.saved_frame_files.extend(frame_files);
                entry.inserted = true;
                changed = true;
            }
            Ok(false) if entry.inserted => {
                failed = true;
                log(notices, "err", "原图追溯未写入", cycle, format!("原记录或其中的相机帧不存在，保留待写的 {} 帧原图引用", frame_files.len()));
            }
            Ok(false) => (),
            Err(error) => {
                failed = true;
                log(notices, "err", "原图追溯写入失败", cycle, error);
            }
        }
    }
    if entry.recording != entry.saved_recording {
        if let Some(evidence) = entry.recording.as_ref() {
            if !evidence.available || entry.frame_files == entry.saved_frame_files {
                match sink.recording(cycle, evidence) {
                    Ok(true) => {
                        entry.saved_recording = Some(evidence.clone());
                        entry.inserted = true;
                        changed = true;
                    }
                    Ok(false) if entry.inserted => {
                        failed = true;
                        log(notices, "err", "录制状态未写入", cycle, "原检测记录不存在，保留待写录制证据");
                    }
                    Ok(false) => (),
                    Err(error) => {
                        failed = true;
                        log(notices, "err", "录制状态写入失败", cycle, error);
                    }
                }
            }
        }
    }
    if changed {
        notices.push(Notice::Updated(cycle.into()));
    }
    if failed || (entry.pending() && entry.failures > 0) {
        entry.defer_retry(now);
    } else if !entry.pending() {
        entry.failures = 0;
        entry.retry_at = 0;
    }
}

fn publish(app: &AppHandle, notices: Vec<Notice>) {
    for notice in notices {
        match notice {
            Notice::Inserted(id) => {
                if let Err(error) = app.emit("history://inserted", id) {
                    app_log(app, "err", "历史通知失败", None, error.to_string());
                }
            }
            Notice::Updated(cycle) => {
                if let Err(error) = app.emit("history://updated", &cycle) {
                    app_log(app, "err", "历史通知失败", Some(&cycle), error.to_string());
                }
            }
            Notice::Log { level, event, cycle, message } => app_log(app, level, event, Some(&cycle), message),
        }
    }
}

/// 审计线程常在下一件已开始后才回报上一件的结果，属于某件的记录必须显式带上它的 cycleId，不能按在途工件补齐。
fn app_log(app: &AppHandle, level: &'static str, event: &'static str, cycle: Option<&str>, message: impl Into<String>) {
    app_record(app, Some(level), event, message.into(), applog::Fields { cycle_id: cycle.map(Into::into), ..applog::Fields::default() });
}

/// level 为 None 时是明细：界面事件流照常显示，日志文件记为 debug。
fn app_record(app: &AppHandle, level: Option<&'static str>, event: &'static str, message: String, fields: applog::Fields) {
    match (app.try_state::<cycle::CycleHost>().is_some(), level) {
        (true, Some(level)) => cycle::log_with(app, level, event, message, fields),
        (true, None) => cycle::trace(app, event, message, fields),
        (false, level) => { applog::emit(level.map_or(applog::Level::Debug, applog::Level::from_tone), event, &message, fields); }
    }
}

fn immutable_record(part: &RecordedPart) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(part).map_err(|error| error.to_string())?;
    let fields = value.as_object_mut().ok_or("检测记录结构无效")?;
    fields.remove("delivery");
    fields.remove("drain_ms");
    // 原图引用是事后补的证据；其余帧、区域、标记都属于不可变检测内容
    if let Some(frames) = fields.get_mut("burst").and_then(|burst| burst.get_mut("frames")).and_then(serde_json::Value::as_array_mut) {
        for frame in frames { if let Some(fields) = frame.as_object_mut() { fields.remove("rawFile"); } }
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Replay {
    Complete,
    /// 还没入库完（数据库暂不可用、原记录未到等），按退避重试
    Pending,
    /// 与自身或数据库已有记录的身份、结构冲突，重试也不会好
    Conflict,
}

fn apply_spooled(
    events: Vec<(u64, Event)>,
    cycle: &str,
    sink: &mut impl Sink,
    now: i64,
) -> (Replay, Vec<u64>, Vec<Notice>) {
    let mut coordinator = Coordinator::new(1, 1);
    let mut notices = Vec::new();
    let receipts = events
        .iter()
        .map(|(sequence, _)| *sequence)
        .collect::<Vec<_>>();
    let mut original = None;
    for (_, event) in &events {
        if let Event::Insert(part) = event {
            let value = match immutable_record(part) {
                Ok(value) => value,
                Err(error) => {
                    log(&mut notices, "err", "持久检测记录结构错误", cycle, error);
                    return (Replay::Conflict, receipts, notices);
                }
            };
            if original.as_ref().is_some_and(|previous| *previous != value) {
                log(
                    &mut notices,
                    "err",
                    "持久检测记录身份冲突",
                    cycle,
                    "同 cycleId 对应不同不可变检测内容",
                );
                return (Replay::Conflict, receipts, notices);
            }
            original = Some(value);
            match sink.find(cycle) {
                Ok(Some(_)) => match sink.matches_record(part) {
                    Ok(true) => (),
                    Ok(false) => {
                        log(
                            &mut notices,
                            "err",
                            "持久检测记录身份冲突",
                            cycle,
                            "数据库原检测内容不同，保留原记录",
                        );
                        return (Replay::Conflict, receipts, notices);
                    }
                    Err(error) => {
                        log(&mut notices, "err", "持久检测记录核对失败", cycle, error);
                        return (Replay::Pending, receipts, notices);
                    }
                },
                Ok(None) => (),
                Err(error) => {
                    log(&mut notices, "err", "持久检测记录核对失败", cycle, error);
                    return (Replay::Pending, receipts, notices);
                }
            }
        }
    }
    for (_, event) in &events {
        notices.extend(coordinator.handle(event.clone(), sink, now));
    }
    let conflict = notices.iter().any(|notice| matches!(notice, Notice::Log { event, .. }
        if matches!(*event, "追溯身份无效" | "PLC 交付身份冲突" | "原图归属失败" | "原图身份冲突" | "原图引用无效" | "录制收尾证据冲突" | "PLC 提交耗时身份冲突")));
    if conflict {
        return (Replay::Conflict, receipts, notices);
    }
    if !coordinator
        .entries
        .get(cycle)
        .is_some_and(|entry| entry.inserted && !entry.pending())
    {
        return (Replay::Pending, receipts, notices);
    }
    let mut replay = Replay::Complete;
    for (_, event) in &events {
        if let Event::Insert(part) = event {
            match sink.matches_record(part) {
                Ok(true) => (),
                Ok(false) => {
                    replay = Replay::Conflict;
                    log(
                        &mut notices,
                        "err",
                        "持久检测记录身份冲突",
                        cycle,
                        "入库后实际检测内容不符",
                    );
                }
                Err(error) => {
                    if replay == Replay::Complete {
                        replay = Replay::Pending;
                    }
                    log(&mut notices, "err", "持久检测记录核对失败", cycle, error);
                }
            }
        }
    }
    (replay, receipts, notices)
}

fn recover_cycle(spool: &mut spool::Spool, cycle: &str, sink: &mut impl Sink, now: i64) -> Result<(bool, Vec<Notice>), String> {
    let events = spool.events(cycle)?;
    if !events.is_empty() && events.iter().all(|(_, event)| matches!(event, Event::Recording(_))) && sink.find(cycle)?.is_none() {
        let Event::Recording(first) = &events[0].1 else { unreachable!() };
        if events.iter().any(|(_, event)| !matches!(event, Event::Recording(outcome) if outcome == first)) {
            return Err("主记录前录制结果冲突".into());
        }
        let mut files = BTreeSet::new();
        if first.available != (first.state == RecorderState::Complete)
            || (first.available
                && (first.directory.is_none()
                    || first.files.is_empty()
                    || !first.errors.is_empty()))
            || (matches!(first.state, RecorderState::Off | RecorderState::NotRetained)
                && (first.directory.is_some() || !first.files.is_empty()))
            || first.files.iter().any(|file| !files.insert(file.frame))
        {
            return Err("主记录前录制结构不一致".into());
        }
        spool.archive_preinsert(&events.iter().map(|(receipt, _)| *receipt).collect::<Vec<_>>())?;
        let mut notices = Vec::new();
        log(&mut notices, "warn", "中断工件未生成最终记录", cycle, "旧版主记录前录制事件已耐久保留至 .interrupted-preinsert；原图保留，未伪造检测结论");
        return Ok((true, notices));
    }
    replay_cycle(spool, cycle, sink, now)
}

/// 启动恢复：恢复不了的件移到 spool 的未恢复目录并记错误，不阻止启动与接件；移不走的留在 spool 由追溯线程重试。
fn recover_all(spool: &mut spool::Spool, sink: &mut impl Sink, now: i64) -> Vec<Notice> {
    let mut notices = Vec::new();
    for cycle in spool.cycles() {
        let reason = match recover_cycle(spool, &cycle, sink, now) {
            Ok((true, cycle_notices)) => {
                notices.extend(cycle_notices);
                continue;
            }
            Ok((false, cycle_notices)) => {
                notices.extend(cycle_notices);
                "待写事件无法入库".to_string()
            }
            Err(error) => error,
        };
        quarantine(spool, &cycle, &reason, &mut notices);
    }
    notices
}

fn quarantine(
    spool: &mut spool::Spool,
    cycle: &str,
    reason: &str,
    notices: &mut Vec<Notice>,
) -> bool {
    match spool.quarantine(cycle) {
        Ok(directory) => {
            log(
                notices,
                "err",
                "持久追溯未恢复",
                cycle,
                format!("{reason}；事件已移到 {}，待人工核对", directory.display()),
            );
            true
        }
        Err(error) => {
            log(
                notices,
                "err",
                "持久追溯未恢复",
                cycle,
                format!("{reason}；移到未恢复目录失败，稍后重试：{error}"),
            );
            false
        }
    }
}

fn replay_cycle(
    spool: &mut spool::Spool,
    cycle: &str,
    sink: &mut impl Sink,
    now: i64,
) -> Result<(bool, Vec<Notice>), String> {
    let (replay, receipts, notices) = apply_spooled(spool.events(cycle)?, cycle, sink, now);
    let complete = replay == Replay::Complete;
    if complete {
        spool.remove(&receipts)?;
    }
    Ok((complete, notices))
}

/// 追溯线程对一件的一次入库尝试。冲突的件、连续 QUARANTINE_ATTEMPTS 次没入库的件移到未恢复目录。
fn retry_cycle(
    spool: &Mutex<spool::Spool>,
    retries: &mut BTreeMap<String, (u32, i64)>,
    cycle: &str,
    sink: &mut impl Sink,
    now: i64,
) -> Vec<Notice> {
    let mut notices = Vec::new();
    if retries.get(cycle).is_some_and(|(_, due)| now < *due) {
        return notices;
    }
    let Ok(guard) = spool.lock() else {
        log(
            &mut notices,
            "err",
            "持久追溯故障",
            cycle,
            "追溯 spool 锁损坏",
        );
        return notices;
    };
    let events = guard.events(cycle);
    drop(guard);
    let conflict = match events {
        Ok(events) => {
            let (replay, receipts, applied) = apply_spooled(events, cycle, sink, now);
            notices.extend(applied);
            if replay == Replay::Complete {
                match spool
                    .lock()
                    .map_err(|_| "追溯 spool 锁损坏".to_string())
                    .and_then(|mut spool| spool.remove(&receipts))
                {
                    Ok(()) => {
                        retries.remove(cycle);
                        return notices;
                    }
                    Err(error) => log(&mut notices, "err", "持久追溯故障", cycle, error),
                }
            }
            replay == Replay::Conflict
        }
        Err(error) => {
            log(&mut notices, "err", "持久追溯读取失败", cycle, error);
            false
        }
    };
    let (failures, due) = retries.entry(cycle.into()).or_default();
    *failures = failures.saturating_add(1);
    *due =
        now.saturating_add((RETRY_BASE_MS << failures.saturating_sub(1).min(5)).min(RETRY_MAX_MS));
    let reason = if conflict {
        "待写事件与已有记录冲突".to_string()
    } else if *failures >= QUARANTINE_ATTEMPTS {
        format!("连续 {failures} 次未能入库")
    } else {
        return notices;
    };
    let moved = match spool.lock() {
        Ok(mut spool) => quarantine(&mut spool, cycle, &reason, &mut notices),
        Err(_) => {
            log(
                &mut notices,
                "err",
                "持久追溯故障",
                cycle,
                "追溯 spool 锁损坏",
            );
            false
        }
    };
    if moved {
        retries.remove(cycle);
    }
    notices
}

fn writer(audit: Audit, rx: Receiver<()>) {
    struct Running(Arc<AtomicBool>);
    impl Drop for Running { fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); } }
    let _running = Running(audit.running.clone());
    let mut sink = AppSink(&audit.app);
    let mut retries = BTreeMap::<String, (u32, i64)>::new();
    let mut last_probe = Instant::now();
    loop {
        let disconnected = matches!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Disconnected));
        if last_probe.elapsed() >= Duration::from_secs(1) {
            audit.maintain();
            last_probe = Instant::now();
        }
        let cycles = match audit.spool.lock() {
            Ok(spool) => spool.cycles(),
            Err(_) => {
                audit.problem(None, "追溯 spool 锁损坏".into());
                return;
            }
        };
        retries.retain(|cycle, _| cycles.contains(cycle));
        for cycle in cycles {
            let notices = retry_cycle(
                &audit.spool,
                &mut retries,
                &cycle,
                &mut sink,
                ly_plc::now_ms(),
            );
            publish(&audit.app, notices);
        }
        if disconnected {
            return;
        }
    }
}

#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;

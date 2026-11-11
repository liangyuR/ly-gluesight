use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

mod legacy;

use crate::applog;
use crate::cycle::{self, FrameView};
use crate::judge::{Judgement, PointState};
use crate::recipe::Recipe;
use crate::recorder::{RecordedRawFile, RecordingOutcome, RecordingState as RecorderState};
use crate::store::{
    BurstRecord, FrameKey, FrameRecord, PartDetail, PartMode, PartRecord, PartShot, PlcDelivery,
    PlcDeliveryState, RecordSource, RecordingEvidence, RecordingState, Store,
};

const PENDING_TTL_MS: i64 = 30 * 60 * 1000;
const RETRY_BASE_MS: i64 = 1000;
const RETRY_MAX_MS: i64 = 30_000;
/// 记录库写不进时内存里最多等这么多件，满了丢最早一件并记错误。
const PENDING_LIMIT: usize = 256;
/// 已写完的件留在内存里的件数（迟到的交付、录制事件合并用），未命中时从记录库读回。
const CACHE_LIMIT: usize = 16;

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
            burst: self.burst.as_ref(),
        }
    }
}

/// 序列化形式是旧版追溯暂存文件（format 4）的事件格式，启动补录按它读。
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
    /// 数值按位保存，NaN、负零原样往返。d_bits 为空即有胶、无位置基准。
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

#[derive(Default)]
struct RecordingBarrier {
    inserted: bool,
    terminal: Option<RecordingOutcome>,
}

type RecordingBarriers = Mutex<BTreeMap<String, RecordingBarrier>>;

/// 录制结果排在主记录之后：主记录到之前由屏障暂存，主记录到达后一起放行。
fn admit(barriers: &RecordingBarriers, event: Event, notices: &mut Vec<Notice>) -> Vec<Event> {
    let cycle = event.cycle_id().to_string();
    let Ok(mut pending) = barriers.lock() else {
        log(notices, "err", "持久追溯故障", &cycle, "录制收尾状态锁损坏");
        return vec![event];
    };
    if let Event::Recording(outcome) = &event {
        if let Some(barrier) = pending.get_mut(&cycle).filter(|barrier| !barrier.inserted) {
            if barrier
                .terminal
                .as_ref()
                .is_some_and(|previous| previous != outcome)
            {
                log(
                    notices,
                    "err",
                    "持久追溯故障",
                    &cycle,
                    "主记录接受前出现另一份录制结果，保留首次结果",
                );
            } else {
                barrier.terminal = Some(outcome.clone());
            }
            return Vec::new();
        }
    }
    let inserted = matches!(event, Event::Insert(_));
    let recorded = matches!(event, Event::Recording(_));
    let mut events = vec![event];
    if inserted {
        if let Some(barrier) = pending.get_mut(&cycle) {
            barrier.inserted = true;
            if let Some(outcome) = barrier.terminal.take() {
                events.push(Event::Recording(outcome));
                pending.remove(&cycle);
            }
        }
    } else if recorded {
        pending.remove(&cycle);
    }
    events
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuditReadiness {
    Ready,
    WaitingRecording,
    WaitingStore,
}

impl AuditReadiness {
    fn require_ready(self) -> Result<(), String> {
        match self {
            Self::Ready => Ok(()),
            Self::WaitingRecording => Err("原图录制尚未收尾".into()),
            Self::WaitingStore => Err("还有检测记录待写入记录库".into()),
        }
    }
}

/// 写库线程每处理完一批就更新：读就绪与原图保护时不用等正在写库的线程。
#[derive(Clone, Default)]
struct Pending {
    cycles: BTreeSet<String>,
    directories: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct Audit {
    /// 追溯事件按提交顺序交给写库线程，调用方（节拍）不等记录库
    queue: Sender<Vec<Event>>,
    /// 已提交、写库线程还没处理完的批次数
    queued: Arc<AtomicUsize>,
    report: Arc<dyn Report>,
    coordinator: Arc<Mutex<Coordinator>>,
    pending: Arc<Mutex<Pending>>,
    /// 旧版追溯暂存里没能补录的件，原图继续保护，待人工核对
    legacy: Arc<legacy::Leftovers>,
    /// 当前问题（界面与告警用），写库恢复后清除
    failure: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
    pending_recordings: Arc<RecordingBarriers>,
}

impl Audit {
    /// `legacy`：旧版本留下的追溯暂存目录，启动时补录一次。
    pub fn new(app: &AppHandle, legacy: PathBuf) -> Result<Self, String> {
        Self::start(
            AppSink(app.clone()),
            Arc::new(AppReport(app.clone())),
            legacy,
        )
    }

    fn start<S: Sink + Send + 'static>(
        mut sink: S,
        report: Arc<dyn Report>,
        legacy: PathBuf,
    ) -> Result<Self, String> {
        let (leftovers, notices, summary) = legacy::import(&legacy, &mut sink, ly_plc::now_ms());
        report.publish(notices);
        if let Some(summary) = summary {
            report.log("err", "旧追溯暂存未补录", None, summary);
        }
        let (queue, rx) = channel();
        let audit = Self {
            queue,
            queued: Arc::default(),
            report,
            coordinator: Arc::new(Mutex::new(Coordinator::new(PENDING_LIMIT, CACHE_LIMIT))),
            pending: Arc::default(),
            legacy: Arc::new(leftovers),
            failure: Arc::default(),
            running: Arc::new(AtomicBool::new(true)),
            pending_recordings: Arc::default(),
        };
        let worker = audit.clone();
        std::thread::Builder::new()
            .name("inspection-audit".into())
            .spawn(move || writer(worker, rx, sink))
            .map_err(|error| format!("追溯线程启动失败：{error}"))?;
        Ok(audit)
    }

    /// Err 是追溯当前问题，只用于界面显示和历史清理、项目切换这类非生产操作的守卫。
    pub(crate) fn readiness(&self) -> Result<AuditReadiness, String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("持久追溯线程已停止".into());
        }
        if let Some(error) = self.failure.lock().map_err(|_| "追溯状态锁损坏")?.as_ref() {
            return Err(error.clone());
        }
        if !self
            .pending_recordings
            .lock()
            .map_err(|_| "录制收尾状态锁损坏")?
            .is_empty()
        {
            return Ok(AuditReadiness::WaitingRecording);
        }
        let pending = !self
            .pending
            .lock()
            .map_err(|_| "追溯状态锁损坏")?
            .cycles
            .is_empty();
        Ok(if pending || self.queued.load(Ordering::SeqCst) > 0 {
            AuditReadiness::WaitingStore
        } else {
            AuditReadiness::Ready
        })
    }

    /// 没有待写记录时执行 `action`；执行期间写库线程暂停，不会有新记录插进来。
    pub fn with_ready<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let coordinator = self.coordinator.lock().map_err(|_| "追溯状态锁损坏")?;
        self.readiness()?.require_ready()?;
        let result = action();
        drop(coordinator);
        result
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
            Ok(false) => self.report.log(
                "warn",
                "录制追溯屏障",
                Some(cycle_id),
                format!("cycleId={cycle_id} 的录制仍未收尾又再次开始，沿用首次屏障"),
            ),
            Err(error) => self
                .report
                .log("err", "录制追溯屏障", Some(cycle_id), error),
        }
    }

    /// 还没写进记录库的件和旧追溯暂存里没补录的件，它们的录制目录不滚动清理。
    pub fn protected_recording_directories(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        if let Some(error) = &self.legacy.unreadable {
            return Err(format!("{error}，暂停清理"));
        }
        let mut pending = self.pending.lock().map_err(|_| "追溯状态锁损坏")?.clone();
        pending.cycles.extend(self.legacy.cycles.iter().cloned());
        pending
            .directories
            .extend(self.legacy.directories.iter().cloned());
        legacy::protected_directories(root, &pending.cycles, &pending.directories)
    }

    /// 还没写进记录库的件。
    pub fn protected_cycle_ids(&self) -> Result<BTreeSet<String>, String> {
        Ok(self
            .pending
            .lock()
            .map_err(|_| "追溯状态锁损坏")?
            .cycles
            .clone())
    }

    /// 已有追溯证据、记录库里却可能没有的件：待写的件，加上旧追溯暂存里没补录的件；后者不阻止接件与切换项目。
    pub fn evidenced_cycle_ids(&self) -> Result<BTreeSet<String>, String> {
        let mut cycles = self.protected_cycle_ids()?;
        cycles.extend(self.legacy.cycles.iter().cloned());
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
            self.report.log("err", "持久追溯故障", cycle, error);
        }
    }

    fn settle(&self, failing: bool) {
        if failing {
            return self.problem(
                None,
                "检测记录写不进记录库，后台退避重试；超过 30 分钟仍未写入的件丢弃并记错误".into(),
            );
        }
        if let Ok(Some(previous)) = self.failure.lock().map(|mut failure| failure.take()) {
            self.report.log(
                "info",
                "持久追溯恢复",
                None,
                format!("此前问题已恢复：{previous}"),
            );
        }
    }

    /// 交给写库线程按序处理，不阻塞调用方。
    fn persist(&self, events: Vec<Event>) {
        let Some(cycle) = events.first().map(|event| event.cycle_id().to_string()) else {
            return;
        };
        self.queued.fetch_add(1, Ordering::SeqCst);
        if self.queue.send(events).is_err() {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            self.problem(
                Some(&cycle),
                format!("cycleId={cycle}：追溯写库线程已停止，本条记录未写入；原检测结论不变"),
            );
        }
    }

    pub fn insert(&self, part: RecordedPart) {
        self.persist(vec![Event::Insert(Box::new(part))])
    }

    /// 主记录与（没建立检测的件的）录制结果一起交给写库线程，先主记录后录制。
    pub fn insert_with_recording(&self, part: RecordedPart, recording: Option<RecordingOutcome>) {
        let mut events = vec![Event::Insert(Box::new(part))];
        if let Some(outcome) = recording {
            self.report.recording(&outcome);
            events.push(Event::Recording(outcome));
        }
        self.persist(events)
    }

    pub fn delivery(&self, cycle_id: &str, delivery: PlcDelivery) {
        self.persist(vec![Event::Delivery(cycle_id.into(), delivery)])
    }

    pub fn submission(&self, cycle_id: &str, delivery: PlcDelivery, drain_ms: Option<u64>) {
        self.persist(vec![Event::Submission(cycle_id.into(), delivery, drain_ms)])
    }

    pub fn recording(&self, outcome: RecordingOutcome) {
        self.report.recording(&outcome);
        self.persist(vec![Event::Recording(outcome)])
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
    fn deleted(&mut self, cycle: &str) -> Result<bool, String>;
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

struct AppSink(AppHandle);

impl Sink for AppSink {
    fn deleted(&mut self, cycle: &str) -> Result<bool, String> {
        self.0
            .try_state::<Store>()
            .ok_or("检测记录数据库尚未初始化")?
            .history_cycle_deleted(cycle)
    }
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
        assert!(pending_limit > 0);
        Self { entries: HashMap::new(), order: VecDeque::new(), pending_limit, cache_limit }
    }

    /// 写入失败、到了退避时间的件再写一次。
    fn retry(&mut self, sink: &mut impl Sink, now: i64) -> Vec<Notice> {
        let mut notices = Vec::new();
        let due = self
            .order
            .iter()
            .filter(|cycle| {
                let entry = &self.entries[*cycle];
                entry.pending() && entry.failures > 0 && now >= entry.retry_at
            })
            .cloned()
            .collect::<Vec<_>>();
        for cycle in due {
            let mut entry = self.entries.remove(&cycle).unwrap();
            flush(&cycle, &mut entry, sink, now, &mut notices);
            self.entries.insert(cycle, entry);
        }
        self.trim(now, &mut notices);
        notices
    }

    /// 只有交付、录制结果而主记录还没到的件不算待写（主记录可能永远不来），只保护它的录制目录。
    fn snapshot(&self) -> Pending {
        let mut pending = Pending::default();
        for (cycle, entry) in self.entries.iter().filter(|(_, entry)| entry.pending()) {
            if entry.record.is_some() || entry.inserted {
                pending.cycles.insert(cycle.clone());
            }
            pending.directories.extend(
                entry
                    .last_recording
                    .as_ref()
                    .and_then(|outcome| outcome.directory.clone()),
            );
        }
        pending
    }

    fn failing(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.pending() && entry.failures > 0)
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
                let mut entry = CycleEntry::default();
                match sink.find(&cycle) {
                    Ok(Some(existing)) => restore_existing(&cycle, &mut entry, existing, &mut notices),
                    Ok(None) if !matches!(event, Event::Insert(_)) => match sink.deleted(&cycle) {
                        Ok(true) => {
                            log(
                                &mut notices,
                                "info",
                                "人工删除的历史件",
                                &cycle,
                                "忽略已删除工件的迟到追溯事件，不自动重建历史记录",
                            );
                            return notices;
                        }
                        Ok(false) => (),
                        Err(error) => {
                            log(&mut notices, "err", "人工删除记录核对失败", &cycle, error)
                        }
                    },
                    Ok(None) => (),
                    Err(error) => log(&mut notices, "err", "追溯原记录读取失败", &cycle, error),
                }
                self.order.push_back(cycle.clone());
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
        while self
            .entries
            .values()
            .filter(|entry| !entry.pending())
            .count()
            > self.cache_limit
        {
            let cycle = self
                .order
                .iter()
                .find(|cycle| !self.entries[*cycle].pending())
                .unwrap()
                .clone();
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

/// 追溯结果的去向：界面通知与日志。
trait Report: Send + Sync {
    fn publish(&self, notices: Vec<Notice>);
    fn log(&self, level: &'static str, event: &'static str, cycle: Option<&str>, message: String);
    fn recording(&self, outcome: &RecordingOutcome);
}

struct AppReport(AppHandle);

impl Report for AppReport {
    fn publish(&self, notices: Vec<Notice>) {
        publish(&self.0, notices)
    }

    fn log(&self, level: &'static str, event: &'static str, cycle: Option<&str>, message: String) {
        app_log(&self.0, level, event, cycle, message)
    }

    fn recording(&self, outcome: &RecordingOutcome) {
        log_recording(&self.0, outcome)
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

/// 一件旧追溯暂存事件补录进记录库。
fn apply_legacy(
    events: Vec<Event>,
    cycle: &str,
    sink: &mut impl Sink,
    now: i64,
) -> (Replay, Vec<Notice>) {
    let mut coordinator = Coordinator::new(1, 1);
    let mut notices = Vec::new();
    match sink.deleted(cycle) {
        Ok(true) => {
            log(
                &mut notices,
                "info",
                "人工删除的历史件",
                cycle,
                "忽略已删除工件的迟到追溯事件，不自动重建历史记录",
            );
            return (Replay::Complete, notices);
        }
        Ok(false) => (),
        Err(error) => {
            log(&mut notices, "err", "人工删除记录核对失败", cycle, error);
            return (Replay::Pending, notices);
        }
    }
    let mut original = None;
    for event in &events {
        if let Event::Insert(part) = event {
            let value = match immutable_record(part) {
                Ok(value) => value,
                Err(error) => {
                    log(&mut notices, "err", "持久检测记录结构错误", cycle, error);
                    return (Replay::Conflict, notices);
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
                return (Replay::Conflict, notices);
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
                        return (Replay::Conflict, notices);
                    }
                    Err(error) => {
                        log(&mut notices, "err", "持久检测记录核对失败", cycle, error);
                        return (Replay::Pending, notices);
                    }
                },
                Ok(None) => (),
                Err(error) => {
                    log(&mut notices, "err", "持久检测记录核对失败", cycle, error);
                    return (Replay::Pending, notices);
                }
            }
        }
    }
    for event in &events {
        notices.extend(coordinator.handle(event.clone(), sink, now));
    }
    let conflict = notices.iter().any(|notice| matches!(notice, Notice::Log { event, .. }
        if matches!(*event, "追溯身份无效" | "PLC 交付身份冲突" | "原图归属失败" | "原图身份冲突" | "原图引用无效" | "录制收尾证据冲突" | "PLC 提交耗时身份冲突")));
    if conflict {
        return (Replay::Conflict, notices);
    }
    if !coordinator
        .entries
        .get(cycle)
        .is_some_and(|entry| entry.inserted && !entry.pending())
    {
        return (Replay::Pending, notices);
    }
    let mut replay = Replay::Complete;
    for event in &events {
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
    (replay, notices)
}

fn writer(audit: Audit, rx: Receiver<Vec<Event>>, mut sink: impl Sink) {
    struct Running(Arc<AtomicBool>);
    impl Drop for Running { fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); } }
    let _running = Running(audit.running.clone());
    let mut last_retry = Instant::now();
    loop {
        let received = rx.recv_timeout(Duration::from_secs(1));
        let disconnected = matches!(received, Err(RecvTimeoutError::Disconnected));
        let events = received.ok();
        let batch = events.is_some();
        let retry = last_retry.elapsed() >= Duration::from_secs(1);
        if !batch && !retry {
            if disconnected {
                return;
            }
            continue;
        }
        let mut notices = Vec::new();
        let failing = {
            let Ok(mut coordinator) = audit.coordinator.lock() else {
                return audit.problem(None, "追溯状态锁损坏".into());
            };
            let now = ly_plc::now_ms();
            for event in events.into_iter().flatten() {
                for event in admit(&audit.pending_recordings, event, &mut notices) {
                    notices.extend(coordinator.handle(event, &mut sink, now));
                }
            }
            if retry {
                notices.extend(coordinator.retry(&mut sink, now));
                last_retry = Instant::now();
            }
            if let Ok(mut pending) = audit.pending.lock() {
                *pending = coordinator.snapshot();
            }
            coordinator.failing()
        };
        if batch {
            audit.queued.fetch_sub(1, Ordering::SeqCst);
        }
        audit.report.publish(notices);
        audit.settle(failing);
        if disconnected {
            return;
        }
    }
}

#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

mod spool;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::cycle::{self, FrameView};
use crate::judge::{Judgement, PointState};
use crate::recipe::Recipe;
use crate::recorder::{RecordedRawFile, RecordingOutcome, RecordingState as RecorderState};
use crate::store::{PartDetail, PartRecord, PartShot, PlcDelivery, PlcDeliveryState, RecordingEvidence, RecordingState, ShotRawFile, Store};

const PENDING_TTL_MS: i64 = 30 * 60 * 1000;
const RETRY_BASE_MS: i64 = 1000;
const RETRY_MAX_MS: i64 = 30_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedPart {
    pub ts: i64,
    pub sn: u32,
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
}

impl RecordedPart {
    fn borrowed(&self) -> PartRecord<'_> {
        PartRecord {
            ts: self.ts,
            sn: self.sn,
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
        Option::<Recipe>::deserialize(deserializer).map(|recipe| recipe.map(Arc::new))
    }
}

mod part_table {
    use super::*;
    #[derive(Serialize, Deserialize)]
    enum Point { Pending, Measured { d_bits: u32, w_bits: u32 }, Gap, Invalid }
    pub fn serialize<S: serde::Serializer>(table: &Option<Vec<PointState>>, serializer: S) -> Result<S::Ok, S::Error> {
        table.as_ref().map(|table| table.iter().map(|point| match point {
            PointState::Pending => Point::Pending,
            PointState::Measured { d, w } => Point::Measured { d_bits: d.to_bits(), w_bits: w.to_bits() },
            PointState::Gap => Point::Gap,
            PointState::Invalid => Point::Invalid,
        }).collect::<Vec<_>>()).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<PointState>>, D::Error> {
        Option::<Vec<Point>>::deserialize(deserializer).map(|table| table.map(|table| table.into_iter().map(|point| match point {
            Point::Pending => PointState::Pending,
            Point::Measured { d_bits, w_bits } => PointState::Measured { d: f32::from_bits(d_bits), w: f32::from_bits(w_bits) },
            Point::Gap => PointState::Gap,
            Point::Invalid => PointState::Invalid,
        }).collect()))
    }
}

fn set_failure(failure: &Mutex<Option<String>>, error: String) -> Result<(), String> {
    *failure.lock().map_err(|_| "追溯状态锁损坏")? = Some(error);
    Ok(())
}

fn spool_io<T>(spool: &Mutex<spool::Spool>, failure: &Mutex<Option<String>>,
    action: impl FnOnce(&mut spool::Spool) -> Result<T, String>,
) -> Result<T, String> {
    let mut spool = spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
    let result = action(&mut spool);
    if let Err(error) = &result { set_failure(failure, error.clone())?; }
    result
}

fn with_ready_spool<T>(spool: &Mutex<spool::Spool>, failure: &Mutex<Option<String>>, running: &AtomicBool, pending_recordings: &Mutex<BTreeSet<String>>,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let spool = spool.lock().map_err(|_| "追溯 spool 锁损坏")?;
    if !running.load(Ordering::SeqCst) { return Err("持久追溯线程已停止，禁止布防".into()); }
    if let Some(error) = failure.lock().map_err(|_| "追溯状态锁损坏")?.as_ref() { return Err(error.clone()); }
    if !pending_recordings.lock().map_err(|_| "录制收尾状态锁损坏")?.is_empty() {
        return Err("原图录制尚未完成耐久接受，禁止下一件布防或历史清理".into());
    }
    if !spool.empty() { return Err("持久追溯仍有待入库事件，禁止下一件布防或历史清理".into()); }
    let result = action();
    drop(spool);
    result
}

fn track_recording(spool: &Mutex<spool::Spool>, failure: &Mutex<Option<String>>, pending_recordings: &Mutex<BTreeSet<String>>, cycle_id: &str) -> Result<(), String> {
    spool_io(spool, failure, |_| {
        if !pending_recordings.lock().map_err(|_| "录制收尾状态锁损坏")?.insert(cycle_id.into()) {
            return Err(format!("cycleId={cycle_id} 的录制仍未完成，拒绝重复开始"));
        }
        Ok(())
    })
}

fn persist_event(spool: &Mutex<spool::Spool>, failure: &Mutex<Option<String>>, pending_recordings: &Mutex<BTreeSet<String>>, event: &Event) -> Result<(), String> {
    spool_io(spool, failure, |spool| {
        spool.append(event)?;
        if let Event::Recording(outcome) = event {
            pending_recordings.lock().map_err(|_| "录制收尾状态锁损坏")?.remove(&outcome.cycle_id);
        }
        Ok(())
    })
}

#[derive(Clone)]
pub struct Audit {
    tx: SyncSender<()>,
    app: AppHandle,
    spool: Arc<Mutex<spool::Spool>>,
    failure: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
    pending_recordings: Arc<Mutex<BTreeSet<String>>>,
}

impl Audit {
    pub fn new(app: &AppHandle, path: PathBuf) -> Result<Self, String> {
        let mut spool = spool::Spool::open(path)?;
        let mut sink = AppSink(app);
        for cycle in spool.cycles() {
            let (complete, notices) = replay_cycle(&mut spool, &cycle, &mut sink, ly_plc::now_ms())?;
            publish(app, notices);
            if !complete { return Err(format!("cycleId={cycle} 的持久追溯尚未恢复，保留 spool 并拒绝启动生产")); }
        }
        let (tx, rx) = sync_channel(1);
        let audit = Self { tx, app: app.clone(), spool: Arc::new(Mutex::new(spool)), failure: Arc::new(Mutex::new(None)), running: Arc::new(AtomicBool::new(true)), pending_recordings: Arc::new(Mutex::new(BTreeSet::new())) };
        let worker = audit.clone();
        std::thread::Builder::new().name("inspection-audit".into()).spawn(move || writer(worker, rx))
            .map_err(|error| format!("追溯线程启动失败：{error}"))?;
        Ok(audit)
    }

    pub fn ready(&self) -> Result<(), String> {
        with_ready_spool(&self.spool, &self.failure, &self.running, &self.pending_recordings, || Ok(()))
    }

    pub fn with_ready<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        with_ready_spool(&self.spool, &self.failure, &self.running, &self.pending_recordings, action)
    }

    pub fn begin_recording(&self, cycle_id: &str) -> Result<(), String> {
        let result = track_recording(&self.spool, &self.failure, &self.pending_recordings, cycle_id);
        if let Err(error) = &result { self.fail(error.clone()); }
        result
    }

    pub fn protected_recording_directories(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        self.spool.lock().map_err(|_| "追溯 spool 锁损坏")?.protected_directories(root)
    }

    pub fn protected_cycle_ids(&self) -> Result<BTreeSet<String>, String> {
        Ok(self.spool.lock().map_err(|_| "追溯 spool 锁损坏")?.cycles())
    }

    fn fail(&self, error: String) {
        let _ = set_failure(&self.failure, error.clone());
        app_log(&self.app, "err", "持久追溯故障", error);
    }

    fn persist(&self, event: Event) -> Result<(), String> {
        let cycle = event.cycle_id().to_string();
        let result = persist_event(&self.spool, &self.failure, &self.pending_recordings, &event);
        if let Err(error) = result {
            let message = format!("cycleId={cycle}：{error}；原检测结论保留，禁止继续生产");
            self.fail(message.clone());
            return Err(message);
        }
        if matches!(self.tx.try_send(()), Err(TrySendError::Disconnected(()))) {
            let error = format!("cycleId={cycle}：事件已持久保存，但追溯线程已停止");
            self.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }

    async fn persist_async(&self, event: Event, timeout: Duration) -> Result<(), String> {
        let audit = self.clone();
        let task = tauri::async_runtime::spawn_blocking(move || audit.persist(event));
        if timeout.is_zero() {
            let message = "持久追溯接收已无剩余处理预算，仍保留落盘尝试但禁止 done".to_string();
            self.fail(message.clone());
            return Err(message);
        }
        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let message = format!("持久追溯接收线程异常：{error}");
                self.fail(message.clone());
                Err(message)
            }
            Err(_) => {
                let message = format!("持久追溯接收超过剩余预算 {} ms，迟到落盘也不能发布本件 done，需排查并重启恢复", timeout.as_millis());
                self.fail(message.clone());
                Err(message)
            }
        }
    }

    pub async fn insert_durable(&self, part: RecordedPart, timeout: Duration) -> Result<(), String> {
        self.persist_async(Event::Insert(Box::new(part)), timeout).await
    }

    pub async fn delivery_durable(&self, cycle_id: &str, delivery: PlcDelivery) -> Result<(), String> {
        self.persist_async(Event::Delivery(cycle_id.into(), delivery), Duration::from_secs(5)).await
    }

    pub async fn submission_durable(&self, cycle_id: &str, delivery: PlcDelivery, drain_ms: Option<u64>) -> Result<(), String> {
        self.persist_async(Event::Submission(cycle_id.into(), delivery, drain_ms), Duration::from_secs(5)).await
    }

    pub fn insert(&self, part: RecordedPart) -> Result<(), String> { self.persist(Event::Insert(Box::new(part))) }

    pub fn delivery(&self, cycle_id: &str, delivery: PlcDelivery) -> Result<(), String> { self.persist(Event::Delivery(cycle_id.into(), delivery)) }

    pub fn recording(&self, outcome: RecordingOutcome) -> Result<(), String> { self.persist(Event::Recording(outcome)) }
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
    fn raw_files(&mut self, cycle: &str, k: usize, files: &[ShotRawFile]) -> Result<bool, String>;
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

    fn raw_files(&mut self, cycle: &str, k: usize, files: &[ShotRawFile]) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.update_shot_raw_files(cycle, k, files)
    }

    fn recording(&mut self, cycle: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
        self.0.try_state::<Store>().ok_or("检测记录数据库尚未初始化")?.update_recording(cycle, evidence)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Inserted(i64),
    Updated(String),
    Log { level: &'static str, event: &'static str, message: String },
}

fn log(notices: &mut Vec<Notice>, level: &'static str, event: &'static str, cycle: &str, message: impl AsRef<str>) {
    notices.push(Notice::Log { level, event, message: format!("cycleId={cycle}：{}；原检测结论不变", message.as_ref()) });
}

#[derive(Default)]
struct CycleEntry {
    inserted: bool,
    record: Option<Box<RecordedPart>>,
    delivery: Option<PlcDelivery>,
    saved_delivery: Option<PlcDelivery>,
    drain_ms: Option<u64>,
    saved_drain_ms: Option<u64>,
    raw_files: BTreeMap<usize, Vec<ShotRawFile>>,
    saved_raw_files: BTreeMap<usize, Vec<ShotRawFile>>,
    shot_keys: Option<BTreeSet<usize>>,
    last_recording: Option<RecordingOutcome>,
    recording: Option<RecordingEvidence>,
    saved_recording: Option<RecordingEvidence>,
    touched_at: i64,
    failures: u32,
    retry_at: i64,
}

impl CycleEntry {
    fn pending(&self) -> bool {
        self.record.is_some() || self.delivery != self.saved_delivery || self.drain_ms != self.saved_drain_ms || self.raw_files != self.saved_raw_files || self.recording != self.saved_recording
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
                    Ok(Some(existing)) => restore(&cycle, &mut entry, existing.summary.delivery, existing.shots, existing.recording, existing.summary.drain_ms, &mut notices),
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

    fn retry(&mut self, sink: &mut impl Sink, now: i64) -> Vec<Notice> {
        let mut notices = Vec::new();
        self.trim(now, &mut notices);
        for cycle in &self.order {
            let entry = self.entries.get_mut(cycle).unwrap();
            if entry.pending() && (entry.record.is_some() || entry.inserted || entry.failures > 0) && now >= entry.retry_at {
                if entry.record.is_none() && entry.failures > 0 {
                    match sink.find(cycle) {
                        Ok(Some(existing)) => restore(cycle, entry, existing.summary.delivery, existing.shots, existing.recording, existing.summary.drain_ms, &mut notices),
                        Ok(None) => (),
                        Err(error) => {
                            entry.defer_retry(now);
                            log(&mut notices, "err", "追溯原记录重读失败", cycle, error);
                            continue;
                        }
                    }
                }
                flush(cycle, entry, sink, now, &mut notices);
            }
        }
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
                    "{reason}，交付待写={}，录制状态待写={}，原图组数={}，已停止自动重试{verdict}",
                    entry.delivery != entry.saved_delivery,
                    entry.recording != entry.saved_recording,
                    entry.raw_files.len()
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

fn raw_file(file: RecordedRawFile) -> Result<(usize, ShotRawFile), String> {
    if file.k >= 64 || !(1..=3).contains(&file.view) || file.width == 0 || file.height == 0 {
        return Err(format!("原图 k={} 或 view={} 越界", file.k, file.view));
    }
    if file.file.is_empty() || file.file.contains(['\\', ':', '\0']) || file.file.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err("原图文件不是录制根目录内的相对路径".into());
    }
    Ok((file.k, ShotRawFile { view: file.view, file: file.file, width: Some(file.width), height: Some(file.height) }))
}

fn merge_files(cycle: &str, entry: &mut CycleEntry, k: usize, incoming: ShotRawFile, notices: &mut Vec<Notice>) -> bool {
    if entry.shot_keys.as_ref().is_some_and(|keys| !keys.contains(&k)) {
        log(notices, "err", "原图归属失败", cycle, format!("原记录没有拍照点 k={k}，忽略 view={}", incoming.view));
        return false;
    }
    let files = entry.raw_files.entry(k).or_default();
    if let Some(previous) = files.iter_mut().find(|file| file.view == incoming.view) {
        if previous.file != incoming.file || (previous.width.is_some() && previous.width != incoming.width)
            || (previous.height.is_some() && previous.height != incoming.height) {
            log(notices, "err", "原图身份冲突", cycle, format!("k={k} view={} 已有关联文件，保留首次原图证据", incoming.view));
            return false;
        }
        if previous == &incoming { return false; }
        *previous = incoming;
    } else {
        files.push(incoming);
        files.sort_by_key(|file| file.view);
    }
    true
}

fn merge_recording(cycle: &str, entry: &mut CycleEntry, outcome: RecordingOutcome, notices: &mut Vec<Notice>) -> bool {
    if entry.last_recording.as_ref() == Some(&outcome) {
        return false;
    }
    entry.last_recording = Some(outcome.clone());
    if !outcome.retention_errors.is_empty() {
        log(notices, "warn", "录制保留清理失败", cycle, outcome.retention_errors.join("；"));
    }
    if !outcome.errors.is_empty()
        || matches!(outcome.state, RecorderState::Failed | RecorderState::Incomplete)
        || (outcome.available && outcome.files.is_empty())
    {
        let directory = outcome.directory.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "无目录".into());
        log(
            notices,
            "err",
            "原图录制未完成",
            cycle,
            format!(
                "状态 {:?}，目录 {directory}，实际原图 {} 张，可用={}，{}",
                outcome.state,
                outcome.files.len(),
                outcome.available,
                outcome.errors.join("；")
            ),
        );
    }
    let mut changed = false;
    let mut errors = outcome.errors;
    let had_files = !outcome.files.is_empty();
    for file in outcome.files {
        match raw_file(file) {
            Ok((k, file)) => changed |= merge_files(cycle, entry, k, file, notices),
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

fn restore(cycle: &str, entry: &mut CycleEntry, delivery: PlcDelivery, shots: Vec<PartShot>, recording: RecordingEvidence, drain_ms: Option<u64>, notices: &mut Vec<Notice>) {
    entry.inserted = true;
    let pending_timing = entry.drain_ms.take();
    entry.drain_ms = drain_ms;
    entry.saved_drain_ms = drain_ms;
    merge_timing(cycle, &mut entry.drain_ms, pending_timing, notices);
    entry.record = None;
    entry.shot_keys = Some(shots.iter().map(|shot| shot.k).collect());
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
    let pending = std::mem::take(&mut entry.raw_files);
    entry.saved_raw_files = shots.into_iter().filter(|shot| !shot.raw_files.is_empty()).map(|shot| (shot.k, shot.raw_files)).collect();
    entry.raw_files = entry.saved_raw_files.clone();
    for (k, files) in pending {
        for file in files {
            merge_files(cycle, entry, k, file, notices);
        }
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
                restore(cycle, entry, part.delivery, part.shots, RecordingEvidence::default(), part.drain_ms, notices);
                notices.push(Notice::Inserted(id));
            }
            Ok(InsertResult::Existing(existing)) => {
                restore(cycle, entry, existing.summary.delivery, existing.shots, existing.recording, existing.summary.drain_ms, notices);
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
    for (k, files) in &entry.raw_files {
        if entry.saved_raw_files.get(k) == Some(files) {
            continue;
        }
        match sink.raw_files(cycle, *k, files) {
            Ok(true) => {
                entry.saved_raw_files.insert(*k, files.clone());
                entry.inserted = true;
                changed = true;
            }
            Ok(false) if entry.inserted => {
                failed = true;
                log(notices, "err", "原图追溯未写入", cycle, format!("原记录或拍照点 k={k} 不存在，保留待写原图引用"));
            }
            Ok(false) => (),
            Err(error) => {
                failed = true;
                log(notices, "err", "原图追溯写入失败", cycle, format!("k={k}：{error}"));
            }
        }
    }
    if entry.recording != entry.saved_recording {
        if let Some(evidence) = entry.recording.as_ref() {
            if !evidence.available || entry.raw_files == entry.saved_raw_files {
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
                    app_log(app, "err", "历史通知失败", error.to_string());
                }
            }
            Notice::Updated(cycle) => {
                if let Err(error) = app.emit("history://updated", cycle) {
                    app_log(app, "err", "历史通知失败", error.to_string());
                }
            }
            Notice::Log { level, event, message } => app_log(app, level, event, message),
        }
    }
}

fn app_log(app: &AppHandle, level: &'static str, event: &'static str, message: impl Into<String>) {
    let message = message.into();
    if app.try_state::<cycle::CycleHost>().is_some() {
        cycle::log(app, level, event, message);
    } else {
        eprintln!("[{level}] {event}：{message}");
    }
}

fn immutable_record(part: &RecordedPart) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(part).map_err(|error| error.to_string())?;
    let fields = value.as_object_mut().ok_or("检测记录结构无效")?;
    fields.remove("delivery");
    fields.remove("drain_ms");
    if let Some(shots) = fields.get_mut("shots").and_then(serde_json::Value::as_array_mut) {
        for shot in shots { if let Some(fields) = shot.as_object_mut() { fields.remove("rawFiles"); } }
    }
    Ok(value)
}

fn apply_spooled(events: Vec<(u64, Event)>, cycle: &str, sink: &mut impl Sink, now: i64) -> (bool, Vec<u64>, Vec<Notice>) {
    let mut coordinator = Coordinator::new(1, 1);
    let mut notices = Vec::new();
    let receipts = events.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>();
    let mut original = None;
    for (_, event) in &events {
        if let Event::Insert(part) = event {
            let value = match immutable_record(part) {
                Ok(value) => value,
                Err(error) => { log(&mut notices, "err", "持久检测记录结构错误", cycle, error); return (false, receipts, notices); }
            };
            if original.as_ref().is_some_and(|previous| *previous != value) {
                log(&mut notices, "err", "持久检测记录身份冲突", cycle, "同 cycleId 对应不同不可变检测内容，保留全部 spool 证据");
                return (false, receipts, notices);
            }
            original = Some(value);
            match sink.find(cycle) {
                Ok(Some(_)) => match sink.matches_record(part) {
                    Ok(true) => (),
                    Ok(false) => {
                        log(&mut notices, "err", "持久检测记录身份冲突", cycle, "数据库原检测内容不同，保留原记录及全部 spool 证据");
                        return (false, receipts, notices);
                    }
                    Err(error) => { log(&mut notices, "err", "持久检测记录核对失败", cycle, error); return (false, receipts, notices); }
                },
                Ok(None) => (),
                Err(error) => { log(&mut notices, "err", "持久检测记录核对失败", cycle, error); return (false, receipts, notices); }
            }
        }
    }
    for (_, event) in &events { notices.extend(coordinator.handle(event.clone(), sink, now)); }
    let conflict = notices.iter().any(|notice| matches!(notice, Notice::Log { event, .. }
        if matches!(*event, "追溯身份无效" | "PLC 交付身份冲突" | "原图归属失败" | "原图身份冲突" | "原图引用无效" | "录制收尾证据冲突" | "PLC 提交耗时身份冲突")));
    let mut complete = !conflict && coordinator.entries.get(cycle).is_some_and(|entry| entry.inserted && !entry.pending());
    if complete {
        for (_, event) in &events {
            if let Event::Insert(part) = event {
                match sink.matches_record(part) {
                    Ok(true) => (),
                    Ok(false) => { complete = false; log(&mut notices, "err", "持久检测记录身份冲突", cycle, "入库后实际检测内容不符，保留全部 spool 证据"); },
                    Err(error) => { complete = false; log(&mut notices, "err", "持久检测记录核对失败", cycle, error); },
                }
            }
        }
    }
    (complete, receipts, notices)
}

fn replay_cycle(spool: &mut spool::Spool, cycle: &str, sink: &mut impl Sink, now: i64) -> Result<(bool, Vec<Notice>), String> {
    let (complete, receipts, notices) = apply_spooled(spool.events(cycle)?, cycle, sink, now);
    if complete { spool.remove(&receipts)?; }
    Ok((complete, notices))
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
            let result = spool_io(&audit.spool, &audit.failure, |spool| spool.probe());
            if let Err(error) = result { audit.fail(error); }
            last_probe = Instant::now();
        }
        let cycles = match audit.spool.lock() {
            Ok(spool) => spool.cycles(),
            Err(_) => { audit.fail("追溯 spool 锁损坏".into()); return; }
        };
        for cycle in cycles {
            let now = ly_plc::now_ms();
            if retries.get(&cycle).is_some_and(|(_, due)| now < *due) { continue; }
            let events = match spool_io(&audit.spool, &audit.failure, |spool| spool.events(&cycle)) {
                Ok(events) => events,
                Err(error) => { audit.fail(error); continue; }
            };
            let (complete, receipts, notices) = apply_spooled(events, &cycle, &mut sink, now);
            publish(&audit.app, notices);
            if complete {
                match spool_io(&audit.spool, &audit.failure, |spool| spool.remove(&receipts)) {
                    Ok(()) => { retries.remove(&cycle); }
                    Err(error) => audit.fail(error),
                }
            } else {
                let (failures, due) = retries.entry(cycle).or_default();
                *failures = failures.saturating_add(1);
                *due = now.saturating_add((RETRY_BASE_MS << failures.saturating_sub(1).min(5)).min(RETRY_MAX_MS));
            }
        }
        if disconnected { return; }
    }
}

#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;

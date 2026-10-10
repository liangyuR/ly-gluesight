use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::cycle::{self, FrameView};
use crate::judge::{Judgement, PointState};
use crate::recipe::Recipe;
use crate::recorder::{RecordedRawFile, RecordingOutcome, RecordingState as RecorderState};
use crate::store::{PartDetail, PartRecord, PartShot, PlcDelivery, PlcDeliveryState, RecordingEvidence, RecordingState, ShotRawFile, Store};

const PENDING_LIMIT: usize = 128;
const CACHE_LIMIT: usize = 512;
const PENDING_TTL_MS: i64 = 30 * 60 * 1000;
const WRITE_ATTEMPTS: u8 = 3;

#[derive(Clone, Debug)]
pub struct RecordedPart {
    pub ts: i64,
    pub sn: u32,
    pub recipe: Option<Arc<Recipe>>,
    pub judgement: Judgement,
    pub drain_ms: Option<u64>,
    pub frames: Vec<FrameView>,
    pub frames_expected: usize,
    pub frames_received: usize,
    pub triggers: u64,
    pub table: Option<Vec<PointState>>,
    pub software_version: String,
    pub cycle_id: String,
    pub bundle_hash: Option<String>,
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
            bundle_hash: self.bundle_hash.as_deref(),
            delivery: &self.delivery,
            shots: &self.shots,
        }
    }
}

enum Event {
    Insert(Box<RecordedPart>),
    Delivery(String, PlcDelivery),
    Recording(RecordingOutcome),
}

impl Event {
    fn cycle_id(&self) -> &str {
        match self {
            Self::Insert(part) => &part.cycle_id,
            Self::Delivery(cycle, _) => cycle,
            Self::Recording(outcome) => &outcome.cycle_id,
        }
    }
}

#[derive(Clone)]
pub struct Audit {
    tx: Sender<Event>,
    app: AppHandle,
}

impl Audit {
    pub fn new(app: &AppHandle) -> Self {
        let (tx, rx) = channel();
        let handle = app.clone();
        if let Err(error) = std::thread::Builder::new().name("inspection-audit".into()).spawn(move || writer(handle, rx)) {
            app_log(app, "err", "追溯线程启动失败", error.to_string());
        }
        Self { tx, app: app.clone() }
    }

    pub fn insert(&self, part: RecordedPart) {
        self.enqueue(Event::Insert(Box::new(part)));
    }

    pub fn delivery(&self, cycle_id: &str, delivery: PlcDelivery) {
        self.enqueue(Event::Delivery(cycle_id.into(), delivery));
    }

    pub fn recording(&self, outcome: RecordingOutcome) {
        self.enqueue(Event::Recording(outcome));
    }

    fn enqueue(&self, event: Event) {
        if let Err(error) = self.tx.send(event) {
            app_log(&self.app, "err", "追溯排队失败", format!("cycleId={}：追溯线程已停止；原检测结论不变", error.0.cycle_id()));
        }
    }
}

enum InsertResult {
    Inserted(i64),
    Existing(Box<PartDetail>),
}

trait Sink {
    fn find(&mut self, cycle: &str) -> Result<Option<PartDetail>, String>;
    fn insert(&mut self, part: &RecordedPart) -> Result<InsertResult, String>;
    fn delivery(&mut self, cycle: &str, delivery: &PlcDelivery) -> Result<bool, String>;
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
    raw_files: BTreeMap<usize, Vec<ShotRawFile>>,
    saved_raw_files: BTreeMap<usize, Vec<ShotRawFile>>,
    shot_keys: Option<BTreeSet<usize>>,
    last_recording: Option<String>,
    recording: Option<RecordingEvidence>,
    saved_recording: Option<RecordingEvidence>,
    touched_at: i64,
    failures: u8,
}

impl CycleEntry {
    fn pending(&self) -> bool {
        self.record.is_some() || self.delivery != self.saved_delivery || self.raw_files != self.saved_raw_files || self.recording != self.saved_recording
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
                    Ok(Some(existing)) => restore(&cycle, &mut entry, existing.summary.delivery, existing.shots, existing.recording, &mut notices),
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
            Event::Recording(outcome) => merge_recording(&cycle, &mut entry, outcome, &mut notices),
        };
        if changed {
            entry.failures = 0;
            flush(&cycle, &mut entry, sink, &mut notices);
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
            if entry.pending() && (entry.record.is_some() || entry.inserted || entry.failures > 0) && entry.failures < WRITE_ATTEMPTS {
                if entry.record.is_none() && entry.failures > 0 {
                    match sink.find(cycle) {
                        Ok(Some(existing)) => restore(cycle, entry, existing.summary.delivery, existing.shots, existing.recording, &mut notices),
                        Ok(None) => (),
                        Err(error) => {
                            entry.failures += 1;
                            log(&mut notices, "err", "追溯原记录重读失败", cycle, error);
                            continue;
                        }
                    }
                }
                flush(cycle, entry, sink, &mut notices);
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

fn raw_file(file: RecordedRawFile) -> Result<(usize, ShotRawFile), String> {
    if file.k >= 64 || !(1..=3).contains(&file.view) {
        return Err(format!("原图 k={} 或 view={} 越界", file.k, file.view));
    }
    if file.file.is_empty() || file.file.contains(['\\', ':', '\0']) || file.file.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err("原图文件不是录制根目录内的相对路径".into());
    }
    let tagged = file.hash.split_once(':').is_some_and(|(algorithm, digest)| {
        !algorithm.is_empty()
            && algorithm.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            && !digest.is_empty()
            && digest.bytes().all(|c| c.is_ascii_hexdigit())
    });
    if !tagged {
        return Err(format!("原图 k={} view={} 缺少标明算法的哈希", file.k, file.view));
    }
    Ok((file.k, ShotRawFile { view: file.view, file: file.file, hash: Some(file.hash) }))
}

fn merge_files(cycle: &str, entry: &mut CycleEntry, k: usize, incoming: ShotRawFile, notices: &mut Vec<Notice>) -> bool {
    if entry.shot_keys.as_ref().is_some_and(|keys| !keys.contains(&k)) {
        log(notices, "err", "原图归属失败", cycle, format!("原记录没有拍照点 k={k}，忽略 view={}", incoming.view));
        return false;
    }
    let files = entry.raw_files.entry(k).or_default();
    if let Some(previous) = files.iter_mut().find(|file| file.view == incoming.view) {
        if previous.file != incoming.file || (previous.hash.is_some() && previous.hash != incoming.hash) {
            log(notices, "err", "原图身份冲突", cycle, format!("k={k} view={} 已有关联文件，保留首次原图证据", incoming.view));
            return false;
        }
        if previous == &incoming || incoming.hash.is_none() {
            return false;
        }
        *previous = incoming;
    } else {
        files.push(incoming);
        files.sort_by_key(|file| file.view);
    }
    true
}

fn merge_recording(cycle: &str, entry: &mut CycleEntry, outcome: RecordingOutcome, notices: &mut Vec<Notice>) -> bool {
    let signature = serde_json::to_string(&outcome).unwrap();
    if entry.last_recording.as_deref() == Some(signature.as_str()) {
        return false;
    }
    entry.last_recording = Some(signature);
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

fn restore(cycle: &str, entry: &mut CycleEntry, delivery: PlcDelivery, shots: Vec<PartShot>, recording: RecordingEvidence, notices: &mut Vec<Notice>) {
    entry.inserted = true;
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

fn flush(cycle: &str, entry: &mut CycleEntry, sink: &mut impl Sink, notices: &mut Vec<Notice>) {
    if entry.failures >= WRITE_ATTEMPTS {
        return;
    }
    if let Some(part) = entry.record.as_ref() {
        match sink.insert(part) {
            Ok(InsertResult::Inserted(id)) => {
                let part = entry.record.take().unwrap();
                restore(cycle, entry, part.delivery, part.shots, RecordingEvidence::default(), notices);
                notices.push(Notice::Inserted(id));
            }
            Ok(InsertResult::Existing(existing)) => {
                restore(cycle, entry, existing.summary.delivery, existing.shots, existing.recording, notices);
                log(notices, "info", "重复追溯入库", cycle, "数据库已有相同 cycleId，保留原记录及原图证据");
            }
            Err(error) => {
                entry.failures += 1;
                log(
                    notices,
                    "err",
                    "追溯入库失败",
                    cycle,
                    format!("{error}；保留待写原判定 {:?}，写入尝试 {}/{WRITE_ATTEMPTS}", part.judgement.verdict, entry.failures),
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
    if failed {
        entry.failures += 1;
    } else {
        entry.failures = 0;
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

fn writer(app: AppHandle, rx: Receiver<Event>) {
    let mut coordinator = Coordinator::new(PENDING_LIMIT, CACHE_LIMIT);
    let mut sink = AppSink(&app);
    let mut retried_at = Instant::now();
    loop {
        let now = ly_plc::now_ms();
        let notices = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => coordinator.handle(event, &mut sink, ly_plc::now_ms()),
            Err(RecvTimeoutError::Timeout) => {
                retried_at = Instant::now();
                coordinator.retry(&mut sink, ly_plc::now_ms())
            }
            Err(RecvTimeoutError::Disconnected) => {
                publish(&app, coordinator.retry(&mut sink, now));
                let cycles = coordinator.order.iter().cloned().collect::<Vec<_>>();
                let mut notices = Vec::new();
                for cycle in cycles {
                    coordinator.evict(&cycle, "追溯线程退出，仍有未写入证据", &mut notices);
                }
                publish(&app, notices);
                return;
            }
        };
        publish(&app, notices);
        if retried_at.elapsed() >= Duration::from_secs(1) {
            publish(&app, coordinator.retry(&mut sink, ly_plc::now_ms()));
            retried_at = Instant::now();
        }
    }
}

#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;

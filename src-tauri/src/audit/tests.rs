use super::*;
use crate::cycle::FrameStatus;
use crate::judge::Verdict;
use crate::store::{HistoryQuery, PartDetail};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("gluesight-audit-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn sink(&self) -> TestSink { self.sink_mode(false) }

    fn sink_mode(&self, deferred: bool) -> TestSink {
        TestSink {
            store: if deferred { Store::open_deferred_recovery(&self.0.join("parts.sqlite")).unwrap() } else { Store::open(&self.0.join("parts.sqlite")).unwrap() },
            inserts: 0,
            delivery_writes: 0,
            raw_writes: 0,
            recording_writes: 0,
            fail_find: 0,
            fail_insert: 0,
            fail_delivery: 0,
            fail_raw: 0,
            fail_recording: 0,
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TestSink {
    store: Store,
    inserts: usize,
    delivery_writes: usize,
    raw_writes: usize,
    recording_writes: usize,
    fail_find: usize,
    fail_insert: usize,
    fail_delivery: usize,
    fail_raw: usize,
    fail_recording: usize,
}

impl TestSink {
    fn detail(&self, cycle: &str) -> PartDetail {
        let page = self.store.query(&HistoryQuery { limit: 500, ..Default::default() }).unwrap();
        let id = page.items.iter().find(|part| part.cycle_id.as_deref() == Some(cycle)).unwrap().id;
        self.store.detail(id).unwrap()
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        (self.inserts, self.delivery_writes, self.raw_writes, self.recording_writes)
    }
}

fn fault(remaining: &mut usize) -> Result<(), String> {
    if *remaining > 0 {
        *remaining -= 1;
        Err("磁盘空间不足（测试注入）".into())
    } else {
        Ok(())
    }
}

impl Sink for TestSink {
    fn find(&mut self, cycle: &str) -> Result<Option<PartDetail>, String> {
        fault(&mut self.fail_find)?;
        self.store.detail_by_cycle(cycle)
    }

    fn insert(&mut self, part: &RecordedPart) -> Result<InsertResult, String> {
        self.inserts += 1;
        fault(&mut self.fail_insert)?;
        insert_store(&self.store, part)
    }

    fn delivery(&mut self, cycle: &str, delivery: &PlcDelivery) -> Result<bool, String> {
        self.delivery_writes += 1;
        fault(&mut self.fail_delivery)?;
        self.store.update_delivery(cycle, delivery)
    }

    fn matches_record(&mut self, part: &RecordedPart) -> Result<bool, String> {
        self.store.matches_record(&part.borrowed())
    }

    fn submission_timing(&mut self, cycle: &str, drain_ms: u64) -> Result<bool, String> {
        self.store.update_submission_timing(cycle, drain_ms)
    }

    fn raw_files(&mut self, cycle: &str, k: usize, files: &[ShotRawFile]) -> Result<bool, String> {
        self.raw_writes += 1;
        fault(&mut self.fail_raw)?;
        self.store.update_shot_raw_files(cycle, k, files)
    }

    fn recording(&mut self, cycle: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
        self.recording_writes += 1;
        fault(&mut self.fail_recording)?;
        self.store.update_recording(cycle, evidence)
    }
}

fn part(cycle: &str, sn: u32) -> RecordedPart {
    let mut doc = crate::recipe::samples().remove(1);
    doc.shots.truncate(2);
    doc.shots[0].view = 2;
    doc.shots[1].view = 3;
    let recipe = Arc::new(doc.build().unwrap());
    let table = vec![PointState::Measured { d: 0.25, w: 4.5 }; recipe.point_count()];
    let shots = recipe
        .shots
        .iter()
        .enumerate()
        .map(|(k, shot)| PartShot {
            k,
            shot_id: shot.id.clone(),
            camera: shot.camera.clone(),
            view: shot.view,
            session: Some(u64::MAX),
            ordinal: Some(k as u64 + 1),
            frame_counter: Some(u64::MAX - 1),
            trigger_counter: Some(u64::MAX - 2),
            status: if k == 0 { FrameStatus::Done } else { FrameStatus::Missing },
            error: (k == 1).then(|| "原始缺帧原因".into()),
            score: (k == 0).then_some(0.75),
            ms: (k == 0).then_some(12),
            raw_files: Vec::new(),
        })
        .collect();
    RecordedPart {
        ts: 1234,
        sn,
        recipe: Some(recipe.clone()),
        judgement: Judgement::error(crate::judge::fault::MISSING_FRAME, "原始检测结论：P2 缺帧"),
        drain_ms: Some(42),
        frames: Vec::new(),
        frames_expected: recipe.shot_count(),
        frames_received: 1,
        triggers: 2,
        table: Some(table),
        software_version: "audit-test".into(),
        cycle_id: cycle.into(),
        bundle_id: Some("frozen-bundle".into()),
        delivery: delivery(PlcDeliveryState::Pending, 10),
        shots,
    }
}

fn delivery(state: PlcDeliveryState, updated_at: i64) -> PlcDelivery {
    PlcDelivery { state, updated_at, message: Some(format!("{state:?}")) }
}

fn outcome(cycle: &str, files: &[(usize, u8)], state: RecorderState) -> RecordingOutcome {
    RecordingOutcome {
        cycle_id: cycle.into(),
        directory: (!matches!(state, RecorderState::Off | RecorderState::NotRetained)).then(|| PathBuf::from(format!("records/cycle_{cycle}"))),
        files: files
            .iter()
            .map(|(k, view)| RecordedRawFile {
                k: *k,
                width: 100,
                height: 60,
                view: *view,
                file: format!("cycle_{cycle}/k{k:03}_P{}_cam1_v{view}.pgm", k + 1),

            })
            .collect(),
        errors: if matches!(state, RecorderState::Failed | RecorderState::Incomplete) {
            vec!["原图磁盘写入失败：拒绝访问".into()]
        } else {
            Vec::new()
        },
        retention_errors: Vec::new(),
        available: state == RecorderState::Complete,
        state,
    }
}

fn retry_due(coordinator: &mut Coordinator, sink: &mut TestSink, cycle: &str) -> Vec<Notice> {
    let now = coordinator.entries[cycle].retry_at;
    coordinator.retry(sink, now)
}

fn has_message(notices: &[Notice], message: &str) -> bool {
    notices.iter().any(|notice| matches!(notice, Notice::Log { message: text, .. } if text.contains(message)))
}

fn original(detail: &PartDetail) -> serde_json::Value {
    serde_json::json!({ "judgement": detail.judgement, "points": detail.points, "frames": detail.frames, "shots": detail.shots.iter().map(|shot| {
        let mut shot = shot.clone();
        shot.raw_files.clear();
        shot
    }).collect::<Vec<_>>() })
}

#[test]
fn recording_before_insert_keeps_all_views_and_original_measurement() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 1), (0, 2), (0, 3)], RecorderState::Complete)), &mut sink, 1);
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 0);
    let record = part("one", 42);
    let expected = serde_json::to_value(&record.judgement).unwrap();
    let notices = coordinator.handle(Event::Insert(Box::new(record)), &mut sink, 2);
    assert!(notices.contains(&Notice::Inserted(1)));
    assert!(notices.contains(&Notice::Updated("one".into())));
    let detail = sink.detail("one");
    assert_eq!(detail.shots[0].raw_files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(detail.shots[0].raw_files[1].file, "cycle_one/k000_P1_cam1_v2.pgm");
    assert_eq!(detail.shots[0].session, Some(u64::MAX));
    assert_eq!(detail.shots[1].error.as_deref(), Some("原始缺帧原因"));
    assert_eq!(serde_json::to_value(detail.judgement).unwrap(), expected);
    assert_eq!(detail.recording.state, RecordingState::Complete);
    assert!(detail.recording.available);
    assert!(!coordinator.entries["one"].pending());
}

#[test]
fn retention_warning_preserves_complete_recording_and_is_logged_once() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Insert(Box::new(part("retention", 42))), &mut sink, 1);
    let before = original(&sink.detail("retention"));
    let mut recording = outcome("retention", &[(0, 1), (0, 2), (0, 3)], RecorderState::Complete);
    recording.retention_errors.push("清理旧录制失败 old-cycle：文件正在使用".into());
    let notices = coordinator.handle(Event::Recording(recording.clone()), &mut sink, 2);
    assert!(notices.iter().any(|notice| matches!(notice, Notice::Log { level: "warn", event: "录制保留清理失败", message }
        if message.contains("cycleId=retention") && message.contains("old-cycle"))));
    assert!(!notices.iter().any(|notice| matches!(notice, Notice::Log { level: "err", .. })));
    let detail = sink.detail("retention");
    assert_eq!(detail.recording.state, RecordingState::Complete);
    assert!(detail.recording.available);
    assert!(detail.recording.errors.is_empty());
    assert_eq!(detail.shots[0].raw_files.len(), 3);
    assert!(detail.shots[0].raw_files.iter().all(|file| !file.file.is_empty()));
    assert_eq!(original(&detail), before);
    let calls = sink.counts();
    assert!(coordinator.handle(Event::Recording(recording), &mut sink, 3).is_empty());
    assert_eq!(sink.counts(), calls);
}

#[test]
fn early_ack_wins_over_insert_snapshot_and_late_submitted_or_failed() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 1);
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Submitted, 30)), &mut sink, 2);
    let mut record = part("one", 42);
    record.delivery.updated_at = 40;
    coordinator.handle(Event::Insert(Box::new(record)), &mut sink, 3);
    let before = sink.detail("one");
    assert_eq!(before.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(before.summary.delivery.updated_at, 40);
    let calls = sink.counts();
    for state in [PlcDeliveryState::Submitted, PlcDeliveryState::Pending, PlcDeliveryState::Failed] {
        assert!(coordinator.handle(Event::Delivery("one".into(), delivery(state, 99)), &mut sink, 4).is_empty());
    }
    assert_eq!(sink.counts(), calls);
    assert_eq!(original(&sink.detail("one")), original(&before));
    assert_eq!(sink.detail("one").summary.delivery, before.summary.delivery);
}

#[test]
fn duplicate_insert_delivery_and_recording_do_not_write_again() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    let record = part("one", 42);
    let recording = outcome("one", &[(0, 2)], RecorderState::Complete);
    coordinator.handle(Event::Insert(Box::new(record.clone())), &mut sink, 1);
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 2);
    coordinator.handle(Event::Recording(recording.clone()), &mut sink, 3);
    let calls = sink.counts();
    let before = serde_json::to_value(sink.detail("one")).unwrap();
    assert!(coordinator.handle(Event::Insert(Box::new(record)), &mut sink, 4).is_empty());
    assert!(coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 5).is_empty());
    assert!(coordinator.handle(Event::Recording(recording), &mut sink, 6).is_empty());
    assert_eq!(sink.counts(), calls);
    assert_eq!(serde_json::to_value(sink.detail("one")).unwrap(), before);
}

#[test]
fn equal_sn_cycles_receive_only_their_own_delivery_and_raw_files() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Recording(outcome("second", &[(1, 3)], RecorderState::Complete)), &mut sink, 1);
    coordinator.handle(Event::Delivery("first".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 2);
    coordinator.handle(Event::Insert(Box::new(part("first", 42))), &mut sink, 3);
    coordinator.handle(Event::Insert(Box::new(part("second", 42))), &mut sink, 4);
    let first = sink.detail("first");
    let second = sink.detail("second");
    assert_eq!(first.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(second.summary.delivery.state, PlcDeliveryState::Pending);
    assert!(first.shots.iter().all(|shot| shot.raw_files.is_empty()));
    assert!(second.shots[1].raw_files[0].file.contains("cycle_second/"));
    assert_eq!(second.summary.retest_of, Some(first.summary.id));
}

#[test]
fn failed_insert_retains_first_conclusion_early_ack_and_recording_for_retry() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    sink.fail_insert = 1;
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 1);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 1)], RecorderState::Incomplete)), &mut sink, 2);
    let record = part("one", 42);
    let notices = coordinator.handle(Event::Insert(Box::new(record.clone())), &mut sink, 3);
    assert!(has_message(&notices, "磁盘空间不足"));
    assert!(has_message(&notices, "原检测结论不变"));
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 0);
    assert_eq!(coordinator.entries["one"].record.as_ref().unwrap().judgement.reason, record.judgement.reason);
    let mut conflicting = record.clone();
    conflicting.judgement.reason = "重复事件不得替换结论".into();
    coordinator.handle(Event::Insert(Box::new(conflicting)), &mut sink, 4);
    assert_eq!(sink.inserts, 1);
    retry_due(&mut coordinator, &mut sink, "one");
    let detail = sink.detail("one");
    assert_eq!(detail.judgement.reason, record.judgement.reason);
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(detail.shots[0].raw_files.len(), 1);
    assert_eq!(detail.recording.state, RecordingState::Incomplete);
    assert_eq!(detail.recording.errors, ["原图磁盘写入失败：拒绝访问"]);
    assert!(!detail.recording.available);
}

#[test]
fn disk_failure_has_independent_history_evidence_and_keeps_good_original_verdict() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    let mut record = part("one", 42);
    record.judgement.verdict = Verdict::Ok;
    record.judgement.plc_code = Verdict::Ok.plc_code();
    record.judgement.fault_code = 0;
    record.judgement.reason = "原始合格结论".into();
    coordinator.handle(Event::Insert(Box::new(record)), &mut sink, 1);
    let before = sink.detail("one");
    assert_eq!(before.recording.state, RecordingState::Pending);
    let notices = coordinator.handle(Event::Recording(outcome("one", &[], RecorderState::Failed)), &mut sink, 2);
    assert!(has_message(&notices, "拒绝访问"));
    let after = sink.detail("one");
    assert_eq!(original(&after), original(&before));
    assert_eq!(after.summary.verdict, Verdict::Ok);
    assert_eq!(after.recording.state, RecordingState::Failed);
    assert!(!after.recording.available);
    assert!(after.recording.errors[0].contains("拒绝访问"));
    assert!(after.shots.iter().all(|shot| shot.raw_files.is_empty()));
}

#[test]
fn off_and_ng_only_not_retained_are_saved_even_when_callbacks_arrive_first() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    for (cycle, state, expected) in [("off", RecorderState::Off, RecordingState::Off), ("ng_only", RecorderState::NotRetained, RecordingState::NotRetained)] {
        coordinator.handle(Event::Recording(outcome(cycle, &[], state)), &mut sink, 1);
        coordinator.handle(Event::Insert(Box::new(part(cycle, 42))), &mut sink, 2);
        let detail = sink.detail(cycle);
        assert_eq!(detail.recording.state, expected);
        assert!(!detail.recording.available);
        assert!(detail.recording.directory.is_none());
        assert!(detail.recording.errors.is_empty());
    }
}

#[test]
fn failed_raw_reference_write_retries_without_exposing_complete_recording_as_available() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Insert(Box::new(part("one", 42))), &mut sink, 1);
    let before = sink.detail("one");
    sink.fail_raw = 1;
    let notices = coordinator.handle(Event::Recording(outcome("one", &[(0, 2)], RecorderState::Complete)), &mut sink, 2);
    assert!(has_message(&notices, "磁盘空间不足"));
    assert_eq!(sink.detail("one").recording.state, RecordingState::Pending);
    assert!(sink.detail("one").shots[0].raw_files.is_empty());
    let notices = retry_due(&mut coordinator, &mut sink, "one");
    assert!(notices.contains(&Notice::Updated("one".into())));
    let after = sink.detail("one");
    assert_eq!(original(&after), original(&before));
    assert_eq!(after.shots[0].raw_files.len(), 1);
    assert!(after.recording.available);
    assert!(!coordinator.entries["one"].pending());
}

#[test]
fn failed_delivery_and_recording_updates_are_retained_until_retry() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Insert(Box::new(part("one", 42))), &mut sink, 1);
    let before = sink.detail("one");
    sink.fail_delivery = 1;
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 2);
    sink.fail_recording = 1;
    coordinator.handle(Event::Recording(outcome("one", &[], RecorderState::Off)), &mut sink, 3);
    retry_due(&mut coordinator, &mut sink, "one");
    let delivery_writes = sink.delivery_writes;
    retry_due(&mut coordinator, &mut sink, "one");
    assert_eq!(sink.delivery_writes, delivery_writes);
    let after = sink.detail("one");
    assert_eq!(after.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(after.recording.state, RecordingState::Off);
    assert_eq!(original(&after), original(&before));
    assert!(!coordinator.entries["one"].pending());
}

#[test]
fn retries_continue_with_capped_backoff_and_pending_capacity_and_expiry_remain_explicit() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    sink.fail_insert = 100;
    let mut coordinator = Coordinator::new(2, 3);
    coordinator.handle(Event::Insert(Box::new(part("failed", 42))), &mut sink, 1);
    let mut now = 1;
    for expected_delay in [1000, 2000, 4000, 8000, 16000, 30000, 30000] {
        assert_eq!(coordinator.entries["failed"].retry_at - now, expected_delay);
        let attempts = sink.inserts;
        let due = coordinator.entries["failed"].retry_at;
        assert!(coordinator.retry(&mut sink, due - 1).is_empty());
        assert_eq!(sink.inserts, attempts);
        coordinator.retry(&mut sink, due);
        now = due;
        assert_eq!(sink.inserts, attempts + 1);
    }
    assert_eq!(sink.inserts, 8);
    assert!(coordinator.entries["failed"].record.is_some());
    coordinator.handle(Event::Delivery("never-formed-1".into(), delivery(PlcDeliveryState::Pending, 10)), &mut sink, now + 1);
    let notices = coordinator.handle(Event::Delivery("never-formed-2".into(), delivery(PlcDeliveryState::Pending, 10)), &mut sink, now + 2);
    assert!(has_message(&notices, "待写 cycle 数量达到上限 2"));
    assert!(has_message(&notices, "待入库原判定"));
    assert_eq!(coordinator.entries.len(), 2);
    let notices = coordinator.retry(&mut sink, PENDING_TTL_MS + now + 3);
    assert!(has_message(&notices, "超过 30 分钟"));
    assert!(coordinator.entries.is_empty());
    assert!(coordinator.order.is_empty());
}

#[test]
fn evicted_cycle_duplicate_insert_cannot_clear_evidence_or_replace_judgement() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(1, 1);
    coordinator.handle(Event::Insert(Box::new(part("one", 42))), &mut sink, 1);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 2)], RecorderState::Complete)), &mut sink, 2);
    let before = sink.detail("one");
    coordinator.handle(Event::Insert(Box::new(part("two", 42))), &mut sink, 3);
    assert!(!coordinator.entries.contains_key("one"));
    let mut duplicate = part("one", 999);
    duplicate.judgement.reason = "错误的重复结论".into();
    let calls = sink.inserts;
    let notices = coordinator.handle(Event::Insert(Box::new(duplicate)), &mut sink, 4);
    assert_eq!(sink.inserts, calls);
    assert!(!notices.iter().any(|notice| matches!(notice, Notice::Inserted(_))));
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 5);
    let after = sink.detail("one");
    assert_eq!(original(&after), original(&before));
    assert_eq!(after.shots[0].raw_files, before.shots[0].raw_files);
    assert_eq!(after.recording, before.recording);
    assert_eq!(after.summary.sn, 42);
    assert_eq!(after.summary.delivery.state, PlcDeliveryState::Acknowledged);
}

#[test]
fn partial_raw_events_union_views_and_conflicting_evidence_is_not_replaced() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Insert(Box::new(part("one", 42))), &mut sink, 1);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 1)], RecorderState::Incomplete)), &mut sink, 2);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 3)], RecorderState::Incomplete)), &mut sink, 3);
    let before = sink.detail("one");
    assert_eq!(before.shots[0].raw_files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 3]);
    let mut conflict = outcome("one", &[(0, 1)], RecorderState::Complete);
    conflict.files[0].file = "other-cycle/changed.pgm".into();
    let notices = coordinator.handle(Event::Recording(conflict), &mut sink, 4);
    assert!(has_message(&notices, "保留首次原图证据"));
    assert!(has_message(&notices, "保留首次收尾状态"));
    let after = sink.detail("one");
    assert_eq!(after.shots[0].raw_files, before.shots[0].raw_files);
    assert_eq!(after.recording, before.recording);
}

#[test]
fn invalid_file_path_or_empty_success_never_becomes_available() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    for (index, invalid) in ["../escape.pgm", "/escape.pgm"].into_iter().enumerate() {
        let cycle = format!("invalid-{index}");
        coordinator.handle(Event::Insert(Box::new(part(&cycle, 42))), &mut sink, 1);
        let mut recording = outcome(&cycle, &[(0, 1)], RecorderState::Complete);
        recording.files[0].file = invalid.into();
        let notices = coordinator.handle(Event::Recording(recording), &mut sink, 2);
        assert!(has_message(&notices, "原图") && has_message(&notices, "原检测结论不变"));
        let detail = sink.detail(&cycle);
        assert!(!detail.recording.available);
        assert!(detail.shots[0].raw_files.is_empty());
        assert!(!detail.recording.errors.is_empty());
    }
    coordinator.handle(Event::Insert(Box::new(part("empty", 42))), &mut sink, 3);
    coordinator.handle(Event::Recording(outcome("empty", &[], RecorderState::Complete)), &mut sink, 4);
    assert_eq!(sink.detail("empty").recording.state, RecordingState::Failed);
    assert!(!sink.detail("empty").recording.available);
}

#[test]
fn not_required_delivery_cannot_be_attached_to_a_plc_transaction() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Delivery("local".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 1);
    let mut record = part("local", 42);
    record.delivery = PlcDelivery::default();
    let notices = coordinator.handle(Event::Insert(Box::new(record)), &mut sink, 2);
    assert!(has_message(&notices, "不能相互替换"));
    assert_eq!(sink.detail("local").summary.delivery.state, PlcDeliveryState::NotRequired);
}

#[test]
fn cache_miss_restores_raw_union_and_ack_before_merging_late_events() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(1, 1);
    coordinator.handle(Event::Insert(Box::new(part("one", 42))), &mut sink, 1);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 1)], RecorderState::Incomplete)), &mut sink, 2);
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 3);
    coordinator.handle(Event::Insert(Box::new(part("two", 42))), &mut sink, 4);
    assert!(!coordinator.entries.contains_key("one"));
    let calls = sink.delivery_writes;
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Submitted, 99)), &mut sink, 5);
    assert_eq!(sink.delivery_writes, calls);
    assert_eq!(sink.detail("one").summary.delivery.state, PlcDeliveryState::Acknowledged);
    coordinator.handle(Event::Recording(outcome("one", &[(0, 3)], RecorderState::Incomplete)), &mut sink, 6);
    assert_eq!(sink.detail("one").shots[0].raw_files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 3]);
}

#[test]
fn concurrent_duplicate_insert_uses_exact_existing_cycle_and_keeps_pending_events() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 1);
    let original = part("one", 42);
    sink.store.insert(&original.borrowed()).unwrap();
    let mut duplicate = original.clone();
    duplicate.recipe = None;
    duplicate.judgement.reason = "重复内容无效也不能替换已有工件".into();
    let notices = coordinator.handle(Event::Insert(Box::new(duplicate)), &mut sink, 2);
    assert!(has_message(&notices, "保留原记录及原图证据"));
    assert_eq!(sink.detail("one").judgement.reason, original.judgement.reason);
    assert_eq!(sink.detail("one").summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
}

#[test]
fn cache_miss_database_failure_retains_ack_and_reloads_existing_evidence_on_retry() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    sink.store.insert(&part("one", 42).borrowed()).unwrap();
    sink.fail_find = 1;
    sink.fail_delivery = 1;
    let mut coordinator = Coordinator::new(8, 16);
    let notices = coordinator.handle(Event::Delivery("one".into(), delivery(PlcDeliveryState::Acknowledged, 20)), &mut sink, 1);
    assert!(has_message(&notices, "磁盘空间不足"));
    assert_eq!(sink.detail("one").summary.delivery.state, PlcDeliveryState::Pending);
    assert!(coordinator.entries["one"].pending());
    retry_due(&mut coordinator, &mut sink, "one");
    assert_eq!(sink.detail("one").summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert!(!coordinator.entries["one"].pending());
}

#[test]
fn changed_events_merge_evidence_without_bypassing_retry_backoff() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    sink.fail_insert = 100;
    let mut coordinator = Coordinator::new(8, 16);
    let record = part("backoff", 42);
    coordinator.handle(Event::Insert(Box::new(record.clone())), &mut sink, 1);
    let due = coordinator.entries["backoff"].retry_at;
    for now in 2..100 {
        let mut conflicting = record.clone();
        conflicting.judgement.reason = "重复事件不得替换原结论".into();
        coordinator.handle(Event::Insert(Box::new(conflicting)), &mut sink, now);
        coordinator.handle(Event::Delivery("backoff".into(), delivery(PlcDeliveryState::Acknowledged, now)), &mut sink, now);
    }
    coordinator.handle(Event::Recording(outcome("backoff", &[(0, 1), (0, 2), (0, 3)], RecorderState::Complete)), &mut sink, due - 1);
    assert_eq!(sink.counts(), (1, 0, 0, 0));
    assert_eq!(coordinator.entries["backoff"].retry_at, due);
    assert_eq!(coordinator.entries["backoff"].failures, 1);
    assert!(coordinator.retry(&mut sink, due - 1).is_empty());
    sink.fail_insert = 0;
    let notices = coordinator.retry(&mut sink, due);
    assert!(notices.contains(&Notice::Inserted(1)));
    let detail = sink.detail("backoff");
    assert_eq!(detail.judgement.reason, record.judgement.reason);
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(detail.summary.delivery.updated_at, 99);
    assert_eq!(detail.recording.state, RecordingState::Complete);
    assert_eq!(coordinator.entries["backoff"].failures, 0);
    assert_eq!(coordinator.entries["backoff"].retry_at, 0);
    let counts = sink.counts();
    assert!(coordinator.retry(&mut sink, due + RETRY_MAX_MS).is_empty());
    assert_eq!(sink.counts(), counts);
}

#[test]
fn retry_counter_and_deadline_saturate_without_stopping_future_attempts() {
    let mut entry = CycleEntry { failures: u32::MAX, ..Default::default() };
    entry.defer_retry(42);
    assert_eq!(entry.failures, u32::MAX);
    assert_eq!(entry.retry_at, 42 + RETRY_MAX_MS);
    entry.defer_retry(i64::MAX - 1);
    assert_eq!(entry.failures, u32::MAX);
    assert_eq!(entry.retry_at, i64::MAX);
}

#[test]
fn real_sqlite_lock_past_three_failures_recovers_original_part_ack_and_recording_without_new_events() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut coordinator = Coordinator::new(8, 16);
    coordinator.handle(Event::Insert(Box::new(part("baseline", 42))), &mut sink, 0);
    let baseline = sink.detail("baseline");
    let lock = rusqlite::Connection::open(dir.0.join("parts.sqlite")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let record = part("locked-cycle", 42);
    coordinator.handle(Event::Insert(Box::new(record.clone())), &mut sink, 1);
    coordinator.handle(Event::Delivery("locked-cycle".into(), delivery(PlcDeliveryState::Acknowledged, 77)), &mut sink, 2);
    let recording = outcome("locked-cycle", &[(0, 1), (0, 2), (0, 3), (1, 1), (1, 2), (1, 3)], RecorderState::Complete);
    coordinator.handle(Event::Recording(recording.clone()), &mut sink, 3);
    for _ in 0..3 {
        let due = coordinator.entries["locked-cycle"].retry_at;
        let notices = coordinator.retry(&mut sink, due);
        assert!(notices.iter().any(|notice| matches!(notice, Notice::Log { level: "err", event: "追溯入库失败", .. })));
    }
    assert_eq!(coordinator.entries["locked-cycle"].failures, 4);
    assert!(coordinator.entries["locked-cycle"].record.is_some());
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
    lock.execute_batch("ROLLBACK").unwrap();
    let due = coordinator.entries["locked-cycle"].retry_at;
    let notices = coordinator.retry(&mut sink, due);
    assert!(notices.iter().any(|notice| matches!(notice, Notice::Inserted(_))));
    let after = sink.detail("locked-cycle");
    assert_eq!(original(&after), original(&baseline));
    assert_eq!(after.summary.cycle_id.as_deref(), Some("locked-cycle"));
    assert_eq!(after.summary.sn, 42);
    assert_eq!(after.summary.bundle_id, record.bundle_id);
    assert_eq!(after.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(after.summary.delivery.updated_at, 77);
    assert_eq!(after.recording.state, RecordingState::Complete);
    assert!(after.recording.available && after.recording.errors.is_empty());
    assert_eq!(after.recording.directory, recording.directory.map(|path| path.to_string_lossy().into_owned()));
    for shot in &after.shots {
        assert_eq!(shot.raw_files.iter().map(|raw| raw.view).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(shot.raw_files.iter().all(|raw| !raw.file.is_empty()));
    }
    assert_eq!(sink.detail("baseline").summary.delivery.state, PlcDeliveryState::Pending);
    assert!(!coordinator.entries["locked-cycle"].pending());
    assert_eq!(coordinator.entries["locked-cycle"].failures, 0);
    let counts = sink.counts();
    assert!(coordinator.retry(&mut sink, due + RETRY_MAX_MS).is_empty());
    assert_eq!(sink.counts(), counts);
}


#[test]
fn durable_spool_survives_coordinator_ttl_capacity_and_replays_out_of_order_evidence_after_restart() {
    let dir = TestDir::new();
    let spool_path = dir.0.join("spool");
    let mut spool = spool::Spool::open(spool_path.clone()).unwrap();
    let mut sink = dir.sink();
    let expected = part("durable", 42);
    let expected_judgement = serde_json::to_value(&expected.judgement).unwrap();
    spool.append(&Event::Recording(outcome("durable", &[(0, 1), (0, 2), (0, 3)], RecorderState::Complete))).unwrap();
    spool.append(&Event::Delivery("durable".into(), delivery(PlcDeliveryState::Acknowledged, 77))).unwrap();
    spool.append(&Event::Insert(Box::new(expected.clone()))).unwrap();
    let mut volatile = Coordinator::new(1, 1);
    sink.fail_insert = usize::MAX;
    for (_, event) in spool.events("durable").unwrap() { volatile.handle(event, &mut sink, 1); }
    volatile.retry(&mut sink, PENDING_TTL_MS + 1);
    assert!(volatile.entries.is_empty());
    assert!(!spool.empty());
    drop(volatile);
    drop(spool);
    let mut reopened = spool::Spool::open(spool_path).unwrap();
    sink.fail_insert = 0;
    assert!(replay_cycle(&mut reopened, "durable", &mut sink, PENDING_TTL_MS + 2).unwrap().0);
    assert!(reopened.empty());
    let detail = sink.detail("durable");
    assert_eq!(serde_json::to_value(&detail.judgement).unwrap(), expected_judgement);
    assert_eq!(detail.summary.sn, expected.sn);
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(detail.summary.delivery.updated_at, 77);
    assert_eq!(detail.recording.state, RecordingState::Complete);
    assert!(detail.recording.available);
    assert_eq!(detail.shots[0].raw_files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 2, 3]);
}

#[test]
fn durable_spool_real_sqlite_lock_keeps_receipts_then_recovers_without_new_events() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool.append(&Event::Insert(Box::new(part("locked-spool", 42)))).unwrap();
    spool.append(&Event::Delivery("locked-spool".into(), delivery(PlcDeliveryState::Acknowledged, 99))).unwrap();
    let lock = rusqlite::Connection::open(dir.0.join("parts.sqlite")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(!replay_cycle(&mut spool, "locked-spool", &mut sink, 1).unwrap().0);
    assert_eq!(spool.events("locked-spool").unwrap().len(), 2);
    lock.execute_batch("ROLLBACK").unwrap();
    assert!(replay_cycle(&mut spool, "locked-spool", &mut sink, 2).unwrap().0);
    assert!(spool.empty());
    let detail = sink.detail("locked-spool");
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(detail.summary.delivery.updated_at, 99);
    assert_eq!(detail.judgement.reason, "原始检测结论：P2 缺帧");
}

#[test]
fn durable_spool_replays_sqlite_commit_before_receipt_cleanup_without_duplicate_or_regression() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool.append(&Event::Insert(Box::new(part("committed", 42)))).unwrap();
    spool.append(&Event::Delivery("committed".into(), delivery(PlcDeliveryState::Acknowledged, 88))).unwrap();
    spool.append(&Event::Recording(outcome("committed", &[(0, 2)], RecorderState::Complete))).unwrap();
    let mut coordinator = Coordinator::new(1, 1);
    for (_, event) in spool.events("committed").unwrap() { coordinator.handle(event, &mut sink, 1); }
    assert!(!coordinator.entries["committed"].pending());
    let before = sink.detail("committed");
    let counts = sink.counts();
    drop(coordinator);
    assert!(replay_cycle(&mut spool, "committed", &mut sink, 2).unwrap().0);
    assert_eq!(sink.counts(), counts);
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
    assert_eq!(original(&sink.detail("committed")), original(&before));
    assert_eq!(sink.detail("committed").recording, before.recording);
    assert_eq!(sink.detail("committed").summary.delivery, before.summary.delivery);
}

#[test]
fn durable_spool_capacity_bad_json_and_partial_commit_fail_closed_without_removing_evidence() {
    let dir = TestDir::new();
    let path = dir.0.join("bounded");
    let mut spool = spool::Spool::with_limits(path.clone(), 1, 1024 * 1024).unwrap();
    spool.append(&Event::Insert(Box::new(part("one", 42)))).unwrap();
    assert!(spool.append(&Event::Insert(Box::new(part("two", 42)))).is_err());
    assert_eq!(spool.cycles().into_iter().collect::<Vec<_>>(), ["one"]);
    drop(spool);
    assert_eq!(spool::Spool::open(path.clone()).unwrap().cycles().len(), 1);
    let partial = path.join("00000000000000000002.tmp");
    std::fs::write(&partial, b"{\"partial\":").unwrap();
    assert!(spool::Spool::open(path.clone()).is_err());
    assert!(partial.is_file());
    std::fs::remove_file(&partial).unwrap();
    let invalid = path.join("00000000000000000002.json");
    std::fs::write(&invalid, b"broken-json").unwrap();
    assert!(spool::Spool::open(path.clone()).is_err());
    assert_eq!(std::fs::read(&invalid).unwrap(), b"broken-json");
    assert!(path.join("00000000000000000001.json").is_file());
}

#[test]
fn durable_spool_protects_promoted_and_pending_directories_before_recording_callback_exists() {
    let dir = TestDir::new();
    let records = dir.0.join("records");
    let promoted = records.join("20261010").join("20261010_120000_000_OK_cycle_active");
    let pending = records.join("_pending").join("20261010_120000_000_cycle_active");
    std::fs::create_dir_all(&promoted).unwrap();
    std::fs::create_dir_all(&pending).unwrap();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool.append(&Event::Insert(Box::new(part("active", 42)))).unwrap();
    let protected = spool.protected_directories(&records).unwrap();
    assert!(protected.contains(&promoted.canonicalize().unwrap()));
    assert!(protected.contains(&pending.canonicalize().unwrap()));
    let outside = dir.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let mut recording = outcome("active", &[], RecorderState::Failed);
    recording.directory = Some(outside);
    spool.append(&Event::Recording(recording)).unwrap();
    assert!(spool.protected_directories(&records).is_err());
}

#[test]
fn durable_spool_roundtrip_preserves_actual_point_states_and_nan_without_business_fingerprints() {
    let dir = TestDir::new();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    let mut record = part("points", 42);
    record.table = Some(vec![PointState::Pending, PointState::Measured { d: f32::NAN, w: 4.5 }, PointState::Gap, PointState::Invalid]);
    spool.append(&Event::Insert(Box::new(record))).unwrap();
    let (_, Event::Insert(record)) = spool.events("points").unwrap().remove(0) else { panic!("wrong persisted event"); };
    let table = record.table.unwrap();
    assert_eq!(table[0], PointState::Pending);
    assert!(matches!(table[1], PointState::Measured { d, w } if d.is_nan() && w == 4.5));
    assert_eq!(table[2], PointState::Gap);
    assert_eq!(table[3], PointState::Invalid);
}

#[test]
#[ignore = "subprocess fixture invoked only by the parent durability regression"]
fn durable_spool_termination_child() {
    let Some(path) = std::env::var_os("GLUESIGHT_AUDIT_SPOOL_CHILD") else { return };
    let root = PathBuf::from(path);
    let mut spool = spool::Spool::open(root.join("spool")).unwrap();
    spool.append(&Event::Insert(Box::new(part("terminated", 42)))).unwrap();
    spool.append(&Event::Delivery("terminated".into(), delivery(PlcDeliveryState::Acknowledged, 101))).unwrap();
    let mut recording = outcome("terminated", &[(0, 1), (0, 2), (0, 3)], RecorderState::Complete);
    let records = root.join("records");
    let actual = records.join("cycle_terminated");
    std::fs::create_dir_all(&actual).unwrap();
    for file in &recording.files {
        let mut pixels = b"P5\n100 60\n255\n".to_vec();
        pixels.extend(vec![127; 6000]);
        std::fs::write(records.join(&file.file), pixels).unwrap();
    }
    recording.directory = Some(actual);
    spool.append(&Event::Recording(recording)).unwrap();
    let ready = std::fs::File::create(root.join("durable-ready")).unwrap();
    ready.sync_all().unwrap();
    loop { std::thread::park(); }
}

#[test]
fn durable_spool_recovers_after_real_process_termination_before_any_sqlite_insert() {
    use std::process::{Command, Stdio};
    struct Child(std::process::Child);
    impl Drop for Child { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let dir = TestDir::new();
    let mut child = Child(Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "audit::tests::durable_spool_termination_child", "--ignored", "--nocapture"])
        .env("GLUESIGHT_AUDIT_SPOOL_CHILD", &dir.0)
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !dir.0.join("durable-ready").exists() {
        assert!(child.0.try_wait().unwrap().is_none(), "durability fixture exited before commit");
        assert!(std::time::Instant::now() < deadline, "durability fixture did not commit in time");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(!dir.0.join("parts.sqlite").exists());
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    let mut sink = dir.sink();
    assert!(replay_cycle(&mut spool, "terminated", &mut sink, 1).unwrap().0);
    let detail = sink.detail("terminated");
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(detail.summary.delivery.updated_at, 101);
    assert_eq!(detail.judgement.reason, "原始检测结论：P2 缺帧");
    assert_eq!(detail.shots[1].error.as_deref(), Some("原始缺帧原因"));
    assert!(detail.recording.available);
    for file in &detail.shots[0].raw_files {
        let image = image::open(dir.0.join("records").join(&file.file)).unwrap();
        assert_eq!((image.width(), image.height()), (100, 60));
    }
    assert!(spool.empty());
}


#[test]
fn durable_submission_keeps_actual_plc_completion_timing_through_restart_and_ack() {
    let dir = TestDir::new();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    let mut sink = dir.sink();
    let mut record = part("timed", 42);
    record.drain_ms = None;
    spool.append(&Event::Submission("timed".into(), delivery(PlcDeliveryState::Submitted, 50), Some(250))).unwrap();
    spool.append(&Event::Insert(Box::new(record))).unwrap();
    spool.append(&Event::Delivery("timed".into(), delivery(PlcDeliveryState::Acknowledged, 100))).unwrap();
    let (complete, notices) = replay_cycle(&mut spool, "timed", &mut sink, 1).unwrap();
    assert!(complete, "{notices:#?}");
    assert_eq!(sink.detail("timed").summary.drain_ms, Some(250));
    assert_eq!(sink.detail("timed").summary.delivery.state, PlcDeliveryState::Acknowledged);
    spool.append(&Event::Submission("timed".into(), delivery(PlcDeliveryState::Failed, 200), None)).unwrap();
    assert!(replay_cycle(&mut spool, "timed", &mut sink, 2).unwrap().0);
    assert_eq!(sink.detail("timed").summary.drain_ms, Some(250));
    assert_eq!(sink.detail("timed").summary.delivery.state, PlcDeliveryState::Acknowledged);
    spool.append(&Event::Submission("timed".into(), delivery(PlcDeliveryState::Submitted, 50), Some(251))).unwrap();
    assert!(!replay_cycle(&mut spool, "timed", &mut sink, 3).unwrap().0);
    assert_eq!(sink.detail("timed").summary.drain_ms, Some(250));
    assert!(!spool.empty());
}


#[test]
fn durable_spool_snapshot_cleanup_preserves_ack_appended_during_replay() {
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    let mut sink = dir.sink();
    spool.append(&Event::Insert(Box::new(part("interleaved", 42)))).unwrap();
    let snapshot = spool.events("interleaved").unwrap();
    spool.append(&Event::Delivery("interleaved".into(), delivery(PlcDeliveryState::Acknowledged, 100))).unwrap();
    let (complete, receipts, notices) = apply_spooled(snapshot, "interleaved", &mut sink, 1);
    assert!(complete, "{notices:#?}");
    assert_eq!(sink.detail("interleaved").summary.delivery.state, PlcDeliveryState::Pending);
    let original_content = original(&sink.detail("interleaved"));
    spool.remove(&receipts).unwrap();
    drop(spool);
    let mut spool = spool::Spool::open(path).unwrap();
    let pending = spool.events("interleaved").unwrap();
    assert_eq!(pending.len(), 1);
    assert!(matches!(&pending[0].1, Event::Delivery(cycle, delivery) if cycle == "interleaved" && delivery.state == PlcDeliveryState::Acknowledged));
    let (complete, notices) = replay_cycle(&mut spool, "interleaved", &mut sink, 2).unwrap();
    assert!(complete, "{notices:#?}");
    assert_eq!(sink.detail("interleaved").summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(original(&sink.detail("interleaved")), original_content);
    assert_eq!(sink.inserts, 1);
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
    assert!(spool.empty());
}

#[test]
fn durable_spool_conflicting_cycle_insert_retains_all_receipts_and_never_replaces_original() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    let original_record = part("same-cycle", 42);
    sink.store.insert(&original_record.borrowed()).unwrap();
    let original_content = original(&sink.detail("same-cycle"));
    let mut conflict = original_record.clone();
    conflict.sn = 43;
    spool.append(&Event::Insert(Box::new(conflict))).unwrap();
    assert!(!replay_cycle(&mut spool, "same-cycle", &mut sink, 1).unwrap().0);
    assert_eq!(sink.detail("same-cycle").summary.sn, 42);
    assert_eq!(original(&sink.detail("same-cycle")), original_content);
    assert_eq!(spool.events("same-cycle").unwrap().len(), 1);
    let mut clean_spool = spool::Spool::open(dir.0.join("two-pending")).unwrap();
    let one = part("not-yet-inserted", 42);
    let mut two = one.clone();
    two.judgement.reason = "different actual result".into();
    clean_spool.append(&Event::Insert(Box::new(one))).unwrap();
    clean_spool.append(&Event::Insert(Box::new(two))).unwrap();
    assert!(!replay_cycle(&mut clean_spool, "not-yet-inserted", &mut sink, 2).unwrap().0);
    assert!(sink.store.detail_by_cycle("not-yet-inserted").unwrap().is_none());
    assert_eq!(clean_spool.events("not-yet-inserted").unwrap().len(), 2);
}

#[test]
fn startup_replay_completes_durable_recording_before_interrupted_pending_is_marked_failed() {
    let dir = TestDir::new();
    let sink = dir.sink();
    let record = part("startup-complete", 42);
    sink.store.insert(&record.borrowed()).unwrap();
    assert_eq!(sink.detail("startup-complete").recording.state, RecordingState::Pending);
    let path = dir.0.join("spool");
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    spool.append(&Event::Recording(outcome("startup-complete", &[(0, 2)], RecorderState::Complete))).unwrap();
    drop(sink);
    drop(spool);
    let mut sink = dir.sink_mode(true);
    let mut spool = spool::Spool::open(path).unwrap();
    assert!(replay_cycle(&mut spool, "startup-complete", &mut sink, 1).unwrap().0);
    assert_eq!(sink.store.finish_interrupted_recordings().unwrap(), 0);
    assert_eq!(sink.detail("startup-complete").recording.state, RecordingState::Complete);
    assert!(sink.detail("startup-complete").recording.available);
}


#[cfg(windows)]
#[test]
fn durable_spool_detects_real_windows_write_denial_without_losing_committed_event() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    spool.append(&Event::Insert(Box::new(part("protected", 42)))).unwrap();
    let locked = std::fs::OpenOptions::new().read(true).share_mode(1).open(path.join(".health")).unwrap();
    assert!(spool.probe().is_err());
    assert_eq!(spool.events("protected").unwrap().len(), 1);
    drop(locked);
    spool.probe().unwrap();
    let mut sink = dir.sink();
    assert!(replay_cycle(&mut spool, "protected", &mut sink, 1).unwrap().0);
    assert!(spool.empty());
}

#[cfg(windows)]
#[test]
fn durable_spool_rejects_windows_junction_roots_and_promoted_recording_children() {
    use std::os::windows::process::CommandExt;
    let dir = TestDir::new();
    let outside = dir.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let sentinel = outside.join("keep.txt");
    std::fs::write(&sentinel, b"preserve").unwrap();
    let linked_root = dir.0.join("linked-root");
    let output = std::process::Command::new("cmd.exe").args(["/C", "mklink", "/J"])
        .arg(&linked_root).arg(&outside).creation_flags(0x08000000).output().unwrap();
    assert!(output.status.success());
    assert!(spool::Spool::open(linked_root.join("spool")).is_err());
    assert!(!outside.join("spool").exists());
    std::fs::remove_dir(&linked_root).unwrap();
    let records = dir.0.join("records");
    let day = records.join("20261010");
    std::fs::create_dir_all(&day).unwrap();
    let promoted = day.join("20261010_120000_000_OK_cycle_active");
    let output = std::process::Command::new("cmd.exe").args(["/C", "mklink", "/J"])
        .arg(&promoted).arg(&outside).creation_flags(0x08000000).output().unwrap();
    assert!(output.status.success());
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool.append(&Event::Insert(Box::new(part("active", 42)))).unwrap();
    assert!(spool.protected_directories(&records).is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserve");
    std::fs::remove_dir(&promoted).unwrap();
}


#[test]
fn durable_spool_failed_append_serializes_ready_and_cleanup_without_an_empty_window() {
    let dir = TestDir::new();
    let spool = Arc::new(Mutex::new(spool::Spool::with_limits(dir.0.join("spool"), 1, 1).unwrap()));
    let failure = Arc::new(Mutex::new(None));
    let running = Arc::new(AtomicBool::new(true));
    let actions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (writer_spool, writer_failure) = (spool.clone(), failure.clone());
    let writer = std::thread::spawn(move || spool_io(&writer_spool, &writer_failure, |spool| {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        spool.append(&Event::Delivery("rejected".into(), delivery(PlcDeliveryState::Acknowledged, 1)))
    }));
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(spool.try_lock().is_err());
    assert!(failure.lock().unwrap().is_none());
    let (attempted_tx, attempted_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let readers = (0..2).map(|_| {
        let (spool, failure, running, actions) = (spool.clone(), failure.clone(), running.clone(), actions.clone());
        let (attempted_tx, finished_tx) = (attempted_tx.clone(), finished_tx.clone());
        std::thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            let result = with_ready_spool(&spool, &failure, &running, &Mutex::new(BTreeMap::new()), || {
                actions.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            finished_tx.send(result).unwrap();
        })
    }).collect::<Vec<_>>();
    for _ in 0..2 { attempted_rx.recv_timeout(Duration::from_secs(5)).unwrap(); }
    assert!(finished_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    let error = writer.join().unwrap().unwrap_err();
    assert!(error.contains("容量不足"));
    for _ in 0..2 { assert_eq!(finished_rx.recv_timeout(Duration::from_secs(5)).unwrap(), Err(error.clone())); }
    for reader in readers { reader.join().unwrap(); }
    assert_eq!(actions.load(Ordering::SeqCst), 0);
    assert!(spool.lock().unwrap().empty());
    assert_eq!(*failure.lock().unwrap(), Some(error));
}

#[test]
fn durable_spool_timeout_failure_does_not_wait_for_the_io_lock() {
    let dir = TestDir::new();
    let spool = Mutex::new(spool::Spool::open(dir.0.join("spool")).unwrap());
    let failure = Arc::new(Mutex::new(None));
    let io = spool.lock().unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let publisher_failure = failure.clone();
    let publisher = std::thread::spawn(move || {
        done_tx.send(set_failure(&publisher_failure, "deadline expired".into())).unwrap();
    });
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Ok(()));
    assert!(io.empty());
    assert_eq!(*failure.lock().unwrap(), Some("deadline expired".into()));
    drop(io);
    publisher.join().unwrap();
    assert_eq!(with_ready_spool(&spool, &failure, &AtomicBool::new(true), &Mutex::new(BTreeMap::new()), || Ok(())), Err("deadline expired".into()));
}

#[cfg(windows)]
#[test]
fn durable_spool_empty_probe_failure_prevents_ready_and_cleanup() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let spool = Mutex::new(spool::Spool::open(path.clone()).unwrap());
    let failure = Mutex::new(None);
    let running = AtomicBool::new(true);
    let locked = std::fs::OpenOptions::new().read(true).share_mode(1).open(path.join(".health")).unwrap();
    let error = spool_io(&spool, &failure, |spool| spool.probe()).unwrap_err();
    assert!(spool.lock().unwrap().empty());
    assert_eq!(with_ready_spool(&spool, &failure, &running, &Mutex::new(BTreeMap::new()), || Ok(())), Err(error.clone()));
    let mut cleaned = false;
    assert_eq!(with_ready_spool(&spool, &failure, &running, &Mutex::new(BTreeMap::new()), || { cleaned = true; Ok(()) }), Err(error));
    assert!(!cleaned);
    drop(locked);
}

#[test]
fn durable_recording_barrier_waits_for_actual_recorder_callback_after_ack_and_empty_spool() {
    use crate::frame::{CounterSource, Frame, FrameImage};
    use crate::recorder::Recorder;
    use crate::settings::RecordMode;
    let dir = TestDir::new();
    let spool = Arc::new(Mutex::new(spool::Spool::open(dir.0.join("spool")).unwrap()));
    let failure = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    let running = AtomicBool::new(true);
    let record = part("slow-recorder", 42);
    track_recording(&spool, &failure, &pending, &record.cycle_id).unwrap();
    for event in [Event::Insert(Box::new(record.clone())),
        Event::Submission(record.cycle_id.clone(), delivery(PlcDeliveryState::Submitted, 20), Some(42)),
        Event::Delivery(record.cycle_id.clone(), delivery(PlcDeliveryState::Acknowledged, 30))] {
        persist_event(&spool, &failure, &pending, &event).unwrap();
    }
    let mut sink = dir.sink();
    assert!(replay_cycle(&mut spool.lock().unwrap(), &record.cycle_id, &mut sink, 1).unwrap().0);
    assert!(spool.lock().unwrap().empty());
    assert_eq!(sink.detail(&record.cycle_id).summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(sink.detail(&record.cycle_id).recording.state, RecordingState::Pending);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let (callback_spool, callback_failure, callback_pending) = (spool.clone(), failure.clone(), pending.clone());
    let callback = Arc::new(move |outcome: RecordingOutcome| {
        entered_tx.send(outcome.clone()).unwrap();
        release_rx.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        done_tx.send(persist_event(&callback_spool, &callback_failure, &callback_pending, &Event::Recording(outcome))).unwrap();
    });
    let recorder = Recorder::new(dir.0.join("records"), Some(callback));
    let recipe = record.recipe.clone().unwrap();
    let mut recording = recorder.begin(RecordMode::All, record.sn, recipe, &record.cycle_id, record.bundle_id.as_deref()).unwrap();
    recorder.frame(&mut recording, &Frame {
        cam: 0, session: u64::MAX, counter: CounterSource::Synthetic,
        frame_counter: u64::MAX - 1, trigger_counter: u64::MAX - 2,
        lost_packets: 0, ts: 777, manual: false,
        images: (0..3).map(|view| Arc::new(FrameImage::new(1, 1, vec![20 + view]))).collect(),
    }, "cam1", 0, 1);
    recorder.finish(recording, record.judgement.verdict, &record.judgement.reason, 10, u64::MAX, Vec::new());
    let actual = entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(actual.state, RecorderState::Incomplete);
    assert_eq!(actual.files.len(), 3);
    assert!(spool.lock().unwrap().empty() && failure.lock().unwrap().is_none());
    let mut actions = 0;
    for _ in 0..2 {
        assert!(with_ready_spool(&spool, &failure, &running, &pending, || { actions += 1; Ok(()) }).unwrap_err().contains("录制尚未完成"));
    }
    assert_eq!(actions, 0);
    release_tx.send(()).unwrap();
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(()));
    assert!(pending.lock().unwrap().is_empty());
    assert!(with_ready_spool(&spool, &failure, &running, &pending, || Ok(())).is_err());
    assert!(replay_cycle(&mut spool.lock().unwrap(), &record.cycle_id, &mut sink, 2).unwrap().0);
    assert_eq!(sink.detail(&record.cycle_id).recording.state, RecordingState::Incomplete);
    assert_eq!(sink.detail(&record.cycle_id).summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(sink.detail(&record.cycle_id).summary.plc_code, record.judgement.plc_code);
    assert_eq!(with_ready_spool(&spool, &failure, &running, &pending, || Ok(())), Ok(()));
    drop(recorder);
}

#[test]
fn durable_recording_barrier_keeps_failed_terminal_append_latched_and_pending() {
    let dir = TestDir::new();
    let spool = Mutex::new(spool::Spool::with_limits(dir.0.join("spool"), 1, 1).unwrap());
    let failure = Mutex::new(None);
    let pending = Mutex::new(BTreeMap::new());
    track_recording(&spool, &failure, &pending, "disk-full-recording").unwrap();
    persist_event(&spool, &failure, &pending, &Event::Recording(outcome("disk-full-recording", &[], RecorderState::Failed))).unwrap();
    let error = persist_event(&spool, &failure, &pending, &Event::Insert(Box::new(part("disk-full-recording", 1)))).unwrap_err();
    assert!(error.contains("容量不足"));
    assert!(spool.lock().unwrap().empty());
    assert!(pending.lock().unwrap().contains_key("disk-full-recording"));
    assert_eq!(*failure.lock().unwrap(), Some(error.clone()));
    let mut cleaned = false;
    assert_eq!(with_ready_spool(&spool, &failure, &AtomicBool::new(true), &pending, || { cleaned = true; Ok(()) }), Err(error));
    assert!(!cleaned);
}

#[test]
fn durable_recording_barrier_handles_synchronous_off_callback_before_insert() {
    use crate::recorder::Recorder;
    use crate::settings::RecordMode;
    let dir = TestDir::new();
    let spool = Arc::new(Mutex::new(spool::Spool::open(dir.0.join("spool")).unwrap()));
    let failure = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    let record = part("off-before-insert", 42);
    track_recording(&spool, &failure, &pending, &record.cycle_id).unwrap();
    let (callback_spool, callback_failure, callback_pending) = (spool.clone(), failure.clone(), pending.clone());
    let recorder = Recorder::new(dir.0.join("records"), Some(Arc::new(move |outcome| {
        persist_event(&callback_spool, &callback_failure, &callback_pending, &Event::Recording(outcome)).unwrap();
    })));
    assert!(recorder.begin(RecordMode::Off, record.sn, record.recipe.clone().unwrap(), &record.cycle_id, record.bundle_id.as_deref()).is_none());
    assert!(pending.lock().unwrap().contains_key(&record.cycle_id));
    assert!(spool.lock().unwrap().empty());
    assert!(with_ready_spool(&spool, &failure, &AtomicBool::new(true), &pending, || Ok(())).is_err());
    persist_event(&spool, &failure, &pending, &Event::Insert(Box::new(record.clone()))).unwrap();
    let mut sink = dir.sink();
    assert!(replay_cycle(&mut spool.lock().unwrap(), &record.cycle_id, &mut sink, 1).unwrap().0);
    assert_eq!(sink.detail(&record.cycle_id).recording.state, RecordingState::Off);
    assert_eq!(with_ready_spool(&spool, &failure, &AtomicBool::new(true), &pending, || Ok(())), Ok(()));
}


#[test]
fn durable_preinsert_recorder_off_crash_has_no_orphan_receipt() {
    use crate::recorder::Recorder;
    use crate::settings::RecordMode;
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let spool = Arc::new(Mutex::new(spool::Spool::open(path.clone()).unwrap()));
    let failure = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    let record = part("acquire-interrupted", 42);
    track_recording(&spool, &failure, &pending, &record.cycle_id).unwrap();
    let (cs, cf, cp) = (spool.clone(), failure.clone(), pending.clone());
    let recorder = Recorder::new(dir.0.join("records"), Some(Arc::new(move |outcome| {
        persist_event(&cs, &cf, &cp, &Event::Recording(outcome)).unwrap();
    })));
    assert!(recorder.begin(RecordMode::Off, record.sn, record.recipe.clone().unwrap(), &record.cycle_id, None).is_none());
    assert!(healthy_spool(&spool, &failure, &AtomicBool::new(true)).is_ok());
    assert_eq!(readiness(&spool.lock().unwrap(), &failure, &AtomicBool::new(true), &pending).unwrap(), AuditReadiness::WaitingRecording);
    drop(recorder);
    drop(spool);
    let reopened = spool::Spool::open(path).unwrap();
    let mut sink = dir.sink();
    assert!(reopened.empty());
    assert!(sink.find(&record.cycle_id).unwrap().is_none());
}

#[test]
fn durable_staged_terminal_append_failure_keeps_insert_receipt_and_latches() {
    let dir = TestDir::new();
    let spool = Mutex::new(spool::Spool::with_limits(dir.0.join("spool"), 1, 1024 * 1024).unwrap());
    let failure = Mutex::new(None);
    let pending = Mutex::new(BTreeMap::new());
    let cycle = "terminal-capacity";
    track_recording(&spool, &failure, &pending, cycle).unwrap();
    persist_event(&spool, &failure, &pending, &Event::Recording(outcome(cycle, &[], RecorderState::Off))).unwrap();
    let error = persist_event(&spool, &failure, &pending, &Event::Insert(Box::new(part(cycle, 1)))).unwrap_err();
    assert!(error.contains("容量不足"));
    assert!(matches!(&spool.lock().unwrap().events(cycle).unwrap()[0].1, Event::Insert(_)));
    assert!(pending.lock().unwrap()[cycle].inserted);
    assert!(pending.lock().unwrap()[cycle].terminal.is_some());
    assert_eq!(healthy_spool(&spool, &failure, &AtomicBool::new(true)), Err(error));
}

#[test]
fn durable_postbegin_health_rejects_synchronous_callback_failure_but_not_own_barrier() {
    use crate::recorder::Recorder;
    use crate::settings::RecordMode;
    let dir = TestDir::new();
    let spool = Arc::new(Mutex::new(spool::Spool::open(dir.0.join("spool")).unwrap()));
    let failure = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    let record = part("postbegin", 42);
    track_recording(&spool, &failure, &pending, &record.cycle_id).unwrap();
    let (cs, cf, cp) = (spool.clone(), failure.clone(), pending.clone());
    let recorder = Recorder::new(dir.0.join("records"), Some(Arc::new(move |mut outcome: RecordingOutcome| {
        outcome.files.push(RecordedRawFile { k: 0, view: 0, file: "bad.pgm".into(), width: 1, height: 1 });
        assert!(persist_event(&cs, &cf, &cp, &Event::Recording(outcome)).is_err());
    })));
    assert!(healthy_spool(&spool, &failure, &AtomicBool::new(true)).is_ok());
    assert!(recorder.begin(RecordMode::Off, record.sn, record.recipe.unwrap(), &record.cycle_id, None).is_none());
    assert!(healthy_spool(&spool, &failure, &AtomicBool::new(true)).is_err());
    assert!(spool.lock().unwrap().empty());
    assert!(pending.lock().unwrap().contains_key(&record.cycle_id));
}

#[test]
fn durable_legacy_recording_only_restart_archives_exact_events_and_protects_raw() {
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let records = dir.0.join("records");
    let raw = records.join("20261010/120000_cycle_legacy-orphan");
    std::fs::create_dir_all(&raw).unwrap();
    std::fs::write(raw.join("k000_P1_cam1_v2.pgm"), b"P5\n1 1\n255\nX").unwrap();
    let mut terminal = outcome("legacy-orphan", &[(0, 2)], RecorderState::Complete);
    terminal.directory = Some(raw.clone());
    terminal.files[0].file = "20261010/120000_cycle_legacy-orphan/k000_P1_cam1_v2.pgm".into();
    terminal.files[0].width = 1;
    terminal.files[0].height = 1;
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    spool.append(&Event::Recording(terminal.clone())).unwrap();
    let original = std::fs::read(path.join("00000000000000000001.json")).unwrap();
    drop(spool);
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    let mut sink = dir.sink_mode(true);
    let (complete, notices) = recover_cycle(&mut spool, "legacy-orphan", &mut sink, 1).unwrap();
    assert!(complete && !notices.is_empty());
    assert!(spool.empty());
    assert!(sink.find("legacy-orphan").unwrap().is_none());
    assert_eq!(std::fs::read(path.join(".interrupted-preinsert/00000000000000000001.json")).unwrap(), original);
    assert_eq!(spool.protected_directories(&records).unwrap(), vec![raw.canonicalize().unwrap()]);
    drop(spool);
    let spool = spool::Spool::open(path).unwrap();
    assert!(spool.empty());
    assert_eq!(spool.protected_directories(&records).unwrap(), vec![raw.canonicalize().unwrap()]);
    assert_eq!(std::fs::read(raw.join("k000_P1_cam1_v2.pgm")).unwrap(), b"P5\n1 1\n255\nX");
}

#[test]
fn durable_legacy_orphan_mixed_conflicting_or_lookup_error_remains_fail_closed() {
    let dir = TestDir::new();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    let mut sink = dir.sink_mode(true);
    for event in [Event::Recording(outcome("mixed", &[], RecorderState::Off)),
        Event::Submission("mixed".into(), delivery(PlcDeliveryState::Submitted, 2), Some(3))] { spool.append(&event).unwrap(); }
    assert!(!recover_cycle(&mut spool, "mixed", &mut sink, 1).unwrap().0);
    assert_eq!(spool.events("mixed").unwrap().len(), 2);
    spool.append(&Event::Recording(outcome("conflict", &[], RecorderState::Off))).unwrap();
    let mut conflicting = outcome("conflict", &[], RecorderState::Failed);
    conflicting.directory = None;
    spool.append(&Event::Recording(conflicting)).unwrap();
    assert!(recover_cycle(&mut spool, "conflict", &mut sink, 1).is_err());
    assert_eq!(spool.events("conflict").unwrap().len(), 2);
    spool.append(&Event::Recording(outcome("lookup", &[], RecorderState::Off))).unwrap();
    sink.fail_find = 1;
    assert!(recover_cycle(&mut spool, "lookup", &mut sink, 1).is_err());
    assert_eq!(spool.events("lookup").unwrap().len(), 1);
    assert!(!dir.0.join("spool/.interrupted-preinsert").exists());
}

#[test]
fn durable_health_probe_recovers_only_exact_own_single_zero_after_restart() {
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    spool.append(&Event::Insert(Box::new(part("probe-restart", 1)))).unwrap();
    drop(spool);
    std::fs::write(path.join(".health"), [0]).unwrap();
    let spool = spool::Spool::open(path.clone()).unwrap();
    assert_eq!(spool.events("probe-restart").unwrap().len(), 1);
    assert!(std::fs::read(path.join(".health")).unwrap().is_empty());
    drop(spool);
    for unknown in [vec![1], vec![0, 0], vec![0, 1, 2]] {
        std::fs::write(path.join(".health"), &unknown).unwrap();
        assert!(spool::Spool::open(path.clone()).is_err());
        assert_eq!(std::fs::read(path.join(".health")).unwrap(), unknown);
        assert!(path.join("00000000000000000001.json").exists());
    }
}


#[test]
fn durable_legacy_synchronous_off_or_failed_orphan_restart_keeps_original_archive() {
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    for (cycle, state) in [("old-off", RecorderState::Off), ("old-failed", RecorderState::Failed)] {
        let mut terminal = outcome(cycle, &[], state);
        terminal.directory = None;
        spool.append(&Event::Recording(terminal)).unwrap();
    }
    let originals = [1, 2].map(|sequence| std::fs::read(path.join(format!("{sequence:020}.json"))).unwrap());
    drop(spool);
    let mut sink = dir.sink_mode(true);
    let mut spool = spool::Spool::open(path.clone()).unwrap();
    for cycle in spool.cycles() {
        assert!(recover_cycle(&mut spool, &cycle, &mut sink, 1).unwrap().0);
        assert!(sink.find(&cycle).unwrap().is_none());
    }
    assert!(spool.empty());
    let mut archived = std::fs::read_dir(path.join(".interrupted-preinsert")).unwrap().map(|entry| std::fs::read(entry.unwrap().path()).unwrap()).collect::<Vec<_>>();
    archived.sort();
    let mut expected = originals.to_vec();
    expected.sort();
    assert_eq!(archived, expected);
    drop(spool);
    let spool = spool::Spool::open(path).unwrap();
    assert!(spool.empty());
    assert!(sink.store.query(&HistoryQuery::default()).unwrap().items.is_empty());
}

#[cfg(windows)]
#[test]
fn durable_postbegin_health_probes_disk_even_with_own_pending_recording() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let spool = Mutex::new(spool::Spool::open(path.clone()).unwrap());
    let failure = Mutex::new(None);
    let pending = Mutex::new(BTreeMap::new());
    track_recording(&spool, &failure, &pending, "before-camera").unwrap();
    let running = AtomicBool::new(true);
    assert!(healthy_spool(&spool, &failure, &running).is_ok());
    let locked = std::fs::OpenOptions::new().read(true).share_mode(1).open(path.join(".health")).unwrap();
    assert!(healthy_spool(&spool, &failure, &running).is_err());
    assert!(failure.lock().unwrap().is_some());
    assert!(pending.lock().unwrap().contains_key("before-camera"));
    drop(locked);
}


#[test]
fn durable_directory_probe_restart_recovers_only_owned_names_and_exact_bytes() {
    for marker in [".health-create.tmp", ".health-commit"] {
        for bytes in [Vec::new(), vec![0]] {
            let dir = TestDir::new();
            let path = dir.0.join("spool");
            let mut spool = spool::Spool::open(path.clone()).unwrap();
            spool.append(&Event::Insert(Box::new(part("marker-restart", 1)))).unwrap();
            let original = std::fs::read(path.join("00000000000000000001.json")).unwrap();
            drop(spool);
            std::fs::write(path.join(marker), &bytes).unwrap();
            let spool = spool::Spool::open(path.clone()).unwrap();
            assert_eq!(spool.events("marker-restart").unwrap().len(), 1);
            assert_eq!(std::fs::read(path.join("00000000000000000001.json")).unwrap(), original);
            assert!(!path.join(".health-create.tmp").exists());
            assert!(!path.join(".health-commit").exists());
        }
        for bytes in [vec![1], vec![0, 0], b"{\"Insert\":{}}".to_vec()] {
            let dir = TestDir::new();
            let path = dir.0.join("spool");
            let mut spool = spool::Spool::open(path.clone()).unwrap();
            spool.append(&Event::Insert(Box::new(part("unknown-marker", 1)))).unwrap();
            drop(spool);
            std::fs::write(path.join(marker), &bytes).unwrap();
            assert!(spool::Spool::open(path.clone()).is_err());
            assert_eq!(std::fs::read(path.join(marker)).unwrap(), bytes);
            assert!(path.join("00000000000000000001.json").exists());
        }
    }
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    drop(spool::Spool::open(path.clone()).unwrap());
    std::fs::write(path.join("00000000000000000001.tmp"), [0]).unwrap();
    assert!(spool::Spool::open(path.clone()).is_err());
    assert_eq!(std::fs::read(path.join("00000000000000000001.tmp")).unwrap(), [0]);
}

#[cfg(windows)]
#[test]
fn durable_directory_probe_cleanup_denial_latches_failure_and_preserves_receipts() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let mut actual = spool::Spool::open(path.clone()).unwrap();
    actual.append(&Event::Insert(Box::new(part("probe-cleanup-denied", 1)))).unwrap();
    let original = std::fs::read(path.join("00000000000000000001.json")).unwrap();
    std::fs::write(path.join(".health-commit"), [0]).unwrap();
    let locked = std::fs::OpenOptions::new().read(true).share_mode(1).open(path.join(".health-commit")).unwrap();
    let spool = Mutex::new(actual);
    let failure = Mutex::new(None);
    let error = spool_io(&spool, &failure, |spool| spool.probe()).unwrap_err();
    assert!(error.contains("恢复中断审计探针失败"));
    assert_eq!(*failure.lock().unwrap(), Some(error));
    assert_eq!(std::fs::read(path.join(".health-commit")).unwrap(), [0]);
    assert_eq!(std::fs::read(path.join("00000000000000000001.json")).unwrap(), original);
    drop(locked);
    assert!(spool.lock().unwrap().probe().is_ok());
}

#[cfg(windows)]
#[test]
fn durable_directory_probe_rejects_create_denied_when_existing_health_is_writable() {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    struct DenyCreate { path: PathBuf, restore: bool }
    impl DenyCreate {
        fn set(path: &std::path::Path, deny: bool) -> Result<(), String> {
            let script = r#"$ErrorActionPreference='Stop'; Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop; $p=$env:GLUESIGHT_PROBE_ACL_PATH; $acl=Get-Acl -LiteralPath $p; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,[System.Security.AccessControl.FileSystemRights]::CreateFiles,[System.Security.AccessControl.InheritanceFlags]::None,[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Deny); if($env:GLUESIGHT_PROBE_ACL_DENY -eq '1'){$acl.AddAccessRule($rule)}else{$acl.RemoveAccessRuleSpecific($rule)}; Set-Acl -LiteralPath $p -AclObject $acl"#;
            let output = std::process::Command::new(std::env::var_os("GLUESIGHT_TEST_PWSH").unwrap_or_else(|| "pwsh".into()))
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .env("GLUESIGHT_PROBE_ACL_PATH", path).env("GLUESIGHT_PROBE_ACL_DENY", if deny { "1" } else { "0" })
                .creation_flags(0x08000000).output().map_err(|error| format!("启动 PowerShell 7 ACL 测试失败：{error}"))?;
            if !output.status.success() {
                return Err(format!("ACL 操作失败 status={} stdout={} stderr={}", output.status, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
            }
            Ok(())
        }
        fn restore(&mut self) -> Result<(), String> {
            Self::set(&self.path, false)?;
            self.restore = false;
            Ok(())
        }
    }
    impl Drop for DenyCreate {
        fn drop(&mut self) {
            if self.restore {
                if let Err(error) = Self::set(&self.path, false) { let _ = writeln!(std::io::stderr(), "ACL 清理失败：{error}"); }
            }
        }
    }
    let dir = TestDir::new();
    let path = dir.0.join("spool");
    let mut actual = spool::Spool::open(path.clone()).unwrap();
    actual.append(&Event::Insert(Box::new(part("probe-create-denied", 1)))).unwrap();
    let original = std::fs::read(path.join("00000000000000000001.json")).unwrap();
    let mut guard = DenyCreate { path: path.clone(), restore: true };
    DenyCreate::set(&path, true).expect("必须实际设置目录 CreateFiles 拒绝权限");
    let mut health = std::fs::OpenOptions::new().write(true).open(path.join(".health")).unwrap();
    health.write_all(&[0]).and_then(|_| health.set_len(0)).and_then(|_| health.sync_all()).unwrap();
    drop(health);
    let spool = Mutex::new(actual);
    let failure = Mutex::new(None);
    let error = spool_io(&spool, &failure, |spool| spool.probe()).unwrap_err();
    assert!(error.contains("探针新建失败"), "{error}");
    assert_eq!(*failure.lock().unwrap(), Some(error));
    assert_eq!(std::fs::read(path.join("00000000000000000001.json")).unwrap(), original);
    guard.restore().expect("必须恢复测试目录原访问权限");
    assert!(spool.lock().unwrap().probe().is_ok());
}

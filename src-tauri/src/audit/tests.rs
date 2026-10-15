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
    for shot in &mut doc.shots { shot.camera = "cam1".into(); }
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
        sn: format!("SN-{sn}"),
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
        source: crate::store::RecordSource::Real, sample: None,
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
fn durable_spool_conflicting_cycle_insert_retains_all_receipts_and_never_replaces_original() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let mut spool = spool::Spool::open(dir.0.join("spool")).unwrap();
    let original_record = part("same-cycle", 42);
    sink.store.insert(&original_record.borrowed()).unwrap();
    let original_content = original(&sink.detail("same-cycle"));
    let mut conflict = original_record.clone();
    conflict.sn = "SN-43".into();
    spool.append(&Event::Insert(Box::new(conflict))).unwrap();
    assert!(!replay_cycle(&mut spool, "same-cycle", &mut sink, 1).unwrap().0);
    assert_eq!(sink.detail("same-cycle").summary.sn, "SN-42");
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
    let mut recording = recorder.begin(RecordMode::All, &record.sn, recipe, &record.cycle_id, record.bundle_id.as_deref()).unwrap();
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
    assert!(recorder.begin(RecordMode::Off, &record.sn, record.recipe.clone().unwrap(), &record.cycle_id, record.bundle_id.as_deref()).is_none());
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

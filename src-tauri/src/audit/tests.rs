use super::*;
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

    fn sink(&self) -> TestSink {
        TestSink {
            store: Store::open(&self.0.join("parts.sqlite")).unwrap(),
            inserts: 0,
            delivery_writes: 0,
            raw_writes: 0,
            recording_writes: 0,
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

impl Sink for TestSink {
    fn deleted(&mut self, cycle: &str) -> Result<bool, String> {
        self.store.history_cycle_deleted(cycle)
    }
    fn find(&mut self, cycle: &str) -> Result<Option<PartDetail>, String> {
        self.store.detail_by_cycle(cycle)
    }

    fn insert(&mut self, part: &RecordedPart) -> Result<InsertResult, String> {
        self.inserts += 1;
        insert_store(&self.store, part)
    }

    fn delivery(&mut self, cycle: &str, delivery: &PlcDelivery) -> Result<bool, String> {
        self.delivery_writes += 1;
        self.store.update_delivery(cycle, delivery)
    }

    fn matches_record(&mut self, part: &RecordedPart) -> Result<bool, String> {
        self.store.matches_record(&part.borrowed())
    }

    fn submission_timing(&mut self, cycle: &str, drain_ms: u64) -> Result<bool, String> {
        self.store.update_submission_timing(cycle, drain_ms)
    }

    fn frame_files(&mut self, cycle: &str, files: &BTreeMap<FrameKey, String>) -> Result<bool, String> {
        self.raw_writes += 1;
        self.store.update_frame_raw_files(cycle, files)
    }

    fn recording(&mut self, cycle: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
        self.recording_writes += 1;
        self.store.update_recording(cycle, evidence)
    }
}

fn delivery(state: PlcDeliveryState, updated_at: i64) -> PlcDelivery {
    PlcDelivery { state, updated_at, message: Some(format!("{state:?}")) }
}

fn outcome(cycle: &str, files: &[(usize, u8)], state: RecorderState) -> RecordingOutcome {
    RecordingOutcome {
        cycle_id: cycle.into(),
        directory: Some(PathBuf::from(format!("records/cycle_{cycle}"))),
        files: files
            .iter()
            .map(|&(camera, seq)| RecordedRawFile {
                width: 100,
                height: 60,
                file: format!("cycle_{cycle}/cam{}_s{seq:06}.pgm", camera + 1),
                frame: FrameKey {
                    camera,
                    seq: u64::from(seq),
                },
            })
            .collect(),
        errors: Vec::new(),
        retention_errors: Vec::new(),
        available: state == RecorderState::Complete,
        state,
    }
}

fn part(cycle: &str, sn: u32) -> RecordedPart {
    let recipe = Arc::new(crate::store::burst_fixture::recipe());
    let mut frames = crate::store::burst_fixture::frames(4, &[(1, 2)]);
    frames[0].metrics = Some(crate::store::burst_fixture::measured(0, 1, 2.0));
    let (judgement, burst) = crate::store::burst_fixture::record(&recipe, frames, None, Some(0));
    RecordedPart {
        mode: Default::default(),
        ts: 1234,
        sn: format!("SN-{sn}"),
        recipe: Some(recipe),
        judgement,
        drain_ms: Some(42),
        frames: Vec::new(),
        frames_expected: 4,
        frames_received: 4,
        triggers: 0,
        table: None,
        software_version: "audit-test".into(),
        cycle_id: cycle.into(),
        bundle_id: Some("frozen-bundle".into()),
        delivery: delivery(PlcDeliveryState::Pending, 10),
        shots: Vec::new(),
        source: RecordSource::Real,
        sample: None,
        burst: Some(burst),
    }
}

fn original(detail: &PartDetail) -> serde_json::Value {
    serde_json::json!({ "judgement": detail.judgement, "points": detail.points, "frames": detail.frames,
        "shots": detail.shots, "burst": detail.burst, "recipeRevision": detail.summary.recipe_revision,
        "bundleId": detail.summary.bundle_id })
}

#[test]
fn durable_spool_replays_sqlite_commit_before_receipt_cleanup_without_duplicate_or_regression() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let (mut spool, _) = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool.append(&Event::Insert(Box::new(part("committed", 42)))).unwrap();
    spool.append(&Event::Delivery("committed".into(), delivery(PlcDeliveryState::Acknowledged, 88))).unwrap();
    spool.append(&Event::Recording(outcome("committed", &[(0, 2)], RecorderState::Complete))).unwrap();
    let mut coordinator = Coordinator::new(1, 1);
    for (_, event) in spool.events("committed").unwrap() { coordinator.handle(event, &mut sink, 1); }
    assert!(!coordinator.entries["committed"].pending());
    let before = sink.detail("committed");
    let counts = sink.counts();
    let stored = before.burst.as_ref().unwrap();
    assert_eq!(
        (stored.frames.len(), stored.judgement.regions.len()),
        (12, 1)
    );
    assert!(stored.judgement.flags.incomplete);
    assert_eq!(before.judgement.verdict, crate::judge::Verdict::NgWidth);
    let snapshot = sink.store.record_recipe(&before.summary).unwrap().unwrap();
    assert_eq!(snapshot.path_segments, stored.segments);
    assert_eq!(snapshot.plan_cameras(), stored.cameras);
    drop(coordinator);
    assert!(replay_cycle(&mut spool, "committed", &mut sink, 2).unwrap().0);
    assert_eq!(sink.counts(), counts);
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
    assert_eq!(original(&sink.detail("committed")), original(&before));
    assert_eq!(sink.detail("committed").recording, before.recording);
    assert_eq!(sink.detail("committed").summary.delivery, before.summary.delivery);
}

#[test]
fn startup_spool_with_unrecoverable_cycle_then_quarantined_and_spool_ready_after_reopen() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let (mut spool, _) = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool
        .append(&Event::Insert(Box::new(part("committed", 42))))
        .unwrap();
    spool
        .append(&Event::Delivery(
            "orphan".into(),
            delivery(PlcDeliveryState::Acknowledged, 88),
        ))
        .unwrap();
    let notices = recover_all(&mut spool, &mut sink, 1);
    assert!(spool.empty());
    assert!(notices.iter().any(|notice| matches!(notice, Notice::Log { event: "持久追溯未恢复", cycle, .. } if cycle == "orphan")));
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
    assert!(spool::Spool::open(dir.0.join("spool")).unwrap().0.empty());
    assert_eq!(
        std::fs::read_dir(dir.0.join("spool").join(".unrecovered"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn spool_with_crash_leftover_tmp_and_old_format_file_then_opens_moves_both_aside_and_appends_after_them(
) {
    let dir = TestDir::new();
    let root = dir.0.join("spool");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("00000000000000000007.tmp"), b"{\"format\":4,\"ev").unwrap();
    std::fs::write(
        root.join("00000000000000000003.json"),
        br#"{"format":3,"event":{}}"#,
    )
    .unwrap();
    let (mut spool, notes) = spool::Spool::open(root.clone()).unwrap();
    assert_eq!(notes.len(), 2);
    assert!(spool.empty());
    assert_eq!(
        std::fs::read_dir(root.join(".unrecovered"))
            .unwrap()
            .count(),
        2
    );
    spool
        .append(&Event::Delivery(
            "next".into(),
            delivery(PlcDeliveryState::Acknowledged, 1),
        ))
        .unwrap();
    assert!(root.join("00000000000000000008.json").is_file());
}

#[test]
fn spool_full_then_events_wait_in_memory_in_order_and_drain_when_space_frees() {
    let dir = TestDir::new();
    let (spool, _) = spool::Spool::with_limits(dir.0.join("spool"), 1, 64 * 1024 * 1024).unwrap();
    let (spool, backlog, barriers) = (
        Mutex::new(spool),
        Mutex::new(VecDeque::new()),
        Mutex::new(BTreeMap::new()),
    );
    for (cycle, at) in [("a", 1), ("b", 2), ("c", 3)] {
        persist_event(
            &spool,
            &backlog,
            &barriers,
            Event::Delivery(cycle.into(), delivery(PlcDeliveryState::Acknowledged, at)),
        )
        .unwrap();
    }
    assert_eq!(
        backlog
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.cycle_id().to_string())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    let mut spool = spool.lock().unwrap();
    let receipts = spool
        .events("a")
        .unwrap()
        .into_iter()
        .map(|(sequence, _)| sequence)
        .collect::<Vec<_>>();
    spool.remove(&receipts).unwrap();
    assert!(drain(&mut spool, &backlog).is_err());
    assert_eq!(spool.cycles(), BTreeSet::from(["b".to_string()]));
    assert_eq!(backlog.lock().unwrap().len(), 1);
}

#[test]
fn cycle_still_unrecorded_after_retry_limit_then_writer_quarantines_it() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let (mut spool, _) = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool
        .append(&Event::Delivery(
            "orphan".into(),
            delivery(PlcDeliveryState::Acknowledged, 88),
        ))
        .unwrap();
    let spool = Mutex::new(spool);
    let mut retries = BTreeMap::new();
    let mut now = 0;
    for _ in 1..QUARANTINE_ATTEMPTS {
        retry_cycle(&spool, &mut retries, "orphan", &mut sink, now);
        now += RETRY_MAX_MS;
    }
    assert!(!spool.lock().unwrap().empty());
    let notices = retry_cycle(&spool, &mut retries, "orphan", &mut sink, now);
    assert!(spool.lock().unwrap().empty());
    assert!(notices.iter().any(|notice| matches!(
        notice,
        Notice::Log {
            event: "持久追溯未恢复",
            ..
        }
    )));
    assert_eq!(
        std::fs::read_dir(dir.0.join("spool").join(".unrecovered"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn quarantined_cycle_keeps_identity_and_recording_protection_after_reopen_without_blocking() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let records = dir.0.join("records");
    std::fs::create_dir_all(records.join("cycle_orphan")).unwrap();
    let (mut spool, _) = spool::Spool::open(dir.0.join("spool")).unwrap();
    let mut recording = outcome("orphan", &[(0, 2)], RecorderState::Complete);
    recording.directory = Some(records.join("cycle_orphan"));
    spool.append(&Event::Recording(recording)).unwrap();
    spool
        .append(&Event::Delivery(
            "orphan".into(),
            delivery(PlcDeliveryState::Acknowledged, 88),
        ))
        .unwrap();
    recover_all(&mut spool, &mut sink, 1);
    assert!(spool.empty());
    let expected = records.join("cycle_orphan").canonicalize().unwrap();
    let (reopened, _) = spool::Spool::open(dir.0.join("spool")).unwrap();
    assert!(reopened.cycles().is_empty());
    assert!(reopened.quarantined_cycles().any(|cycle| cycle == "orphan"));
    assert_eq!(
        reopened
            .protected_directories(&records, &VecDeque::new())
            .unwrap(),
        vec![expected]
    );
}

#[test]
fn manually_deleted_history_consumes_stale_spool_without_resurrecting_or_blocking_readiness() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let original = part("deleted-spool", 8);
    let id = sink.store.insert(&original.borrowed()).unwrap();
    sink.store
        .update_delivery(
            "deleted-spool",
            &delivery(PlcDeliveryState::Acknowledged, 88),
        )
        .unwrap();
    sink.store
        .update_recording(
            "deleted-spool",
            &RecordingEvidence {
                state: RecordingState::Off,
                ..Default::default()
            },
        )
        .unwrap();
    let package = dir.0.join("restore.sqlite");
    sink.store.export_parts(&[id], &package).unwrap();
    sink.store.delete_history(&[id]).unwrap();
    let (mut spool, _) = spool::Spool::open(dir.0.join("spool")).unwrap();
    spool
        .append(&Event::Insert(Box::new(original.clone())))
        .unwrap();
    assert!(
        recover_cycle(&mut spool, "deleted-spool", &mut sink, 2)
            .unwrap()
            .0
    );
    assert!(spool.empty());
    assert!(sink
        .store
        .detail_by_cycle("deleted-spool")
        .unwrap()
        .is_none());
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 0);
    let restored = sink
        .store
        .import_parts(&package, "default", |_| Ok(()))
        .unwrap()
        .ids[&id];
    spool.append(&Event::Insert(Box::new(original))).unwrap();
    spool
        .append(&Event::Delivery(
            "deleted-spool".into(),
            delivery(PlcDeliveryState::Submitted, 100),
        ))
        .unwrap();
    assert!(
        recover_cycle(&mut spool, "deleted-spool", &mut sink, 3)
            .unwrap()
            .0
    );
    assert!(spool.empty());
    assert_eq!(
        sink.store.detail(restored).unwrap().summary.delivery.state,
        PlcDeliveryState::Acknowledged
    );
    assert_eq!(sink.store.query(&HistoryQuery::default()).unwrap().total, 1);
}

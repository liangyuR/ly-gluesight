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
        TestSink { store: Store::open(&self.0.join("parts.sqlite")).unwrap(), failing_inserts: 0 }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TestSink {
    store: Store,
    /// 接下来这么多次写主记录失败（记录库暂不可用）
    failing_inserts: usize,
}

impl TestSink {
    fn detail(&self, cycle: &str) -> PartDetail {
        let page = self.store.query(&HistoryQuery { limit: 500, ..Default::default() }).unwrap();
        let id = page.items.iter().find(|part| part.cycle_id.as_deref() == Some(cycle)).unwrap().id;
        self.store.detail(id).unwrap()
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
        if self.failing_inserts > 0 {
            self.failing_inserts -= 1;
            return Err("database is locked".into());
        }
        insert_store(&self.store, part)
    }

    fn delivery(&mut self, cycle: &str, delivery: &PlcDelivery) -> Result<bool, String> {
        self.store.update_delivery(cycle, delivery)
    }

    fn matches_record(&mut self, part: &RecordedPart) -> Result<bool, String> {
        self.store.matches_record(&part.borrowed())
    }

    fn submission_timing(&mut self, cycle: &str, drain_ms: u64) -> Result<bool, String> {
        self.store.update_submission_timing(cycle, drain_ms)
    }

    fn frame_files(&mut self, cycle: &str, files: &BTreeMap<FrameKey, String>) -> Result<bool, String> {
        self.store.update_frame_raw_files(cycle, files)
    }

    fn recording(&mut self, cycle: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
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
                frame: FrameKey { camera, seq: u64::from(seq) },
            })
            .collect(),
        errors: Vec::new(),
        retention_errors: Vec::new(),
        available: state == RecorderState::Complete,
        state,
    }
}

fn part_with(cycle: &str, sn: u32, frames: Vec<FrameRecord>) -> RecordedPart {
    let recipe = Arc::new(crate::store::burst_fixture::recipe());
    let (judgement, burst) = crate::store::burst_fixture::record(&recipe, frames, None, Some(0));
    RecordedPart {
        mode: Default::default(),
        ts: 1234,
        sn: format!("SN-{sn}"),
        recipe: Some(recipe),
        judgement,
        drain_ms: None,
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
        burst: Some(burst),
    }
}

fn part(cycle: &str, sn: u32) -> RecordedPart {
    part_with(cycle, sn, crate::store::burst_fixture::frames(4, &[(1, 2)]))
}

#[test]
fn part_larger_than_old_4mb_event_limit_then_written_with_every_frame() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let frames = (1..=1294u64)
        .flat_map(|seq| (0..3).map(move |camera| (camera, seq)))
        .map(|(camera, seq)| FrameRecord {
            camera,
            seq,
            status: crate::burst_judge::FrameState::Measured,
            frame_counter: Some(seq),
            trigger_counter: Some(seq),
            region: None,
            metrics: Some(crate::burst_judge::FrameMetrics {
                widths: (0..200).map(|station| Some(3.5 + (seq as f32 * 0.37 + station as f32 * 0.0137) % 1.0)).collect(),
                ..crate::store::burst_fixture::measured(camera, seq, 4.0)
            }),
            raw_file: None,
        })
        .collect();
    let big = part_with("new2-scale", 2, frames);
    assert!(serde_json::to_vec(&big).unwrap().len() > 4 * 1024 * 1024);
    let mut coordinator = Coordinator::new(PENDING_LIMIT, CACHE_LIMIT);
    coordinator.handle(Event::Insert(Box::new(big)), &mut sink, 1);
    assert!(coordinator.snapshot().cycles.is_empty());
    assert_eq!(sink.detail("new2-scale").burst.unwrap().frames.len(), 3 * 1294);
}

#[test]
fn store_unavailable_then_part_waits_and_backoff_retry_writes_it_with_later_submission() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    sink.failing_inserts = 1;
    let mut coordinator = Coordinator::new(PENDING_LIMIT, CACHE_LIMIT);
    coordinator.handle(Event::Insert(Box::new(part("retry", 1))), &mut sink, 0);
    coordinator.handle(
        Event::Submission("retry".into(), delivery(PlcDeliveryState::Submitted, 20), Some(7)),
        &mut sink,
        10,
    );
    assert!(coordinator.failing());
    assert_eq!(coordinator.snapshot().cycles, BTreeSet::from(["retry".to_string()]));
    assert!(sink.store.detail_by_cycle("retry").unwrap().is_none());
    coordinator.retry(&mut sink, RETRY_BASE_MS);
    assert!(!coordinator.failing());
    assert!(coordinator.snapshot().cycles.is_empty());
    let detail = sink.detail("retry");
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Submitted);
    assert_eq!(detail.summary.drain_ms, Some(7));
}

#[test]
fn legacy_spool_with_oversized_insert_and_quarantined_events_then_imported_and_orphan_kept() {
    let dir = TestDir::new();
    let mut sink = dir.sink();
    let root = dir.0.join("audit-spool");
    let unrecovered = root.join(".unrecovered");
    std::fs::create_dir_all(&unrecovered).unwrap();
    let write = |path: PathBuf, event: Event| {
        std::fs::write(path, serde_json::to_vec(&serde_json::json!({ "format": 4, "event": event })).unwrap()).unwrap()
    };
    write(unrecovered.join("oversized-41.json"), Event::Insert(Box::new(part("new2", 41))));
    write(unrecovered.join("00000000000000000042.json"), Event::Delivery("new2".into(), delivery(PlcDeliveryState::Acknowledged, 88)));
    write(unrecovered.join("00000000000000000043.json"), Event::Recording(outcome("new2", &[(0, 2)], RecorderState::Complete)));
    write(root.join("00000000000000000044.json"), Event::Delivery("orphan".into(), delivery(PlcDeliveryState::Acknowledged, 90)));
    std::fs::write(root.join(".health"), b"").unwrap();

    let (leftovers, _, summary) = legacy::import(&root, &mut sink, 1);

    let detail = sink.detail("new2");
    assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(detail.recording.state, RecordingState::Complete);
    assert!(detail.burst.unwrap().frames.iter().any(|frame| (frame.camera, frame.seq, frame.raw_file.is_some()) == (0, 2, true)));
    assert_eq!(leftovers.cycles, BTreeSet::from(["orphan".to_string()]));
    assert!(summary.is_some_and(|summary| summary.contains("orphan")));
    assert!(!unrecovered.exists());
    let remaining = std::fs::read_dir(&root).unwrap().map(|item| item.unwrap().file_name()).collect::<Vec<_>>();
    assert_eq!(remaining, ["00000000000000000044.json"]);
}

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("gluesight-store-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.join("parts.sqlite")
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn recipe() -> Recipe {
    let mut doc = crate::recipe::samples().remove(1);
    for (k, shot) in doc.shots.iter_mut().enumerate() {
        shot.camera = "cam1".into();
        shot.view = (k % 3 + 1) as u8;
    }
    doc.build().unwrap()
}

fn shots(recipe: &Recipe) -> Vec<PartShot> {
    recipe
        .shots
        .iter()
        .enumerate()
        .map(|(k, s)| PartShot {
            k,
            shot_id: s.id.clone(),
            camera: s.camera.clone(),
            view: s.view,
            session: (k != 1).then_some(u64::MAX),
            ordinal: (k != 1).then_some(k as u64 + 1),
            frame_counter: (k != 1).then_some(u64::MAX - 1),
            trigger_counter: (k != 1).then_some(u64::MAX - 2),
            status: [FrameStatus::Done, FrameStatus::Missing, FrameStatus::Error, FrameStatus::Done][k],
            error: (k != 0).then(|| format!("P{} 原因", k + 1)),
            score: (k != 1).then_some(0.75),
            ms: (k != 1).then_some(12),
            raw_files: Vec::new(),
        })
        .collect()
}

fn table(recipe: &Recipe) -> Vec<PointState> {
    let mut table = vec![PointState::Measured { d: Some(0.25), w: 4.5 }; recipe.point_count()];
    table[1] = PointState::Gap;
    table[2] = PointState::Invalid;
    table[3] = PointState::Pending;
    table[4] = PointState::Measured { d: None, w: 4.25 };
    table[5] = PointState::OutOfRange { w: Some(3.75) };
    table[6] = PointState::OutOfRange { w: None };
    table
}

fn save(store: &Store, recipe: &Recipe, cycle: &str, shots: &[PartShot], delivery: &PlcDelivery, table: Option<&[PointState]>) -> Result<i64, String> {
    save_as(store, recipe, cycle, shots, delivery, table, RecordSource::Real)
}

fn save_as(store: &Store, recipe: &Recipe, cycle: &str, shots: &[PartShot], delivery: &PlcDelivery, table: Option<&[PointState]>, source: RecordSource) -> Result<i64, String> {
    let judgement = Judgement::error(crate::judge::fault::MISSING_FRAME, "P2 缺帧，P3 定位失败，P4 测量出错");
    store.insert(&PartRecord {
        ts: 1234,
        sn: "SN-42",
        recipe: Some(recipe),
        judgement: &judgement,
        drain_ms: Some(321),
        frames: &[],
        frames_expected: recipe.shot_count(),
        frames_received: 3,
        triggers: 4,
        table,
        software_version: "test-p0",
        cycle_id: Some(cycle),
        bundle_id: Some("frozen-bundle-id"),
        delivery,
        shots,
        source, sample: None,
    })
}

#[test]
fn offline_parts_stay_out_of_real_history_statistics_and_rejudge_and_vice_versa() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let shots = shots(&recipe);
    let delivery = PlcDelivery::default();
    let real = save_as(&store, &recipe, "real-part", &shots, &delivery, None, RecordSource::Real).unwrap();
    let synthetic = save_as(&store, &recipe, "synthetic-part", &shots, &delivery, None, RecordSource::OfflineSynthetic).unwrap();
    let sample = save_as(&store, &recipe, "sample-part", &shots, &delivery, None, RecordSource::OfflineSample).unwrap();
    let mut debug_recipe = recipe.clone();
    debug_recipe.revision_id = format!("candidate-{}-r355", recipe.id);
    let debug = save_as(&store, &debug_recipe, "debug-part", &shots, &delivery, None, RecordSource::OfflineCandidate).unwrap();
    for (scope, expected) in [(HistoryScope::Real, vec![real]), (HistoryScope::Offline, vec![sample, synthetic]), (HistoryScope::Debug, vec![debug])] {
        let query = HistoryQuery { scope, ..Default::default() };
        let page = store.query(&query).unwrap();
        assert_eq!(page.items.iter().map(|p| p.id).collect::<Vec<_>>(), expected, "{scope:?}");
        assert_eq!((page.total, page.counts.err), (expected.len() as u64, expected.len() as u64), "{scope:?}");
        assert_eq!(store.counts_since(0, scope).unwrap().err, expected.len() as u64, "{scope:?}");
        assert_eq!(store.measurements(&query, &[], 10).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), expected, "{scope:?}");
    }
}

#[test]
fn neighbors_follow_list_filters_and_never_cross_record_scope() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let shots = shots(&recipe);
    let delivery = PlcDelivery::default();
    let insert = |cycle: &str, verdict: Verdict, source: RecordSource| {
        let mut judgement = Judgement::error(0, "回看");
        judgement.verdict = verdict;
        store.insert(&PartRecord { ts: 1234, sn: "SN-42", recipe: Some(&recipe), judgement: &judgement, drain_ms: None, frames: &[],
            frames_expected: recipe.shot_count(), frames_received: recipe.shot_count(), triggers: 4, table: None, software_version: "test",
            cycle_id: Some(cycle), bundle_id: None, delivery: &delivery, shots: &shots, source, sample: None }).unwrap()
    };
    let a = insert("a", Verdict::NgGap, RecordSource::Real);
    let b = insert("b", Verdict::Ok, RecordSource::Real);
    let offline = insert("c", Verdict::NgGap, RecordSource::OfflineSample);
    let d = insert("d", Verdict::NgWidth, RecordSource::Real);
    insert("e", Verdict::Ok, RecordSource::Real);
    let ng = HistoryQuery { verdicts: vec![Verdict::NgGap, Verdict::NgWidth], ..Default::default() };
    assert_eq!(store.neighbors(&HistoryQuery::default(), b).unwrap(), (Some(a), Some(d)));
    assert_eq!(store.neighbors(&ng, d).unwrap(), (Some(a), None));
    assert_eq!(store.neighbors(&HistoryQuery { scope: HistoryScope::Offline, ..Default::default() }, offline).unwrap(), (None, None));
}

#[test]
fn identity_shots_and_measurements_round_trip() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let mut shots = shots(&recipe);
    shots[0].raw_files = vec![ShotRawFile { view: 1, file: "cycle/P1_v1.pgm".into() , width: None, height: None }];
    let delivery = PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 1235, message: Some("等待提交".into()) };
    let cycle = store.reserve_cycle_id().unwrap();
    let table = table(&recipe);
    let id = save(&store, &recipe, &cycle, &shots, &delivery, Some(&table)).unwrap();

    let page = store
        .query(&HistoryQuery {
            from: Some(1000),
            to: Some(2000),
            sn: Some("42".into()),
            recipe_id: Some(recipe.id.clone()),
            verdicts: vec![Verdict::ErrInspect],
            ..Default::default()
        })
        .unwrap();
    assert_eq!((page.total, page.counts.err, page.items.len()), (1, 1, 1));
    let summary = &page.items[0];
    assert_eq!(summary.cycle_id.as_deref(), Some(cycle.as_str()));
    assert_eq!(summary.bundle_id.as_deref(), Some("frozen-bundle-id"));
    assert_eq!(summary.delivery, delivery);
    let detail = store.detail(id).unwrap();
    assert_eq!(store.detail_by_cycle(&cycle).unwrap().unwrap().summary.id, id);
    assert!(store.detail_by_cycle("no-such-cycle").unwrap().is_none());
    assert_eq!(detail.software_version, "test-p0");
    assert_eq!(detail.triggers, 4);
    assert_eq!(detail.shots.len(), recipe.shot_count());
    assert_eq!(serde_json::to_value(&detail.shots).unwrap(), serde_json::to_value(&shots).unwrap());
    assert_eq!(detail.shots[0].session, Some(u64::MAX));
    assert_eq!(detail.shots[1].status, FrameStatus::Missing);
    assert_eq!(detail.shots[1].frame_counter, None);
    let points = detail.points.unwrap();
    assert_eq!(&points.st[..7], &[ST_MEASURED, ST_GAP, ST_INVALID, ST_PENDING, ST_NO_REF, ST_OUT_OF_RANGE, ST_OUT_OF_RANGE]);
    assert_eq!(&points.d[..7], &[Some(0.25), None, None, None, None, None, None]);
    assert_eq!(&points.w[..7], &[Some(4.5), None, None, None, Some(4.25), Some(3.75), None]);
    assert_eq!(points.table(), table);
    let rows = store.measurements(&HistoryQuery::default(), &[id], 20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].table, table);
    assert!(store.measurements(&HistoryQuery { recipe_id: Some("other".into()), ..Default::default() }, &[], 20).unwrap().is_empty());
}

#[test]
fn delivery_updates_preserve_original_judgement_and_reject_stale_regressions() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let table = table(&recipe);
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 10, message: None }, Some(&table))
        .unwrap();
    let before = store.detail(id).unwrap();
    for (state, updated_at) in [(PlcDeliveryState::Submitted, 11), (PlcDeliveryState::Failed, 12), (PlcDeliveryState::Acknowledged, 13)] {
        assert!(store.update_delivery("cycle", &PlcDelivery { state, updated_at, message: Some("PLC 交付".into()) }).unwrap());
    }
    assert!(!store.update_delivery("cycle", &PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 9, message: None }).unwrap());
    assert!(store.update_delivery("cycle", &PlcDelivery { state: PlcDeliveryState::Failed, updated_at: 14, message: None }).is_err());
    assert!(!store.update_delivery("unknown", &PlcDelivery::default()).unwrap());
    let after = store.detail(id).unwrap();
    assert_eq!(serde_json::to_value(before.judgement).unwrap(), serde_json::to_value(after.judgement).unwrap());
    assert_eq!(serde_json::to_value(before.points).unwrap(), serde_json::to_value(after.points).unwrap());
    assert_eq!(serde_json::to_value(before.shots).unwrap(), serde_json::to_value(after.shots).unwrap());
    assert_eq!(after.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(after.summary.verdict, before.summary.verdict);
    assert_eq!(after.summary.reason, before.summary.reason);

    save(&store, &recipe, "local", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    assert!(store.update_delivery("local", &PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 10, message: None }).is_err());
}

#[test]
fn durable_ack_recovery_matches_cycle_after_reopen_and_preserves_original_evidence() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let table = table(&recipe);
    let cycles = store.reserve_cycle_ids(4).unwrap();
    for (cycle, state) in cycles.iter().zip([PlcDeliveryState::Submitted, PlcDeliveryState::Failed, PlcDeliveryState::NotRequired, PlcDeliveryState::Acknowledged]) {
        save(&store, &recipe, cycle, &shots(&recipe), &PlcDelivery { state, updated_at: 200, message: Some("原交付记录".into()) }, Some(&table)).unwrap();
    }
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    let before = store.detail_by_cycle(&cycles[0]).unwrap().unwrap();
    assert!(store.recover_acknowledgement(&cycles[0], "SN-42", 17, 100).unwrap());
    assert!(!store.recover_acknowledgement(&cycles[0], "SN-42", 17, 999).unwrap());
    let after = store.detail_by_cycle(&cycles[0]).unwrap().unwrap();
    assert_eq!(after.summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(after.summary.delivery.updated_at, 200);
    assert!(after.summary.delivery.message.as_ref().unwrap().contains("序号 17"));
    let mut original = serde_json::to_value(&before).unwrap();
    let mut recovered = serde_json::to_value(&after).unwrap();
    original["summary"].as_object_mut().unwrap().remove("delivery");
    recovered["summary"].as_object_mut().unwrap().remove("delivery");
    assert_eq!(original, recovered);
    assert_eq!(store.detail_by_cycle(&cycles[1]).unwrap().unwrap().summary.delivery.state, PlcDeliveryState::Failed);
    assert!(store.recover_acknowledgement(&cycles[1], "SN-43", 18, 101).is_err());
    assert!(store.recover_acknowledgement(&cycles[1], "SN-42", 18, 101).unwrap());
    assert!(store.recover_acknowledgement(&cycles[2], "SN-42", 19, 102).is_err());
    assert!(!store.recover_acknowledgement(&cycles[3], "SN-42", 20, 103).unwrap());
    assert!(store.recover_acknowledgement("ffffffffffffffffffffffffffffffff", "SN-42", 17, 100).is_err());
    assert!(store.recover_acknowledgement("invalid", "SN-42", 17, 100).is_err());
    assert!(store.recover_acknowledgement(&cycles[1], "SN-42", 0, 100).is_err());
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    assert!(!store.recover_acknowledgement(&cycles[0], "SN-42", 17, 100).unwrap());
    assert_eq!(serde_json::to_value(store.detail_by_cycle(&cycles[0]).unwrap().unwrap()).unwrap(), serde_json::to_value(after).unwrap());
}

#[test]
fn raw_file_completion_adds_views_without_replacing_identity() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let files: Vec<_> = (1..=3).map(|view| ShotRawFile { view, file: format!("cycle/P1_v{view}.pgm"), width: None, height: None }).collect();
    assert!(store.update_shot_raw_files("cycle", 0, &files[..1]).unwrap());
    assert!(store.update_shot_raw_files("cycle", 0, &files).unwrap());
    assert!(store.update_shot_raw_files("cycle", 0, &files).unwrap());
    assert!(store.update_shot_raw_files("cycle", 0, &files[..1]).is_err());
    let mut replacement = files.clone();
    replacement[0].file = "cycle/P2_v1.pgm".into();
    assert!(store.update_shot_raw_files("cycle", 0, &replacement).is_err());
    for file in ["../outside.pgm", "C:/outside.pgm", "/outside.pgm", "cycle\\P1.pgm"] {
        assert!(store.update_shot_raw_files("cycle", 1, &[ShotRawFile { view: 1, file: file.into() , width: None, height: None }]).is_err());
    }
    assert!(store.update_shot_raw_files("cycle", 1, &[files[0].clone(), files[0].clone()]).is_err());
    assert!(!store.update_shot_raw_files("unknown", 0, &files).unwrap());
    assert_eq!(store.detail(id).unwrap().shots[0].raw_files, files);
}

#[test]
fn incomplete_shot_identity_and_duplicate_cycle_roll_back() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let mut shots = shots(&recipe);
    let delivery = PlcDelivery::default();
    assert!(save(&store, &recipe, "missing", &shots[..3], &delivery, None).is_err());
    shots[1].k = 0;
    assert!(save(&store, &recipe, "duplicate-shot", &shots, &delivery, None).is_err());
    shots[1].k = 1;
    shots[1].view = 1;
    assert!(save(&store, &recipe, "wrong-view", &shots, &delivery, None).is_err());
    shots[1].view = 2;
    assert!(save(&store, &recipe, "wrong-points", &shots, &delivery, Some(&[])).is_err());
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 0);
    let id = save(&store, &recipe, "same-cycle", &shots, &delivery, None).unwrap();
    assert!(save(&store, &recipe, "same-cycle", &shots, &delivery, None).is_err());
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 1);
    assert!(store.detail(id).unwrap().retests.is_empty());
}

#[test]
fn identical_content_with_new_version_is_allowed_but_snapshot_collision_is_rejected() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    save(&store, &recipe, "first", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let mut newer = recipe.clone();
    newer.version += 1;
    newer.revision_id = format!("{}-v{}", newer.id, newer.version);
    save(&store, &newer, "reverted-content", &shots(&newer), &PlcDelivery::default(), None).unwrap();
    newer.shots[0].camera = "changed-camera".into();
    assert!(save(&store, &newer, "forged-revision_id", &shots(&newer), &PlcDelivery::default(), None).is_err());
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 2);
    assert_eq!(store.recipe_snapshot(&recipe.revision_id).unwrap().unwrap().shots[0].camera, recipe.shots[0].camera);
    assert_eq!(store.recipe_snapshot(&newer.revision_id).unwrap().unwrap().shots[0].camera, recipe.shots[0].camera, "同一 revision_id 的已存快照不被改写");
}

#[test]
fn locked_old_database_is_not_renamed_or_replaced() {
    let db = TestDb::new();
    let old = Connection::open(db.path()).unwrap();
    old.execute_batch("PRAGMA journal_mode=WAL; PRAGMA user_version=73; CREATE TABLE legacy(data TEXT); INSERT INTO legacy VALUES ('committed'); BEGIN IMMEDIATE; INSERT INTO legacy VALUES ('uncommitted');").unwrap();
    assert!(Store::open(&db.path()).is_err());
    assert!(db.path().exists());
    assert!(!std::fs::read_dir(&db.0).unwrap().filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().ends_with(".bak")));
    old.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(old.query_row("SELECT COUNT(*) FROM legacy", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    drop(old);
    assert!(Store::open(&db.path()).unwrap().backup_path().is_some());
}

#[test]
fn recording_available_requires_actual_refs_and_complete_error_free_evidence() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let complete = RecordingEvidence { state: RecordingState::Complete, available: true, directory: Some("records/cycle".into()), errors: Vec::new() };
    assert!(store.update_recording("cycle", &complete).is_err(), "还没有原图引用");
    store
        .update_shot_raw_files("cycle", 0, &[ShotRawFile { view: 1, file: "cycle/k000_P1_cam1_v1.pgm".into() , width: None, height: None }])
        .unwrap();
    for invalid in [
        RecordingEvidence { state: RecordingState::Failed, ..complete.clone() },
        RecordingEvidence { available: false, ..complete.clone() },
        RecordingEvidence { directory: None, ..complete.clone() },
        RecordingEvidence { errors: vec!["磁盘错误".into()], ..complete.clone() },
    ] {
        assert!(store.update_recording("cycle", &invalid).is_err());
    }
    assert!(store.update_recording("cycle", &complete).unwrap());
    assert_eq!(store.detail(id).unwrap().recording, complete);
}

#[test]
fn pending_history_protection_includes_partial_directory_and_raw_only_references() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let root = db.0.join("records");
    let directory = root.join("_pending").join("20261010_000000_000_cycle_partial");
    let raw_directory = root.join("_pending").join("20261010_000000_000_cycle_raw-only");
    let recipe = recipe();
    let mut shots = shots(&recipe);
    shots[0].raw_files = vec![ShotRawFile { view: 1,
        file: "_pending/20261010_000000_000_cycle_raw-only/image.pgm".into() , width: None, height: None }];
    save(&store, &recipe, "pending-references", &shots, &PlcDelivery::default(), None).unwrap();
    store.update_recording("pending-references", &RecordingEvidence {
        state: RecordingState::Incomplete, available: false,
        directory: Some(directory.to_string_lossy().into_owned()), errors: vec!["原图未完整落盘".into()],
    }).unwrap();
    let mut expected = vec![directory, raw_directory];
    expected.sort();
    assert_eq!(store.pending_recording_directories(&root, HistoryScope::Real).unwrap(), expected);
    store.conn.lock().unwrap().execute("UPDATE part_shots SET raw_files='invalid-json'", []).unwrap();
    assert!(store.pending_recording_directories(&root, HistoryScope::Real).is_err());
}

#[test]
fn rolling_retention_prune_marks_history_raw_images_unavailable() {
    use crate::recorder::{Cleanup, Recorder, RecordingOutcome};
    use std::sync::Arc;
    let db = TestDb::new();
    let store = Arc::new(Store::open(&db.path()).unwrap());
    let root = db.0.join("Özil").join("records-offline");
    let old = root.join("20000101").join("20000101_000000_000_OK_cycle_old");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("k000_P1_cam1_v1.pgm"), b"P5\n1 1\n255\n\0").unwrap();
    let recipe = recipe();
    let id = save(&store, &recipe, "old", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    store.update_shot_raw_files("old", 0, &[ShotRawFile { view: 1, file: "20000101/20000101_000000_000_OK_cycle_old/k000_P1_cam1_v1.pgm".into(), width: None, height: None }]).unwrap();
    let evidence = RecordingEvidence { state: RecordingState::Complete, available: true, directory: Some(old.to_string_lossy().into_owned()), errors: Vec::new() };
    assert!(store.update_recording("old", &evidence).unwrap());
    let (tx, outcomes) = std::sync::mpsc::channel::<RecordingOutcome>();
    let released = store.clone();
    let recorder = Recorder::guarded(root, Some(Arc::new(move |outcome| tx.send(outcome).unwrap())), Cleanup {
        protection: Arc::new(|| Ok(Vec::new())), warning: Arc::new(|_| {}),
        release: Arc::new(move |directories| released.release_recordings(directories)),
    });
    let recipe = Arc::new(recipe);
    let mut rec = recorder.begin(crate::settings::RecordMode::All, "SN-43", recipe.clone(), "new", None).unwrap();
    let frame = crate::frame::Frame { cam: 0, session: 1, counter: crate::frame::CounterSource::Synthetic, frame_counter: 1, trigger_counter: 1,
        lost_packets: 0, ts: 1, manual: false, images: (0..3).map(|_| Arc::new(crate::frame::FrameImage::new(1, 1, vec![9]))).collect() };
    recorder.frame(&mut rec, &frame, "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "新件", 1, u64::MAX, Vec::new());
    let outcome = outcomes.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(outcome.retention_errors.is_empty(), "{:?}", outcome.retention_errors);
    assert!(!old.exists(), "超过保留件数的旧录制应被清理");
    let recording = store.detail(id).unwrap().recording;
    assert!(!recording.available, "原图目录已删除，历史记录不能再标为原图可用");
    assert_eq!(recording.state, RecordingState::NotRetained);
    assert!(!recording.errors.is_empty(), "清理后应写明不再可用的原因");
}

#[test]
fn identity_batches_commit_together_and_failed_batches_expose_no_partial_ids() {
    let db = TestDb::new();
    let count = |conn: &Connection| conn.query_row("SELECT count(*) FROM cycle_ids", [], |row| row.get::<_, i64>(0)).unwrap();
    let ids;
    {
        let store = Store::open(&db.path()).unwrap();
        let conn = Connection::open(db.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_third BEFORE INSERT ON cycle_ids
            WHEN (SELECT count(*) FROM cycle_ids) = 2 BEGIN SELECT RAISE(ABORT, 'disk failure'); END;").unwrap();
        assert!(store.reserve_cycle_ids(4).is_err());
        assert_eq!(count(&conn), 0);
        conn.execute_batch("DROP TRIGGER reject_third;").unwrap();
        assert!(store.reserve_cycle_ids(0).is_err());
        assert!(store.reserve_cycle_ids(33).is_err());
        ids = store.reserve_cycle_ids(32).unwrap();
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 32);
    }
    let reopened = Store::open(&db.path()).unwrap();
    assert!(!ids.contains(&reopened.reserve_cycle_id().unwrap()));
    assert_eq!(count(&Connection::open(db.path()).unwrap()), 33);
}

fn pre_identity_v2(db: &TestDb) -> Connection {
    let conn = Connection::open(db.path()).unwrap();
    conn.execute_batch(r#"PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS cycle_ids (
                 id TEXT PRIMARY KEY NOT NULL DEFAULT (lower(hex(randomblob(16)))),
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS parts (
                 id INTEGER PRIMARY KEY,
                 ts INTEGER NOT NULL,
                 sn INTEGER NOT NULL,
                 recipe_id TEXT,
                 recipe_version INTEGER,
                 recipe_hash TEXT,
                 trigger_mode TEXT,
                 verdict TEXT NOT NULL,
                 plc_code INTEGER NOT NULL,
                 fault_code INTEGER NOT NULL,
                 reason TEXT NOT NULL,
                 drain_ms INTEGER,
                 frames_expected INTEGER NOT NULL,
                 frames_received INTEGER NOT NULL,
                 triggers INTEGER NOT NULL,
                 retest_of INTEGER,
                 software_version TEXT NOT NULL,
                 judgement TEXT NOT NULL,
                 frames TEXT NOT NULL,
                 cycle_id TEXT,
                 bundle_hash TEXT,
                 delivery_state TEXT NOT NULL,
                 delivery_updated_at INTEGER NOT NULL,
                 delivery_message TEXT,
                 layout_hash TEXT,
                 recording_state TEXT NOT NULL DEFAULT 'pending',
                 recording_available INTEGER NOT NULL DEFAULT 0,
                 recording_directory TEXT,
                 recording_errors TEXT NOT NULL DEFAULT '[]'
             );
             CREATE INDEX IF NOT EXISTS parts_ts ON parts(ts);
             CREATE INDEX IF NOT EXISTS parts_sn ON parts(sn);
             CREATE UNIQUE INDEX IF NOT EXISTS parts_cycle ON parts(cycle_id) WHERE cycle_id IS NOT NULL;
             CREATE INDEX IF NOT EXISTS parts_unresolved_delivery ON parts(cycle_id)
                 WHERE cycle_id IS NOT NULL AND delivery_state IN ('pending','submitted','failed');
             CREATE TABLE IF NOT EXISTS part_points (
                 part_id INTEGER PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
                 format INTEGER NOT NULL,
                 data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS recipe_snapshots (
                 hash TEXT PRIMARY KEY,
                 recipe_id TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS part_shots (
                 part_id INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
                 k INTEGER NOT NULL,
                 shot_id TEXT NOT NULL,
                 camera TEXT NOT NULL,
                 view INTEGER NOT NULL CHECK(view BETWEEN 1 AND 3),
                 session TEXT,
                 ordinal TEXT,
                 frame_counter TEXT,
                 trigger_counter TEXT,
                 status TEXT NOT NULL,
                 error TEXT,
                 score REAL,
                 ms INTEGER,
                 raw_files TEXT NOT NULL,
                 PRIMARY KEY (part_id, k),
                 UNIQUE (part_id, shot_id)
             );
PRAGMA user_version=2;"#).unwrap();
    conn
}

/// 未知的新版本库，数据只在未合并的 WAL 里：备份必须先合并 WAL 才完整。
fn future_version_only_in_wal(db: &TestDb) {
    use rusqlite::config::DbConfig;
    assert!(73 > DB_VERSION);
    let old = Connection::open(db.path()).unwrap();
    old.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version=73; CREATE TABLE legacy(data TEXT); INSERT INTO legacy VALUES ('only-in-wal');").unwrap();
    old.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true).unwrap();
    drop(old);
    assert!(db.path().with_file_name("parts.sqlite-wal").metadata().unwrap().len() > 0);
}

/// 身份迁移前的 v2 库，带一条引用旧快照的记录；`migrated_once` 模拟做过一半的身份迁移。
fn pre_identity_v2_with_record(db: &TestDb, migrated_once: bool) {
    let old = pre_identity_v2(db);
    if migrated_once {
        old.execute_batch("ALTER TABLE parts ADD COLUMN recipe_revision TEXT; ALTER TABLE parts ADD COLUMN bundle_id TEXT;
            ALTER TABLE recipe_snapshots ADD COLUMN revision_id TEXT;
            CREATE TABLE identity_migrations(name TEXT PRIMARY KEY);
            CREATE TABLE explicit_recipe_records(part_id INTEGER PRIMARY KEY, recipe_revision TEXT NOT NULL);
            CREATE TABLE legacy_bundle_refs(recipe_id TEXT NOT NULL, bundle_id TEXT NOT NULL UNIQUE, legacy_directory TEXT NOT NULL);").unwrap();
    }
    let recipe = recipe();
    old.execute("INSERT INTO recipe_snapshots(hash,recipe_id,version,json) VALUES('old-reference',?1,?2,?3)",
        params![recipe.id, recipe.version, serde_json::to_string(&recipe).unwrap()]).unwrap();
    old.execute("INSERT INTO parts(ts,sn,recipe_id,recipe_version,recipe_hash,verdict,plc_code,fault_code,reason,frames_expected,
        frames_received,triggers,software_version,judgement,frames,bundle_hash,delivery_state,delivery_updated_at,recording_state)
        VALUES(1,42,?1,?2,'old-reference','errInspect',90,1,'old',4,0,0,'old','{}','[]','old-directory','acknowledged',99,'off')",
        params![recipe.id, recipe.version]).unwrap();
}

#[test]
fn mismatched_database_version_is_backed_up_completely_and_rebuilt_without_migration() {
    const OLD_REFERENCE: &[(&str, &str)] = &[
        ("SELECT recipe_hash FROM parts", "old-reference"),
        ("SELECT CAST(COUNT(*) AS TEXT) FROM recipe_snapshots WHERE hash='old-reference'", "1"),
    ];
    // (场景, 构造旧库, 旧库版本, 备份里必须原样保留的数据 (SQL, 文本结果))
    let cases: [(&str, fn(&TestDb), i64, &[(&str, &str)]); 3] = [
        ("WAL 未合并的未知新版本", future_version_only_in_wal, 73, &[("SELECT data FROM legacy", "only-in-wal")]),
        ("身份迁移前的 v2", |db| pre_identity_v2_with_record(db, false), 2, OLD_REFERENCE),
        ("做过一半身份迁移的 v2", |db| pre_identity_v2_with_record(db, true), 2, OLD_REFERENCE),
    ];
    for (case, build, old_version, preserved_rows) in cases {
        let db = TestDb::new();
        build(&db);
        let recipe = recipe();
        let store = Store::open(&db.path()).unwrap();
        let backup = store.backup_path().expect("旧库必须备份").to_path_buf();
        let preserved = Connection::open(&backup).unwrap();
        for (sql, expected) in preserved_rows {
            assert_eq!(preserved.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap(), *expected, "{case}: {sql}");
        }
        assert_eq!(preserved.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), old_version, "{case}");
        assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 0, "{case}");
        assert!(store.recipe_snapshot(&recipe.revision_id).unwrap().is_none(), "{case}");
        {
            let conn = store.conn.lock().unwrap();
            assert!(conn.prepare("SELECT data FROM legacy").is_err(), "{case}");
            assert_eq!(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), DB_VERSION, "{case}");
        }
        let cycle = store.reserve_cycle_id().unwrap();
        save(&store, &recipe, "new-cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
        drop(store);
        let reopened = Store::open(&db.path()).unwrap();
        assert!(reopened.backup_path().is_none(), "{case}");
        assert_eq!(reopened.query(&HistoryQuery::default()).unwrap().total, 1, "{case}");
        assert_eq!(reopened.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM cycle_ids", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "{case}");
        assert_ne!(reopened.reserve_cycle_id().unwrap(), cycle, "{case}");
        drop(preserved);
        assert!(backup.is_file(), "{case}");
    }
}

#[test]
fn historical_version_floor_prevents_deleted_recipe_revision_reuse_and_keeps_old_snapshot() {
    let db = TestDb::new();
    let history = Store::open(&db.path()).unwrap();
    let recipes_dir = db.0.join("recipes");
    let recipes = crate::recipe::RecipeStore::open(recipes_dir.clone()).unwrap();
    let mut doc = crate::recipe::samples().remove(1); doc.id = "OLD-HISTORY".into(); doc.product_code = 60004; doc.version = 7;
    let original = recipes.save(doc.clone(), None).unwrap();
    save(&history, &original, "old-cycle", &shots(&original), &PlcDelivery::default(), Some(&table(&original))).unwrap();
    std::fs::remove_file(recipes_dir.join("OLD-HISTORY.json")).unwrap();
    drop(recipes);
    let recipes = crate::recipe::RecipeStore::open(recipes_dir).unwrap();
    recipes.seed_version_floor(&history.recipe_version_floors().unwrap()).unwrap();
    doc.version = recipes.next_version(&doc.id).unwrap();
    doc.shots[0].camera = "new-camera".into();
    doc.sync_imaging();
    doc.plan_version = recipes.plan_version_for(&doc).unwrap();
    assert_eq!(doc.version, 8);
    let rebuilt = recipes.save_published(doc.clone(), None).unwrap();
    save(&history, &rebuilt, "new-cycle", &shots(&rebuilt), &PlcDelivery::default(), Some(&table(&rebuilt))).unwrap();
    assert_eq!(history.query(&HistoryQuery::default()).unwrap().total, 2);
    assert_eq!(history.recipe_snapshot(&original.revision_id).unwrap().unwrap().shots[0].camera, original.shots[0].camera);
    assert_eq!(history.recipe_snapshot(&rebuilt.revision_id).unwrap().unwrap().shots[0].camera, "new-camera");
    history.conn.lock().unwrap().execute_batch("ALTER TABLE parts RENAME COLUMN recipe_version TO unavailable_version;").unwrap();
    assert!(history.recipe_version_floors().is_err());
}

#[test]
fn failed_shot_persistence_rolls_back_the_part_and_snapshot() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    store.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_shot BEFORE INSERT ON part_shots
        BEGIN SELECT RAISE(ABORT,'forced shot persistence failure'); END;").unwrap();
    assert!(save(&store, &recipe, "failed-source", &shots(&recipe), &PlcDelivery::default(), None).unwrap_err().contains("forced shot"));
    {
        let conn = store.conn.lock().unwrap();
        for table in ["parts", "recipe_snapshots"] {
            assert_eq!(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        }
        conn.execute_batch("DROP TRIGGER reject_shot;").unwrap();
    }
    let id = save(&store, &recipe, "successful-source", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    drop(store);
    let reopened = Store::open(&db.path()).unwrap();
    assert_eq!(reopened.detail(id).unwrap().summary.recipe_revision.as_deref(), Some(recipe.revision_id.as_str()));
}


#[test]
fn durable_record_identity_checks_actual_fields_while_allowing_later_delivery_and_raw_evidence() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let table = table(&recipe);
    let shots = shots(&recipe);
    let pending = PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 1235, message: None };
    save(&store, &recipe, "record-identity", &shots, &pending, Some(&table)).unwrap();
    let judgement = Judgement::error(crate::judge::fault::MISSING_FRAME, "P2 缺帧，P3 定位失败，P4 测量出错");
    let record = PartRecord { ts: 1234, sn: "SN-42", recipe: Some(&recipe), judgement: &judgement, drain_ms: None,
        frames: &[], frames_expected: recipe.shot_count(), frames_received: 3, triggers: 4, table: Some(&table),
        software_version: "test-p0", cycle_id: Some("record-identity"), bundle_id: Some("frozen-bundle-id"), delivery: &pending, shots: &shots, source: crate::store::RecordSource::Real, sample: None };
    assert!(store.matches_record(&record).unwrap());
    store.update_delivery("record-identity", &PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 9999, message: None }).unwrap();
    store.update_shot_raw_files("record-identity", 0, &[ShotRawFile { view: 1, file: "records/cycle/k000.pgm".into(), width: Some(2), height: Some(3) }]).unwrap();
    assert!(store.matches_record(&record).unwrap());
    let mut wrong_shots = shots.clone();
    wrong_shots[0].view = 2;
    let mut changed = recipe.clone();
    changed.product_code += 1;
    for (field, mismatched) in [
        ("sn", PartRecord { sn: "SN-43", ..record }),
        ("ts", PartRecord { ts: record.ts + 1, ..record }),
        ("triggers", PartRecord { triggers: record.triggers + 1, ..record }),
        ("shots", PartRecord { shots: &wrong_shots, ..record }),
        ("table", PartRecord { table: None, ..record }),
        ("recipe", PartRecord { recipe: Some(&changed), ..record }),
    ] {
        assert!(!store.matches_record(&mismatched).unwrap(), "{field}");
    }
}

#[test]
fn candidate_source_upgrade_preserves_existing_records_and_relations() {
    let db = TestDb::new();
    let recipe = recipe();
    let shots = shots(&recipe);
    let delivery = PlcDelivery::default();
    let points = table(&recipe);
    let store = Store::open(&db.path()).unwrap();
    let real = save_as(&store, &recipe, "before-real", &shots, &delivery, Some(&points), RecordSource::Real).unwrap();
    let sample = save_as(&store, &recipe, "before-offline", &shots, &delivery, Some(&points), RecordSource::OfflineSample).unwrap();
    drop(store);
    let conn = Connection::open(db.path()).unwrap();
    let sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE name='parts'", [], |r| r.get(0)).unwrap();
    let definition = sql[sql.find('(').unwrap()..].replace(
        "CHECK(source IN ('real','offlineSynthetic','offlineSample','offlineCandidate'))",
        "CHECK(source IN ('real','offlineSynthetic','offlineSample'))");
    conn.execute_batch(&format!("PRAGMA foreign_keys=OFF; BEGIN; CREATE TABLE old_parts {definition}; INSERT INTO old_parts SELECT * FROM parts; DROP TABLE parts; ALTER TABLE old_parts RENAME TO parts; COMMIT;")).unwrap();
    conn.execute_batch("PRAGMA wal_autocheckpoint=0;").unwrap();
    conn.execute("UPDATE parts SET reason='saved in WAL' WHERE id=?1", [real]).unwrap();
    let store = Store::open(&db.path()).unwrap();
    assert!(store.backup_path().is_none());
    for id in [real, sample] {
        let detail = store.detail(id).unwrap();
        assert_eq!(detail.shots.len(), shots.len());
        assert_eq!(store.recipe_snapshot(&recipe.revision_id).unwrap().unwrap().revision_id, recipe.revision_id);
    }
    let mut debug_recipe = recipe.clone();
    debug_recipe.revision_id = format!("candidate-{}-r355", recipe.id);
    let debug = save_as(&store, &debug_recipe, "after-debug", &shots, &delivery, None, RecordSource::OfflineCandidate).unwrap();
    assert_eq!(store.detail(debug).unwrap().summary.source, RecordSource::OfflineCandidate);
    let backup_path = std::fs::read_dir(&db.0).unwrap().map(|p| p.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().contains("before-candidate")).unwrap();
    let backup = Connection::open(backup_path).unwrap();
    let reason: String = backup.query_row("SELECT reason FROM parts WHERE id=?1", [real], |r| r.get(0)).unwrap();
    assert_eq!(reason, "saved in WAL");
    let before_points: i64 = backup.query_row("SELECT count(*) FROM part_points WHERE part_id IN (?1,?2)", [real, sample], |r| r.get(0)).unwrap();
    assert_eq!(before_points, 2);
    drop(conn);
    let conn = store.conn.lock().unwrap();
    let after_points: i64 = conn.query_row("SELECT count(*) FROM part_points WHERE part_id IN (?1,?2)", [real, sample], |r| r.get(0)).unwrap();
    assert_eq!(after_points, 2);
    let errors: i64 = conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
    assert_eq!(errors, 0);
    drop(conn);
    assert_eq!(store.counts_since(0, HistoryScope::Real).unwrap().err, 1);
}

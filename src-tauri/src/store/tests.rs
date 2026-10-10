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
        shot.camera = format!("cam{}", k % 3 + 1);
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
            status: [FrameStatus::Done, FrameStatus::Missing, FrameStatus::LocateFailed, FrameStatus::Error][k],
            error: (k != 0).then(|| format!("P{} 原因", k + 1)),
            score: (k != 1).then_some(0.75),
            ms: (k != 1).then_some(12),
            raw_files: Vec::new(),
        })
        .collect()
}

fn table(recipe: &Recipe) -> Vec<PointState> {
    let mut table = vec![PointState::Measured { d: 0.25, w: 4.5 }; recipe.point_count()];
    table[1] = PointState::Gap;
    table[2] = PointState::Invalid;
    table[3] = PointState::Pending;
    table
}

fn save(store: &Store, recipe: &Recipe, cycle: &str, shots: &[PartShot], delivery: &PlcDelivery, table: Option<&[PointState]>) -> Result<i64, String> {
    let judgement = Judgement::error(crate::judge::fault::MISSING_FRAME, "P2 缺帧，P3 定位失败，P4 测量出错");
    store.insert(&PartRecord {
        ts: 1234,
        sn: 42,
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
        bundle_id: Some("frozen-bundle-revision_id"),
        delivery,
        shots,
    })
}

#[test]
fn identity_shots_and_measurements_round_trip() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let mut shots = shots(&recipe);
    shots[0].raw_files = vec![ShotRawFile { view: 1, file: "cycle/P1_v1.pgm".into(), revision_id: Some("raw-revision_id".into()) }];
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
    assert_eq!(summary.bundle_id.as_deref(), Some("frozen-bundle-revision_id"));
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
    assert_eq!(&points.st[..4], &[ST_MEASURED, ST_GAP, ST_INVALID, ST_PENDING]);
    assert_eq!(&points.w[..4], &[Some(4.5), None, None, None]);
    let rows = store.measurements(&HistoryQuery::default(), &[id], 20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cycle_id, summary.cycle_id);
    assert_eq!(rows[0].bundle_id, summary.bundle_id);
    assert_eq!(rows[0].delivery, delivery);
    assert_eq!(rows[0].layout_hash.as_deref(), Some(measurement_layout_hash(&recipe).as_str()));
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
    assert!(store.recover_acknowledgement(&cycles[0], 42, 17, 100).unwrap());
    assert!(!store.recover_acknowledgement(&cycles[0], 42, 17, 999).unwrap());
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
    assert!(store.recover_acknowledgement(&cycles[1], 43, 18, 101).is_err());
    assert!(store.recover_acknowledgement(&cycles[1], 42, 18, 101).unwrap());
    assert!(store.recover_acknowledgement(&cycles[2], 42, 19, 102).is_err());
    assert!(!store.recover_acknowledgement(&cycles[3], 42, 20, 103).unwrap());
    assert!(store.recover_acknowledgement("ffffffffffffffffffffffffffffffff", 42, 17, 100).is_err());
    assert!(store.recover_acknowledgement("invalid", 42, 17, 100).is_err());
    assert!(store.recover_acknowledgement(&cycles[1], 42, 0, 100).is_err());
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    assert!(!store.recover_acknowledgement(&cycles[0], 42, 17, 100).unwrap());
    assert_eq!(serde_json::to_value(store.detail_by_cycle(&cycles[0]).unwrap().unwrap()).unwrap(), serde_json::to_value(after).unwrap());
}

#[test]
fn durable_ack_recovery_write_failure_does_not_change_record() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let cycle = store.reserve_cycle_id().unwrap();
    let id = save(&store, &recipe, &cycle, &shots(&recipe), &PlcDelivery { state: PlcDeliveryState::Submitted, updated_at: 10, message: None }, None).unwrap();
    let before = serde_json::to_value(store.detail(id).unwrap()).unwrap();
    store.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_ack BEFORE UPDATE OF delivery_state ON parts BEGIN SELECT RAISE(ABORT, 'test disk write failure'); END;").unwrap();
    assert!(store.recover_acknowledgement(&cycle, 42, 1, 20).unwrap_err().contains("test disk write failure"));
    assert_eq!(serde_json::to_value(store.detail(id).unwrap()).unwrap(), before);
    store.conn.lock().unwrap().execute_batch("DROP TRIGGER fail_ack;").unwrap();
    assert!(store.recover_acknowledgement(&cycle, 42, 1, 20).unwrap());
}

#[test]
fn raw_file_completion_enriches_hashes_without_replacing_identity() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let mut files: Vec<_> = (1..=3).map(|view| ShotRawFile { view, file: format!("cycle/P1_v{view}.pgm"), revision_id: None }).collect();
    assert!(store.update_shot_raw_files("cycle", 0, &files[..1]).unwrap());
    assert!(store.update_shot_raw_files("cycle", 0, &files).unwrap());
    for file in &mut files {
        file.revision_id = Some(format!("revision_id{}", file.view));
    }
    assert!(store.update_shot_raw_files("cycle", 0, &files).unwrap());
    assert!(store.update_shot_raw_files("cycle", 0, &files[..1]).is_err());
    let mut replacement = files.clone();
    replacement[0].file = "cycle/P2_v1.pgm".into();
    assert!(store.update_shot_raw_files("cycle", 0, &replacement).is_err());
    replacement = files.clone();
    replacement[0].revision_id = Some("another-image".into());
    assert!(store.update_shot_raw_files("cycle", 0, &replacement).is_err());
    for file in ["../outside.pgm", "C:/outside.pgm", "/outside.pgm", "cycle\\P1.pgm"] {
        assert!(store.update_shot_raw_files("cycle", 1, &[ShotRawFile { view: 1, file: file.into(), revision_id: None }]).is_err());
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
    save(&store, &newer, "reverted-content", &shots(&newer), &PlcDelivery::default(), None).unwrap();
    newer.shots[0].camera = "changed-camera".into();
    assert!(save(&store, &newer, "forged-revision_id", &shots(&newer), &PlcDelivery::default(), None).unwrap_err().contains("不同快照"));
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 2);
    assert_eq!(store.recipe_snapshot(&recipe.revision_id).unwrap().unwrap().shots[0].camera, recipe.shots[0].camera);
}

#[test]
fn missing_measurements_are_returned_and_purge_cascades_to_shots() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let first = save(&store, &recipe, "first", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let second = save(&store, &recipe, "second", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    assert_eq!(store.detail(first).unwrap().retests, [second]);
    assert_eq!(store.detail(second).unwrap().summary.retest_of, Some(first));
    let rows = store.measurements(&HistoryQuery::default(), &[first, second], 20).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.table.is_empty()));
    assert_eq!(store.purge_before(1235).unwrap(), 2);
    assert_eq!(store.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM part_shots", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert!(store.detail(first).is_err());
}

#[test]
fn mismatched_database_version_checkpoints_wal_and_keeps_complete_backup() {
    use rusqlite::config::DbConfig;
    assert!(73 > DB_VERSION);
    let db = TestDb::new();
    let old = Connection::open(db.path()).unwrap();
    old.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version=73; CREATE TABLE legacy(data TEXT); INSERT INTO legacy VALUES ('only-in-wal');").unwrap();
    old.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true).unwrap();
    drop(old);
    assert!(db.path().with_file_name("parts.sqlite-wal").metadata().unwrap().len() > 0);
    let store = Store::open(&db.path()).unwrap();
    let backup = store.backup_path().unwrap().to_path_buf();
    let old = Connection::open(&backup).unwrap();
    assert_eq!(old.query_row("SELECT data FROM legacy", [], |r| r.get::<_, String>(0)).unwrap(), "only-in-wal");
    assert_eq!(old.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 73);
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 0);
    assert!(store.conn.lock().unwrap().prepare("SELECT data FROM legacy").is_err());
    assert_eq!(store.conn.lock().unwrap().pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), DB_VERSION);
    let cycle = store.reserve_cycle_id().unwrap();
    drop(old);
    drop(store);
    let reopened = Store::open(&db.path()).unwrap();
    assert!(reopened.backup_path().is_none());
    assert_eq!(reopened.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM cycle_ids", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_ne!(reopened.reserve_cycle_id().unwrap(), cycle);
    assert!(backup.exists());
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
fn recording_evidence_defaults_pending_and_does_not_change_measurement_or_judgement() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let table = table(&recipe);
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), Some(&table)).unwrap();
    let before = store.detail(id).unwrap();
    assert_eq!(before.recording, RecordingEvidence::default());
    let evidence = RecordingEvidence {
        state: RecordingState::Incomplete,
        available: false,
        directory: Some("records/actual-cycle".into()),
        errors: vec!["view=3 写盘失败".into(), "part.json 拒绝访问".into()],
    };
    assert!(store.update_recording("cycle", &evidence).unwrap());
    assert!(store.update_recording("cycle", &evidence).unwrap());
    assert!(!store.update_recording("unknown", &evidence).unwrap());
    let after = store.detail(id).unwrap();
    assert_eq!(after.recording, evidence);
    assert_eq!(serde_json::to_value(&after.judgement).unwrap(), serde_json::to_value(&before.judgement).unwrap());
    assert_eq!(serde_json::to_value(&after.points).unwrap(), serde_json::to_value(&before.points).unwrap());
    assert_eq!(serde_json::to_value(&after.shots).unwrap(), serde_json::to_value(&before.shots).unwrap());
    assert!(store.update_recording("cycle", &RecordingEvidence::default()).is_err());
    assert!(store.update_recording("cycle", &RecordingEvidence { state: RecordingState::Off, ..Default::default() }).is_err());
    assert_eq!(store.detail(id).unwrap().recording, evidence);
}

#[test]
fn recording_available_requires_actual_hashed_refs_and_complete_error_free_evidence() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let complete = RecordingEvidence { state: RecordingState::Complete, available: true, directory: Some("records/cycle".into()), errors: Vec::new() };
    assert!(store.update_recording("cycle", &complete).unwrap_err().contains("带哈希原图引用"));
    store
        .update_shot_raw_files("cycle", 0, &[ShotRawFile { view: 1, file: "cycle/k000_P1_cam1_v1.pgm".into(), revision_id: Some("fnv1a64:1234567890abcdef".into()) }])
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
fn version_one_database_is_backed_up_and_recreated_with_pending_recording_schema() {
    let db = TestDb::new();
    let old = Connection::open(db.path()).unwrap();
    old.execute_batch("PRAGMA user_version=1; CREATE TABLE legacy(version TEXT); INSERT INTO legacy VALUES ('step6-before-recording-evidence');").unwrap();
    drop(old);
    let store = Store::open(&db.path()).unwrap();
    assert_eq!(DB_VERSION, 2);
    let backup = Connection::open(store.backup_path().unwrap()).unwrap();
    assert_eq!(backup.query_row("SELECT version FROM legacy", [], |row| row.get::<_, String>(0)).unwrap(), "step6-before-recording-evidence");
    let recipe = recipe();
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    assert_eq!(store.detail(id).unwrap().recording.state, RecordingState::Pending);
    assert!(!store.detail(id).unwrap().recording.available);
}

#[test]
fn reopening_fails_pending_recordings_once_without_changing_original_evidence_or_terminal_states() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    assert_eq!(store.interrupted_recordings, 0);
    let recipe = recipe();
    let table = table(&recipe);
    let delivery = PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 99, message: Some("PLC 已确认".into()) };
    let directory = db.0.join("records/cycle-interrupted");
    std::fs::create_dir_all(&directory).unwrap();
    let pixels = b"P5\n1 1\n255\n\x7f";
    std::fs::write(directory.join("k000_P1_cam1_v1.pgm"), pixels).unwrap();
    let partial = vec![ShotRawFile {
        view: 1,
        file: "records/cycle-interrupted/k000_P1_cam1_v1.pgm".into(),
        revision_id: Some(format!("fnv1a64:{}", crate::release::fnv_hex(pixels))),
    }];
    let mut partial_shots = shots(&recipe);
    partial_shots[0].raw_files = partial.clone();
    let pending = save(&store, &recipe, "cycle-interrupted", &partial_shots, &delivery, Some(&table)).unwrap();
    store.update_recording("cycle-interrupted", &RecordingEvidence {
        directory: Some(directory.to_string_lossy().into_owned()),
        errors: vec!["view=2 写入尚未完成".into()],
        ..Default::default()
    }).unwrap();
    store.conn.lock().unwrap().execute("UPDATE parts SET recording_available=1 WHERE id=?1", [pending]).unwrap();
    let empty_pending = save(&store, &recipe, "cycle-empty-pending", &shots(&recipe), &delivery, Some(&table)).unwrap();
    let mut ids = vec![pending, empty_pending];
    for (cycle, state) in [
        ("cycle-complete", RecordingState::Complete),
        ("cycle-off", RecordingState::Off),
        ("cycle-not-retained", RecordingState::NotRetained),
        ("cycle-incomplete", RecordingState::Incomplete),
        ("cycle-failed", RecordingState::Failed),
    ] {
        let id = save(&store, &recipe, cycle, &shots(&recipe), &delivery, Some(&table)).unwrap();
        if state == RecordingState::Complete {
            store.update_shot_raw_files(cycle, 0, &[ShotRawFile {
                view: 1,
                file: "records/cycle-complete/k000_P1_cam1_v1.pgm".into(),
                revision_id: Some("fnv1a64:1234567890abcdef".into()),
            }]).unwrap();
        }
        store.update_recording(cycle, &RecordingEvidence {
            state,
            available: state == RecordingState::Complete,
            directory: (state == RecordingState::Complete).then(|| "records/cycle-complete".into()),
            errors: match state {
                RecordingState::Incomplete => vec!["view=3 写盘失败".into()],
                RecordingState::Failed => vec!["磁盘已满".into()],
                _ => Vec::new(),
            },
        }).unwrap();
        ids.push(id);
    }
    let before: Vec<_> = ids.iter().map(|id| serde_json::to_value(store.detail(*id).unwrap()).unwrap()).collect();
    drop(store);

    let reopened = Store::open(&db.path()).unwrap();
    assert_eq!(reopened.interrupted_recordings, 2);
    assert!(reopened.backup_path().is_none());
    assert!(directory.is_dir());
    let recovered = reopened.detail(pending).unwrap();
    assert_eq!(recovered.recording.state, RecordingState::Failed);
    assert!(!recovered.recording.available);
    assert_eq!(recovered.recording.errors, ["view=2 写入尚未完成", "检测服务在录制完成前退出，原图完整性未确认"]);
    assert_eq!(recovered.shots[0].raw_files, partial);
    let mut expected = before.clone();
    for item in &mut expected[..2] {
        item["recording"]["state"] = serde_json::json!("failed");
        item["recording"]["available"] = serde_json::json!(false);
        item["recording"]["errors"].as_array_mut().unwrap().push(serde_json::json!("检测服务在录制完成前退出，原图完整性未确认"));
    }
    let after: Vec<_> = ids.iter().map(|id| serde_json::to_value(reopened.detail(*id).unwrap()).unwrap()).collect();
    assert_eq!(after, expected);
    drop(reopened);

    let reopened_again = Store::open(&db.path()).unwrap();
    assert_eq!(reopened_again.interrupted_recordings, 0);
    let again: Vec<_> = ids.iter().map(|id| serde_json::to_value(reopened_again.detail(*id).unwrap()).unwrap()).collect();
    assert_eq!(again, after);
}

#[test]
fn interrupted_recording_recovery_rolls_back_the_whole_batch_if_one_update_fails() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let first = save(&store, &recipe, "recovery-first", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let second = save(&store, &recipe, "recovery-second", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let before = vec![(first, store.detail(first).unwrap().recording), (second, store.detail(second).unwrap().recording)];
    store.conn.lock().unwrap().execute_batch(
        "CREATE TRIGGER reject_recording_recovery BEFORE UPDATE OF recording_state ON parts
         WHEN OLD.cycle_id='recovery-second'
         BEGIN SELECT RAISE(ABORT, 'controlled recovery write failure'); END;",
    ).unwrap();
    drop(store);
    let error = Store::open(&db.path()).err().expect("startup recovery must report the failed write");
    assert!(error.contains("controlled recovery write failure"));
    let conn = Connection::open(db.path()).unwrap();
    let persisted = {
        let mut stmt = conn.prepare("SELECT id,recording_state,recording_available,recording_directory,recording_errors FROM parts ORDER BY id").unwrap();
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, recording_row(row, 1)?))).unwrap();
        rows.collect::<Result<Vec<_>, _>>().unwrap()
    };
    assert_eq!(persisted, before);
    conn.execute_batch("DROP TRIGGER reject_recording_recovery;").unwrap();
    drop(conn);
    let reopened = Store::open(&db.path()).unwrap();
    assert_eq!(reopened.interrupted_recordings, 2);
    for id in [first, second] {
        assert_eq!(reopened.detail(id).unwrap().recording.state, RecordingState::Failed);
        assert_eq!(reopened.detail(id).unwrap().recording.errors, ["检测服务在录制完成前退出，原图完整性未确认"]);
    }
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
        file: "_pending/20261010_000000_000_cycle_raw-only/image.pgm".into(), revision_id: Some("fnv1a64:1234567890abcdef".into()) }];
    save(&store, &recipe, "pending-references", &shots, &PlcDelivery::default(), None).unwrap();
    store.update_recording("pending-references", &RecordingEvidence {
        state: RecordingState::Incomplete, available: false,
        directory: Some(directory.to_string_lossy().into_owned()), errors: vec!["原图未完整落盘".into()],
    }).unwrap();
    let mut expected = vec![directory, raw_directory];
    expected.sort();
    assert_eq!(store.pending_recording_directories(&root).unwrap(), expected);
    store.conn.lock().unwrap().execute("UPDATE part_shots SET raw_files='invalid-json'", []).unwrap();
    assert!(store.pending_recording_directories(&root).is_err());
}

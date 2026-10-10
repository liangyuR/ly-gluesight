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
        bundle_id: Some("frozen-bundle-id"),
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
    assert_eq!(&points.st[..4], &[ST_MEASURED, ST_GAP, ST_INVALID, ST_PENDING]);
    assert_eq!(&points.w[..4], &[Some(4.5), None, None, None]);
    let rows = store.measurements(&HistoryQuery::default(), &[id], 20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cycle_id, summary.cycle_id);
    assert_eq!(rows[0].bundle_id, summary.bundle_id);
    assert_eq!(rows[0].delivery, delivery);
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
fn recording_available_requires_actual_refs_and_complete_error_free_evidence() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let id = save(&store, &recipe, "cycle", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let complete = RecordingEvidence { state: RecordingState::Complete, available: true, directory: Some("records/cycle".into()), errors: Vec::new() };
    assert!(store.update_recording("cycle", &complete).unwrap_err().contains("原图引用"));
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

     width: None, height: None }];
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

             width: None, height: None }]).unwrap();
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
        file: "_pending/20261010_000000_000_cycle_raw-only/image.pgm".into() , width: None, height: None }];
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

#[test]
fn restart_ack_recovery_only_visits_unresolved_database_cycles_and_preserves_audit() {
    use crate::plc_session::PlcSession;
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let cycles = store.reserve_cycle_ids(4).unwrap();
    for (cycle, state) in cycles.iter().zip([PlcDeliveryState::Pending, PlcDeliveryState::Submitted, PlcDeliveryState::Failed, PlcDeliveryState::NotRequired]) {
        save(&store, &recipe, cycle, &shots(&recipe), &PlcDelivery { state, updated_at: 200, message: None }, Some(&table(&recipe))).unwrap();
    }
    let pending = |cycle: &str, seq: u32| serde_json::json!({
        "request":{"protocolVersion":1,"requestSeq":seq,"sn":42,"productCode":1,"shotCount":4,"planVersion":7,"planReserved":0,"cameraShots":[2,1,1]},
        "result":{"requestSeq":seq,"sn":42,"resultCode":1,"faultCode":0},
        "phase":"releasing","cycleId":cycle,"acknowledged":true,"startedAt":1000
    });
    let mut audit = Vec::new();
    let mut append = |cycle: &str, seq: u32| {
        audit.extend(serde_json::to_vec(&serde_json::json!({"version":1,"ts":3000,"action":"acknowledged","lastRequestSeq":seq,"pending":pending(cycle,seq)})).unwrap());
        audit.push(b'\n');
    };
    let old_cycles = (0..4).flat_map(|_| store.reserve_cycle_ids(32).unwrap()).collect::<Vec<_>>();
    for (index, cycle) in old_cycles.iter().enumerate() {
        save(&store, &recipe, cycle, &shots(&recipe), &PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 200, message: Some("original ACK".into()) }, None).unwrap();
        append(cycle, index as u32 + 100);
    }
    let purged_cycles = (0..4).flat_map(|_| store.reserve_cycle_ids(32).unwrap()).collect::<Vec<_>>();
    for (index, cycle) in purged_cycles.iter().enumerate() {
        save(&store, &recipe, cycle, &shots(&recipe), &PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 200, message: None }, None).unwrap();
        store.conn.lock().unwrap().execute("UPDATE parts SET ts=0 WHERE cycle_id=?1", [cycle]).unwrap();
        append(cycle, index as u32 + 1000);
    }
    assert_eq!(store.purge_before(1).unwrap(), 128);
    assert!(purged_cycles.iter().all(|cycle| store.detail_by_cycle(cycle).unwrap().is_none()));
    append(&cycles[1], 8);
    append(&cycles[3], 9);
    let journal_path = db.0.join("plc-handshake.json");
    let audit_path = journal_path.with_extension("audit.jsonl");
    std::fs::write(&audit_path, &audit).unwrap();
    std::fs::write(&journal_path, serde_json::to_vec(&serde_json::json!({"version":1,"lastRequestSeq":7,"pending":pending(&cycles[0],7),"lastResolution":null})).unwrap()).unwrap();
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    let old_before = serde_json::to_value(store.detail_by_cycle(&old_cycles[0]).unwrap().unwrap()).unwrap();
    let unresolved = store.unresolved_delivery_cycles().unwrap();
    assert_eq!(unresolved, cycles[..3].iter().cloned().collect());
    let query_plan: String = store.conn.lock().unwrap().query_row("EXPLAIN QUERY PLAN SELECT cycle_id FROM parts WHERE cycle_id IS NOT NULL AND delivery_state IN ('pending','submitted','failed')", [], |row| row.get(3)).unwrap();
    assert!(query_plan.contains("parts_unresolved_delivery"), "{query_plan}");
    let session = PlcSession::open(journal_path.clone());
    assert!(session.pending() && session.acknowledged());
    let recovered = session.recover_acknowledgements(&unresolved);
    assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
    assert_eq!(recovered.receipts.len(), 2);
    for receipt in &recovered.receipts {
        assert!(store.recover_acknowledgement(&receipt.cycle_id, receipt.sn, receipt.request_seq, receipt.ts).unwrap());
    }
    assert_eq!(store.detail_by_cycle(&cycles[0]).unwrap().unwrap().summary.delivery.updated_at, 1000);
    assert_eq!(store.detail_by_cycle(&cycles[1]).unwrap().unwrap().summary.delivery.updated_at, 3000);
    assert_eq!(store.unresolved_delivery_cycles().unwrap(), [cycles[2].clone()].into_iter().collect());
    assert_eq!(serde_json::to_value(store.detail_by_cycle(&old_cycles[0]).unwrap().unwrap()).unwrap(), old_before);
    assert_eq!(store.detail_by_cycle(&cycles[3]).unwrap().unwrap().summary.delivery.state, PlcDeliveryState::NotRequired);
    assert!(session.recover_acknowledgements(&store.unresolved_delivery_cycles().unwrap()).receipts.is_empty());
    store.conn.lock().unwrap().execute("DELETE FROM parts WHERE cycle_id=?1", [&cycles[2]]).unwrap();
    let empty = store.unresolved_delivery_cycles().unwrap();
    assert!(empty.is_empty());
    std::fs::rename(&audit_path, audit_path.with_extension("preserved.jsonl")).unwrap();
    std::fs::create_dir(&audit_path).unwrap();
    assert!(session.recover_acknowledgements(&empty).errors.is_empty());
    assert_eq!(std::fs::read(audit_path.with_extension("preserved.jsonl")).unwrap(), audit);
}

#[test]
fn unresolved_delivery_query_reports_database_failure_instead_of_empty_success() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    store.conn.lock().unwrap().execute_batch("ALTER TABLE parts RENAME TO unavailable_parts").unwrap();
    assert!(store.unresolved_delivery_cycles().unwrap_err().contains("parts"));
}


fn legacy_v2(db: &TestDb) -> Connection {
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

fn legacy_snapshot(conn: &Connection, reference: &str, recipe: &Recipe) -> String {
    let mut json = serde_json::to_value(recipe).unwrap();
    json.as_object_mut().unwrap().remove("revisionId");
    json["hash"] = reference.into();
    let json = serde_json::to_string(&json).unwrap();
    conn.execute("INSERT INTO recipe_snapshots(hash,recipe_id,version,json) VALUES(?1,?2,?3,?4)",
        params![reference, recipe.id, recipe.version, json]).unwrap();
    json
}

#[test]
fn legacy_v2_adds_revisions_without_erasing_snapshots_points_raw_or_acknowledgements() {
    let db = TestDb::new();
    let original = recipe();
    let legacy = legacy_v2(&db);
    let old_json = legacy_snapshot(&legacy, "legacy-reference", &original);
    let measured = table(&original);
    let blob = encode(&measured);
    let raw = r#"[{"view":1,"file":"cycle/P1_v1.pgm","hash":"legacy-unverified-reference"}]"#;
    let judgement = serde_json::to_string(&Judgement::error(1, "original conclusion")).unwrap();
    for version in [original.version, original.version + 1] {
        legacy.execute("INSERT INTO parts(ts,sn,recipe_id,recipe_version,recipe_hash,trigger_mode,verdict,plc_code,fault_code,reason,
            frames_expected,frames_received,triggers,software_version,judgement,frames,cycle_id,bundle_hash,
            delivery_state,delivery_updated_at,delivery_message,layout_hash,recording_state,recording_available,recording_directory)
            VALUES(1234,42,?1,?2,'legacy-reference','fly','errInspect',90,1,'original conclusion',4,1,4,'legacy',?3,'[]',?4,
            'legacy-bundle-directory','acknowledged',99,'PLC confirmed','unused-old-layout','complete',1,'records/cycle')",
            params![original.id, version, judgement, format!("legacy-cycle-{version}")]).unwrap();
        let part = legacy.last_insert_rowid();
        legacy.execute("INSERT INTO part_points(part_id,format,data) VALUES(?1,2,?2)", params![part, blob]).unwrap();
        legacy.execute("INSERT INTO part_shots(part_id,k,shot_id,camera,view,session,ordinal,frame_counter,trigger_counter,status,raw_files)
            VALUES(?1,0,?2,?3,?4,?5,'1','2','3','done',?6)",
            params![part, original.shots[0].id, original.shots[0].camera, original.shots[0].view, u64::MAX.to_string(), raw]).unwrap();
    }
    drop(legacy);
    let store = Store::open(&db.path()).unwrap();
    assert!(store.backup_path().is_none());
    assert_eq!(store.interrupted_recordings, 0);
    let columns = table_columns(&store.conn.lock().unwrap(), "parts").unwrap();
    for name in ["recipe_hash", "bundle_hash", "layout_hash", "recipe_revision", "bundle_id"] { assert!(columns.contains(name)); }
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 2);
    for (index, version) in [original.version, original.version + 1].into_iter().enumerate() {
        let detail = store.detail(index as i64 + 1).unwrap();
        let revision = format!("{}-v{version}", original.id);
        assert_eq!(detail.summary.recipe_revision.as_deref(), Some(revision.as_str()));
        assert_eq!(detail.summary.recipe_version, Some(version));
        let bundle = detail.summary.bundle_id.as_deref().unwrap();
        assert!(bundle.starts_with("legacy-bundle-"));
        assert_ne!(bundle, "legacy-bundle-directory");
        assert_eq!(store.resolve_bundle(&original.id, bundle).unwrap(), (bundle.into(), Some("legacy-bundle-directory".into())));
        assert_eq!(detail.summary.delivery, PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 99, message: Some("PLC confirmed".into()) });
        assert_eq!(detail.shots[0].session, Some(u64::MAX));
        assert_eq!(detail.shots[0].raw_files, [ShotRawFile { view: 1, file: "cycle/P1_v1.pgm".into() , width: None, height: None }]);
        assert!(detail.recording.available);
        let snapshot = store.recipe_snapshot(&revision).unwrap().unwrap();
        assert_eq!((snapshot.id.as_str(), snapshot.version), (original.id.as_str(), version));
        assert!(same_measurement_layout(&snapshot, &original));
    }
    {
        let conn = store.conn.lock().unwrap();
        assert_eq!(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert_eq!(conn.query_row("SELECT json FROM recipe_snapshots WHERE hash='legacy-reference'", [], |r| r.get::<_, String>(0)).unwrap(), old_json);
        assert_eq!(conn.query_row("SELECT raw_files FROM part_shots WHERE part_id=1", [], |r| r.get::<_, String>(0)).unwrap(), raw);
        assert_eq!(conn.query_row("SELECT data FROM part_points WHERE part_id=1", [], |r| r.get::<_, Vec<u8>>(0)).unwrap(), blob);
        assert_eq!(conn.query_row("SELECT layout_hash FROM parts WHERE id=1", [], |r| r.get::<_, String>(0)).unwrap(), "unused-old-layout");
    }
    let mut next = crate::recipe::samples().remove(1);
    next.version = original.version + 2;
    let next = next.build().unwrap();
    save(&store, &next, "new-cycle", &shots(&next), &PlcDelivery::default(), None).unwrap();
    let mapped_bundle = store.detail(1).unwrap().summary.bundle_id.unwrap();
    drop(store);
    let reopened = Store::open(&db.path()).unwrap();
    assert_eq!(reopened.detail(1).unwrap().summary.bundle_id.as_deref(), Some(mapped_bundle.as_str()));
    assert_eq!(reopened.query(&HistoryQuery::default()).unwrap().total, 3);
    assert_eq!(reopened.recipe_snapshot(&next.revision_id).unwrap().unwrap().version, next.version);
    assert!(reopened.detail(1).unwrap().recording.available);
}

#[test]
fn ambiguous_legacy_revision_keeps_original_snapshots_and_readable_history_without_reproduction() {
    let db = TestDb::new();
    let legacy = legacy_v2(&db);
    let original = recipe();
    let first = legacy_snapshot(&legacy, "old-one", &original);
    let mut conflicting = original.clone();
    conflicting.shots[0].camera = "another-camera".into();
    let second = legacy_snapshot(&legacy, "old-two", &conflicting);
    let judgement = serde_json::to_string(&Judgement::error(1, "original")).unwrap();
    for token in ["old-one", "old-two"] {
        legacy.execute("INSERT INTO parts(ts,sn,recipe_id,recipe_version,recipe_hash,verdict,plc_code,fault_code,reason,frames_expected,
            frames_received,triggers,software_version,judgement,frames,delivery_state,delivery_updated_at,recording_state,recording_available,recording_directory)
            VALUES(1,42,?1,?2,?3,'errInspect',90,1,'original',4,0,0,'legacy',?4,'[]','acknowledged',99,'complete',1,'records/cycle')",
            params![original.id, original.version, token, judgement]).unwrap();
    }
    drop(legacy);
    let store = Store::open(&db.path()).unwrap();
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 2);
    for id in [1, 2] {
        let detail = store.detail(id).unwrap();
        assert!(detail.summary.recipe_revision.is_none());
        assert_eq!(detail.summary.delivery.state, PlcDeliveryState::Acknowledged);
        assert!(detail.recording.available);
    }
    assert!(store.recipe_snapshot(&original.revision_id).unwrap().is_none());
    let conn = store.conn.lock().unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM recipe_snapshots WHERE revision_id IS NULL", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT json FROM recipe_snapshots WHERE hash='old-one'", [], |r| r.get::<_, String>(0)).unwrap(), first);
    assert_eq!(conn.query_row("SELECT json FROM recipe_snapshots WHERE hash='old-two'", [], |r| r.get::<_, String>(0)).unwrap(), second);
}

#[test]
fn fresh_schema_and_raw_references_have_no_content_digest_fields() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let conn = store.conn.lock().unwrap();
    let columns = table_columns(&conn, "parts").unwrap();
    for name in ["recipe_hash", "bundle_hash", "layout_hash"] { assert!(!columns.contains(name)); }
    assert!(!table_columns(&conn, "recipe_snapshots").unwrap().contains("hash"));
    let file: ShotRawFile = serde_json::from_value(serde_json::json!({"view":2,"file":"cycle/P1_v2.pgm","hash":"ignored", "revisionId":"ignored"})).unwrap();
    assert_eq!(serde_json::to_value(file).unwrap(), serde_json::json!({"view":2,"file":"cycle/P1_v2.pgm"}));
}

#[test]
fn snapshot_version_metadata_and_explicit_revision_must_agree() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let original = recipe();
    save(&store, &original, "first", &shots(&original), &PlcDelivery::default(), None).unwrap();
    let mut forged = original.clone();
    forged.revision_id = "external-revision".into();
    assert!(save(&store, &forged, "forged", &shots(&forged), &PlcDelivery::default(), None).unwrap_err().contains("修订号与实际"));
    store.conn.lock().unwrap().execute("UPDATE recipe_snapshots SET version=version+1", []).unwrap();
    assert!(store.recipe_snapshot(&original.revision_id).unwrap_err().contains("版本损坏"));
    assert!(save(&store, &original, "second", &shots(&original), &PlcDelivery::default(), None).unwrap_err().contains("版本损坏"));
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 1);
}

#[test]
fn legacy_digest_token_is_ignored_when_actual_recipe_id_and_version_match() {
    let db = TestDb::new();
    let original = recipe();
    let conn = legacy_v2(&db);
    legacy_snapshot(&conn, "existing-reference", &original);
    conn.execute("INSERT INTO parts(ts,sn,recipe_id,recipe_version,recipe_hash,verdict,plc_code,fault_code,reason,frames_expected,
        frames_received,triggers,software_version,judgement,frames,delivery_state,delivery_updated_at,recording_state)
        VALUES(1,42,?1,?2,'missing-reference','errInspect',90,1,'original',4,0,0,'legacy','{}','[]','notRequired',0,'off')",
        params![original.id, original.version]).unwrap();
    drop(conn);
    let store = Store::open(&db.path()).unwrap();
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().items[0].recipe_revision.as_deref(), Some(original.revision_id.as_str()));
    assert_eq!(store.recipe_snapshot(&original.revision_id).unwrap().unwrap().version, original.version);
    let conn = store.conn.lock().unwrap();
    assert_eq!(conn.query_row("SELECT recipe_hash FROM parts", [], |r| r.get::<_, String>(0)).unwrap(), "missing-reference");
}

#[test]
fn missing_or_ambiguous_actual_snapshot_preserves_history_without_using_legacy_tokens() {
    let db = TestDb::new();
    let conn = legacy_v2(&db);
    let original = recipe();
    legacy_snapshot(&conn, "ignored-one", &original);
    let mut second = original.clone(); second.version += 1; second.revision_id = format!("{}-v{}", second.id, second.version);
    second.shots[0].camera = "different-camera".into();
    legacy_snapshot(&conn, "ignored-two", &second);
    for (id, version, token) in [(&original.id, original.version, Some("wrong-token")),
        (&original.id, second.version, None), (&original.id, second.version + 1, Some("ignored-one")),
        (&"unknown-recipe".to_string(), 1, Some("ignored-one"))] {
        conn.execute("INSERT INTO parts(ts,sn,recipe_id,recipe_version,recipe_hash,verdict,plc_code,fault_code,reason,frames_expected,
            frames_received,triggers,software_version,judgement,frames,delivery_state,delivery_updated_at,recording_state)
            VALUES(1,42,?1,?2,?3,'errInspect',90,1,'original',4,0,0,'legacy','{}','[]','notRequired',0,'off')", params![id, version, token]).unwrap();
    }
    drop(conn);
    let store = Store::open(&db.path()).unwrap();
    let rows = store.query(&HistoryQuery::default()).unwrap().items;
    assert_eq!(rows.len(), 4);
    assert!(rows[0].recipe_revision.is_none());
    assert!(rows[1].recipe_revision.is_none());
    assert_eq!(rows[2].recipe_revision.as_deref(), Some(second.revision_id.as_str()));
    assert_eq!(rows[3].recipe_revision.as_deref(), Some(original.revision_id.as_str()));
    assert!(store.recipe_snapshot(&format!("{}-v{}", original.id, second.version + 1)).unwrap().is_none());
    assert_eq!(store.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM parts", [], |r| r.get::<_, i64>(0)).unwrap(), 4);
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
fn legacy_bundle_ids_are_distinct_stable_and_new_ids_without_mapping_stay_unresolved() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let first = store.register_legacy_bundle("A", "old-directory").unwrap();
    assert_ne!(first, "old-directory");
    assert_eq!(store.register_legacy_bundle("A", "old-directory").unwrap(), first);
    let second = store.register_legacy_bundle("B", "old-directory").unwrap();
    assert_ne!(first, second);
    assert_eq!(store.resolve_bundle("A", &first).unwrap(), (first.clone(), Some("old-directory".into())));
    assert_eq!(store.resolve_bundle("B", &first).unwrap(), (first.clone(), None));
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    assert_eq!(store.resolve_bundle("A", "old-directory").unwrap(), (first, Some("old-directory".into())));
    assert_eq!(store.resolve_bundle("A", "legacy-bundle-missing").unwrap(), ("legacy-bundle-missing".into(), None));
}

#[test]
fn raw_dimensions_enrich_legacy_refs_but_cannot_be_replaced_or_partially_specified() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    save(&store, &recipe, "dims", &shots(&recipe), &PlcDelivery::default(), None).unwrap();
    let mut raw = ShotRawFile { view: 1, file: "cycle/P1_v1.pgm".into(), width: None, height: None };
    assert!(store.update_shot_raw_files("dims", 0, &[raw.clone()]).unwrap());
    raw.width = Some(100);
    assert!(store.update_shot_raw_files("dims", 0, &[raw.clone()]).is_err());
    raw.height = Some(60);
    assert!(store.update_shot_raw_files("dims", 0, &[raw.clone()]).unwrap());
    raw.width = Some(200);
    assert!(store.update_shot_raw_files("dims", 0, &[raw]).is_err());
}

#[test]
fn corrective_marker_never_uses_previous_synthetic_snapshot_to_guess_legacy_layout() {
    let db = TestDb::new();
    let conn = legacy_v2(&db);
    conn.execute_batch("ALTER TABLE recipe_snapshots ADD COLUMN revision_id TEXT;
        ALTER TABLE parts ADD COLUMN recipe_revision TEXT; ALTER TABLE parts ADD COLUMN bundle_id TEXT;").unwrap();
    let first = recipe();
    legacy_snapshot(&conn, "old-first", &first);
    let mut second = first.clone(); second.version += 1; second.shots[0].camera = "another-camera".into();
    legacy_snapshot(&conn, "old-second", &second);
    let mut synthetic = first.clone(); synthetic.version += 2; synthetic.revision_id = format!("{}-v{}", synthetic.id, synthetic.version);
    let synthetic_json = serde_json::to_string(&synthetic).unwrap();
    conn.execute("INSERT INTO recipe_snapshots(revision_id,recipe_id,version,json) VALUES(?1,?2,?3,?4)",
        params![synthetic.revision_id, synthetic.id, synthetic.version, synthetic_json]).unwrap();
    let judgement = serde_json::to_string(&Judgement::error(1, "original")).unwrap();
    for token in [Some("old-first"), None] {
        conn.execute("INSERT INTO parts(ts,sn,recipe_id,recipe_version,recipe_hash,recipe_revision,verdict,plc_code,fault_code,reason,frames_expected,
            frames_received,triggers,software_version,judgement,frames,delivery_state,delivery_updated_at,recording_state)
            VALUES(1,42,?1,?2,?3,?4,'errInspect',90,1,'original',4,0,0,'legacy',?5,'[]','acknowledged',99,'off')",
            params![synthetic.id, synthetic.version, token, synthetic.revision_id, judgement]).unwrap();
    }
    drop(conn);
    let store = Store::open(&db.path()).unwrap();
    assert!(store.detail(1).unwrap().summary.recipe_revision.is_none());
    assert_eq!(store.detail(2).unwrap().summary.recipe_revision.as_deref(), Some(synthetic.revision_id.as_str()));
    assert_eq!(store.detail(1).unwrap().summary.delivery.state, PlcDeliveryState::Acknowledged);
    assert_eq!(store.recipe_snapshot(&synthetic.revision_id).unwrap().unwrap().shots[0].camera, first.shots[0].camera);
    assert_eq!(store.conn.lock().unwrap().query_row("SELECT json FROM recipe_snapshots WHERE revision_id=?1", [&synthetic.revision_id], |r| r.get::<_, String>(0)).unwrap(), synthetic_json);
    drop(store);
    let reopened = Store::open(&db.path()).unwrap();
    assert!(reopened.detail(1).unwrap().summary.recipe_revision.is_none());
    assert_eq!(reopened.detail(2).unwrap().summary.recipe_revision.as_deref(), Some(synthetic.revision_id.as_str()));
}


#[test]
fn deferred_recovery_allows_durable_recording_replay_before_failing_remaining_pending() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let delivery = PlcDelivery { state: PlcDeliveryState::Submitted, updated_at: 9, message: None };
    let completed = save(&store, &recipe, "durable-completed", &shots(&recipe), &delivery, None).unwrap();
    let interrupted = save(&store, &recipe, "actually-interrupted", &shots(&recipe), &delivery, None).unwrap();
    let original = store.detail(completed).unwrap().summary;
    drop(store);
    let reopened = Store::open_deferred_recovery(&db.path()).unwrap();
    assert_eq!(reopened.detail(completed).unwrap().recording.state, RecordingState::Pending);
    reopened.update_shot_raw_files("durable-completed", 0, &[ShotRawFile {
        view: 1, file: "records/durable-completed/k000.pgm".into(), width: Some(2), height: Some(3),
    }]).unwrap();
    let evidence = RecordingEvidence { state: RecordingState::Complete, available: true,
        directory: Some("records/durable-completed".into()), errors: Vec::new() };
    reopened.update_recording("durable-completed", &evidence).unwrap();
    assert_eq!(reopened.finish_interrupted_recordings().unwrap(), 1);
    let detail = reopened.detail(completed).unwrap();
    assert_eq!(detail.recording, evidence);
    assert_eq!(serde_json::to_value(&detail.summary).unwrap(), serde_json::to_value(original).unwrap());
    assert_eq!(reopened.detail(interrupted).unwrap().recording.state, RecordingState::Failed);
    assert_eq!(reopened.finish_interrupted_recordings().unwrap(), 0);
}


#[test]
fn submission_timing_is_a_single_cycle_observation_and_never_replaces_existing_measurement() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = recipe();
    let delivery = PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 9, message: None };
    let id = save(&store, &recipe, "observed-submission", &shots(&recipe), &delivery, None).unwrap();
    store.conn.lock().unwrap().execute("UPDATE parts SET drain_ms=NULL WHERE id=?1", [id]).unwrap();
    let before = store.detail(id).unwrap();
    assert!(!store.update_submission_timing("another-cycle", 53).unwrap());
    assert!(store.update_submission_timing("observed-submission", 53).unwrap());
    assert!(store.update_submission_timing("observed-submission", 53).unwrap());
    assert!(store.update_submission_timing("observed-submission", 52).is_err());
    assert!(store.update_submission_timing("observed-submission", u64::MAX).is_err());
    let mut expected = serde_json::to_value(before).unwrap();
    expected["summary"]["drainMs"] = serde_json::json!(53);
    assert_eq!(serde_json::to_value(store.detail(id).unwrap()).unwrap(), expected);
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
    let mut record = PartRecord { ts: 1234, sn: 42, recipe: Some(&recipe), judgement: &judgement, drain_ms: None,
        frames: &[], frames_expected: recipe.shot_count(), frames_received: 3, triggers: 4, table: Some(&table),
        software_version: "test-p0", cycle_id: Some("record-identity"), bundle_id: Some("frozen-bundle-id"), delivery: &pending, shots: &shots };
    assert!(store.matches_record(&record).unwrap());
    store.update_delivery("record-identity", &PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 9999, message: None }).unwrap();
    store.update_shot_raw_files("record-identity", 0, &[ShotRawFile { view: 1, file: "records/cycle/k000.pgm".into(), width: Some(2), height: Some(3) }]).unwrap();
    assert!(store.matches_record(&record).unwrap());
    record.sn = 43;
    assert!(!store.matches_record(&record).unwrap());
    record.sn = 42;
    record.ts += 1;
    assert!(!store.matches_record(&record).unwrap());
    record.ts -= 1;
    record.triggers += 1;
    assert!(!store.matches_record(&record).unwrap());
    record.triggers -= 1;
    let mut wrong_shots = shots.clone();
    wrong_shots[0].view = 2;
    record.shots = &wrong_shots;
    assert!(!store.matches_record(&record).unwrap());
    record.shots = &shots;
    record.table = None;
    assert!(!store.matches_record(&record).unwrap());
    record.table = Some(&table);
    let mut changed = recipe.clone();
    changed.product_code += 1;
    record.recipe = Some(&changed);
    assert!(!store.matches_record(&record).unwrap());
}

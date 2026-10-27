use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gs-log-storage-{}-{}-{}",
            std::process::id(),
            ly_plc::now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn data(&self, name: &str) -> PathBuf {
        let data = self.0.join(name);
        fs::create_dir_all(data.join("config")).unwrap();
        data
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn legacy_logs_wal_and_recordings_migrate_with_references_and_preserve_project_assets() {
    let fixture = Fixture::new();
    let data = fixture.data("data");
    let logs = data.join("logs");
    let project = data.join("projects/A");
    let old = project.join("records/20261027/part_cycle_c1");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(project.join("audit-spool")).unwrap();
    fs::create_dir_all(project.join("workspaces/A/images")).unwrap();
    fs::write(logs.join("gluesight-2026-10-27.jsonl"), b"runtime").unwrap();
    fs::write(old.join("cam1_s000001.pgm"), b"pixels").unwrap();
    fs::write(project.join("workspaces/A/images/taught.pgm"), b"teaching").unwrap();
    let plc = rusqlite::Connection::open(logs.join("plc.db")).unwrap();
    plc.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE logs(message TEXT); INSERT INTO logs VALUES('wal-entry');").unwrap();
    let db = rusqlite::Connection::open(project.join("inspection.db")).unwrap();
    db.execute_batch("CREATE TABLE parts(recording_directory TEXT);")
        .unwrap();
    db.execute(
        "INSERT INTO parts VALUES(?1)",
        [old.to_string_lossy().as_ref()],
    )
    .unwrap();
    fs::write(
        project.join("audit-spool/c1.json"),
        serde_json::json!({"Recording":{"directory":old}}).to_string(),
    )
    .unwrap();
    let layout = prepare(&data, &fixture.0.join("gs_logs"), "test").unwrap();
    let external = layout.vision("A").join("records/20261027/part_cycle_c1");
    assert_eq!(
        fs::read(external.join("cam1_s000001.pgm")).unwrap(),
        b"pixels"
    );
    assert_eq!(fs::read(old.join("cam1_s000001.pgm")).unwrap(), b"pixels");
    assert_eq!(
        fs::read(project.join("workspaces/A/images/taught.pgm")).unwrap(),
        b"teaching"
    );
    let moved: String = db
        .query_row("SELECT recording_directory FROM parts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(Path::new(&moved), external);
    let spool: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join("audit-spool/c1.json")).unwrap()).unwrap();
    assert_eq!(spool["Recording"]["directory"], serde_json::json!(external));
    let copied = rusqlite::Connection::open(layout.runtime().join("plc.db")).unwrap();
    assert_eq!(
        copied
            .query_row("SELECT message FROM logs", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal-entry"
    );
    assert_eq!(
        fs::read(layout.runtime().join("gluesight-2026-10-27.jsonl")).unwrap(),
        b"runtime"
    );
    assert_eq!(recording_project(&project).unwrap(), layout.vision("A"));
}

#[test]
fn project_database_with_leftover_wal_sidecars_migrates_recording_references_and_images() {
    let fixture = Fixture::new();
    let data = fixture.data("data");
    let project = data.join("projects/A");
    let old = project.join("records/20261027/part_cycle_c1");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("cam1_s000001.pgm"), b"P5\n1 1\n255\n\x7f").unwrap();
    let seed = fixture.0.join("seed");
    fs::create_dir_all(&seed).unwrap();
    let writer = rusqlite::Connection::open(seed.join("inspection.db")).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE parts(recording_directory TEXT); CREATE TABLE part_frames(raw_file TEXT);").unwrap();
    writer
        .execute(
            "INSERT INTO parts VALUES(?1)",
            [old.to_string_lossy().as_ref()],
        )
        .unwrap();
    let relative = "20261027/part_cycle_c1/cam1_s000001.pgm";
    writer
        .execute("INSERT INTO part_frames VALUES(?1)", [relative])
        .unwrap();
    for name in ["inspection.db", "inspection.db-wal", "inspection.db-shm"] {
        fs::copy(seed.join(name), project.join(name)).unwrap();
    }
    drop(writer);
    assert!(project.join("inspection.db-wal").is_file());
    assert!(project.join("inspection.db-shm").is_file());
    let layout = prepare(&data, &fixture.0.join("gs_logs"), "test").unwrap();
    let recordings = layout.vision("A").join("records");
    let db = rusqlite::Connection::open(project.join("inspection.db")).unwrap();
    let directory: String = db
        .query_row("SELECT recording_directory FROM parts", [], |row| {
            row.get(0)
        })
        .unwrap();
    let image: String = db
        .query_row("SELECT raw_file FROM part_frames", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        Path::new(&directory),
        recordings.join("20261027/part_cycle_c1")
    );
    assert_eq!(image, relative);
    assert_eq!(
        fs::read(recordings.join(&image)).unwrap(),
        b"P5\n1 1\n255\n\x7f"
    );
    assert_eq!(
        fs::read(old.join("cam1_s000001.pgm")).unwrap(),
        b"P5\n1 1\n255\n\x7f"
    );
}

#[test]
fn completed_migration_never_reimports_retained_legacy_copies_or_rebinds_root() {
    let fixture = Fixture::new();
    let data = fixture.data("data");
    fs::create_dir_all(data.join("logs")).unwrap();
    fs::write(data.join("logs/old.jsonl"), b"old").unwrap();
    let first = prepare(&data, &fixture.0.join("first-root"), "test").unwrap();
    let runtime = first.runtime();
    drop(first);
    fs::remove_file(runtime.join("old.jsonl")).unwrap();
    fs::write(data.join("logs/new-legacy.jsonl"), b"late").unwrap();
    let restarted = prepare(&data, &fixture.0.join("another-home"), "test").unwrap();
    assert_eq!(restarted.runtime(), runtime);
    assert!(!runtime.join("old.jsonl").exists());
    assert!(!runtime.join("new-legacy.jsonl").exists());
    assert!(!fixture.0.join("another-home").exists());
}

#[test]
fn matching_destination_releases_staged_copies_and_completed_restart_cleans_remaining_stage_without_reimport(
) {
    let fixture = Fixture::new();
    let data = fixture.data("data");
    fs::create_dir_all(data.join("logs")).unwrap();
    fs::write(data.join("logs/same.jsonl"), b"same").unwrap();
    let config = Config {
        schema: 1,
        root: fixture.0.join("logs"),
        owner_data: data.canonicalize().unwrap(),
        environment: "test".into(),
        instance: "1111111111111111".into(),
    };
    fs::write(config_file(&data), serde_json::to_string(&config).unwrap()).unwrap();
    let runtime = config.runtime();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("same.jsonl"), b"same").unwrap();
    let layout = prepare(&data, &fixture.0.join("ignored"), "test").unwrap();
    assert_eq!(fs::read(runtime.join("same.jsonl")).unwrap(), b"same");
    assert!(!runtime.join(".legacy-staging").exists());
    drop(layout);
    fs::create_dir_all(runtime.join(".legacy-staging")).unwrap();
    fs::write(runtime.join(".legacy-staging/same.jsonl"), b"same").unwrap();
    fs::remove_file(runtime.join("same.jsonl")).unwrap();
    let _restarted = prepare(&data, &fixture.0.join("ignored"), "test").unwrap();
    assert!(!runtime.join(".legacy-staging").exists());
    assert!(!runtime.join("same.jsonl").exists());
    assert_eq!(fs::read(data.join("logs/same.jsonl")).unwrap(), b"same");
}

#[test]
fn conflicting_destination_blocks_startup_without_rewriting_history_and_can_resume() {
    let fixture = Fixture::new();
    let data = fixture.data("data");
    let project = data.join("projects/A");
    fs::create_dir_all(project.join("records/day")).unwrap();
    fs::write(project.join("records/day/frame.pgm"), b"source").unwrap();
    let config = Config {
        schema: 1,
        root: fixture.0.join("logs"),
        owner_data: data.canonicalize().unwrap(),
        environment: "test".into(),
        instance: "0123456789abcdef".into(),
    };
    fs::write(config_file(&data), serde_json::to_string(&config).unwrap()).unwrap();
    let target = config.vision("A").join("records/day/frame.pgm");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"conflict").unwrap();
    let db = rusqlite::Connection::open(project.join("inspection.db")).unwrap();
    db.execute_batch("CREATE TABLE parts(recording_directory TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO parts VALUES(?1)",
        [project.join("records/day").to_string_lossy().as_ref()],
    )
    .unwrap();
    assert!(prepare(&data, &fixture.0.join("ignored"), "test").is_err());
    let original: String = db
        .query_row("SELECT recording_directory FROM parts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(Path::new(&original), project.join("records/day"));
    assert_eq!(fs::read(&target).unwrap(), b"conflict");
    fs::remove_file(&target).unwrap();
    let _layout = prepare(&data, &fixture.0.join("ignored"), "test").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"source");
    let moved: String = db
        .query_row("SELECT recording_directory FROM parts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(Path::new(&moved), target.parent().unwrap());
}

#[test]
fn independent_data_roots_are_isolated_and_cloned_configuration_is_rejected_after_owner_exits() {
    let fixture = Fixture::new();
    let data_a = fixture.data("a");
    let data_b = fixture.data("b");
    let data_c = fixture.data("c");
    let root = fixture.0.join("shared");
    let a = prepare(&data_a, &root, "test").unwrap();
    let b = prepare(&data_b, &root, "test").unwrap();
    assert_ne!(a.runtime(), b.runtime());
    assert_ne!(a.vision("same-project"), b.vision("same-project"));
    let original = a.vision("same-project").join("records/owned.pgm");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"original").unwrap();
    drop(a);
    fs::copy(config_file(&data_a), config_file(&data_c)).unwrap();
    assert!(prepare(&data_c, &root, "test").is_err());
    assert!(recording_project(&data_c.join("projects/same-project")).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"original");
    let restarted = prepare(&data_a, &root, "test").unwrap();
    assert_eq!(
        fs::read(restarted.vision("same-project").join("records/owned.pgm")).unwrap(),
        b"original"
    );
}

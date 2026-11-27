use super::*;
use crate::offline::test_dir::TestDir;
use crate::operation::{Operation, Operations, CANCELLED_MESSAGE};
use crate::store::{burst_fixture, PartMode, PartRecord, PlcDelivery, RecordSource};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct TestFiles(PathBuf);

impl TestFiles {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gluesight-diagnostic-stream-{}-{}-{}",
            std::process::id(),
            ly_plc::now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestFiles {
    fn drop(&mut self) {
        for database in ["parts.sqlite", "plc.sqlite"] {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(self.0.join(format!("{database}{suffix}")));
            }
        }
        let _ = fs::remove_dir(&self.0);
    }
}

fn anonymous_operation() -> Operation {
    Operation::with_events(Operations::default(), None, |_| {}).unwrap()
}

#[test]
fn cancelled_archive_removes_the_temporary_zip_and_preserves_an_existing_export() {
    let dir = TestDir::new("diagnostics-tests", "cancel-archive");
    let path = dir.join("diagnostics.zip");
    let temporary = path.with_extension("zip.tmp");
    let mut existing = ZipWriter::new(Vec::new());
    existing
        .add("README.md", b"existing export", false)
        .unwrap();
    let existing = existing.finish().unwrap();
    fs::write(&path, &existing).unwrap();

    let tasks = Operations::default();
    let task_id = "cancel-diagnostic-export";
    let operation = Operation::with_events(tasks.clone(), Some(task_id.into()), |_| {}).unwrap();
    let result = operation.run(|operation| {
        write_archive(&path, operation, |package| {
            package.add("README.md", b"new export")?;
            assert!(temporary.is_file());
            assert!(tasks.cancel(task_id));
            Ok(())
        })
    });

    assert_eq!(result.unwrap_err(), CANCELLED_MESSAGE);
    assert!(!temporary.exists());
    assert_eq!(fs::read(path).unwrap(), existing);
}

#[test]
fn invalid_plc_row_keeps_read_logs_in_the_zip_and_records_the_failure() {
    let files = TestFiles::new();
    let path = files.0.join("plc.sqlite");
    let _logbook = ly_plc::Logbook::open(&path, std::sync::Arc::new(|_| {}), 30).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let ts = ly_plc::now_ms();
    conn.execute(
        "INSERT INTO plc_log (ts,level,category,message,context) VALUES (?1,'info','part','开工','requestSeq=3'), (?2,'warn','write',x'FF',NULL)",
        [ts, ts + 1],
    )
    .unwrap();
    drop(conn);

    let mut package = Package::new(Cursor::new(Vec::new()));
    let count = write_plc_logs(
        &mut package,
        &path,
        ts,
        ts + 1,
        MAX_PLC_LOGS,
        &anonymous_operation(),
    )
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(package.notes.len(), 1);
    assert!(package.notes[0].starts_with("PLC 日志读取失败"));
    let packed = &package.files[0];
    let mut archive = zip::ZipArchive::new(package.zip.finish().unwrap()).unwrap();
    let mut jsonl = String::new();
    archive
        .by_name("logs/plc.jsonl")
        .unwrap()
        .read_to_string(&mut jsonl)
        .unwrap();
    assert_eq!(packed.bytes, jsonl.len() as u64);
    assert_eq!(
        packed.sha256,
        format!("{:x}", Sha256::digest(jsonl.as_bytes()))
    );
    let entry: serde_json::Value = serde_json::from_str(&jsonl).unwrap();
    assert_eq!(entry["message"], "开工");
    assert_eq!(entry["category"], "part");
    assert_eq!(entry["context"], "requestSeq=3");
    assert_eq!(entry["t"], applog::local_time(ts).unwrap());
}

#[test]
fn streamed_record_entries_keep_explicit_order_full_details_and_checksums() {
    let files = TestFiles::new();
    let store = Store::open(&files.0.join("parts.sqlite")).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(4));
    let insert = |sn: &str, ts| {
        store
            .insert(&PartRecord {
                mode: PartMode::Production,
                ts,
                sn,
                recipe: Some(&recipe),
                judgement: &judgement,
                drain_ms: None,
                frames: &[],
                frames_expected: burst.expected_total.unwrap_or(0) as usize,
                frames_received: burst.judgement.group_count as usize,
                triggers: 4,
                table: None,
                software_version: "test-stream",
                cycle_id: Some(sn),
                bundle_id: None,
                delivery: &PlcDelivery::default(),
                shots: &[],
                source: RecordSource::Real,
                burst: Some(&burst),
            })
            .unwrap()
    };
    let older = insert("stream-old", 10);
    let newer = insert("stream-new", 20);
    let missing = newer + 100;
    let request = ExportRequest {
        from: 1000,
        to: 1001,
        scope: None,
        history_ids: vec![older, missing, newer],
        images: ImageMode::Abnormal,
        max_image_parts: 1,
    };
    let expected = [older, newer].map(|id| store.detail(id).unwrap());

    let mut package = Package::new(Cursor::new(Vec::new()));
    let records =
        write_part_entries(&mut package, &store, &request, &anonymous_operation()).unwrap();
    assert_eq!(records.count, 2);
    assert_eq!(records.images.len(), 1);
    assert_eq!(records.images[0].id, newer);
    assert_eq!(package.notes.len(), 1);
    assert!(package.notes[0].contains(&format!("#{missing}")));
    package.add("after.txt", b"after records").unwrap();
    let manifest_entries = package.files;
    let mut archive = zip::ZipArchive::new(package.zip.finish().unwrap()).unwrap();
    let mut entry_bytes = |name| {
        let mut data = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut data)
            .unwrap();
        data
    };

    let csv = entry_bytes("records/parts.csv");
    assert!(csv.starts_with(b"\xef\xbb\xbf"));
    let mut expected_csv = crate::history::CSV_HEADER.as_bytes().to_vec();
    for detail in &expected {
        expected_csv.extend_from_slice(crate::history::csv_row(&detail.summary).as_bytes());
    }
    assert_eq!(csv, expected_csv);

    let jsonl = entry_bytes("records/parts.jsonl");
    let actual: Vec<serde_json::Value> = std::str::from_utf8(&jsonl)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        actual,
        expected.map(|detail| serde_json::to_value(detail).unwrap())
    );
    assert_eq!(entry_bytes("after.txt"), b"after records");

    for (name, data) in [("records/parts.csv", csv), ("records/parts.jsonl", jsonl)] {
        let packed = manifest_entries
            .iter()
            .find(|entry| entry.path == name)
            .unwrap();
        assert_eq!(packed.bytes, data.len() as u64);
        assert_eq!(packed.sha256, format!("{:x}", Sha256::digest(data)));
    }
}

#[test]
fn streamed_records_keep_the_most_recent_abnormal_image_candidate() {
    let files = TestFiles::new();
    let store = Store::open(&files.0.join("parts.sqlite")).unwrap();
    insert_summary(&store, "pass", 30, Verdict::OkWithExcursion);
    let recent = insert_summary(&store, "recent", 20, Verdict::NgWidth);
    insert_summary(&store, "imported-old", 10, Verdict::ErrInspect);
    let request = ExportRequest {
        from: 0,
        to: 31,
        scope: None,
        history_ids: Vec::new(),
        images: ImageMode::Abnormal,
        max_image_parts: 1,
    };
    let mut package = Package::new(Cursor::new(Vec::new()));
    let records =
        write_part_entries(&mut package, &store, &request, &anonymous_operation()).unwrap();
    assert_eq!(
        records
            .images
            .iter()
            .map(|part| part.id)
            .collect::<Vec<_>>(),
        [recent]
    );
}

#[test]
fn time_range_record_stream_rejects_a_bad_detail_without_manifest_entries() {
    let files = TestFiles::new();
    let path = files.0.join("parts.sqlite");
    let store = Store::open(&path).unwrap();
    let bad = insert_summary(&store, "bad", 10, Verdict::ErrInspect);
    insert_summary(&store, "valid", 20, Verdict::ErrInspect);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE parts SET tags='[' WHERE id=?1", [bad])
        .unwrap();
    let request = ExportRequest {
        from: 0,
        to: 31,
        scope: None,
        history_ids: Vec::new(),
        images: ImageMode::None,
        max_image_parts: 0,
    };
    let mut package = Package::new(Cursor::new(Vec::new()));
    assert!(write_part_entries(&mut package, &store, &request, &anonymous_operation()).is_err());
    assert!(package.files.is_empty());
}

fn insert_summary(store: &Store, sn: &str, ts: i64, verdict: Verdict) -> i64 {
    let mut judgement = crate::judge::Judgement::error(95, "test");
    judgement.verdict = verdict;
    store
        .insert(&PartRecord {
            mode: PartMode::Production,
            ts,
            sn,
            recipe: None,
            judgement: &judgement,
            drain_ms: None,
            frames: &[],
            frames_expected: 0,
            frames_received: 0,
            triggers: 0,
            table: None,
            software_version: "test-stream",
            cycle_id: Some(sn),
            bundle_id: None,
            delivery: &PlcDelivery::default(),
            shots: &[],
            source: RecordSource::Real,
            burst: None,
        })
        .unwrap()
}

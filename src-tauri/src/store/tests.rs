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

#[test]
fn manual_tags_survive_reopen_and_keep_history_filter_counts_neighbors_and_rejudge_aligned() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let ids: Vec<_> = ["tag-a", "tag-b", "tag-c"]
        .into_iter()
        .map(|cycle| {
            store
                .insert(&burst_part(&recipe, cycle, &judgement, &burst))
                .unwrap()
        })
        .collect();
    assert_eq!(
        store
            .set_history_tags(
                &[ids[0], ids[2]],
                &[
                    " 故障件 ".into(),
                    "".into(),
                    "故障件".into(),
                    "参考件".into()
                ]
            )
            .unwrap(),
        ["故障件", "参考件"]
    );
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    let query = HistoryQuery {
        tag: Some("故障件".into()),
        ..Default::default()
    };
    let page = store.query(&query).unwrap();
    assert_eq!(
        page.items.iter().map(|part| part.id).collect::<Vec<_>>(),
        [ids[2], ids[0]]
    );
    assert_eq!(page.total, 2);
    assert_eq!(
        page.counts.ok + page.counts.excursion + page.counts.ng + page.counts.err,
        2
    );
    assert_eq!(
        store.neighbors(&query, ids[0]).unwrap(),
        (None, Some(ids[2]))
    );
    assert_eq!(
        store
            .measurements(&query, &[], 10)
            .unwrap()
            .iter()
            .map(|part| part.id)
            .collect::<Vec<_>>(),
        [ids[2], ids[0]]
    );
    let detail = store.detail(ids[0]).unwrap();
    assert_eq!(detail.summary.tags, ["故障件", "参考件"]);
    assert_eq!(detail.judgement.verdict, judgement.verdict);
    assert!(crate::history::csv_row(&detail.summary).contains("故障件、参考件"));
    store.set_history_tags(&[ids[0]], &[]).unwrap();
    assert_eq!(store.query(&query).unwrap().total, 1);
}

#[test]
fn invalid_or_stale_history_tag_selection_leaves_every_selected_record_unchanged() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let id = store
        .insert(&burst_part(&recipe, "tag-validation", &judgement, &burst))
        .unwrap();
    store.set_history_tags(&[id], &["参考件".into()]).unwrap();
    for invalid in [
        vec!["长".repeat(25)],
        (0..9).map(|n| n.to_string()).collect(),
        vec!["故障\n件".into()],
    ] {
        assert!(store.set_history_tags(&[id], &invalid).is_err());
        assert_eq!(store.detail(id).unwrap().summary.tags, ["参考件"]);
    }
    assert!(store
        .set_history_tags(&[id, id + 100], &["故障件".into()])
        .is_err());
    assert_eq!(store.detail(id).unwrap().summary.tags, ["参考件"]);
}

#[test]
fn manually_deleted_history_stays_absent_and_explicit_reimport_uses_a_fresh_nonreusable_identity() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let record = burst_part(&recipe, "delete-restore", &judgement, &burst);
    let id = store.insert(&record).unwrap();
    store.set_history_tags(&[id], &["故障件".into()]).unwrap();
    assert!(store.delete_history(&[id]).is_err());
    assert!(store.detail(id).is_ok());
    store
        .update_recording(
            "delete-restore",
            &RecordingEvidence {
                state: RecordingState::Off,
                ..Default::default()
            },
        )
        .unwrap();
    let package = db.0.join("restore.sqlite");
    store.export_parts(&[id], &package).unwrap();
    assert!(store.delete_history(&[id, id + 100]).is_err());
    assert!(store.detail(id).is_ok());
    store.delete_history(&[id]).unwrap();
    assert!(store.detail(id).is_err());
    assert!(store.burst_frames(id).unwrap().is_empty());
    assert!(store.insert(&record).is_err());
    assert!(store.history_cycle_deleted("delete-restore").unwrap());
    let merged = store.import_parts(&package, "default", |_| Ok(())).unwrap();
    let restored = merged.ids[&id];
    assert!(restored > id);
    assert_eq!(store.detail(restored).unwrap().summary.tags, ["故障件"]);
    assert!(store.history_cycle_deleted("delete-restore").unwrap());
    store.delete_history(&[restored]).unwrap();
    drop(store);
    let store = Store::open(&db.path()).unwrap();
    let fresh = store
        .insert(&burst_part(&recipe, "after-delete", &judgement, &burst))
        .unwrap();
    assert!(fresh > restored);
    assert!(store.detail(id).is_err());
    assert!(store.detail(restored).is_err());
    assert!(store.history_cycle_deleted("delete-restore").unwrap());
}

#[test]
fn legacy_history_without_tags_upgrades_in_place_and_imports_without_losing_existing_records() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let id = store
        .insert(&burst_part(&recipe, "legacy-tags", &judgement, &burst))
        .unwrap();
    let package = db.0.join("legacy.sqlite");
    store.export_parts(&[id], &package).unwrap();
    drop(store);
    for path in [&package, &db.path()] {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("ALTER TABLE parts DROP COLUMN tags")
            .unwrap();
    }
    let store = Store::open(&db.path()).unwrap();
    assert!(store.backup_path().is_none());
    assert!(store.detail(id).unwrap().summary.tags.is_empty());
    let imported = Store::open_for(&db.0.join("target.sqlite"), "target").unwrap();
    let merged = imported
        .import_parts(&package, "target", |_| Ok(()))
        .unwrap();
    assert_eq!(merged.inserted, 1);
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 1);
    assert!(imported
        .detail(merged.ids[&id])
        .unwrap()
        .summary
        .tags
        .is_empty());
}

#[test]
fn malformed_package_tags_are_rejected_before_any_history_is_imported() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let id = store
        .insert(&burst_part(
            &recipe,
            "invalid-package-tags",
            &judgement,
            &burst,
        ))
        .unwrap();
    let package = db.0.join("invalid.sqlite");
    store.export_parts(&[id], &package).unwrap();
    let imported = Store::open_for(&db.0.join("target.sqlite"), "target").unwrap();
    for invalid in [
        "{}".to_string(),
        "[42]".into(),
        serde_json::to_string(&vec!["长".repeat(25)]).unwrap(),
    ] {
        let conn = Connection::open(&package).unwrap();
        conn.execute("UPDATE parts SET tags = ?1", [invalid])
            .unwrap();
        drop(conn);
        assert!(imported
            .import_parts(&package, "target", |_| Ok(()))
            .is_err());
        assert_eq!(imported.query(&HistoryQuery::default()).unwrap().total, 0);
    }
}

#[test]
fn manual_history_deletion_only_removes_exclusive_cycle_directories_inside_the_project_recording_root(
) {
    for case in [
        "exclusive",
        "shared",
        "descendant",
        "ancestor",
        "outside",
        "parent",
    ] {
        let db = TestDb::new();
        let store = Store::open(&db.path()).unwrap();
        let recipe = burst_fixture::recipe();
        let (judgement, burst) =
            burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
        let root = crate::project::records::record_roots(&db.0, "default")
            .unwrap()
            .swap_remove(0);
        let day = root.join("20261108");
        let leaf = "20261108_120000_OK_cycle_delete-images";
        let target = match case {
            "outside" => db.0.join("outside").join(leaf),
            "parent" => day.clone(),
            _ => day.join(leaf),
        };
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let marker = target.join("original.pgm");
        std::fs::write(&marker, b"history image").unwrap();
        let unrelated = root.join("unrelated.pgm");
        std::fs::write(&unrelated, b"keep").unwrap();
        let id = store
            .insert(&burst_part(&recipe, "delete-images", &judgement, &burst))
            .unwrap();
        store
            .update_recording(
                "delete-images",
                &RecordingEvidence {
                    state: RecordingState::Incomplete,
                    directory: Some(target.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        let shared = match case {
            "shared" => Some(target.clone()),
            "descendant" => Some(target.join("another-record")),
            "ancestor" => Some(day.clone()),
            _ => None,
        };
        if let Some(shared) = shared {
            std::fs::create_dir_all(&shared).unwrap();
            store
                .insert(&burst_part(&recipe, "keep-images", &judgement, &burst))
                .unwrap();
            store
                .update_recording(
                    "keep-images",
                    &RecordingEvidence {
                        state: RecordingState::Incomplete,
                        directory: Some(shared.to_string_lossy().into_owned()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let warnings = store.delete_history_with_recordings(&db.0, &[id]).unwrap();
        assert!(store.detail(id).is_err(), "{case}");
        assert_eq!(marker.exists(), case != "exclusive", "{case}");
        assert_eq!(warnings.is_empty(), case == "exclusive", "{case}");
        assert!(unrelated.exists(), "{case}");
    }
}

fn burst_part<'a>(recipe: &'a Recipe, cycle: &'a str, judgement: &'a Judgement, burst: &'a BurstRecord) -> PartRecord<'a> {
    PartRecord {
        mode: PartMode::Production,
        ts: 1234,
        sn: "SN-B1",
        recipe: Some(recipe),
        judgement,
        drain_ms: None,
        frames: &[],
        frames_expected: burst.expected_total.unwrap_or(0) as usize,
        frames_received: burst.judgement.group_count as usize,
        triggers: burst.device_triggers.unwrap_or(0).into(),
        table: None,
        software_version: "test-burst",
        cycle_id: Some(cycle),
        bundle_id: None,
        delivery: &PlcDelivery {
            state: PlcDeliveryState::NotRequired,
            updated_at: 0,
            message: None,
        },
        shots: &[],
        source: RecordSource::Real,
        burst: Some(burst),
    }
}

#[test]
fn overview_keeps_verdicts_and_single_frame_loads_only_selected_metrics() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let mut frames = burst_fixture::frames(4, &[]);
    for frame in &mut frames {
        frame.metrics = Some(burst_fixture::measured(frame.camera, frame.seq, 2.0));
    }
    let metrics = frames[0].metrics.as_mut().unwrap();
    metrics.widths[0] = None;
    metrics.measurement_evidence = Some(crate::burst_judge::MeasurementEvidence {
        version: 1,
        stations: (0..metrics.widths.len())
            .map(|i| crate::burst_judge::StationEvidence {
                state: if i == 0 {
                    crate::burst_judge::StationEvidenceState::Clipped
                } else {
                    crate::burst_judge::StationEvidenceState::Exact
                },
                width_lower_bound_mm: (i == 0).then_some(9.0),
                ..Default::default()
            })
            .collect(),
        exact_stations: metrics.widths.len() as u32 - 1,
        clipped_stations: 1,
        max_width_lower_bound_mm: Some(9.0),
        present_ratio: Some(1.0),
        ..Default::default()
    });
    let (judgement, burst) = burst_fixture::record(&recipe, frames, None, Some(0));
    let id = store
        .insert(&burst_part(&recipe, "overview-part", &judgement, &burst))
        .unwrap();
    let full = store.detail(id).unwrap().burst.unwrap();
    let overview = store.detail_overview(id).unwrap().burst.unwrap();
    let full_evidence = full.frames[0]
        .metrics
        .as_ref()
        .unwrap()
        .measurement_evidence
        .as_ref()
        .unwrap();
    let summary_evidence = overview.frames[0]
        .metrics
        .as_ref()
        .unwrap()
        .measurement_evidence
        .as_ref()
        .unwrap();
    assert!(!full_evidence.stations.is_empty());
    assert!(summary_evidence.stations.is_empty());
    let mut expected_summary = full_evidence.clone();
    expected_summary.stations.clear();
    assert_eq!(summary_evidence, &expected_summary);
    assert_eq!(
        serde_json::to_value(&overview.frame_verdicts).unwrap(),
        serde_json::to_value(&full.frame_verdicts).unwrap()
    );
    assert!(overview
        .frames
        .iter()
        .all(|frame| frame.metrics.as_ref().unwrap().widths.is_empty()));
    let expected = full
        .frames
        .iter()
        .find(|frame| frame.camera == 0 && frame.seq == 1)
        .unwrap();
    assert!(!expected.metrics.as_ref().unwrap().widths.is_empty());
    rusqlite::Connection::open(db.path())
        .unwrap()
        .execute(
            "UPDATE part_frames SET metrics='broken' WHERE part_id=?1 AND camera=1 AND seq=2",
            [id],
        )
        .unwrap();
    let selected = store.detail_frame(id, 0, 1).unwrap().burst.unwrap();
    assert_eq!(selected.frames, vec![expected.clone()]);
    assert!(selected.frame_verdicts.is_empty());
    assert!(store.detail_frame(id, 0, 100).is_err());
    assert!(store.detail(id).is_err());
}

#[test]
fn upgrading_v10_removes_retired_records_without_losing_production_and_prevents_replay() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let cycles = [
        "production-kept",
        "retired-synthetic",
        "retired-sample",
        "retired-candidate",
    ];
    let ids: Vec<_> = cycles
        .iter()
        .map(|cycle| {
            store
                .insert(&burst_part(&recipe, cycle, &judgement, &burst))
                .unwrap()
        })
        .collect();
    store
        .set_history_tags(&[ids[0]], &["保留生产".into()])
        .unwrap();
    let retained = store.detail(ids[0]).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch("DROP TRIGGER production_source_insert; DROP TRIGGER production_source_update; PRAGMA ignore_check_constraints = ON; PRAGMA user_version = 10;").unwrap();
        for (id, source) in
            ids[1..]
                .iter()
                .zip(["offlineSynthetic", "offlineSample", "offlineCandidate"])
        {
            conn.execute(
                "UPDATE parts SET source = ?2, recording_directory = ?3 WHERE id = ?1",
                params![id, source, format!("not-a-production-path-{id}")],
            )
            .unwrap();
        }
    }
    drop(store);
    for _ in 0..2 {
        let store = Store::open(&db.path()).unwrap();
        assert!(
            store.backup_path().is_none(),
            "v10 production data must be upgraded in place"
        );
        let page = store.query(&HistoryQuery::default()).unwrap();
        assert_eq!(
            page.items.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![ids[0]]
        );
        let production = store.detail(ids[0]).unwrap();
        assert_eq!(production.summary.tags, retained.summary.tags);
        assert_eq!(
            production.burst.as_ref().unwrap().frames,
            retained.burst.as_ref().unwrap().frames
        );
        for (id, cycle) in ids[1..].iter().zip(&cycles[1..]) {
            assert!(store.history_cycle_deleted(cycle).unwrap());
            assert!(store
                .insert(&burst_part(&recipe, cycle, &judgement, &burst))
                .is_err());
            let conn = store.conn.lock().unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM part_frames WHERE part_id=?1",
                    [id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM retired_recordings", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            11
        );
    }
}

/// 防的回归（PR #292 评审）：相机级 `detect`、相机级或配方级 `expert` 已停用、读入时丢弃，按它们测的历史件快照与当前配方
/// 被当成同一测量布局，只按规则重判就复用了口径不同的已存指标。旧快照里这些键为空（null / {}）或只有标定引用时照常复用。
#[test]
fn snapshot_with_retired_measure_overrides_needs_raw_remeasurement() {
    let current = crate::recipe::samples_burst().build().unwrap();
    type Edit = fn(&mut serde_json::Value);
    let cases: [(&str, Edit, bool); 5] = [
        (
            "相机级检测参数",
            |s| s["inspect"][0]["detect"] = serde_json::json!({ "searchMm": 6.0, "polarity": "dark", "widthRange": [1.0, 5.0] }),
            false,
        ),
        (
            "相机级专家参数",
            |s| s["inspect"][1]["expert"] = serde_json::json!({ "presentRatio": 0.4 }),
            false,
        ),
        (
            "配方级专家参数",
            |s| s["expert"] = serde_json::json!({ "trackSharpMin": 0.6 }),
            false,
        ),
        (
            "旧快照的空值",
            |s| {
                s["expert"] = serde_json::json!({});
                s["inspect"][0]["detect"] = serde_json::Value::Null;
                s["inspect"][0]["expert"] = serde_json::json!({});
            },
            true,
        ),
        (
            "相机级标定引用",
            |s| s["inspect"][0]["calib"] = "cam1-v1".into(),
            true,
        ),
    ];
    for (case, edit, reusable) in cases {
        let mut snapshot = serde_json::to_value(&current).unwrap();
        edit(&mut snapshot);
        let original = Recipe::from_snapshot(snapshot).unwrap();
        assert_eq!(
            same_measurement_layout(&original, &current),
            reusable,
            "{case}"
        );
    }
}

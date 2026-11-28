use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gluesight-store-{}-{}-{}",
            std::process::id(),
            ly_plc::now_ms(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
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
fn covering_index_upgrade_preserves_scoped_and_cross_project_history() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let insert = |store: &Store, cycle, ts| {
        store
            .insert(&PartRecord {
                ts,
                ..burst_part(&recipe, cycle, &judgement, &burst)
            })
            .unwrap()
    };
    let earlier = insert(&store, "index-upgrade-earlier", 90);
    let tied = insert(&store, "index-upgrade-tied", 100);
    let newer = insert(&store, "index-upgrade-newer", 100);
    let other = Store::open_for(&db.path(), "other-project").unwrap();
    insert(&other, "index-upgrade-other", 100);
    drop(other);
    store
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "DROP INDEX parts_source_ts_id_verdict;
         DROP INDEX parts_project_source_ts_id_verdict;
         CREATE INDEX parts_source_ts ON parts(source, ts);
         CREATE INDEX parts_project_ts ON parts(project_id, ts);
         PRAGMA user_version = 11;",
        )
        .unwrap();
    let query = HistoryQuery {
        from: Some(80),
        offset: 1,
        limit: 2,
        ..Default::default()
    };
    let scoped = HistoryQuery {
        project: Some(store.project().into()),
        ..query.clone()
    };
    let cases = [(query, [newer, tied]), (scoped, [tied, earlier])].map(|(query, ids)| {
        let before = store.query(&query).unwrap();
        (query, ids, serde_json::to_value(before).unwrap())
    });
    drop(store);

    let upgraded = Store::open(&db.path()).unwrap();
    assert!(upgraded.backup_path().is_none());
    for (query, ids, before) in cases {
        let after = upgraded.query(&query).unwrap();
        assert_eq!(
            after.items.iter().map(|part| part.id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(serde_json::to_value(after).unwrap(), before);
    }
    let index: String = upgraded
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT group_concat(name) FROM (
                 SELECT name FROM sqlite_schema WHERE type='index'
                 AND name IN ('parts_source_ts', 'parts_source_ts_id_verdict',
                              'parts_project_ts', 'parts_project_source_ts_id_verdict')
                 ORDER BY name
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        index,
        "parts_project_source_ts_id_verdict,parts_source_ts_id_verdict"
    );
}

#[test]
fn appending_a_recipe_revision_accepts_legacy_fields_and_rechecks_changed_snapshots() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let append = |cycle| store.insert(&burst_part(&recipe, cycle, &judgement, &burst));
    append("snapshot-first").unwrap();
    append("snapshot-unchanged").unwrap();

    let conn = Connection::open(db.path()).unwrap();
    conn.execute(
        "UPDATE recipe_snapshots SET json = json_set(json, '$.planVersion', 14)
         WHERE project_id = ?1 AND revision_id = ?2",
        params![store.project(), recipe.revision_id],
    )
    .unwrap();
    append("snapshot-legacy").unwrap();

    conn.execute(
        "UPDATE recipe_snapshots SET json = json_set(json, '$.name', '另一份配方')
         WHERE project_id = ?1 AND revision_id = ?2",
        params![store.project(), recipe.revision_id],
    )
    .unwrap();
    assert!(append("snapshot-conflicting").is_err());
    assert_eq!(store.query(&HistoryQuery::default()).unwrap().total, 3);
}

#[test]
fn reading_history_rechecks_cached_recipe_after_snapshot_content_or_identity_changes() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let id = store
        .insert(&burst_part(
            &recipe,
            "history-snapshot-edit",
            &judgement,
            &burst,
        ))
        .unwrap();
    let read = || {
        store
            .detail_frame_with_recipe(id, 0, 1)
            .map(|(_, recipe)| recipe)
    };
    assert_eq!(read().unwrap().name, recipe.name);

    let conn = Connection::open(db.path()).unwrap();
    conn.execute(
        "UPDATE recipe_snapshots
         SET json = json_set(json, '$.name', '更新后的快照', '$.expert', json('{\"trackSharpMin\":0.6}'))
         WHERE project_id = ?1 AND revision_id = ?2",
        params![store.project(), recipe.revision_id],
    )
    .unwrap();
    let updated = read().unwrap();
    assert_eq!(updated.name, "更新后的快照");
    assert!(!same_measurement_layout(&updated, &recipe));

    conn.execute(
        "UPDATE recipe_snapshots SET version = version + 1 WHERE project_id = ?1 AND revision_id = ?2",
        params![store.project(), recipe.revision_id],
    )
    .unwrap();
    assert!(read().is_err());
}

#[test]
fn history_comparison_checks_recipe_even_without_a_burst_record() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let id = store
        .insert(&PartRecord {
            burst: None,
            frames_expected: 0,
            ..burst_part(&recipe, "history-without-burst", &judgement, &burst)
        })
        .unwrap();
    let (detail, original) = store.history_comparison_record(id).unwrap();
    assert!(detail.burst.is_none());
    assert_eq!(original.unwrap().revision_id, recipe.revision_id);

    Connection::open(db.path()).unwrap().execute(
        "UPDATE recipe_snapshots SET version = version + 1 WHERE project_id = ?1 AND revision_id = ?2",
        params![store.project(), recipe.revision_id],
    ).unwrap();
    assert!(store.detail(id).is_ok());
    assert!(store.history_comparison_record(id).is_err());
}

#[test]
fn history_comparison_reads_frame_metadata_and_rejects_changed_malformed_metrics() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[(2, 4)]), None, Some(4));
    let cycle = "history-comparison-metadata";
    let id = store
        .insert(&burst_part(&recipe, cycle, &judgement, &burst))
        .unwrap();
    let files = BTreeMap::from([(FrameKey { camera: 0, seq: 1 }, "cam1/1.pgm".into())]);
    assert!(store.update_frame_raw_files(cycle, &files).unwrap());

    let overview = store.detail_overview(id).unwrap();
    let (options, original) = store.history_comparison_record(id).unwrap();
    assert_eq!(original.unwrap().revision_id, recipe.revision_id);
    assert_eq!(options.recording, overview.recording);
    let mut detail_burst = overview.burst.unwrap();
    let options_burst = options.burst.unwrap();
    assert_eq!(options_burst.cameras, detail_burst.cameras);
    assert_eq!(options_burst.judgement, detail_burst.judgement);
    for frame in &mut detail_burst.frames {
        frame.metrics = None;
    }
    assert_eq!(options_burst.frames, detail_burst.frames);

    let external = Connection::open(db.path()).unwrap();
    let previous: String = external
        .query_row(
            "SELECT metrics FROM part_frames WHERE part_id=?1 AND camera=1 AND seq=2",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    let damaged = format!("{{}}{}", " ".repeat(previous.len() - 2));
    assert_eq!(
        external
            .execute(
                "UPDATE part_frames SET metrics=?1 WHERE part_id=?2 AND camera=1 AND seq=2",
                params![damaged, id],
            )
            .unwrap(),
        1
    );
    assert!(store
        .comparison_input(id, HistoryCompareInput::RawImages)
        .is_err());
    assert!(store.history_comparison_record(id).is_err());
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
    let legacy = Connection::open(db.path()).unwrap();
    legacy
        .execute_batch(
            "DROP TRIGGER IF EXISTS part_tags_insert;
             DROP TRIGGER IF EXISTS part_tags_update;
             DROP TABLE IF EXISTS part_tags;",
        )
        .unwrap();
    legacy
        .execute("UPDATE parts SET tags = '[' WHERE id = ?1", [ids[1]])
        .unwrap();
    drop(legacy);
    let store = Store::open(&db.path()).unwrap();
    let query = HistoryQuery {
        from: Some(1234),
        to: Some(1235),
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
    Connection::open(db.path())
        .unwrap()
        .execute("UPDATE parts SET ts = 1235 WHERE id = ?1", [ids[2]])
        .unwrap();
    assert_eq!(store.query(&query).unwrap().total, 0);
    let following = HistoryQuery {
        from: Some(1235),
        to: Some(1236),
        ..query
    };
    assert_eq!(store.query(&following).unwrap().items[0].id, ids[2]);
}

#[test]
fn latest_history_ids_keep_the_newest_ties_within_the_time_range() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let insert = |cycle, ts| {
        store
            .insert(&PartRecord {
                ts,
                ..burst_part(&recipe, cycle, &judgement, &burst)
            })
            .unwrap()
    };
    let first = insert("newer", 20);
    let tie = insert("same-time", 20);
    let older = insert("imported-older", 10);
    insert("before-range", 9);
    insert("at-upper-bound", 30);
    let query = HistoryQuery {
        from: Some(10),
        to: Some(30),
        offset: 4,
        limit: 1,
        ..Default::default()
    };
    assert_eq!(store.latest_history_ids(&query, 2).unwrap(), [tie, first]);
    assert_eq!(
        store.latest_history_ids(&query, 4).unwrap(),
        [tie, first, older]
    );
}

#[test]
fn history_export_keeps_one_snapshot_while_writer_commits_across_the_old_page_boundary() {
    use std::sync::mpsc::channel;
    use std::thread;

    const WAIT: Duration = Duration::from_secs(5);

    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let insert = |store: &Store, sn: &str, ts| {
        store.insert(&PartRecord {
            mode: PartMode::Production,
            ts,
            sn,
            recipe: None,
            judgement: &Judgement::error(95, "无配方"),
            drain_ms: None,
            frames: &[],
            frames_expected: 0,
            frames_received: 0,
            triggers: 0,
            table: None,
            software_version: "test",
            cycle_id: Some(sn),
            bundle_id: None,
            delivery: &PlcDelivery::default(),
            shots: &[],
            source: RecordSource::Real,
            burst: None,
        })
    };
    let ids: Vec<_> = (0..501)
        .map(|n| insert(&store, &format!("csv-{n}"), 1234).unwrap())
        .collect();
    let tag = "待复核".to_owned();
    for ids in ids.chunks(500) {
        store
            .set_history_tags(ids, std::slice::from_ref(&tag))
            .unwrap();
    }
    insert(&store, "not-tagged", 1235).unwrap();
    let query = HistoryQuery {
        tag: Some(tag.clone()),
        offset: 4,
        limit: 2,
        ..Default::default()
    };

    let (start_write, wait_for_first_row) = channel();
    let (committed, wait_for_commit) = channel();
    let mut exported = Vec::new();
    let (visited, written) = thread::scope(|threads| {
        let store = &store;
        let tag = &tag;
        let first = ids[0];
        let writer = threads.spawn(move || -> Result<i64, String> {
            wait_for_first_row
                .recv_timeout(WAIT)
                .map_err(|e| e.to_string())?;
            let new = insert(store, "arrived-during-export", 1236)?;
            store.set_history_tags(&[new], std::slice::from_ref(tag))?;
            store.set_history_tags(&[first], &["已复核".into()])?;
            let _ = committed.send(());
            Ok(new)
        });
        let read = store.visit_history(&query, |part| {
            if exported.is_empty() {
                start_write.send(()).map_err(|e| e.to_string())?;
                wait_for_commit
                    .recv_timeout(WAIT)
                    .map_err(|e| e.to_string())?;
            }
            exported.push((part.id, part.tags));
            Ok(())
        });
        drop(start_write);
        (read, writer.join().unwrap())
    });
    visited.expect("导出游标打开期间应能提交写入");
    let new = written.unwrap();
    assert_eq!(
        exported.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ids.iter().rev().copied().collect::<Vec<_>>()
    );
    assert!(exported
        .iter()
        .all(|(_, tags)| tags.as_slice() == std::slice::from_ref(&tag)));

    let mut latest = Vec::new();
    store
        .visit_history(&query, |part| {
            latest.push(part.id);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        latest,
        std::iter::once(new)
            .chain(ids[1..].iter().rev().copied())
            .collect::<Vec<_>>()
    );
}

#[test]
fn history_csv_publishes_complete_files_and_removes_output_on_read_failure() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let older = store
        .insert(&burst_part(&recipe, "csv-older", &judgement, &burst))
        .unwrap();
    store
        .insert(&burst_part(&recipe, "csv-newer", &judgement, &burst))
        .unwrap();
    let query = HistoryQuery::default();
    let path = db.0.join("complete.csv");
    crate::history::export_csv(&store, &query, &path).unwrap();
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.starts_with("\u{feff}时间,来源,SN,"));
    assert_eq!(file.matches("\r\n").count(), 3);
    assert!(file.contains("\"csv-older\"") && file.contains("\"csv-newer\""));
    assert!(!path.with_extension("csv.tmp").exists());

    Connection::open(db.path())
        .unwrap()
        .execute("UPDATE parts SET tags = '[' WHERE id = ?1", [older])
        .unwrap();
    let path = db.0.join("failed.csv");
    assert!(crate::history::export_csv(&store, &query, &path).is_err());
    assert!(!path.exists() && !path.with_extension("csv.tmp").exists());
}

#[test]
fn verdict_filter_keeps_page_and_total_exact_with_legacy_verdicts_in_facets() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let ids: Vec<_> = ["err-first", "err-second", "ok", "legacy", "untagged"]
        .into_iter()
        .map(|cycle| {
            store
                .insert(&burst_part(&recipe, cycle, &judgement, &burst))
                .unwrap()
        })
        .collect();
    store
        .set_history_tags(&ids[..4], &["待复核".into()])
        .unwrap();
    drop(store);
    let conn = Connection::open(db.path()).unwrap();
    for (id, verdict) in ids.iter().zip([
        "ERR_INSPECT",
        "ERR_INSPECT",
        "OK",
        "LEGACY_ERR",
        "ERR_INSPECT",
    ]) {
        conn.execute(
            "UPDATE parts SET verdict=?1 WHERE id=?2",
            params![verdict, id],
        )
        .unwrap();
    }
    drop(conn);

    let store = Store::open(&db.path()).unwrap();
    let page = store
        .query(&HistoryQuery {
            verdicts: vec![Verdict::ErrInspect],
            include_verdict_facets: true,
            tag: Some("待复核".into()),
            offset: 1,
            limit: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(
        page.counts,
        VerdictCounts {
            err: 2,
            ..Default::default()
        }
    );
    assert_eq!(
        page.items.iter().map(|p| p.id).collect::<Vec<_>>(),
        [ids[0]]
    );
    assert_eq!(
        page.verdict_facets,
        Some(VerdictCounts {
            ok: 1,
            err: 3,
            ..Default::default()
        })
    );
}

#[test]
fn production_counts_keep_the_date_and_project_scope_when_latest_tags_are_invalid() {
    let db = TestDb::new();
    let store = Store::open_for(&db.path(), "line-a").unwrap();
    let other = Store::open_for(&db.path(), "line-b").unwrap();
    let midnight = 1_700_006_400_000;
    let insert = |store: &Store, cycle_id, ts, mode, verdict: Verdict| {
        let judgement = Judgement {
            verdict,
            plc_code: if mode == PartMode::Production {
                verdict.plc_code()
            } else {
                0
            },
            fault_code: 0,
            reason: String::new(),
            segments: Vec::new(),
            gaps: Vec::new(),
        };
        store
            .insert(&PartRecord {
                mode,
                ts,
                sn: cycle_id,
                recipe: None,
                judgement: &judgement,
                drain_ms: None,
                frames: &[],
                frames_expected: 0,
                frames_received: 0,
                triggers: 0,
                table: None,
                software_version: "test",
                cycle_id: Some(cycle_id),
                bundle_id: None,
                delivery: &PlcDelivery::default(),
                shots: &[],
                source: RecordSource::Real,
                burst: None,
            })
            .unwrap()
    };
    use PartMode::{MaintenanceCollect, MaintenanceShadow, Production};
    insert(
        &store,
        "before-midnight",
        midnight - 1,
        Production,
        Verdict::ErrInspect,
    );
    insert(
        &store,
        "at-midnight",
        midnight,
        Production,
        Verdict::OkWithExcursion,
    );
    let latest = insert(&store, "latest", midnight + 1, Production, Verdict::NgWidth);
    insert(
        &store,
        "trial",
        midnight + 2,
        MaintenanceShadow,
        Verdict::Ok,
    );
    insert(
        &store,
        "collection",
        midnight + 3,
        MaintenanceCollect,
        Verdict::ErrInspect,
    );
    insert(
        &other,
        "other-project",
        midnight + 4,
        Production,
        Verdict::ErrInspect,
    );
    Connection::open(db.path())
        .unwrap()
        .execute("UPDATE parts SET tags = '[' WHERE id = ?1", [latest])
        .unwrap();

    assert!(store.summary(latest).is_err());
    assert_eq!(
        store.counts_since(midnight, HistoryScope::Real).unwrap(),
        VerdictCounts {
            excursion: 1,
            ng: 1,
            ..Default::default()
        }
    );
}

#[test]
fn history_and_maintenance_reads_see_only_committed_project_records_during_an_import() {
    use std::sync::mpsc::channel;
    use std::thread;

    const WAIT: Duration = Duration::from_secs(5);

    let db = TestDb::new();
    let target = Store::open_for(&db.path(), "line-a").unwrap();
    let other = Store::open_for(&db.path(), "line-b").unwrap();
    let source = Store::open_for(&db.0.join("source.sqlite"), "line-a").unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let insert_trial = |store: &Store, recipe: &Recipe, cycle| {
        store
            .insert(&PartRecord {
                mode: PartMode::MaintenanceShadow,
                ..burst_part(recipe, cycle, &judgement, &burst)
            })
            .unwrap()
    };
    let committed = insert_trial(&target, &recipe, "committed-trial");
    let other_recipe = Recipe {
        name: "另一条线的原配方".into(),
        ..recipe.clone()
    };
    let other_id = insert_trial(&other, &other_recipe, "other-project-trial");
    assert_eq!(
        Connection::open(db.path())
            .unwrap()
            .execute(
                "UPDATE recipe_snapshots SET json = json_set(json, '$.planVersion', 14)
         WHERE project_id = ?1 AND revision_id = ?2",
                params![other.project(), other_recipe.revision_id],
            )
            .unwrap(),
        1
    );
    let incoming = insert_trial(&source, &recipe, "incoming-trial");
    let package = db.0.join("trial-package.sqlite");
    source.export_parts(&[incoming], &package).unwrap();
    let query = HistoryQuery {
        project: Some("line-a".into()),
        limit: 1,
        ..HistoryQuery::default()
    };
    let read_history = || -> Result<_, String> {
        Ok((
            target.maintenance_statistics(HistoryScope::Real)?,
            target.query(&query)?,
            target.neighbors(&query, committed)?,
            target.detail_overview(committed)?,
            target.detail_frame_with_recipe(other_id, 0, 1)?,
            target.detail_by_cycle("incoming-trial")?,
        ))
    };

    let (entered, wait_for_import) = channel();
    let (release, wait_for_release) = channel();
    let (read, imported) = thread::scope(|threads| {
        let store = &target;
        let import = threads.spawn(move || {
            store.import_parts(&package, "line-a", |_| {
                entered.send(()).map_err(|e| e.to_string())?;
                wait_for_release
                    .recv_timeout(Duration::from_secs(15))
                    .map_err(|e| e.to_string())
            })
        });
        let read = wait_for_import.recv_timeout(WAIT).and_then(|()| {
            let (result, wait_for_read) = channel();
            let read_history = &read_history;
            threads.spawn(move || {
                let _ = result.send(read_history());
            });
            wait_for_read.recv_timeout(WAIT)
        });
        let _ = release.send(());
        (read, import.join().unwrap())
    });
    let imported = imported.unwrap();
    assert_eq!(imported.inserted, 1);
    let imported_id = imported.ids[&incoming];
    let result_during_import = read.expect("历史读取在导入提交前未返回").unwrap();
    let result_after_import = read_history().unwrap();

    let expected_trial = |ok| TrialProgress {
        recipe_id: recipe.id.clone(),
        recipe_version: recipe.version,
        counts: VerdictCounts {
            ok,
            ..Default::default()
        },
        ng: Vec::new(),
    };
    let first_frame = burst
        .frames
        .iter()
        .find(|frame| (frame.camera, frame.seq) == (0, 1))
        .unwrap();
    let expected_snapshot = serde_json::to_value(&other_recipe).unwrap();
    for (
        (stats, page, neighbors, overview, (selected, selected_recipe), incoming),
        total,
        first,
        newer,
    ) in [
        (result_during_import, 1, committed, None),
        (result_after_import, 2, imported_id, Some(imported_id)),
    ] {
        let trial = expected_trial(total);
        assert_eq!(page.total, total);
        assert_eq!(page.counts, trial.counts);
        assert_eq!(stats.trial, [trial]);
        assert_eq!(
            page.items.iter().map(|part| part.id).collect::<Vec<_>>(),
            [first]
        );
        assert_eq!(neighbors, (None, newer));
        assert_eq!(overview.summary.id, committed);
        let overview = overview.burst.unwrap();
        assert_eq!(overview.frames.len(), burst.frames.len());
        assert!(overview.frame_verdicts.iter().any(|frame| (
            frame.camera,
            frame.seq,
            frame.verdict
        ) == (0, 1, Verdict::Ok)));
        assert_eq!(selected.summary.id, other_id);
        assert_eq!(selected.summary.project_id, other.project());
        assert_eq!(
            serde_json::to_value(selected_recipe).unwrap(),
            expected_snapshot
        );
        assert_eq!(
            selected.burst.unwrap().frames.as_slice(),
            std::slice::from_ref(first_frame)
        );
        assert_eq!(incoming.map(|part| part.summary.id), newer);
    }
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
    let tagged = HistoryQuery {
        tag: Some("故障件".into()),
        ..Default::default()
    };
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
    assert_eq!(store.query(&tagged).unwrap().total, 0);
    assert!(store
        .rejudge_reader()
        .unwrap()
        .burst_frames(id)
        .unwrap()
        .is_empty());
    assert!(store.insert(&record).is_err());
    assert!(store.history_cycle_deleted("delete-restore").unwrap());
    let merged = store.import_parts(&package, "default", |_| Ok(())).unwrap();
    let restored = merged.ids[&id];
    assert!(restored > id);
    assert_eq!(store.detail(restored).unwrap().summary.tags, ["故障件"]);
    let page = store.query(&tagged).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, restored);
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
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS part_tags_insert;
             DROP TRIGGER IF EXISTS part_tags_update;
             DROP TABLE IF EXISTS part_tags;
             ALTER TABLE parts DROP COLUMN tags;",
        )
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

fn burst_part<'a>(
    recipe: &'a Recipe,
    cycle: &'a str,
    judgement: &'a Judgement,
    burst: &'a BurstRecord,
) -> PartRecord<'a> {
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
fn rejudge_reader_reads_during_uncommitted_write_and_refreshes_after_commit() {
    use std::sync::mpsc::channel;
    use std::thread;

    const WAIT: Duration = Duration::from_secs(5);
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[(2, 4)]), None, Some(0));
    let record = burst_part(&recipe, "rejudge-live-writer", &judgement, &burst);
    let id = store.insert(&record).unwrap();

    let (initial_read, wait_for_read) = channel();
    let (write_finished, wait_for_write) = channel();
    let (initial, refreshed) = thread::scope(|threads| {
        let mut writer = store.conn.lock().unwrap();
        let write = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        write
            .execute("UPDATE parts SET sn = 'SN-COMMITTED' WHERE id = ?1", [id])
            .unwrap();
        let store = &store;
        let revision = recipe.revision_id.as_str();
        let read = threads.spawn(move || -> Result<_, String> {
            let reader = store.rejudge_reader()?;
            let query = HistoryQuery::default();
            let rows = reader.measurements(&query, &[id], 1)?;
            let snapshot = reader.recipe_snapshot(store.project(), revision)?;
            let frames = reader.burst_frames(id)?;
            initial_read
                .send((rows, snapshot, frames))
                .map_err(|error| error.to_string())?;
            wait_for_write
                .recv_timeout(WAIT)
                .map_err(|error| error.to_string())?;
            reader.measurements(&query, &[id], 1)
        });
        let initial = wait_for_read.recv_timeout(WAIT);
        let finished = if initial.is_ok() {
            write.commit()
        } else {
            write.rollback()
        };
        drop(writer);
        let _ = write_finished.send(());
        let refreshed = read.join().unwrap();
        finished.unwrap();
        (initial, refreshed)
    });

    let (rows, snapshot, frames) = initial.expect("重判读取不能等主连接释放写锁");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].id, rows[0].sn.as_str()), (id, record.sn));
    assert_eq!(snapshot.unwrap().revision_id, recipe.revision_id);
    let mut expected_frames = burst.frames.clone();
    expected_frames.sort_by_key(|frame| (frame.camera, frame.seq));
    assert_eq!(
        serde_json::to_value(frames).unwrap(),
        serde_json::to_value(expected_frames).unwrap()
    );

    let refreshed = refreshed.unwrap();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(
        (refreshed[0].id, refreshed[0].sn.as_str()),
        (id, "SN-COMMITTED")
    );
}

#[test]
fn large_history_frame_table_rejects_malformed_metrics_in_the_last_frame() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let mut frames = burst_fixture::frames(4, &[]);
    for frame in &mut frames {
        Arc::make_mut(frame.metrics.as_mut().unwrap()).widths = vec![Some(4.0); 25_000];
    }
    let (judgement, burst) = burst_fixture::record(&recipe, frames, None, Some(0));
    let id = store
        .insert(&burst_part(
            &recipe,
            "malformed-history-frame",
            &judgement,
            &burst,
        ))
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE part_frames SET metrics='{}' WHERE part_id=?1 AND camera=2 AND seq=4",
            [id],
        )
        .unwrap();

    assert!(store.rejudge_reader().unwrap().burst_frames(id).is_err());
}

#[test]
fn registering_raw_files_keeps_the_batch_atomic_and_allows_the_same_path_on_retry() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let cycle = "register-raw-files";
    let id = store
        .insert(&burst_part(&recipe, cycle, &judgement, &burst))
        .unwrap();
    let first = FrameKey { camera: 0, seq: 1 };
    let saved = FrameKey { camera: 0, seq: 2 };
    let missing = FrameKey { camera: 0, seq: 5 };
    let original = BTreeMap::from([(saved, "cam1/2.pgm".to_owned())]);
    assert!(store.update_frame_raw_files(cycle, &original).unwrap());

    let reader = store.rejudge_reader().unwrap();
    let linked = || {
        reader
            .burst_frames(id)
            .unwrap()
            .into_iter()
            .filter_map(|frame| {
                frame.raw_file.map(|file| {
                    (
                        FrameKey {
                            camera: frame.camera,
                            seq: frame.seq,
                        },
                        file,
                    )
                })
            })
            .collect::<BTreeMap<_, _>>()
    };
    for (later, conflict) in [(missing, false), (saved, true)] {
        let result = store.update_frame_raw_files(
            cycle,
            &BTreeMap::from([
                (first, "cam1/1.pgm".into()),
                (later, "cam1/different.pgm".into()),
            ]),
        );
        assert!(matches!(
            (conflict, result),
            (false, Ok(false)) | (true, Err(_))
        ));
        assert_eq!(linked(), original);
    }

    let mut retry = original;
    retry.insert(first, "cam1/1.pgm".into());
    assert!(store.update_frame_raw_files(cycle, &retry).unwrap());
    assert_eq!(linked(), retry);
}

#[test]
fn overview_keeps_verdicts_and_single_frame_loads_only_selected_metrics() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let mut frames = burst_fixture::frames(4, &[]);
    for frame in &mut frames {
        frame.metrics = Some(Arc::new(burst_fixture::measured(
            frame.camera,
            frame.seq,
            2.0,
        )));
    }
    let metrics = Arc::make_mut(frames[0].metrics.as_mut().unwrap());
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
    let detail = store.detail(id).unwrap();
    assert_eq!(
        serde_json::to_value(store.history_import_record(id).unwrap()).unwrap(),
        serde_json::to_value((&detail.summary, &detail.recording)).unwrap(),
    );
    let full = detail.burst.unwrap();
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
    assert!(store.history_import_record(id).is_err());
}

#[test]
fn inverted_history_region_returns_a_detail_error_and_preserves_import_read() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&recipe, burst_fixture::frames(4, &[]), None, Some(4));
    let cycle = "inverted-region";
    let id = store
        .insert(&burst_part(&recipe, cycle, &judgement, &burst))
        .unwrap();
    let files = BTreeMap::from([(FrameKey { camera: 0, seq: 1 }, "cam1/1.pgm".into())]);
    assert!(store.update_frame_raw_files(cycle, &files).unwrap());
    let original = store.detail(id).unwrap();
    let original_burst = original.burst.as_ref().unwrap();
    assert!(!original_burst.frame_verdicts.is_empty());
    let (stored, stored_recipe) = store
        .comparison_input(id, HistoryCompareInput::StoredMetrics)
        .unwrap();
    let stored = stored.burst.unwrap();
    assert_eq!(stored_recipe.unwrap().revision_id, recipe.revision_id);
    assert_eq!(stored.frames, original_burst.frames);
    assert!(stored.frame_verdicts.is_empty());
    let (raw, raw_recipe) = store
        .comparison_input(id, HistoryCompareInput::RawImages)
        .unwrap();
    let raw = raw.burst.unwrap();
    assert_eq!(raw_recipe.unwrap().revision_id, recipe.revision_id);
    let expected_raw_frames: Vec<_> = original_burst
        .frames
        .iter()
        .map(|frame| FrameRecord {
            metrics: None,
            ..frame.clone()
        })
        .collect();
    assert_eq!(raw.frames, expected_raw_frames);
    assert!(raw.frame_verdicts.is_empty());

    Connection::open(db.path())
        .unwrap()
        .execute(
            "UPDATE parts SET burst_judgement = json_set(burst_judgement,
               '$.regions[0].region.firstSeq', json_extract(burst_judgement, '$.regions[0].region.lastSeq'),
               '$.regions[0].region.lastSeq', json_extract(burst_judgement, '$.regions[0].region.firstSeq'))
             WHERE id = ?1",
            [id],
        )
        .unwrap();

    let error = store.detail_overview(id).unwrap_err();
    assert!(error.contains("帧组起止序号颠倒"), "{error}");
    for input in [
        HistoryCompareInput::StoredMetrics,
        HistoryCompareInput::RawImages,
    ] {
        let error = store.comparison_input(id, input).unwrap_err();
        assert!(error.contains("帧组起止序号颠倒"), "{error}");
    }
    let (summary, recording) = store.history_import_record(id).unwrap();
    assert_eq!(summary.burst, original.summary.burst);
    assert_eq!(
        serde_json::to_value(summary).unwrap(),
        serde_json::to_value(original.summary).unwrap(),
    );
    assert_eq!(recording, original.recording);
}

#[test]
fn named_frame_uses_the_record_project_camera_order_and_preserves_missing_rows() {
    let db = TestDb::new();
    let store = Store::open_for(&db.path(), "line-a").unwrap();
    let other = Store::open_for(&db.path(), "line-b").unwrap();
    let original = burst_fixture::recipe();
    let (judgement, burst) =
        burst_fixture::record(&original, burst_fixture::frames(4, &[]), None, Some(4));
    store
        .insert(&burst_part(&original, "normal-order", &judgement, &burst))
        .unwrap();

    let mut doc = crate::recipe::samples_burst();
    doc.inspect.reverse();
    let reordered = doc.build().unwrap();
    let mut frames = burst_fixture::frames(4, &[(2, 3)]);
    let target = frames
        .iter_mut()
        .find(|frame| (frame.camera, frame.seq) == (2, 1))
        .unwrap();
    target.raw_file = Some("rec/cam1_s000001.pgm".into());
    let (judgement, burst) = burst_fixture::record(&reordered, frames, None, Some(4));
    let id = other
        .insert(&burst_part(
            &reordered,
            "reverse-camera-order",
            &judgement,
            &burst,
        ))
        .unwrap();
    Connection::open(db.path())
        .unwrap()
        .execute(
            "UPDATE part_frames SET metrics='broken' WHERE part_id=?1 AND camera=0 AND seq=1",
            [id],
        )
        .unwrap();

    let (selected, camera) = store.detail_frame_by_camera(id, "cam1", 1).unwrap();
    assert_eq!(camera, 2);
    assert_eq!(selected.summary.project_id, "line-b");
    assert_eq!(
        selected.burst.unwrap().frames,
        store.detail_frame(id, 2, 1).unwrap().burst.unwrap().frames
    );

    let (missing, camera) = store.detail_frame_by_camera(id, "cam1", 3).unwrap();
    assert_eq!(camera, 2);
    assert_eq!(missing.burst.unwrap().frames[0].status, FrameState::Missing);
    assert!(store.detail_frame_by_camera(id, "cam1", 5).is_err());
    assert!(store.detail_frame_by_camera(id, "camX", 1).is_err());
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
        conn.execute_batch("DROP INDEX parts_maintenance; DROP TRIGGER production_source_insert; DROP TRIGGER production_source_update; PRAGMA ignore_check_constraints = ON; PRAGMA user_version = 10;").unwrap();
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

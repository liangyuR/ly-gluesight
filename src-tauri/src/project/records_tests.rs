use super::*;
use crate::recipe::Recipe;
use crate::store::{
    burst_fixture, PartMode, PartRecord, PlcDelivery, PlcDeliveryState, RecordingEvidence,
    RecordingState,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gluesight-records-{name}-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const PARTS: &str = "id, retest_of, ts, sn, recipe_id, recipe_version, recipe_revision, trigger_mode, verdict, plc_code, fault_code, \
    reason, drain_ms, frames_expected, frames_received, triggers, software_version, judgement, frames, cycle_id, bundle_id, \
    delivery_state, delivery_updated_at, delivery_message, recording_state, recording_available, recording_directory, \
    recording_errors, sample, source, expected_total, tolerance_total, device_triggers, camera_frames, flags, burst_judgement, mode";

/// 在项目 `project` 下记一件真机件：第一帧有原图引用，原图与录制目录写在该项目的录制根下。
fn part(data: &Path, store: &Store, recipe: &Recipe, cycle: &str, sn: &str, ts: i64) {
    let root = record_root(data, store.project(), RecordSource::Real).unwrap();
    let raw = format!("2026/{cycle}/cam1_s000001.pgm");
    std::fs::create_dir_all(root.join("2026").join(cycle)).unwrap();
    std::fs::write(root.join(&raw), b"P5").unwrap();
    let mut frames = burst_fixture::frames(4, &[]);
    frames[0].raw_file = Some(raw);
    let (judgement, burst) = burst_fixture::record(recipe, frames, None, Some(0));
    store
        .insert(&PartRecord {
            mode: PartMode::Production,
            ts,
            sn,
            recipe: Some(recipe),
            judgement: &judgement,
            drain_ms: None,
            frames: &[],
            frames_expected: burst.expected_total.unwrap_or(0) as usize,
            frames_received: burst.judgement.group_count as usize,
            triggers: 0,
            table: None,
            software_version: "test",
            cycle_id: Some(cycle),
            bundle_id: None,
            delivery: &PlcDelivery {
                state: PlcDeliveryState::NotRequired,
                updated_at: 0,
                message: None,
            },
            shots: &[],
            source: RecordSource::Real,
            sample: None,
            burst: Some(&burst),
        })
        .unwrap();
    store
        .update_recording(
            cycle,
            &RecordingEvidence {
                state: RecordingState::Complete,
                available: true,
                directory: Some(root.join("2026").join(cycle).to_string_lossy().into_owned()),
                errors: Vec::new(),
            },
        )
        .unwrap();
}

/// 项目目录里的 9 版旧记录库：先按现行接口记件，再按 9 版表结构（没有项目列，快照按修订号）转存。
fn legacy(data: &Path, project: &str, write: impl FnOnce(&Store)) {
    let path = crate::project::dir(data, project).join("inspection.db");
    let scratch = data.join(format!("{project}-scratch.db"));
    write(&Store::open_for(&scratch, project).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("fixtures/inspection-v9.sql"))
        .unwrap();
    conn.execute("ATTACH DATABASE ?1 AS v10", [scratch.to_string_lossy()])
        .unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO parts ({PARTS}) SELECT {PARTS} FROM v10.parts;
         INSERT INTO part_frames SELECT * FROM v10.part_frames;
         INSERT INTO recipe_snapshots (revision_id, recipe_id, version, json) SELECT revision_id, recipe_id, version, json FROM v10.recipe_snapshots;
         INSERT INTO workspace_instances (id, recipe_id) SELECT id, recipe_id FROM v10.workspace_instances;
         DETACH DATABASE v10;"
    ))
    .unwrap();
}

fn write_json(path: &Path, value: serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// 防的回归：项目旧库并入本机库时丢件、把复制项目带来的同一工件存两份、复测链断开、两个项目同修订号的配方快照混用、
/// 记录找不到所在项目的原图、重新编号后候选采集轮次与复测目录仍指着旧编号；以及再启动（含合并后改名前中断）时重复合并。
#[test]
fn two_project_databases_merge_once_with_renumbered_references_and_project_snapshots() {
    let data = TempDir::new("migrate");
    let data = data.0.as_path();
    std::fs::create_dir_all(data.join("config")).unwrap();
    std::fs::write(data.join("config/log-storage.json"), serde_json::json!({
        "schema": 1, "root": data.join("gs_logs"), "environment": "test", "instance": "6666666666666666",
        "ownerData": data.canonicalize().unwrap(),
    }).to_string()).unwrap();
    for id in ["line-a", "line-b"] {
        crate::project::create_dir(
            data,
            &crate::project::ProjectMeta::new(id, id, "", None),
            |_| Ok(()),
        )
        .unwrap();
    }
    let recipe_a = burst_fixture::recipe();
    let recipe_b = Recipe {
        plan_version: recipe_a.plan_version + 7,
        ..recipe_a.clone()
    };
    legacy(data, "line-a", |store| {
        part(data, store, &recipe_a, "a1", "SN-A1", 100);
        part(data, store, &recipe_a, "a2", "SN-A2", 200);
    });
    legacy(data, "line-b", |store| {
        part(data, store, &recipe_b, "a1", "SN-A1", 100);
        part(data, store, &recipe_b, "b1", "SN-B", 300);
        part(data, store, &recipe_b, "b2", "SN-B", 400);
    });
    let b = crate::project::dir(data, "line-b");
    write_json(
        &b.join("recipe-capture/r1/round.json"),
        serde_json::json!({"origin": {"kind": "history", "cycleId": "b2", "recordId": 3}}),
    );
    write_json(
        &b.join("workspaces/comparisons/2/2-100-0.json"),
        serde_json::json!({"id": "2-100-0", "historyId": 2, "cycleId": "b1"}),
    );
    write_json(
        &b.join("workspaces/comparisons/9/9-100-0.json"),
        serde_json::json!({"id": "9-100-0", "historyId": 9, "cycleId": "gone"}),
    );

    let store = Store::open_for(&database(data), "line-a").unwrap();
    let notes = migrate(data, &store, "line-a");
    assert_eq!(
        notes.iter().filter(|(level, _)| *level == "info").count(),
        2,
        "{notes:?}"
    );
    let counts = |store: &Store| {
        store
            .project_records()
            .unwrap()
            .into_iter()
            .map(|(id, (n, ..))| (id, n))
            .collect::<Vec<_>>()
    };
    assert_eq!(counts(&store), [("line-a".into(), 2), ("line-b".into(), 2)]);

    let a1 = store.detail(1).unwrap();
    assert_eq!(
        (
            a1.summary.project_id.as_str(),
            a1.summary.cycle_id.as_deref()
        ),
        ("line-a", Some("a1"))
    );
    let b2 = store.detail_by_cycle("b2").unwrap().unwrap();
    let b1 = store.detail_by_cycle("b1").unwrap().unwrap();
    assert_eq!(
        (b1.summary.id, b2.summary.id, b2.summary.retest_of),
        (3, 4, Some(3))
    );
    assert_eq!(
        store
            .record_recipe(&a1.summary)
            .unwrap()
            .unwrap()
            .plan_version,
        recipe_a.plan_version
    );
    assert_eq!(
        store
            .record_recipe(&b2.summary)
            .unwrap()
            .unwrap()
            .plan_version,
        recipe_b.plan_version
    );
    let raw = b2.burst.as_ref().unwrap().frames[0]
        .raw_file
        .clone()
        .unwrap();
    assert!(record_root(data, &b2.summary.project_id, b2.summary.source)
        .unwrap()
        .join(raw)
        .is_file());

    assert_eq!(
        read_json(&b.join("recipe-capture/r1/round.json"))["origin"]["recordId"],
        4
    );
    let comparison = read_json(&b.join("workspaces/comparisons/3/3-100-0.json"));
    assert_eq!(
        (comparison["historyId"].as_i64(), comparison["id"].as_str()),
        (Some(3), Some("3-100-0"))
    );
    assert!(!b.join("workspaces/comparisons/2").exists());
    assert!(!b.join("workspaces/comparisons/9").exists());
    assert!(!crate::project::dir(data, "line-a")
        .join("inspection.db")
        .exists());
    let retired: Vec<PathBuf> = std::fs::read_dir(&b)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("inspection.db.merged-")
        })
        .collect();
    assert_eq!(retired.len(), 1);

    drop(store);
    let store = Store::open_for(&database(data), "line-a").unwrap();
    assert!(migrate(data, &store, "line-a").is_empty());
    std::fs::copy(&retired[0], b.join("inspection.db")).unwrap();
    assert!(migrate(data, &store, "line-a").is_empty());
    assert!(!b.join("inspection.db").exists());
    assert_eq!(counts(&store), [("line-a".into(), 2), ("line-b".into(), 2)]);
    assert_eq!(
        read_json(&b.join("recipe-capture/r1/round.json"))["origin"]["recordId"],
        4
    );
}

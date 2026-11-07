use super::*;
use crate::recipe::{Recipe, RecipeStore};
use crate::store::{
    burst_fixture, PartMode, PartRecord, PlcDelivery, PlcDeliveryState, RecordSource,
    RecordingEvidence, RecordingState,
};
use std::sync::Arc;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gluesight-project-transfer-{name}-{}-{}",
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

/// 本机记录库，当前项目为 `project`。
fn local_store(data: &Path, project: &str) -> Store {
    Store::open_for(&super::super::records::database(data), project).unwrap()
}

/// 在 `store` 的当前项目下记一件带录制目录 `directory` 的检测记录，返回记录编号。
fn record(store: &Store, recipe: &Recipe, cycle: &str, directory: &Path) -> i64 {
    let (judgement, burst) =
        burst_fixture::record(recipe, burst_fixture::frames(4, &[]), None, Some(0));
    let id = store
        .insert(&PartRecord {
            mode: PartMode::Production,
            ts: 1234,
            sn: "SN-T1",
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
                state: RecordingState::Incomplete,
                available: false,
                directory: Some(directory.to_string_lossy().into_owned()),
                errors: vec!["测试".into()],
            },
        )
        .unwrap();
    id
}

/// 来源机上的项目：一份已保存的生产配方（带修订来源）、一轮记了绝对原图路径与来源记录的采集，本机记录库里一件带原图目录与
/// 配方快照的检测记录。
fn source_project(data: &Path) -> (PathBuf, Arc<Recipe>, Store) {
    let root = super::super::create_dir(
        data,
        &ProjectMeta::new("line-a", "A 线", "三相机", None),
        |_| Ok(()),
    )
    .unwrap();
    let store = RecipeStore::open_with_floors(
        root.join("recipes"),
        data.join("state").join("plan-version"),
        &BTreeMap::new(),
    )
    .unwrap();
    let recipe = store.save(crate::recipe::samples_burst(), None).unwrap();
    let round = root.join("recipe-capture").join("capture-1");
    std::fs::create_dir_all(&round).unwrap();
    std::fs::write(round.join("a.pgm"), b"P5").unwrap();
    let directory = root.join("records").join("c1");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("part.json"), "{}").unwrap();
    let store = local_store(data, "line-a");
    let id = record(&store, &recipe, "c1", &directory);
    let original = round.join("a.pgm").to_string_lossy().into_owned();
    std::fs::write(
        round.join("round.json"),
        serde_json::json!({
            "origin": {"kind": "history", "cycleId": "c1", "recordId": id},
            "frames": [{"cameras": [{"originalPath": original}]}],
        })
        .to_string(),
    )
    .unwrap();
    (root, recipe, store)
}

fn export(data: &Path, root: &Path, store: &Store) -> PathBuf {
    let package = data.join("exports").join("line-a.zip");
    std::fs::create_dir_all(package.parent().unwrap()).unwrap();
    export_to(
        root,
        Some(store),
        true,
        &data.join("cache").join("export"),
        &package,
    )
    .unwrap();
    package
}

#[test]
fn damaged_or_tampered_package_is_refused_without_leaving_a_project_behind() {
    let (from, to) = (TempDir::new("bad-from"), TempDir::new("bad-to"));
    let (source, _, store) = source_project(&from.0);
    let package = export(&from.0, &source, &store);
    let manifest = inspect(&package).unwrap();
    let local = local_store(&to.0, "default");
    let rewrite = |name: &str, change: &dyn Fn(&mut Manifest)| -> PathBuf {
        let mut zip = open_archive(&package).unwrap();
        let path = from.0.join(name);
        let mut out = zip::ZipWriter::new(File::create(&path).unwrap());
        let mut altered = manifest.clone();
        change(&mut altered);
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).unwrap();
            let name = entry.name().unwrap().into_owned();
            if name == MANIFEST {
                continue;
            }
            out.start_file(name, SimpleFileOptions::default()).unwrap();
            std::io::copy(&mut entry, &mut out).unwrap();
        }
        for extra in altered
            .files
            .iter()
            .filter(|f| !manifest.files.iter().any(|m| m.path == f.path))
        {
            out.start_file(extra.path.as_str(), SimpleFileOptions::default())
                .unwrap();
        }
        out.start_file(MANIFEST, SimpleFileOptions::default())
            .unwrap();
        out.write_all(&serde_json::to_vec(&altered).unwrap())
            .unwrap();
        out.finish().unwrap();
        path
    };
    let cases = [
        rewrite("checksum.zip", &|m| m.files[0].sha256 = "0".repeat(64)),
        rewrite("outside.zip", &|m| {
            m.files.push(PackageFile {
                path: "project/../escape.txt".into(),
                bytes: 0,
                sha256: "0".repeat(64),
            })
        }),
    ];
    for package in cases {
        assert!(import_from(
            &to.0,
            &package,
            "line-b",
            "B 线",
            &to.0.join("state").join("plan-version"),
            &to.0.join("cache"),
            &local,
        )
        .is_err());
        assert!(!super::super::dir(&to.0, "line-b").exists());
        assert!(local.project_records().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(to.0.join("cache")).unwrap().count(),
            0,
            "暂存目录应已清理"
        );
    }
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = Vec::new();
    walk(root, "", &mut files, &mut Vec::new()).unwrap();
    files
        .into_iter()
        .map(|(_, path)| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

fn copy(data: &Path, capture_images: bool) -> Result<ProjectImported, String> {
    copy_from(
        data,
        "line-a",
        capture_images,
        &New {
            id: "line-a2",
            name: "A 线副本",
            description: "",
        },
        &data.join("state").join("plan-version"),
        &data.join("cache"),
    )
}

/// 防的回归：复制项目改写不到自己的目录、复用来源的计划版本、改动来源项目，或把本机共用的检测记录再复制一份
/// （同一工件出现两次）。
#[test]
fn copied_project_gets_its_own_paths_and_plan_versions_and_leaves_the_source_untouched() {
    let data = TempDir::new("copy");
    let (source, recipe, store) = source_project(&data.0);
    let before = snapshot_files(&source);
    let copied = copy(&data.0, true).unwrap();
    assert_eq!(copied.meta.copied_from.as_deref(), Some("line-a"));
    let target = super::super::dir(&data.0, "line-a2");
    let round = read_value(
        &target
            .join("recipe-capture")
            .join("capture-1")
            .join("round.json"),
    )
    .unwrap();
    let original = round["frames"][0]["cameras"][0]["originalPath"]
        .as_str()
        .unwrap();
    assert!(Path::new(original).starts_with(&target), "{original}");
    assert!(!target.join("inspection.db").exists());
    assert_eq!(
        store
            .project_records()
            .unwrap()
            .into_iter()
            .map(|(id, (n, ..))| (id, n))
            .collect::<Vec<_>>(),
        [("line-a".to_string(), 1)]
    );
    let plan_file = data.0.join("state").join("plan-version");
    let loaded = RecipeStore::open_with_floors(target.join("recipes"), plan_file, &BTreeMap::new())
        .unwrap()
        .get(&recipe.id)
        .unwrap();
    assert!(loaded.plan_version > recipe.plan_version);
    assert_eq!(loaded.revision_id, recipe.revision_id);
    assert_eq!(snapshot_files(&source), before, "来源项目不应被改动");
    assert_eq!(std::fs::read_dir(data.0.join("cache")).unwrap().count(), 0);
}

#[test]
fn transferred_navigation_assets_keep_release_identity_and_damaged_maps_do_not_block_production() {
    for importing in [false, true] {
        let data = TempDir::new(if importing {
            "navigation-import"
        } else {
            "navigation-copy"
        });
        let (source, recipe, store) = source_project(&data.0);
        let candidate = PathBuf::from("workspaces")
            .join(&recipe.id)
            .join("navigation");
        let frozen = PathBuf::from("recipe-support")
            .join(&recipe.id)
            .join("release-navigation");
        let snapshot = serde_json::json!({
            "format": 1,
            "recipeId": recipe.id,
            "recipeRevision": recipe.revision_id,
            "bundleId": "release-navigation",
            "map": null
        });
        let assets = [
            (candidate.join("map.json"), b"{\"draft\":true}".to_vec()),
            (candidate.join("image.png"), b"candidate-image".to_vec()),
            (
                frozen.join("snapshot.json"),
                serde_json::to_vec(&snapshot).unwrap(),
            ),
            (frozen.join("image.png"), b"damaged-image".to_vec()),
        ];
        for (relative, bytes) in &assets {
            let path = source.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let before = snapshot_files(&source);
        let plan_file = data.0.join("state").join("plan-version");
        let (transferred, target) = if importing {
            let package = data.0.join("navigation.zip");
            export_to(&source, None, false, &data.0.join("cache/export"), &package).unwrap();
            (
                import_from(
                    &data.0,
                    &package,
                    "line-b",
                    "B 线",
                    &plan_file,
                    &data.0.join("cache"),
                    &store,
                )
                .unwrap(),
                super::super::dir(&data.0, "line-b"),
            )
        } else {
            (
                copy(&data.0, false).unwrap(),
                super::super::dir(&data.0, "line-a2"),
            )
        };
        for (relative, bytes) in &assets {
            assert_eq!(std::fs::read(target.join(relative)).unwrap(), *bytes);
        }
        assert_eq!(snapshot_files(&source), before);
        let loaded =
            RecipeStore::open_with_floors(target.join("recipes"), plan_file, &BTreeMap::new())
                .unwrap()
                .get(&recipe.id)
                .unwrap();
        assert_eq!(loaded.revision_id, recipe.revision_id);
        assert!(loaded.plan_version > recipe.plan_version);
        assert!(transferred
            .notes
            .iter()
            .any(|note| note.contains("导航") || note.contains("辅助")));
        assert!(!target.join("inspection.db").exists());
    }
}

/// 示教件原图可选：不带时包里与副本里只留轮次记录、不留 PGM，记了原图路径的轮次也不因此判成引用丢失；带时原样带上。
#[test]
fn capture_images_option_decides_whether_teaching_frames_travel_with_the_project() {
    for capture_images in [false, true] {
        let data = TempDir::new(if capture_images {
            "capture-on"
        } else {
            "capture-off"
        });
        let (source, _, store) = source_project(&data.0);
        let package = data.0.join("capture.zip");
        export_to(
            &source,
            None,
            capture_images,
            &data.0.join("cache/export"),
            &package,
        )
        .unwrap();
        let packaged = inspect(&package)
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == "project/recipe-capture/capture-1/a.pgm");
        assert_eq!(packaged, capture_images);
        let plan_file = data.0.join("state/plan-version");
        import_from(
            &data.0,
            &package,
            "line-b",
            "B 线",
            &plan_file,
            &data.0.join("cache"),
            &store,
        )
        .unwrap();
        copy_from(
            &data.0,
            "line-a",
            capture_images,
            &New {
                id: "line-a2",
                name: "A 线副本",
                description: "",
            },
            &plan_file,
            &data.0.join("cache"),
        )
        .unwrap();
        for id in ["line-b", "line-a2"] {
            let round = super::super::dir(&data.0, id).join("recipe-capture/capture-1");
            assert!(round.join("round.json").is_file(), "{id}");
            assert_eq!(round.join("a.pgm").is_file(), capture_images, "{id}");
        }
        assert!(source.join("recipe-capture/capture-1/a.pgm").is_file());
    }
}

fn external_layout(data: &Path, instance: &str) {
    std::fs::create_dir_all(data.join("config")).unwrap();
    std::fs::write(
        data.join("config/log-storage.json"),
        serde_json::json!({
            "schema": 1, "root": data.join("gs_logs"), "environment": "test", "instance": instance,
            "ownerData": data.canonicalize().unwrap(),
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn external_recording_conflict_refuses_import_without_overwriting_existing_files() {
    let (from, to) = (TempDir::new("conflict-from"), TempDir::new("conflict-to"));
    let (source, _, store) = source_project(&from.0);
    let package = export(&from.0, &source, &store);
    let local = local_store(&to.0, "default");
    external_layout(&to.0, "4444444444444444");
    let conflict = super::super::dir(&to.0, "blocked");
    let conflict_recordings = crate::log_storage::recording_project(&conflict).unwrap();
    std::fs::create_dir_all(&conflict_recordings).unwrap();
    std::fs::write(conflict_recordings.join("preserve"), "existing").unwrap();
    assert!(import_from(
        &to.0,
        &package,
        "blocked",
        "冲突",
        &to.0.join("state/plan-version"),
        &to.0.join("cache"),
        &local,
    )
    .is_err());
    assert!(local.project_records().unwrap().is_empty());
    assert!(!conflict.exists());
    assert_eq!(
        std::fs::read_to_string(conflict_recordings.join("preserve")).unwrap(),
        "existing"
    );
}

#[test]
fn interrupted_recording_install_is_removed_before_commit_and_preserved_after_project_commit() {
    let data = TempDir::new("interrupted-install");
    external_layout(&data.0, "5555555555555555");
    let pending_project = super::super::dir(&data.0, "pending");
    let pending = crate::log_storage::recording_project(&pending_project).unwrap();
    std::fs::create_dir_all(pending.join("records")).unwrap();
    std::fs::write(pending.join("records/frame.pgm"), b"pending").unwrap();
    std::fs::write(
        pending.join(PENDING_RECORDINGS),
        r#"{"projectId":"pending"}"#,
    )
    .unwrap();
    let committed_project = super::super::create_dir(
        &data.0,
        &ProjectMeta::new("committed", "已提交", "", None),
        |_| Ok(()),
    )
    .unwrap();
    let committed = crate::log_storage::recording_project(&committed_project).unwrap();
    std::fs::create_dir_all(committed.join("records")).unwrap();
    std::fs::write(committed.join("records/frame.pgm"), b"committed").unwrap();
    std::fs::write(
        committed.join(PENDING_RECORDINGS),
        r#"{"projectId":"committed"}"#,
    )
    .unwrap();
    let unfinished_stage = pending
        .parent()
        .unwrap()
        .join("project-recordings-pending-123-0");
    std::fs::create_dir_all(&unfinished_stage).unwrap();
    std::fs::write(
        unfinished_stage.join(PENDING_RECORDINGS),
        r#"{"projectId":"pending"}"#,
    )
    .unwrap();
    manage::purge_trash(&data.0).unwrap();
    assert!(!pending.exists());
    assert!(!unfinished_stage.exists());
    assert!(manage::check_new_id(&data.0, "pending").is_ok());
    assert_eq!(
        std::fs::read(committed.join("records/frame.pgm")).unwrap(),
        b"committed"
    );
    assert!(committed_project.join("project.json").is_file());
    assert!(!committed.join(PENDING_RECORDINGS).exists());
}

/// 防的回归：导出“含检测记录”把本机别的项目的记录也带走；导入时记录编号与本机已有的冲突却不重新编号、采集轮次里的来源
/// 记录编号没跟着改，或本机已有的同一工件（同一 cycleId）被再存一份。
#[test]
fn exported_records_are_only_this_projects_and_import_renumbers_them_into_the_local_history() {
    let (from, to) = (TempDir::new("records-from"), TempDir::new("records-to"));
    let (source, recipe, store) = source_project(&from.0);
    let other = local_store(&from.0, "line-x");
    record(&other, &recipe, "x1", &from.0.join("x1"));
    let package = export(&from.0, &source, &store);
    let packaged = from.0.join("packaged.db");
    std::io::copy(
        &mut open_archive(&package)
            .unwrap()
            .by_name("project/inspection.db")
            .unwrap(),
        &mut File::create(&packaged).unwrap(),
    )
    .unwrap();
    let projects: Vec<(String, String)> = rusqlite::Connection::open(&packaged)
        .unwrap()
        .prepare("SELECT project_id, cycle_id FROM parts")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(projects, [("line-a".to_string(), "c1".to_string())]);

    let local = local_store(&to.0, "default");
    let existing = record(&local, &recipe, "d1", &to.0.join("d1"));
    let plan_file = to.0.join("state/plan-version");
    let origin = |id: &str| {
        read_value(&super::super::dir(&to.0, id).join("recipe-capture/capture-1/round.json"))
            .unwrap()["origin"]["recordId"]
            .as_i64()
            .unwrap()
    };
    import_from(
        &to.0,
        &package,
        "line-b",
        "B 线",
        &plan_file,
        &to.0.join("cache"),
        &local,
    )
    .unwrap();
    let page = local
        .query(&crate::store::HistoryQuery {
            project: Some("line-b".into()),
            ..Default::default()
        })
        .unwrap();
    let imported = &page.items[0];
    assert_eq!((page.total, imported.cycle_id.as_deref()), (1, Some("c1")));
    assert_ne!(imported.id, existing);
    assert_eq!(origin("line-b"), imported.id);
    let directory = local
        .detail(imported.id)
        .unwrap()
        .recording
        .directory
        .unwrap();
    assert!(
        Path::new(&directory).starts_with(super::super::dir(&to.0, "line-b")),
        "{directory}"
    );
    assert!(local.record_recipe(imported).unwrap().is_some());

    let again = import_from(
        &to.0,
        &package,
        "line-c",
        "C 线",
        &plan_file,
        &to.0.join("cache"),
        &local,
    )
    .unwrap();
    assert!(
        again.notes.iter().any(|note| note.contains("同一工件")),
        "{:?}",
        again.notes
    );
    assert!(!local.project_records().unwrap().contains_key("line-c"));
    assert_eq!(origin("line-c"), imported.id);
}

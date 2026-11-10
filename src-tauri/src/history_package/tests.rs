use super::*;
use crate::operation::Operations;
use crate::recipe::{Recipe, RecipeStore};
use crate::store::{
    burst_fixture, PartMode, PartRecord, PlcDelivery, PlcDeliveryState, RecordingEvidence,
};
use std::io::Write;
use std::sync::Arc;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gluesight-history-package-{name}-{}-{}",
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

fn operation() -> Operation {
    Operation::with_events(Operations::default(), None, |_| {}).unwrap()
}

/// 一台机器：本机记录库（当前项目 `project`）与该项目的真机录制根。
struct Machine {
    store: Store,
    root: PathBuf,
    project: &'static str,
}

fn machine(dir: &Path, project: &'static str) -> Machine {
    let root = dir.join("records");
    std::fs::create_dir_all(&root).unwrap();
    Machine {
        store: Store::open_for(&dir.join("inspection.db"), project).unwrap(),
        root,
        project,
    }
}

fn saved_recipe(dir: &Path) -> Arc<Recipe> {
    RecipeStore::open_with_floors(dir.join("recipes"), &BTreeMap::new())
    .unwrap()
    .save(crate::recipe::samples_burst(), None)
    .unwrap()
}

/// 在当前项目下记一件录制完整的段配方记录：原图在 `<录制根>/20261107/<目录>/`，每帧内容不同。返回记录编号。
fn recorded(m: &Machine, recipe: &Recipe, cycle: &str, sn: &str) -> i64 {
    let directory = format!("20261107_120000_001_OK_cycle_{cycle}");
    let path = m.root.join("20261107").join(&directory);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("part.json"),
        serde_json::json!({"cycleId": cycle}).to_string(),
    )
    .unwrap();
    let cameras = recipe.plan_cameras();
    let mut frames = burst_fixture::frames(4, &[]);
    for frame in &mut frames {
        let file = format!("{}_s{:06}.pgm", cameras[frame.camera], frame.seq);
        std::fs::write(
            path.join(&file),
            format!("P5 {cycle} {} {}", frame.camera, frame.seq),
        )
        .unwrap();
        frame.raw_file = Some(format!("20261107/{directory}/{file}"));
    }
    let (judgement, burst) = burst_fixture::record(recipe, frames, None, Some(0));
    m.store
        .insert(&PartRecord {
            mode: PartMode::Production,
            ts: ly_plc::now_ms(),
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
            burst: Some(&burst),
        })
        .unwrap();
    m.store
        .update_recording(
            cycle,
            &RecordingEvidence {
                state: RecordingState::Complete,
                available: true,
                directory: Some(path.to_string_lossy().into_owned()),
                errors: Vec::new(),
            },
        )
        .unwrap();
    m.store.detail_by_cycle(cycle).unwrap().unwrap().summary.id
}

fn export(m: &Machine, ids: &[i64], scratch: &Path, target: &Path) -> PackageManifest {
    export_package(
        &m.store,
        ids,
        true,
        &ExportSource {
            project: m.project,
            project_name: "A 线",
            roots: std::slice::from_ref(&m.root),
        },
        scratch,
        target,
        &operation(),
    )
    .unwrap()
}

fn count(m: &Machine) -> u64 {
    m.store
        .project_records()
        .unwrap()
        .get(m.project)
        .map_or(0, |r| r.0)
}

fn imported_entries(m: &Machine) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(m.root.join(IMPORTED))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// 把包 `from` 改写为 `to`：每个条目（含清单）经 `edit` 处理。
fn rewrite(from: &Path, to: &Path, edit: impl Fn(&str, Vec<u8>) -> Vec<u8>) {
    let mut zip = ZipArchive::new(std::fs::File::open(from).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(std::fs::File::create(to).unwrap());
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        out.start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&edit(&name, bytes)).unwrap();
    }
    out.finish().unwrap();
}

#[test]
fn exported_parts_import_on_another_machine_with_same_measurements_and_images_under_local_root() {
    let temp = TempDir::new("roundtrip");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let ids = [
        recorded(&a, &recipe, "c-one", "SN-1"),
        recorded(&a, &recipe, "c-two", "SN-2"),
    ];
    a.store
        .set_history_tags(&[ids[0]], &["故障件".into(), "参考件".into()])
        .unwrap();
    let package = temp.0.join("parts.zip");
    export(&a, &ids, &temp.0.join("scratch"), &package);

    let db = temp.0.join("packaged.db");
    let mut zip = transfer::open_zip(&package, LABEL).unwrap();
    std::io::copy(
        &mut zip.by_name(DATABASE).unwrap(),
        &mut std::fs::File::create(&db).unwrap(),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let paths: Vec<String> = conn
        .prepare("SELECT recording_directory FROM parts UNION ALL SELECT raw_file FROM part_frames")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        paths
            .iter()
            .all(|p| !p.contains(':') && !p.contains('\\') && !Path::new(p).is_absolute()),
        "{paths:?}"
    );
    drop(conn);

    let b = machine(&temp.0.join("b"), "line-b");
    let result = import_package(&b.store, b.project, &b.root, &package, &operation()).unwrap();
    assert_eq!(
        (result.imported, result.skipped.len(), result.ids.len()),
        (2, 0, 2)
    );
    for (&id, &new_id) in ids.iter().zip(&result.ids) {
        let original = a.store.detail(id).unwrap();
        let copy = b.store.detail(new_id).unwrap();
        assert_eq!(copy.summary.tags, original.summary.tags);
        assert_eq!(copy.summary.project_id, "line-b");
        assert_eq!(
            (
                &copy.summary.cycle_id,
                &copy.summary.sn,
                copy.summary.ts,
                copy.summary.verdict
            ),
            (
                &original.summary.cycle_id,
                &original.summary.sn,
                original.summary.ts,
                original.summary.verdict
            )
        );
        assert!(copy.recording.available);
        assert!(imported_recording(Path::new(
            copy.recording.directory.as_deref().unwrap()
        )));
        let (from, to) = (original.burst.unwrap().frames, copy.burst.unwrap().frames);
        assert_eq!(from.len(), to.len());
        for (f, t) in from.iter().zip(&to) {
            assert_eq!(
                (&f.metrics, f.status, f.region),
                (&t.metrics, t.status, t.region)
            );
            let file = t.raw_file.as_deref().unwrap();
            assert!(file.starts_with("imported/"), "{file}");
            assert_eq!(
                std::fs::read(b.root.join(file)).unwrap(),
                std::fs::read(a.root.join(f.raw_file.as_deref().unwrap())).unwrap()
            );
        }
        let revision = copy.summary.recipe_revision.as_deref().unwrap();
        assert!(b
            .store
            .recipe_snapshot("line-b", revision)
            .unwrap()
            .is_some());
    }
    let entries = imported_entries(&b);
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(entries
        .iter()
        .all(|e| !b.root.join(IMPORTED).join(e).join(PENDING).exists()));
}

#[test]
fn idle_compressed_part_exports_original_pgm_bytes_and_imports_as_plain_pgm() {
    // 防的回归（IS-2）：原图空闲压缩过的件导出时把 .gsr 带进包里（旧软件导入后读不了图），或包里的 .pgm 不是录制时的原字节；
    // 压缩只该找录制完整、原图可用的入库件，原图被清理（引用已释放）的件不再是候选
    let temp = TempDir::new("compressed");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let id = recorded(&a, &recipe, "c-zip", "SN-9");
    let directory = "20261107_120000_001_OK_cycle_c-zip";
    let recording = a.root.join("20261107").join(directory);
    let mut originals = BTreeMap::new();
    for (index, entry) in std::fs::read_dir(&recording).unwrap().enumerate() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "pgm") {
            let mut bytes = b"P5\n# bayer RG\n8 6\n255\n".to_vec();
            bytes.extend((0..48u8).map(|i| i.wrapping_mul(3) ^ index as u8));
            std::fs::write(&path, &bytes).unwrap();
            originals.insert(
                path.file_name().unwrap().to_string_lossy().into_owned(),
                bytes,
            );
        }
    }
    assert_eq!(
        a.store.compressible_recordings(0, 8).unwrap(),
        [(id, "c-zip".to_string(), recording.clone())]
    );
    assert_eq!(
        crate::raw_compress::compress_all(&recording),
        originals.len()
    );

    let package = temp.0.join("parts.zip");
    let manifest = export(&a, &[id], &temp.0.join("scratch"), &package);
    assert!(
        manifest.files.iter().all(|f| !f.path.ends_with(".gsr")),
        "{:?}",
        manifest.files
    );
    let mut zip = transfer::open_zip(&package, LABEL).unwrap();
    for (name, original) in &originals {
        let mut bytes = Vec::new();
        zip.by_name(&format!("{PARTS}/{directory}/{name}"))
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(&bytes, original, "{name}");
    }

    let b = machine(&temp.0.join("b"), "line-b");
    let result = import_package(&b.store, b.project, &b.root, &package, &operation()).unwrap();
    assert_eq!(result.imported, 1);
    for (name, original) in &originals {
        assert_eq!(
            &std::fs::read(b.root.join(IMPORTED).join(directory).join(name)).unwrap(),
            original,
            "{name}"
        );
    }
    a.store
        .release_recordings(std::slice::from_ref(&recording))
        .unwrap();
    assert!(a.store.compressible_recordings(0, 8).unwrap().is_empty());
}

#[test]
fn importing_known_parts_skips_them_and_adds_only_new_ones() {
    let temp = TempDir::new("duplicates");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let one = recorded(&a, &recipe, "c-one", "SN-1");
    let two = recorded(&a, &recipe, "c-two", "SN-2");
    let (first, both) = (temp.0.join("first.zip"), temp.0.join("both.zip"));
    export(&a, &[one], &temp.0.join("s1"), &first);
    export(&a, &[one, two], &temp.0.join("s2"), &both);
    let b = machine(&temp.0.join("b"), "line-b");

    import_package(&b.store, b.project, &b.root, &first, &operation()).unwrap();
    let partial = import_package(&b.store, b.project, &b.root, &both, &operation()).unwrap();
    assert_eq!(partial.imported, 1);
    assert_eq!(
        partial
            .skipped
            .iter()
            .map(|s| s.sn.as_str())
            .collect::<Vec<_>>(),
        ["SN-1"]
    );
    let again = import_package(&b.store, b.project, &b.root, &both, &operation()).unwrap();
    assert_eq!((again.imported, again.skipped.len()), (0, 2));
    assert_eq!(count(&b), 2);
    assert_eq!(imported_entries(&b).len(), 2);
}

#[test]
fn broken_or_conflicting_package_is_rejected_without_leaving_records_or_images() {
    let temp = TempDir::new("rejected");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let id = recorded(&a, &recipe, "c-one", "SN-1");
    let package = temp.0.join("parts.zip");
    export(&a, &[id], &temp.0.join("scratch"), &package);

    let corrupted = temp.0.join("corrupted.zip");
    rewrite(&package, &corrupted, |name, mut bytes| {
        if name.ends_with("_s000002.pgm") {
            bytes[0] ^= 1;
        }
        bytes
    });
    let newer = temp.0.join("newer.zip");
    rewrite(&package, &newer, |name, bytes| {
        if name != "manifest.json" {
            return bytes;
        }
        let mut manifest: Value = serde_json::from_slice(&bytes).unwrap();
        manifest["format"] = 99.into();
        serde_json::to_vec(&manifest).unwrap()
    });
    let cases = [
        ("损坏", corrupted, false, "校验和不符"),
        ("版本不支持", newer, false, "不受本机软件支持"),
        ("配方快照冲突", package, true, "配方快照"),
    ];
    for (label, path, conflicting, expected) in cases {
        let dir = temp.0.join(format!("b-{label}"));
        let b = machine(&dir, "line-b");
        if conflicting {
            let mut changed = (*recipe).clone();
            changed.name = "本机同号的另一版配方".into();
            recorded(&b, &changed, "c-local", "SN-L");
        }
        let before = (count(&b), imported_entries(&b));
        let error = import_package(&b.store, b.project, &b.root, &path, &operation()).unwrap_err();
        assert!(error.contains(expected), "{label}：{error}");
        assert_eq!((count(&b), imported_entries(&b)), before, "{label}");
    }
}

/// 防的回归：本机同一修订的配方快照是升级前写的（带已退役的 `planVersion`），与包里的快照只差这个字段，导入却被当成
/// 快照冲突整包拒绝。
#[test]
fn snapshot_differing_only_by_retired_plan_version_is_not_a_conflict() {
    let temp = TempDir::new("legacy-snapshot");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let id = recorded(&a, &recipe, "c-one", "SN-1");
    let package = temp.0.join("parts.zip");
    export(&a, &[id], &temp.0.join("scratch"), &package);
    let b = machine(&temp.0.join("b"), "line-b");
    recorded(&b, &recipe, "c-local", "SN-L");
    let db = rusqlite::Connection::open(temp.0.join("b").join("inspection.db")).unwrap();
    let json: String = db
        .query_row(
            "SELECT json FROM recipe_snapshots WHERE revision_id = ?1",
            [&recipe.revision_id],
            |r| r.get(0),
        )
        .unwrap();
    let mut legacy: Value = serde_json::from_str(&json).unwrap();
    legacy["planVersion"] = 14.into();
    db.execute(
        "UPDATE recipe_snapshots SET json = ?1 WHERE revision_id = ?2",
        rusqlite::params![legacy.to_string(), recipe.revision_id],
    )
    .unwrap();
    drop(db);
    let imported = import_package(&b.store, b.project, &b.root, &package, &operation()).unwrap();
    assert_eq!(imported.imported, 1);
}

#[test]
fn interrupted_import_keeps_committed_images_and_removes_uncommitted_ones() {
    let temp = TempDir::new("recover");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let id = recorded(&a, &recipe, "c-kept", "SN-K");
    let package = temp.0.join("parts.zip");
    export(&a, &[id], &temp.0.join("scratch"), &package);
    let b = machine(&temp.0.join("b"), "line-b");
    import_package(&b.store, b.project, &b.root, &package, &operation()).unwrap();
    let imported = b.root.join(IMPORTED);
    let kept = imported.join(&imported_entries(&b)[0]);
    let orphan = imported.join("x_cycle_c-orphan");
    let staging = imported.join(format!("{STAGING_PREFIX}1-0"));
    for dir in [&kept, &orphan, &staging] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(PENDING), "line-b").unwrap();
    }

    assert!(recover(&b.store, &b.root).is_empty());
    assert!(kept.join("part.json").is_file() && !kept.join(PENDING).exists());
    assert!(!orphan.exists() && !staging.exists());
}

#[test]
fn expired_imported_parts_hand_their_images_to_cleanup_but_local_recordings_stay_with_the_recorder()
{
    let temp = TempDir::new("purge");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let id = recorded(&a, &recipe, "c-imported", "SN-I");
    let package = temp.0.join("parts.zip");
    export(&a, &[id], &temp.0.join("scratch"), &package);
    let b = machine(&temp.0.join("b"), "line-b");
    recorded(&b, &saved_recipe(&temp.0.join("b")), "c-local", "SN-L");
    import_package(&b.store, b.project, &b.root, &package, &operation()).unwrap();

    let (deleted, orphaned) = b
        .store
        .purge_before(ly_plc::now_ms() + 1, &Default::default())
        .unwrap();
    assert_eq!(deleted, 2);
    let directories: Vec<&str> = orphaned.iter().map(|(_, _, d)| d.as_str()).collect();
    assert_eq!(directories.len(), 1, "{directories:?}");
    assert!(imported_recording(Path::new(directories[0])));
}

/// 按天清理跳过被示教件引用的历史件：防的回归是过期的示教件那一件连记录带录制被删，示教件从此读不了原图。
#[test]
fn expired_parts_referenced_by_teaching_rounds_keep_their_record_and_recording() {
    let temp = TempDir::new("purge-keep");
    let a = machine(&temp.0.join("a"), "line-a");
    let recipe = saved_recipe(&temp.0.join("a"));
    let id = recorded(&a, &recipe, "c-imported", "SN-I");
    let package = temp.0.join("parts.zip");
    export(&a, &[id], &temp.0.join("scratch"), &package);
    let b = machine(&temp.0.join("b"), "line-b");
    recorded(&b, &saved_recipe(&temp.0.join("b")), "c-local", "SN-L");
    import_package(&b.store, b.project, &b.root, &package, &operation()).unwrap();

    let keep = std::collections::BTreeSet::from(["c-imported".to_string()]);
    let (deleted, orphaned) = b.store.purge_before(ly_plc::now_ms() + 1, &keep).unwrap();
    assert_eq!((deleted, orphaned.len()), (1, 0), "{orphaned:?}");
    assert!(b.store.detail_by_cycle("c-imported").unwrap().is_some());
    assert!(b.store.detail_by_cycle("c-local").unwrap().is_none());
}

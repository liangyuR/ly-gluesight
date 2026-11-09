use super::*;
use crate::plan::SegmentKind::Straight;
use crate::project::ProjectMeta;
use crate::workspace::pieces_tests::measured_teaching;

const ROUND: &str = "capture-1-0";

struct Dirs {
    base: PathBuf,
    source: PathBuf,
    target: PathBuf,
    package: PathBuf,
    staging: PathBuf,
}

impl Dirs {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "gluesight-recipe-package-{label}-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        let staging = base.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        Self {
            source: base.join("source"),
            target: base.join("target"),
            package: base.join("recipe.zip"),
            staging,
            base,
        }
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// 开发机项目里示教好的候选：示教件轮次带原图，候选目录里还有重画走廊留下的旧示教帧、旧轮次的测量与发布快照，
/// 示教帧的标定引用是开发机项目里的绝对路径；workspace.json 还带着已删除的验证样本字段（旧候选）。
fn source_candidate(project: &Path) -> Workspace {
    let measured = measured_teaching(&[Straight], 100.0);
    let text = serde_json::to_string(&measured)
        .unwrap()
        .replace("\"teach\"", &format!("\"{ROUND}\""));
    let mut w: Workspace = serde_json::from_str(&text).unwrap();
    let calib = project
        .join("calib")
        .join("device")
        .join("plane_calib_cam1.json");
    w.camera_teaching[0].frame.calib_tag = json!({"path": calib, "value": null});
    let mut legacy = serde_json::to_value(&w).unwrap();
    legacy["samples"] = json!([{"historyId": 5, "sampleId": null, "expected": "OK"}]);
    legacy["sampleBank"] = json!([{"id": "capture-7-0", "name": "独立采集", "geometryTag": [], "expected": "OK", "createdAt": 1, "captureId": "capture-7-0"}]);
    legacy["validation"] = json!({"revision": 1, "passed": false, "checkedAt": 1, "checks": [], "samples": [], "environmentTag": null});
    let dir = project.join("workspaces").join(&w.doc.id);
    write(
        &dir.join("workspace.json"),
        &serde_json::to_string_pretty(&legacy).unwrap(),
    );
    for t in &w.camera_teaching {
        write(
            &dir.join("images").join(format!("{}.pgm", t.frame.id)),
            "P5 1 1 255 x",
        );
    }
    write(&dir.join("images").join("orphan-1.pgm"), "P5 1 1 255 x");
    write(
        &dir.join("published").join(format!("{}-v1.json", w.doc.id)),
        "{}",
    );
    write(
        &dir.join("calibration").join(ROUND).join("features.jsonl"),
        "{}\n",
    );
    write(
        &dir.join("calibration")
            .join("capture-0-0")
            .join("features.jsonl"),
        "{}\n",
    );
    let round = project.join("recipe-capture").join(ROUND);
    write(
        &round.join("round.json"),
        &json!({"format": 3, "roundId": ROUND, "recipeId": w.doc.id}).to_string(),
    );
    write(
        &round.join("frames.jsonl"),
        &format!(
            "{}\n",
            json!({"camera": 0, "seq": 1, "path": "cam1/000001.pgm"})
        ),
    );
    write(&round.join("cam1").join("000001.pgm"), "P5 1 1 255 raw");
    w
}

/// 现场项目：同编号的旧候选带它的发布快照，旧候选与别的配方各有一个采集轮次。
fn target_project(project: &Path, id: &str) {
    let dir = project.join("workspaces").join(id);
    write(&dir.join("workspace.json"), "old");
    write(&dir.join("published").join(format!("{id}-v3.json")), "{}");
    let capture = project.join("recipe-capture");
    write(
        &capture.join("capture-9-0").join("round.json"),
        &json!({"roundId": "capture-9-0", "recipeId": id}).to_string(),
    );
    write(
        &capture.join("capture-8-0").join("round.json"),
        &json!({"roundId": "capture-8-0", "recipeId": "OTHER"}).to_string(),
    );
}

fn quiet() -> Operation {
    Operation::with_events(crate::operation::Operations::default(), None, |_| {}).unwrap()
}

fn export(dirs: &Dirs, w: &Workspace, images: bool) -> Result<PackageManifest, String> {
    let meta = ProjectMeta::new("dev", "开发", "", None);
    export_candidate(&dirs.source, &meta, w, images, &dirs.package, &quiet())
}

fn exported_with(dirs: &Dirs, images: bool) -> Workspace {
    let w = source_candidate(&dirs.source);
    export(dirs, &w, images).unwrap();
    target_project(&dirs.target, &w.doc.id);
    w
}

fn exported(dirs: &Dirs) -> Workspace {
    exported_with(dirs, false)
}

fn staged(dirs: &Dirs) -> Workspace {
    let (manifest, mut w) = stage(&dirs.package, &dirs.staging, &quiet()).unwrap();
    let local = Local {
        has_production: true,
        next_version: 4,
    };
    rebase(
        &mut w,
        &local,
        Path::new(&manifest.source_root),
        &dirs.target,
    )
    .unwrap();
    w
}

/// 回传包只带候选与示教件轮次记录（原图、旧示教帧、旧轮次测量、发布快照都不带）；导入后版本号按本机配方库、
/// 回到草稿、路径改到本机项目，旧候选带的验证样本字段读入时丢弃，本机的发布快照保留，旧候选的轮次换成包里的轮次，别的配方不受影响。
#[test]
fn package_skips_raw_images_and_import_rebases_onto_local_recipe() {
    let dirs = Dirs::new("roundtrip");
    let source = exported(&dirs);
    let names: BTreeSet<String> = inspect(&dirs.package)
        .unwrap()
        .files
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert!(names.contains(&format!("rounds/{ROUND}/frames.jsonl")));
    assert!(!names.iter().any(|n| n.ends_with("000001.pgm")
        || n.contains("orphan-1")
        || n.contains("published")
        || n.contains("capture-0-0")));

    let w = staged(&dirs);
    install(&dirs.target, &dirs.staging, &w).unwrap();

    let dir = dirs.target.join("workspaces").join(&source.doc.id);
    let installed: Workspace =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workspace.json")).unwrap())
            .unwrap();
    assert_eq!(installed.doc.version, 4);
    assert!(matches!(installed.publish, PublishState::Draft));
    assert_eq!(
        installed.camera_teaching[0].frame.calib_tag["path"].as_str(),
        dirs.target
            .join("calib")
            .join("device")
            .join("plane_calib_cam1.json")
            .to_str()
    );
    assert_eq!(installed.teaching_piece, source.teaching_piece);
    assert!(installed.camera_teaching.iter().all(|t| dir
        .join("images")
        .join(format!("{}.pgm", t.frame.id))
        .is_file()));
    assert!(dir
        .join("published")
        .join(format!("{}-v3.json", source.doc.id))
        .is_file());
    let capture = dirs.target.join("recipe-capture");
    assert!(capture.join(ROUND).join("frames.jsonl").is_file());
    assert!(!capture.join(ROUND).join("cam1").exists());
    assert!(!capture.join("capture-9-0").exists());
    assert!(capture.join("capture-8-0").exists());
}

/// 落盘中途失败（包里的轮次没能移入）时撤回已做的移动：本机旧候选、发布快照与旧轮次原样留在原处。
#[test]
fn failed_install_restores_previous_candidate_and_rounds() {
    let dirs = Dirs::new("rollback");
    let source = exported(&dirs);
    let w = staged(&dirs);
    std::fs::remove_dir_all(dirs.staging.join(ROUNDS).join(ROUND)).unwrap();

    assert!(install(&dirs.target, &dirs.staging, &w).is_err());

    let dir = dirs.target.join("workspaces").join(&source.doc.id);
    assert_eq!(
        std::fs::read_to_string(dir.join("workspace.json")).unwrap(),
        "old"
    );
    assert!(dir
        .join("published")
        .join(format!("{}-v3.json", source.doc.id))
        .is_file());
    let capture = dirs.target.join("recipe-capture");
    assert!(capture.join("capture-9-0").join("round.json").is_file());
    assert!(!capture.join(ROUND).exists());
}

/// 选“包含示教件原图”导出：示教件轮次的原图随包走，导入后原样落在本机示教件轮次里，不用再从历史记录补回。
#[test]
fn package_with_teaching_images_restores_round_images() {
    let dirs = Dirs::new("images");
    exported_with(&dirs, true);
    assert!(inspect(&dirs.package).unwrap().teaching_images);

    let w = staged(&dirs);
    install(&dirs.target, &dirs.staging, &w).unwrap();

    let frame = Path::new(ROUND).join("cam1").join("000001.pgm");
    assert_eq!(
        std::fs::read(dirs.target.join("recipe-capture").join(&frame)).unwrap(),
        std::fs::read(dirs.source.join("recipe-capture").join(&frame)).unwrap()
    );
}

/// 示教件轮次缺原图（例如它本身是从不带原图的包导入的）时，不能导出一个自称带原图、导入后却不能试测的包；不带原图照常导出。
#[test]
fn export_with_images_rejects_round_missing_a_frame() {
    let dirs = Dirs::new("missing-frame");
    let w = source_candidate(&dirs.source);
    std::fs::remove_file(
        dirs.source
            .join("recipe-capture")
            .join(ROUND)
            .join("cam1")
            .join("000001.pgm"),
    )
    .unwrap();

    assert!(export(&dirs, &w, true).is_err());
    assert!(!dirs.package.exists());
    assert!(export(&dirs, &w, false).is_ok());
}

/// 把 `dirs.package` 重写成格式 1（清单没有 teachingImages），`extra` 为额外塞进包里的条目。
fn rewrite_as_format_one(dirs: &Dirs, target: &Path, extra: Option<(String, PathBuf)>) {
    let unzip = dirs.base.join("unzip");
    let _ = std::fs::remove_dir_all(&unzip);
    let manifest = inspect(&dirs.package).unwrap();
    let mut zip = transfer::open_zip(&dirs.package, LABEL).unwrap();
    transfer::extract_listed(&mut zip, &manifest.files, &unzip, |_| true, LABEL).unwrap();
    let mut entries: Vec<(String, PathBuf)> = manifest
        .files
        .iter()
        .map(|f| (f.path.clone(), unzip.join(&f.path)))
        .collect();
    entries.extend(extra);
    let mut value = serde_json::to_value(&manifest).unwrap();
    value["format"] = json!(1);
    value.as_object_mut().unwrap().remove("teachingImages");
    transfer::write_zip(target, entries, |files| {
        value["files"] = serde_json::to_value(files).unwrap();
        value
    })
    .unwrap();
}

/// 0.2.x 导出的格式 1 包（清单没有 teachingImages）照样能导入，按不带原图处理；不带原图的包里夹了原图则整包拒绝。
#[test]
fn format_one_package_imports_without_images_and_rejects_smuggled_frames() {
    let dirs = Dirs::new("format1");
    let source = exported(&dirs);

    let old = dirs.base.join("old.zip");
    rewrite_as_format_one(&dirs, &old, None);
    let (manifest, w) = stage(&old, &dirs.staging, &quiet()).unwrap();
    assert!(!manifest.teaching_images);
    assert_eq!(w.doc.id, source.doc.id);

    let smuggled = dirs.base.join("smuggled.zip");
    let frame = dirs
        .source
        .join("recipe-capture")
        .join(ROUND)
        .join("cam1")
        .join("000001.pgm");
    rewrite_as_format_one(
        &dirs,
        &smuggled,
        Some((format!("rounds/{ROUND}/cam1/000001.pgm"), frame)),
    );
    assert!(inspect(&smuggled).is_err());
}

/// 本机相机设备配置与示教件采集时不同、或本机没有这台相机时给出提示（不拦导入）；一致的相机不提。
#[test]
fn camera_notes_flag_changed_or_absent_cameras_only() {
    let round = json!({"cameras": [
        {"cameraId": "cam1", "cameraConfig": {"serial": "A"}},
        {"cameraId": "cam2", "cameraConfig": {"serial": "B"}},
        {"cameraId": "cam3", "cameraConfig": {"serial": "C"}}
    ]});
    let same: BTreeMap<String, Value> = [
        ("cam1".to_string(), json!({"serial": "A"})),
        ("cam2".to_string(), json!({"serial": "B"})),
        ("cam3".to_string(), json!({"serial": "C"})),
    ]
    .into();
    assert!(camera_notes(&[round.clone()], &same).is_empty());

    let mut other = same.clone();
    other.insert("cam2".into(), json!({"serial": "X"}));
    other.remove("cam3");
    let notes = camera_notes(&[round], &other);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("cam2") && notes[0].contains("cam3") && !notes[0].contains("cam1"));
}

/// 换示教件的对照还没确认时，迁移起点里的示教帧（人断开对应后已不在当前示教帧里）也随包带走，改回对照时图像不缺。
#[test]
fn package_carries_teaching_frames_of_an_unconfirmed_pass_alignment() {
    let dirs = Dirs::new("pass-alignment");
    let mut w = source_candidate(&dirs.source);
    let mut pending = w.camera_teaching[0].clone();
    pending.frame.id = "pending-1".into();
    w.pass_alignment = Some(crate::workspace::PassAlignment {
        old_piece: crate::workspace::OldPiece {
            record_id: 1,
            sn: "SN-1".into(),
            ts: 1,
            passes: vec![10],
        },
        new_passes: vec![10],
        suggested: Vec::new(),
        rows: Vec::new(),
        confirmed: false,
        source: Some(crate::workspace::AlignSource {
            segments: w.doc.segments.clone(),
            corridors: Default::default(),
            teaching: vec![pending],
            cuts: Vec::new(),
        }),
        carry: Default::default(),
        removed: Vec::new(),
    });
    let dir = dirs.source.join("workspaces").join(&w.doc.id);
    write(
        &dir.join("workspace.json"),
        &serde_json::to_string_pretty(&w).unwrap(),
    );
    write(&dir.join("images").join("pending-1.pgm"), "P5 1 1 255 x");
    export(&dirs, &w, false).unwrap();
    target_project(&dirs.target, &w.doc.id);
    staged(&dirs);
    assert!(dirs
        .staging
        .join(CANDIDATE)
        .join("images")
        .join("pending-1.pgm")
        .is_file());
}

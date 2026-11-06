use super::*;
use crate::judge::Verdict;
use crate::plan::SegmentKind::Straight;
use crate::project::ProjectMeta;
use crate::workspace::pieces_tests::measured_teaching;
use crate::workspace::Sample;

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
/// 示教帧的标定引用是开发机项目里的绝对路径，另有一个来自检测历史的验证样本。
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
    w.samples.push(Sample {
        history_id: Some(5),
        sample_id: None,
        expected: Verdict::Ok,
    });
    let dir = project.join("workspaces").join(&w.doc.id);
    write(
        &dir.join("workspace.json"),
        &serde_json::to_string_pretty(&w).unwrap(),
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
    write(&round.join("frames.jsonl"), "{}\n");
    write(&round.join("cam1").join("000001.pgm"), "P5 1 1 255 x");
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

fn exported(dirs: &Dirs) -> Workspace {
    let w = source_candidate(&dirs.source);
    let meta = ProjectMeta::new("dev", "开发", "", None);
    export_candidate(&dirs.source, &meta, &w, &dirs.package).unwrap();
    target_project(&dirs.target, &w.doc.id);
    w
}

fn staged(dirs: &Dirs) -> (Workspace, Vec<String>) {
    let (manifest, mut w) = stage(&dirs.package, &dirs.staging).unwrap();
    let local = Local {
        production: Some((format!("{}-v3", w.doc.id), 7)),
        next_version: 4,
    };
    let notes = rebase(
        &mut w,
        &local,
        Path::new(&manifest.source_root),
        &dirs.target,
    )
    .unwrap();
    (w, notes)
}

/// 回传包只带候选与示教件轮次记录（原图、旧示教帧、旧轮次测量、发布快照都不带）；导入后候选基于本机生产配方、
/// 回到草稿、路径改到本机项目、验证样本去掉，本机的发布快照保留，旧候选的轮次换成包里的轮次，别的配方不受影响。
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

    let (w, notes) = staged(&dirs);
    install(&dirs.target, &dirs.staging, &w).unwrap();

    let dir = dirs.target.join("workspaces").join(&source.doc.id);
    let installed: Workspace =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workspace.json")).unwrap())
            .unwrap();
    assert_eq!(
        installed.base_revision,
        Some(format!("{}-v3", source.doc.id))
    );
    assert_eq!((installed.doc.version, installed.doc.plan_version), (4, 7));
    assert!(matches!(installed.publish, PublishState::Draft));
    assert!(installed.samples.is_empty() && notes.len() == 1);
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
    let (w, _) = staged(&dirs);
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

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::plan::SegmentKind::Straight;
use crate::workspace::burst::{calibration_round, CameraTeaching};
use crate::workspace::burst_tests::{burst_workspace, capture_round, DEVICE};

const ROUND: &str = "capture-100-0";

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn temp() -> Dir {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("gluesight-transfer-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    Dir(dir)
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// 段配方候选：cam1 用 ROUND 第 1 帧示教，ROUND 加入多轮标定并有复核记录；磁盘上有示教帧、标定特征与轮次原图。
/// 候选处于发布失败状态、带基准版本与一个来源机历史记录样本，导入后都应清掉。
fn fixture(root: &Path) -> (Workspace, PathBuf, PathBuf) {
    let mut w = burst_workspace(&[Straight]);
    let round = capture_round(ROUND, &w.doc, 2, DEVICE, &[]);
    let frame = FrozenImage {
        id: "corridor-1".into(), view: 1, source: format!("capture:{ROUND}#1"), captured_at: 1, size: [4, 2], camera: "cam1".into(),
        camera_tag: json!(DEVICE), calib_tag: Value::Null, geometry_tag: json!({"round": ROUND, "camera": "cam1", "seq": 1}),
        exposure_us: None, gain_db: None, history_id: None,
    };
    w.camera_teaching.push(CameraTeaching { camera: "cam1".into(), frame, round: ROUND.into(), seq: 1 });
    w.calibration.rounds.push(calibration_round(&w.doc, &round, false, &[]));
    w.capture_review.rounds.insert(ROUND.into(), RoundReview {
        group_count: 2, cuts: Vec::new(), status: ReviewStatus::Draft, excluded_groups: Vec::new(),
        segment_signature: Value::Null, capture_signature: Value::Null, template_version: None,
    });
    w.samples.push(Sample { history_id: Some(7), sample_id: None, expected: crate::judge::Verdict::Ok });
    w.base_revision = Some("rev-source".into());
    w.publish = PublishState::Failed { message: "来源机发布失败".into() };
    let dir = root.join("workspaces").join(&w.doc.id);
    write(&dir.join("images").join("corridor-1.pgm"), b"P5 4 2 255\nabcdefgh");
    write(&dir.join("calibration").join(ROUND).join("features.jsonl"), b"{\"camera\":0}\n");
    let captures = root.join("recipe-capture");
    let round_dir = captures.join(ROUND);
    write(&round_dir.join("round.json"), &serde_json::to_vec_pretty(&round).unwrap());
    write(&round_dir.join("frames.jsonl"), b"{\"camera\":0,\"seq\":1}\n");
    for camera in w.doc.plan_cameras() {
        for seq in 1..=2 { write(&round_dir.join(&camera).join(format!("{seq:06}.pgm")), format!("P5 1 1 255\n{camera}{seq}").as_bytes()); }
    }
    (w, dir, captures)
}

fn package_file(root: &Path, w: &Workspace, dir: &Path, captures: &Path) -> PathBuf {
    let package = collect_candidate(w, dir, captures, |_| Ok(())).unwrap();
    let (bytes, _) = write_package(std::io::Cursor::new(Vec::new()), package).unwrap();
    let path = root.join("package.zip");
    std::fs::write(&path, bytes.into_inner()).unwrap();
    path
}

fn target(id: &str, product_code: u16) -> ImportTarget {
    ImportTarget { id: id.into(), name: "迁入配方".into(), product_code }
}

#[test]
fn exported_candidate_round_trips_with_referenced_round_images_and_matching_checksums() {
    // 防的回归：导出漏掉候选引用的采集轮次原图、示教帧或标定特征，或解包后的内容与来源不一致
    let root = temp();
    let (w, dir, captures) = fixture(&root.0);
    let path = package_file(&root.0, &w, &dir, &captures);
    let staging = root.0.join("staging");
    std::fs::create_dir(&staging).unwrap();
    let manifest = extract(&path, &staging).unwrap();
    assert_eq!(manifest.rounds, vec![ROUND.to_string()]);
    assert_eq!(manifest.recipe_id, w.doc.id);
    let mut expected = vec![
        ("workspace/images/corridor-1.pgm".to_string(), dir.join("images").join("corridor-1.pgm")),
        (format!("workspace/calibration/{ROUND}/features.jsonl"), dir.join("calibration").join(ROUND).join("features.jsonl")),
        (format!("rounds/{ROUND}/round.json"), captures.join(ROUND).join("round.json")),
        (format!("rounds/{ROUND}/frames.jsonl"), captures.join(ROUND).join("frames.jsonl")),
    ];
    for camera in w.doc.plan_cameras() {
        for seq in 1..=2 { expected.push((format!("rounds/{ROUND}/{camera}/{seq:06}.pgm"), captures.join(ROUND).join(&camera).join(format!("{seq:06}.pgm")))); }
    }
    for (inside, source) in &expected {
        let staged = inside.split('/').fold(staging.clone(), |p, part| p.join(part));
        assert_eq!(std::fs::read(staged).unwrap(), std::fs::read(source).unwrap(), "{inside}");
        let listed = manifest.files.iter().find(|f| &f.path == inside).unwrap();
        assert_eq!(listed.sha256, format!("{:x}", Sha256::digest(std::fs::read(source).unwrap())));
    }
    let staged: Workspace = serde_json::from_str(&std::fs::read_to_string(staging.join("workspace").join("workspace.json")).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&staged).unwrap(), serde_json::to_value(&w).unwrap());
    assert_eq!(manifest.files.len(), expected.len() + 1);
}

fn craft(path: &Path, manifest: &Value, entries: &[(&str, &[u8])]) {
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    for (name, bytes) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.start_file(MANIFEST, SimpleFileOptions::default()).unwrap();
    zip.write_all(&serde_json::to_vec(manifest).unwrap()).unwrap();
    zip.finish().unwrap();
}

#[test]
fn tampered_or_unknown_packages_are_rejected_without_writing_outside_staging() {
    // 防的回归：zip slip 写出暂存目录、校验和不符或清单外的文件被接受、不认识的格式版本被当作当前格式导入
    let root = temp();
    let doc_bytes = serde_json::to_vec(&burst_workspace(&[Straight])).unwrap();
    let doc: &[u8] = &doc_bytes;
    let image: &[u8] = b"P5 1 1 255\nx";
    let other: &[u8] = b"P5 1 1 255\ny";
    let entry = |path: &str, bytes: &[u8]| json!({"path": path, "bytes": bytes.len(), "sha256": format!("{:x}", Sha256::digest(bytes))});
    let manifest = |format: u32, files: Vec<Value>| json!({
        "format": format, "kind": "candidate", "recipeId": "SRC", "name": "来源", "productCode": 9, "version": 1, "exportedAt": 1,
        "appVersion": "0", "rounds": [], "notes": [], "files": files,
    });
    let valid = manifest(1, vec![entry(CANDIDATE_FILE, doc), entry("workspace/images/a.pgm", image)]);
    let cases: [(&str, Value, Vec<(&str, &[u8])>, bool); 6] = [
        ("合规包", valid.clone(), vec![(CANDIDATE_FILE, doc), ("workspace/images/a.pgm", image)], true),
        ("../ 路径穿越", manifest(1, vec![entry(CANDIDATE_FILE, doc), entry("../evil.pgm", image)]), vec![(CANDIDATE_FILE, doc), ("../evil.pgm", image)], false),
        ("清单合规但条目穿越", valid.clone(), vec![(CANDIDATE_FILE, doc), ("../../evil.pgm", image)], false),
        ("校验和不符", valid.clone(), vec![(CANDIDATE_FILE, doc), ("workspace/images/a.pgm", other)], false),
        ("清单外条目", valid.clone(), vec![(CANDIDATE_FILE, doc), ("workspace/images/a.pgm", image), ("workspace/images/b.pgm", image)], false),
        ("未知格式版本", manifest(2, vec![entry(CANDIDATE_FILE, doc), entry("workspace/images/a.pgm", image)]), vec![(CANDIDATE_FILE, doc), ("workspace/images/a.pgm", image)], false),
    ];
    for (i, (label, manifest, entries, accepted)) in cases.into_iter().enumerate() {
        let case = root.0.join(format!("case-{i}"));
        let staging = case.join("a").join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let path = case.join("package.zip");
        craft(&path, &manifest, &entries);
        assert_eq!(extract(&path, &staging).is_ok(), accepted, "{label}");
        assert!(!case.join("evil.pgm").exists() && !case.join("a").join("evil.pgm").exists(), "{label}");
    }
}

#[test]
fn conflicting_round_id_is_renamed_with_all_candidate_references_and_import_starts_as_draft() {
    // 防的回归：本机已有同名采集轮次时覆盖或混用本机轮次，或改名后示教帧、多轮标定、复核记录与标定特征仍指向旧编号；
    // 导入的候选带着来源机的发布状态、基准版本或历史记录样本直接生效
    let root = temp();
    let (w, dir, captures) = fixture(&root.0);
    let path = package_file(&root.0, &w, &dir, &captures);
    let staging = root.0.join("staging");
    std::fs::create_dir(&staging).unwrap();
    let manifest = extract(&path, &staging).unwrap();
    let (imported, renames, _) = prepare_import(&staging, &manifest, &target("NEW-1", 77), std::iter::empty(), |id| id == ROUND).unwrap();
    let renamed = "capture-100-1";
    assert_eq!(renames.get(ROUND).map(String::as_str), Some(renamed));
    assert_eq!(imported.camera_teaching[0].round, renamed);
    assert_eq!(imported.camera_teaching[0].frame.source, format!("capture:{renamed}#1"));
    assert_eq!(imported.calibration.rounds[0].round_id, renamed);
    assert!(imported.capture_review.rounds.contains_key(renamed) && !imported.capture_review.rounds.contains_key(ROUND));
    let round: Value = serde_json::from_str(&std::fs::read_to_string(staging.join("rounds").join(ROUND).join("round.json")).unwrap()).unwrap();
    assert_eq!((round["roundId"].as_str(), round["recipeId"].as_str()), (Some(renamed), Some("NEW-1")));
    assert!(staging.join("workspace").join("calibration").join(renamed).join("features.jsonl").is_file());
    assert_eq!((imported.doc.id.as_str(), imported.doc.product_code), ("NEW-1", 77));
    assert!(imported.pending.is_none() && imported.validation.is_none() && imported.base_revision.is_none());
    assert_eq!(imported.publish, PublishState::Draft);
    assert!(imported.samples.iter().all(|s| s.history_id.is_none()));
}

#[test]
fn import_with_existing_recipe_id_or_product_code_is_rejected() {
    // 防的回归：导入覆盖或重复本机已有的配方编号（不区分大小写）、复用已占用的产品代码
    let root = temp();
    let (w, dir, captures) = fixture(&root.0);
    let path = package_file(&root.0, &w, &dir, &captures);
    for (label, target, accepted) in [("编号大小写冲突", target("local-a", 50), false), ("产品代码冲突", target("NEW-2", 12), false), ("不冲突", target("NEW-3", 50), true)] {
        let staging = root.0.join(format!("staging-{}", target.id));
        std::fs::create_dir(&staging).unwrap();
        let manifest = extract(&path, &staging).unwrap();
        let existing = [("LOCAL-A", 12u16)];
        let result = prepare_import(&staging, &manifest, &target, existing.iter().map(|(id, code)| (*id, *code)), |_| false);
        assert_eq!(result.is_ok(), accepted, "{label}");
    }
}

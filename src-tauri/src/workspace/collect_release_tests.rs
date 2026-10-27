use super::collect_release::{collect_checks, CollectInputs};
use super::*;
use crate::camera::CameraConfig;
use crate::recipe::{burst_template, legacy_camera_id, RecipeStore};

fn cameras() -> Vec<String> {
    (0..3).map(legacy_camera_id).collect()
}

fn rig() -> Vec<CameraConfig> {
    cameras()
        .into_iter()
        .map(|id| CameraConfig {
            id,
            ..CameraConfig::default()
        })
        .collect()
}

/// 刚建的候选：三台计划相机、一个待标定的直线段，走廊没示教。
fn untaught() -> RecipeDoc {
    burst_template("LINE-NEW".into(), "新产线".into(), 21, &cameras())
}

fn failed(checks: &[Check]) -> Vec<&str> {
    checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| c.name.as_str())
        .collect()
}

/// 在线投运：未示教、未标定、没验证的候选也能发布采集版，配方库接受并标成采集版。防采集版被正式发布门槛挡住。
#[test]
fn untaught_uncalibrated_candidate_then_collect_checks_pass_and_store_saves_collect() {
    let mut doc = untaught();
    assert!(doc.build().unwrap().ready_burst().is_err());
    let checks = collect_checks(
        &doc,
        CollectInputs {
            rig: &rig(),
            production: None,
            conflict: None,
            base_matches: true,
        },
    );
    assert_eq!(failed(&checks), Vec::<&str>::new());

    let dir = std::env::temp_dir().join(format!(
        "gluesight-collect-publish-{}-{}",
        std::process::id(),
        ly_plc::now_ms()
    ));
    let store = RecipeStore::open(dir.clone()).unwrap();
    doc.stage = RecipeStage::Collect;
    doc.version = store.next_version(&doc.id).unwrap();
    doc.plan_version = store.plan_version_for(&doc).unwrap();
    let saved = store.save_published(doc, None).unwrap();
    assert!(saved.is_collect());
    assert_eq!(saved.plan_cameras(), cameras());
    let _ = std::fs::remove_dir_all(dir);
}

/// 采集版至少要定下三台不同的计划相机（都在相机组、单视角），型号不冲突、有一段、没有正式版；
/// 缺任何一项都拒绝并指出是哪一项。
#[test]
fn missing_plan_camera_or_formal_exists_then_collect_check_fails() {
    let formal = crate::recipe::samples_burst().build().unwrap();
    type Case = (
        &'static str,
        fn(&mut RecipeDoc, &mut Vec<CameraConfig>),
        &'static str,
    );
    let cases: [Case; 5] = [
        (
            "只有两台计划相机",
            |d, _| {
                d.inspect.pop();
            },
            "计划相机",
        ),
        (
            "两台计划相机重复",
            |d, _| d.inspect[2].camera = d.inspect[0].camera.clone(),
            "计划相机",
        ),
        (
            "计划相机不在相机组",
            |_, rig| {
                rig.pop();
            },
            "相机组",
        ),
        (
            "计划相机是三目拼接",
            |_, rig| rig[0].view_count = 3,
            "相机组",
        ),
        ("没有段", |d, _| d.segments.clear(), "段"),
    ];
    for (case, edit, name) in cases {
        let (mut doc, mut rig) = (untaught(), rig());
        edit(&mut doc, &mut rig);
        let checks = collect_checks(
            &doc,
            CollectInputs {
                rig: &rig,
                production: None,
                conflict: None,
                base_matches: true,
            },
        );
        assert_eq!(failed(&checks), vec![name], "{case}");
    }
    let doc = untaught();
    let conflict = collect_checks(
        &doc,
        CollectInputs {
            rig: &rig(),
            production: None,
            conflict: Some("OTHER".into()),
            base_matches: true,
        },
    );
    assert_eq!(failed(&conflict), vec!["PLC 型号"], "型号被其他配方占用");
    let downgrade = collect_checks(
        &doc,
        CollectInputs {
            rig: &rig(),
            production: Some(&formal),
            conflict: None,
            base_matches: true,
        },
    );
    assert_eq!(failed(&downgrade), vec!["生产版本"], "已有正式版");
}

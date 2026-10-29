use super::collect_release::{collect_checks, CollectInputs};
use super::*;
use crate::camera::CameraConfig;
use crate::recipe::burst_template;

fn cameras() -> Vec<String> {
    (1..=3).map(|k| format!("cam{k}")).collect()
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

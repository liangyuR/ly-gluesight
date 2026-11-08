use super::burst::CameraTeaching;
use super::burst_release::{
    burst_checks, publish_blocker, teach_trial_check, BurstInputs, TeachTrial,
};
use super::burst_tests::{burst_workspace, capture_round, configs, DEVICE};
use super::*;
use crate::plan::{
    PartCalib, SegmentCalib,
    SegmentKind::{self, Corner, Straight},
};

/// 每段每台检测相机都选了示教帧（1000×700，采集时设备标签为 device）、画了样例走廊（落在帧内），相机有当量。
fn taught(kinds: &[SegmentKind], device: &str) -> Workspace {
    let mut w = burst_workspace(kinds);
    let corridor = crate::recipe::samples_burst().inspect[0].segments[0]
        .corridor
        .clone();
    for inspect in &mut w.doc.inspect {
        inspect.segments = (1..=kinds.len() as u32)
            .map(|segment| crate::recipe::SegmentCorridor {
                segment,
                corridor: corridor.clone(),
                sector_deg: None,
                color_model: None,
            })
            .collect();
    }
    for (camera, segment) in w
        .doc
        .plan_cameras()
        .into_iter()
        .flat_map(|c| (1..=kinds.len() as u32).map(move |s| (c.clone(), s)))
    {
        w.camera_teaching.push(CameraTeaching {
            camera: camera.clone(),
            round: "capture-teach".into(),
            seq: 1,
            frame: FrozenImage {
                id: format!("corridor-{camera}-{segment}"),
                view: 1,
                source: "capture:capture-teach#1".into(),
                captured_at: 1,
                size: [1000, 700],
                camera: camera.clone(),
                camera_tag: json!(device),
                calib_tag: Value::Null,
                geometry_tag: Value::Null,
                exposure_us: None,
                gain_db: None,
                history_id: None,
            },
            segment: Some(segment),
        });
    }
    w.teaching_piece = Some(crate::workspace::pieces_tests::piece("capture-teach", 1));
    w
}

/// 整件标定 ΣN_exp = total，并逐段写 N_exp（均分，余数给最后一段；与应用标定的结果同形）。
fn calibrate(w: &mut Workspace, total: u32) {
    let calib = SegmentCalib {
        rounds: 10,
        min: total - 1,
        max: total + 1,
        mean: f64::from(total),
    };
    w.doc.part_calib = Some(PartCalib {
        expected_total: total,
        calib: calib.clone(),
    });
    let n = w.doc.segments.len() as u32;
    for (i, segment) in w.doc.segments.iter_mut().enumerate() {
        let frames = if i as u32 + 1 == n {
            total - total / n * (n - 1)
        } else {
            total / n
        };
        (segment.expected_frames, segment.calib) = (
            Some(frames),
            Some(SegmentCalib {
                mean: f64::from(frames),
                min: frames - 1,
                max: frames + 1,
                ..calib.clone()
            }),
        );
    }
}

/// 设备就绪、基于当前生产版本，当前设备标签都是 DEVICE，没有任何验证样本（D-SB-46 发布不要求）：只剩候选自身的检查项决定能否发布。
fn checks(w: &Workspace, cameras: &BTreeMap<String, Value>) -> Vec<Check> {
    burst_checks(
        w,
        BurstInputs {
            device: Ok(()),
            cameras,
            base_matches: true,
        },
    )
}

fn check<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("缺少验证项 {name}：{checks:?}"))
}

#[test]
fn publish_blocked_when_plan_cameras_not_three_or_corridor_missing() {
    // 防的回归（规格 §1 Plan.cameras = 3；§4 每台相机的检测走廊；D-SB-46）：少一台计划相机、某台没选示教帧、走廊超出示教帧、
    // 缺像素当量，或示教帧采集后设备参数变了，仍能通过发布前检查；或都满足了却因为没有验证样本而不能发布
    type Edit = fn(&mut Workspace, &mut BTreeMap<String, Value>);
    let cases: [(&str, Edit, bool); 6] = [
        ("三台相机都示教好", |_, _| {}, true),
        (
            "计划相机只有 2 台",
            |w, _| {
                w.doc.inspect.truncate(2);
                w.camera_teaching.retain(|t| t.camera != "cam3");
            },
            false,
        ),
        (
            "一台相机没选示教帧",
            |w, _| w.camera_teaching.retain(|t| t.camera != "cam2"),
            false,
        ),
        (
            "走廊超出示教帧",
            |w, _| w.doc.inspect[0].segments[1].corridor[0] = [1200.0, 480.0],
            false,
        ),
        (
            "缺像素当量",
            |w, _| w.doc.inspect[1].mm_per_px = None,
            false,
        ),
        (
            "示教帧采集后设备参数改变",
            |_, current| {
                current.insert("cam3".into(), json!("device-b"));
            },
            false,
        ),
    ];
    for (name, edit, passed) in cases {
        let mut w = taught(&[Straight, Corner], DEVICE);
        calibrate(&mut w, 30);
        let mut current = configs(&w.doc, DEVICE);
        edit(&mut w, &mut current);
        let checks = checks(&w, &current);
        assert_eq!(
            (
                check(&checks, "走廊示教").passed,
                checks.iter().all(|c| c.passed)
            ),
            (passed, passed),
            "{name}：{checks:?}"
        );
    }
}


#[test]
fn changed_frame_rate_blocks_publish_until_teaching_measurement_configuration_restored() {
    let mut w = taught(&[Straight, Corner], DEVICE);
    calibrate(&mut w, 20);
    let mut round = capture_round("rate-teaching", &w.doc, 20, DEVICE, &[]);
    for camera in &mut round.cameras {
        camera.camera_config =
            json!({"triggerSource": "Line0", "triggerActivation": "LevelHigh", "frameRate": 10.0});
    }
    w.calibration
        .rounds
        .push(super::burst::calibration_round(&w.doc, &round, &[]));
    let mut current: BTreeMap<String, Value> = round
        .cameras
        .iter()
        .map(|c| (c.camera_id.clone(), c.camera_config.clone()))
        .collect();
    assert!(check(&checks(&w, &current), "示教件采集参数").passed);
    current.get_mut("cam1").unwrap()["frameRate"] = json!(20.0);
    assert!(!check(&checks(&w, &current), "示教件采集参数").passed);
    current.get_mut("cam1").unwrap()["frameRate"] = json!(10.0);
    assert!(check(&checks(&w, &current), "示教件采集参数").passed);
}

/// D-SB-59：示教帧上部分段没测成只提醒、不拦发布（生产时这些段不测）；一段都没测成拦发布。
/// 按 `workspace_publish` 用的同一门槛（`publish_blocker`）判能否发布。
#[test]
fn teach_trial_failures_warn_when_partial_and_block_publish_only_when_nothing_measures() {
    let trial = |segment, outcome: Result<(), &str>| TeachTrial {
        segment,
        camera: "cam1".into(),
        outcome: outcome.map_err(str::to_owned),
        incomplete: false,
    };
    let bad = || Err("runs：n_map（bad_param）");
    for (trials, publishable) in [
        (vec![trial(1, Ok(())), trial(2, Ok(()))], true),
        (vec![trial(1, Ok(())), trial(2, bad())], true),
        (vec![trial(1, bad()), trial(2, bad())], false),
        (
            vec![TeachTrial {
                incomplete: true,
                ..trial(1, Ok(()))
            }],
            true,
        ),
    ] {
        let checks: Vec<Check> = teach_trial_check(&trials).into_iter().collect();
        assert_eq!(
            publish_blocker(&checks).is_none(),
            publishable,
            "{}",
            checks[0].detail
        );
        let failed = trials.iter().any(|t| t.outcome.is_err() || t.incomplete);
        assert_eq!(!checks[0].passed, failed, "没测成的段要列出来提醒");
    }
}

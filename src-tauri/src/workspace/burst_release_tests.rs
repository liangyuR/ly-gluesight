use super::burst::{CalibrationRound, CameraTeaching};
use super::burst_release::{bank_round, burst_checks, BurstInputs};
use super::burst_tests::{burst_workspace, capture_round, configs, DEVICE};
use super::*;
use crate::plan::{
    PartCalib, SegmentCalib,
    SegmentKind::{self, Corner, Straight},
};
use crate::recipe_capture::CaptureRoundV2;

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
fn sample_round_used_for_teaching_or_calibration_or_from_other_device_is_rejected() {
    // 防的回归（规格 §0 独立验证、D-SB-35）：用作走廊示教或多轮标定的同一轮又当独立验证样本（自己验自己），
    // 或换了相机 / 设备配置的轮次混进验证样本
    type Edit = fn(&mut Workspace, &mut CaptureRoundV2, &mut BTreeMap<String, Value>);
    let cases: [(&str, Edit, bool); 5] = [
        ("独立的新轮次", |_, _, _| {}, true),
        (
            "已用于走廊示教",
            |w, _, _| w.camera_teaching[1].round = "capture-sample".into(),
            false,
        ),
        (
            "已加入多轮标定",
            |w, round, _| {
                w.calibration.rounds.push(CalibrationRound {
                    round_id: round.round_id.clone(),
                    standard: false,
                    created_at: 1,
                    cameras: w.doc.plan_cameras(),
                    camera_config: vec![json!(DEVICE); 3],
                    group_count: round.group_count,
                    camera_frames: vec![20; 3],
                    incomplete_groups: 0,
                    loss_alarms: 0,
                    corridors: vec![Value::Null; 3],
                    x_means: vec![None; 3],
                    edge_means: Vec::new(),
                    heading: Default::default(),
                })
            },
            false,
        ),
        (
            "采集后设备配置改变",
            |_, _, current| {
                current.insert("cam2".into(), json!("device-b"));
            },
            false,
        ),
        (
            "缺一台计划相机、多一台别的相机",
            |_, round, _| round.cameras[2].camera_id = "cam9".into(),
            false,
        ),
    ];
    for (name, edit, accepted) in cases {
        let mut w = taught(&[Straight], DEVICE);
        let mut round = capture_round("capture-sample", &w.doc, 20, DEVICE, &[]);
        let mut current = configs(&w.doc, DEVICE);
        edit(&mut w, &mut round, &mut current);
        let result = bank_round(&mut w, &round, &current, Verdict::Ok);
        assert_eq!(result.is_ok(), accepted, "{name}：{result:?}");
        let banked = (w.sample_bank.len(), w.samples.len());
        assert_eq!(
            banked,
            if accepted { (1, 1) } else { (0, 0) },
            "{name}：拒绝时不进样本库"
        );
    }
}

#[test]
fn changed_frame_rate_blocks_calibration_until_original_configuration_restored() {
    let mut w = taught(&[Straight, Corner], DEVICE);
    calibrate(&mut w, 20);
    let mut round = capture_round("rate-calibration", &w.doc, 20, DEVICE, &[]);
    for camera in &mut round.cameras {
        camera.camera_config =
            json!({"triggerSource": "Line0", "triggerActivation": "LevelHigh", "frameRate": 10.0});
    }
    w.calibration
        .rounds
        .push(super::burst::calibration_round(&w.doc, &round, false, &[]));
    let mut current: BTreeMap<String, Value> = round
        .cameras
        .iter()
        .map(|c| (c.camera_id.clone(), c.camera_config.clone()))
        .collect();
    assert!(check(&checks(&w, &current), "标定采集参数").passed);
    current.get_mut("cam1").unwrap()["frameRate"] = json!(20.0);
    assert!(!check(&checks(&w, &current), "标定采集参数").passed);
    current.get_mut("cam1").unwrap()["frameRate"] = json!(10.0);
    assert!(check(&checks(&w, &current), "标定采集参数").passed);
}

use super::burst::{CalibrationRound, CameraTeaching};
use super::burst_release::{
    bank_round, burst_checks, publish_blocker, teach_trial_check, BurstInputs, TeachTrial,
};
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
fn short_straight_warning_matches_runtime_offset_frames() {
    // 防的回归（D-SB-40）：发布检查只按 2 × 过渡帧 + 1 提示，没算胶偏判定再去掉的区域首尾各一帧和段跳过帧，
    // 中间直线段 N_exp 为 5、6 帧时运行时胶偏取不到帧（标胶偏未判），发布前却不提示
    use crate::segmentation::{resolve_regions, Boundary, RegionKind, SegOutcome};
    // (段类型, 直线段下标, 直线段的 skip_head, skip_tail)
    let layouts: [(&[SegmentKind], usize, u32, u32); 4] = [
        (&[Corner, Straight, Corner], 1, 0, 0),
        (&[Corner, Straight, Corner], 1, 3, 0),
        (&[Straight, Corner], 0, 0, 0),
        (&[Straight, Corner], 0, 4, 0),
    ];
    for (kinds, k, skip_head, skip_tail) in layouts {
        for n in 3..=10 {
            let mut w = taught(kinds, DEVICE);
            calibrate(&mut w, 30);
            let segment = &mut w.doc.segments[k];
            (
                segment.expected_frames,
                segment.skip_head,
                segment.skip_tail,
            ) = (Some(n), skip_head, skip_tail);
            let segments = &w.doc.segments;
            let mut cut = 0;
            let boundaries = segments[..segments.len() - 1]
                .iter()
                .map(|s| {
                    cut += u64::from(s.expected_frames.unwrap());
                    Boundary {
                        after_segment: s.index,
                        frame_seq: cut,
                        confidence: 1.0,
                    }
                })
                .collect();
            let total = segments
                .iter()
                .map(|s| u64::from(s.expected_frames.unwrap()))
                .sum();
            let index = segments[k].index;
            let core: u64 = resolve_regions(
                segments,
                &SegOutcome::Boundaries(boundaries),
                total,
                0.5,
                w.doc.boundary_guard,
            )
            .iter()
            .filter(|r| r.kind == RegionKind::Straight { segment: index })
            .map(|r| r.last_seq - r.first_seq + 1)
            .sum();
            let checks = checks(&w, &configs(&w.doc, DEVICE));
            let warned = check(&checks, "段帧数")
                .detail
                .split('；')
                .any(|item| item.starts_with(&format!("第 {index} 段")) && item.contains("胶偏"));
            assert_eq!(
                warned,
                core < 3,
                "{kinds:?} 第 {index} 段 N_exp {n}、跳过 {skip_head}/{skip_tail}：核心区域 {core} 帧"
            );
        }
    }
}

#[test]
fn sample_round_used_for_teaching_or_calibration_or_from_other_device_is_rejected() {
    // 防的回归（规格 §0 独立验证、D-SB-35）：用作走廊示教或示教件测量的同一轮又当独立验证样本（自己验自己），
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
            "已作为示教件测量",
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
    };
    let bad = || Err("runs：n_map（bad_param）");
    for (trials, publishable) in [
        (vec![trial(1, Ok(())), trial(2, Ok(()))], true),
        (vec![trial(1, Ok(())), trial(2, bad())], true),
        (vec![trial(1, bad()), trial(2, bad())], false),
    ] {
        let checks: Vec<Check> = teach_trial_check(&trials).into_iter().collect();
        assert_eq!(
            publish_blocker(&checks).is_none(),
            publishable,
            "{}",
            checks[0].detail
        );
        let failed = trials.iter().any(|t| t.outcome.is_err());
        assert_eq!(!checks[0].passed, failed, "没测成的段要列出来提醒");
    }
}

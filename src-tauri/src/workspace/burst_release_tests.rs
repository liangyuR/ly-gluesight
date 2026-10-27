use super::*;
use super::burst::{CalibrationRound, CameraTeaching};
use super::burst_release::{bank_round, burst_checks, debug_sizes, BurstInputs};
use super::burst_tests::{burst_workspace, capture_round, configs, DEVICE};
use crate::plan::{OffsetMeasure, OffsetRef, PartCalib, SegmentCalib, SegmentKind::{self, Corner, Straight}};
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
    w
}

/// 整件标定 ΣN_exp = total，并逐段写 N_exp（均分，余数给最后一段；与应用标定的结果同形）。
fn calibrate(w: &mut Workspace, total: u32) {
    let calib = SegmentCalib { rounds: 10, min: total - 1, max: total + 1, mean: f64::from(total) };
    w.doc.part_calib = Some(PartCalib { expected_total: total, calib: calib.clone() });
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
    checks.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("缺少验证项 {name}：{checks:?}"))
}

#[test]
fn validation_without_expected_total_blocks_publish_and_names_each_segment() {
    // 防的回归（规格 §1 未标定的计划不能发布；D-SB-45 修订 D-SB-21）：没有整件 ΣN_exp 也能通过验证去发布；多段配方只有
    // 整件 ΣN_exp、各段 N_exp 没标定也能发布（生产无法按累计 N_exp 预测每帧所属段、选不了走廊）；验证项不逐段列出待定原因
    // （名称，段类型，整件标定，逐段标定，能否发布，段标定详情里必须出现的文字）
    let cases: [(&str, &[SegmentKind], Option<u32>, bool, bool, &[&str]); 4] = [
        (
            "三段都待标定",
            &[Straight, Corner, Straight],
            None,
            false,
            false,
            &[
                "还差 10 轮",
                "第 1 段（直线）待标定",
                "第 2 段（弯道）待标定",
                "第 3 段（直线）待标定",
            ],
        ),
        (
            "三段只有整件 ΣN_exp",
            &[Straight, Corner, Straight],
            Some(30),
            false,
            false,
            &["整件预期 30 ± 3", "第 2 段（弯道）待逐段标定"],
        ),
        (
            "三段都已逐段标定",
            &[Straight, Corner, Straight],
            Some(30),
            true,
            true,
            &["整件预期 30 ± 3", "第 2 段（弯道）N_exp 10 ± 1"],
        ),
        (
            "单段已标定",
            &[Straight],
            Some(20),
            true,
            true,
            &["整件预期 20 ± 1", "第 1 段（直线）N_exp 20 ± 1"],
        ),
    ];
    for (name, kinds, total, per_segment, publishable, texts) in cases {
        let mut w = taught(kinds, DEVICE);
        if let Some(total) = total { calibrate(&mut w, total); }
        if !per_segment && kinds.len() > 1 {
            w.doc
                .segments
                .iter_mut()
                .for_each(|s| (s.expected_frames, s.calib) = (None, None));
        }
        // 单段直线段的 x_ref 已由标准件得出，不让它影响本用例
        if let [segment] = w.doc.segments.as_mut_slice() {
            segment.offset_ref = Some(OffsetRef {
                measure: OffsetMeasure::AbsoluteX,
                camera: "cam1".into(),
                x_ref: Some(0.1),
                spread: Some(0.05),
                edge: None,
            });
        }
        let checks = checks(&w, &configs(&w.doc, DEVICE));
        let segment = check(&checks, "段标定");
        assert_eq!((segment.passed, checks.iter().all(|c| c.passed)), (publishable, publishable), "{name}：{checks:?}");
        for text in texts { assert!(segment.detail.contains(text), "{name}：段标定详情缺“{text}”：{}", segment.detail); }
    }
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
        assert_eq!((check(&checks, "走廊示教").passed, checks.iter().all(|c| c.passed)), (passed, passed), "{name}：{checks:?}");
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
        assert_eq!(banked, if accepted { (1, 1) } else { (0, 0) }, "{name}：拒绝时不进样本库");
    }
}

#[test]
fn burst_candidate_debug_run_gets_teaching_frame_sizes_in_plan_camera_order() {
    // 防的回归（决策记录第九节第 13、14 条）：示教帧尺寸按示教登记顺序而不是计划相机顺序交给 Candidate::new_burst
    // （它按 inspect 顺序逐台配对），走廊测量图套错相机的帧尺寸。
    let mut w = taught(&[Straight, Corner, Straight], DEVICE);
    calibrate(&mut w, 30);
    let cameras = w.doc.plan_cameras();
    for teaching in &mut w.camera_teaching {
        teaching.frame.size = [
            1000 + 100 * cameras.iter().position(|c| *c == teaching.camera).unwrap() as u32,
            700,
        ];
    }
    w.camera_teaching.reverse();
    let sizes = debug_sizes(&w, &configs(&w.doc, DEVICE)).unwrap();
    assert_eq!(sizes, [[1000, 700], [1100, 700], [1200, 700]]);
}

#[test]
fn changed_frame_rate_blocks_calibration_until_original_configuration_restored() {
    let mut w = taught(&[Straight, Corner], DEVICE);
    calibrate(&mut w, 20);
    let mut round = capture_round("rate-calibration", &w.doc, 20, DEVICE, &[]);
    for camera in &mut round.cameras {
        camera.camera_config = json!({"triggerSource": "Line0", "triggerActivation": "LevelHigh", "frameRate": 10.0});
    }
    w.calibration.rounds.push(super::burst::calibration_round(&w.doc, &round, false, &[]));
    let mut current: BTreeMap<String, Value> = round.cameras.iter().map(|c| (c.camera_id.clone(), c.camera_config.clone())).collect();
    assert!(check(&checks(&w, &current), "标定采集参数").passed);
    current.get_mut("cam1").unwrap()["frameRate"] = json!(20.0);
    assert!(!check(&checks(&w, &current), "标定采集参数").passed);
    current.get_mut("cam1").unwrap()["frameRate"] = json!(10.0);
    assert!(check(&checks(&w, &current), "标定采集参数").passed);
}

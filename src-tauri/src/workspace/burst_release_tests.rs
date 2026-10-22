use super::*;
use super::burst::{camera_teaching_views, measure_round, progress_v2, CalibrationRound, CameraTeaching, StepIdV2, TargetV2};
use super::burst_release::{bank_round, burst_checks, judge_round, sample_result, BurstInputs};
use super::burst_tests::{burst_workspace, capture_round, configs, measured, DEVICE};
use crate::plan::{OffsetMeasure, OffsetRef, PartCalib, SegmentCalib, SegmentKind::{self, Corner, Straight}};
use crate::recipe_capture::CaptureRoundV2;
use progress::StepStatus;

/// 每台计划相机都选了示教帧（1000×700，采集时设备标签为 device）；样例配方的走廊落在帧内、有当量。
fn taught(kinds: &[SegmentKind], device: &str) -> Workspace {
    let mut w = burst_workspace(kinds);
    for camera in w.doc.plan_cameras() {
        w.camera_teaching.push(CameraTeaching { camera: camera.clone(), round: "capture-teach".into(), seq: 1, frame: FrozenImage {
            id: format!("corridor-{camera}"), view: 1, source: "capture:capture-teach#1".into(), captured_at: 1, size: [1000, 700],
            camera: camera.clone(), camera_tag: json!(device), calib_tag: Value::Null, geometry_tag: Value::Null,
            exposure_us: None, gain_db: None, history_id: None } });
    }
    w
}

/// 整件标定 ΣN_exp = total；单段计划同时写这一段的 N_exp（与应用标定的结果相同）。
fn calibrate(w: &mut Workspace, total: u32) {
    let calib = SegmentCalib { rounds: 10, min: total - 1, max: total + 1, mean: f64::from(total) };
    w.doc.part_calib = Some(PartCalib { expected_total: total, calib: calib.clone() });
    if let [segment] = w.doc.segments.as_mut_slice() { (segment.expected_frames, segment.calib) = (Some(total), Some(calib)); }
}

fn passing_sample() -> SampleResult {
    SampleResult { history_id: None, sample_id: Some("capture-sample".into()), name: "独立采集".into(), sn: String::new(),
        expected: Verdict::Ok, actual: Some(Verdict::Ok), passed: true, reason: String::new() }
}

/// 设备就绪、样本通过、基于当前生产版本，当前设备标签都是 DEVICE：只剩候选自身的验证项决定能否发布。
fn checks(w: &Workspace, cameras: &BTreeMap<String, Value>) -> Vec<Check> {
    burst_checks(w, BurstInputs { device: Ok(()), cameras, samples: &[passing_sample()], base_matches: true })
}

fn check<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("缺少验证项 {name}：{checks:?}"))
}

#[test]
fn validation_without_expected_total_blocks_publish_and_names_each_segment() {
    // 防的回归（规格 §1 未标定的计划不能发布；决策补充“多段配方的发布与运行”）：没有整件 ΣN_exp 也能通过验证去发布，
    // 或要求每段 N_exp 都标定、使多段计划在分段算法就绪前永远发不了；验证项不逐段列出待定原因
    // （名称，段类型，整件标定，能否发布，段标定详情里必须出现的文字）
    let cases: [(&str, &[SegmentKind], Option<u32>, bool, &[&str]); 3] = [
        ("三段都待标定", &[Straight, Corner, Straight], None, false, &["还差 10 轮", "第 1 段（直线）待标定", "第 2 段（弯道）待标定", "第 3 段（直线）待标定"]),
        ("三段只有整件 ΣN_exp", &[Straight, Corner, Straight], Some(30), true, &["整件预期 30 ± 3", "第 2 段（弯道）待分段算法"]),
        ("单段已标定", &[Straight], Some(20), true, &["整件预期 20 ± 1", "第 1 段（直线）N_exp 20 ± 1"]),
    ];
    for (name, kinds, total, publishable, texts) in cases {
        let mut w = taught(kinds, DEVICE);
        if let Some(total) = total { calibrate(&mut w, total); }
        // 单段直线段的 x_ref 已由标准件得出，不让它影响本用例
        if let [segment] = w.doc.segments.as_mut_slice() {
            segment.offset_ref = Some(OffsetRef { measure: OffsetMeasure::AbsoluteX, camera: "cam1".into(), x_ref: Some(0.1), spread: Some(0.05) });
        }
        let checks = checks(&w, &configs(&w.doc, DEVICE));
        let segment = check(&checks, "段标定");
        assert_eq!((segment.passed, checks.iter().all(|c| c.passed)), (publishable, publishable), "{name}：{checks:?}");
        for text in texts { assert!(segment.detail.contains(text), "{name}：段标定详情缺“{text}”：{}", segment.detail); }
    }
}

#[test]
fn publish_blocked_when_plan_cameras_not_three_or_corridor_missing() {
    // 防的回归（规格 §1 Plan.cameras = 3；§4 每台相机的检测走廊）：少一台计划相机、某台没选示教帧、走廊超出示教帧、
    // 缺像素当量，或示教帧采集后设备参数变了，仍能通过验证去发布
    type Edit = fn(&mut Workspace, &mut BTreeMap<String, Value>);
    let cases: [(&str, Edit, bool); 6] = [
        ("三台相机都示教好", |_, _| {}, true),
        ("计划相机只有 2 台", |w, _| { w.doc.inspect.truncate(2); w.camera_teaching.truncate(2); }, false),
        ("一台相机没选示教帧", |w, _| w.camera_teaching.retain(|t| t.camera != "cam2"), false),
        ("走廊超出示教帧", |w, _| w.doc.inspect[0].corridor[0] = [1200.0, 480.0], false),
        ("缺像素当量", |w, _| w.doc.inspect[1].mm_per_px = None, false),
        ("示教帧采集后设备参数改变", |_, current| { current.insert("cam3".into(), json!("device-b")); }, false),
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
fn validation_sample_with_frame_count_outside_tolerance_only_warns() {
    // 防的回归（规格 §2 帧数超出容差只记告警，该件照常检测）：帧组数超出 ΣN_exp ± Σtol 的独立样本被判不符合预期或 ERR，
    // 或超出时原因里不提示
    for (name, group_count, warned) in [("帧组数 20 在 20 ± 1 内", 20, false), ("帧组数 30 超出 20 ± 1", 30, true)] {
        let mut w = taught(&[Straight], DEVICE);
        calibrate(&mut w, 20);
        let round = capture_round("capture-sample", &w.doc, group_count, DEVICE, &[]);
        let metrics = measure_round(&w.doc.plan_cameras(), &round, |_, _, _| Ok(measured(0.0)));
        let judgement = judge_round(&w.doc, &round, &metrics);
        let sample = Sample { history_id: None, sample_id: Some(round.round_id.clone()), expected: Verdict::Ok };
        let result = sample_result(&sample, "独立采集".into(), String::new(), &judgement, w.doc.expected_total(), 1);
        assert_eq!((result.actual, result.passed), (Some(Verdict::Ok), true), "{name}：{result:?}");
        assert_eq!(result.reason.contains("超出预期 20 ± 1"), warned, "{name}：{}", result.reason);
    }
}

#[test]
fn sample_round_used_for_teaching_or_calibration_or_from_other_device_is_rejected() {
    // 防的回归（规格 §0 独立验证、D-SB-35）：用作走廊示教或多轮标定的同一轮又当独立验证样本（自己验自己），
    // 或换了相机 / 设备配置的轮次混进验证样本
    type Edit = fn(&mut Workspace, &mut CaptureRoundV2, &mut BTreeMap<String, Value>);
    let cases: [(&str, Edit, bool); 5] = [
        ("独立的新轮次", |_, _, _| {}, true),
        ("已用于走廊示教", |w, _, _| w.camera_teaching[1].round = "capture-sample".into(), false),
        ("已加入多轮标定", |w, round, _| w.calibration.rounds.push(CalibrationRound { round_id: round.round_id.clone(), standard: false, created_at: 1,
            cameras: w.doc.plan_cameras(), camera_config: vec![json!(DEVICE); 3], group_count: round.group_count, camera_frames: vec![20; 3],
            incomplete_groups: 0, loss_alarms: 0, corridors: vec![Value::Null; 3], x_means: vec![None; 3] }), false),
        ("采集后设备配置改变", |_, _, current| { current.insert("cam2".into(), json!("device-b")); }, false),
        ("缺一台计划相机、多一台别的相机", |_, round, _| round.cameras[2].camera_id = "cam9".into(), false),
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
fn progress_reports_calibration_blocker_before_validation() {
    // 防的回归（规格 §1 待标定状态在流程中可见）：走廊已示教、还没标定时，进度把“下一步”指向验证，或验证步骤的阻塞原因
    // 跳不到多轮标定；验证没通过时段标定、走廊示教这两项跳到验证页而不是对应页面
    type Edit = fn(&mut Workspace);
    let cases: [(&str, Edit, Vec<TargetV2>); 2] = [
        ("还没验证", |_| {}, vec![TargetV2::Calibration]),
        ("验证未通过：走廊示教、段标定", |w| {
            w.camera_teaching.retain(|t| t.camera != "cam2");
            w.validation = Some(Validation { revision: w.revision, passed: false, checked_at: 1, environment_tag: Value::Null, samples: Vec::new(), fixture: Vec::new(),
                checks: ["走廊示教", "段标定"].iter().map(|name| Check { name: (*name).into(), passed: false, detail: "未通过".into() }).collect() });
        }, vec![TargetV2::Corridor { camera: "cam2".into() }, TargetV2::Calibration]),
    ];
    for (name, edit, targets) in cases {
        let mut w = taught(&[Straight, Corner], DEVICE);
        edit(&mut w);
        let progress = progress_v2(&w, &camera_teaching_views(&w), 1, None);
        let step = |id: StepIdV2| progress.steps.iter().find(|s| s.id == id).unwrap();
        let calibration = step(StepIdV2::Calibration);
        assert_ne!(calibration.status, StepStatus::Done, "{name}");
        assert_eq!(calibration.blockers.first().map(|b| &b.target), Some(&TargetV2::Calibration), "{name}：{:?}", calibration.blockers);
        let validation = step(StepIdV2::Validation);
        assert_eq!(validation.blockers.iter().map(|b| b.target.clone()).filter(|t| *t != TargetV2::Capture).collect::<Vec<_>>(), targets, "{name}：{:?}", validation.blockers);
        assert!(progress.next.as_ref().is_some_and(|next| next.step != StepIdV2::Validation && next.step != StepIdV2::Publish), "{name}：{:?}", progress.next);
    }
}

#[test]
fn ready_burst_candidate_yields_debug_run_inputs_in_plan_camera_order() {
    // 防的回归（决策记录第九节第 13、14 条；规格 §6 离线仿真）：候选试运行不分配方类型、一律过旧拍照点的 Recipe::ready，
    // 走廊与整件 ΣN_exp 都已就绪的段配方候选被拒，“候选试运行一圈”对段配方必然报错；或示教帧尺寸不按计划相机顺序
    // 交给 Candidate::new_burst（它按 inspect 顺序逐台配对），走廊测量图套错相机的帧尺寸
    let mut w = taught(&[Straight, Corner, Straight], DEVICE);
    calibrate(&mut w, 30);
    w.instance_id = 1;
    for (teaching, width) in w.camera_teaching.iter_mut().zip([1000, 1100, 1200]) { teaching.frame.size = [width, 700]; }
    w.camera_teaching.reverse();
    let recipe = debug_recipe(&w).unwrap();
    assert!(recipe.is_burst());
    assert_eq!(burst_release::debug_sizes(&w, &configs(&w.doc, DEVICE)).unwrap(), [[1000, 700], [1100, 700], [1200, 700]]);
}

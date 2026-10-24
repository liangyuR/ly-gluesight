use super::*;
use super::burst::{add_calibration_round, apply_calibration, calibration_round, calibration_view, check_round, confirm_tolerance,
    measure_round, segment_round, update_segments, write_features, BOUNDARIES_FILE, FEATURES_FILE};
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::plan::{new_segment, OffsetMeasure, OffsetRef, PathSegment, SegmentCalib, SegmentKind::{self, Corner, Straight}};
use crate::recipe_capture::CaptureRoundV2;
use super::capture_review::{apply_capture_review, CaptureReviewEdit};

pub(super) const DEVICE: &str = "device-a";

/// 段配方候选：三台模拟相机 cam1–cam3、走廊已示教，段按 kinds 新建（N_exp 待标定）。
pub(super) fn burst_workspace(kinds: &[SegmentKind]) -> Workspace {
    let mut doc = crate::recipe::samples_burst();
    doc.segments = kinds.iter().enumerate().map(|(i, &kind)| new_segment(i as u32 + 1, kind, &doc.segment_defaults)).collect();
    doc.build().unwrap();
    Workspace::new(doc, None)
}

/// 一轮完整的段配方示教采集：计划相机都在，采集时设备配置为 config；missing 为 (本轮相机下标, seq) 的缺帧。
pub(super) fn capture_round(id: &str, doc: &RecipeDoc, group_count: u64, config: &str, missing: &[(usize, u64)]) -> CaptureRoundV2 {
    let mut missing = missing.to_vec();
    missing.sort_by_key(|&(camera, seq)| (seq, camera));
    let frames: Vec<u64> = (0..3).map(|camera| group_count - missing.iter().filter(|m| m.0 == camera).count() as u64).collect();
    let mut incomplete: Vec<u64> = missing.iter().map(|m| m.1).collect();
    incomplete.dedup();
    serde_json::from_value(json!({
        "format": 2, "roundId": id, "recipeId": doc.id,
        "cameras": doc.plan_cameras().iter().zip(&frames).map(|(camera, n)| json!({"cameraId": camera, "cameraConfig": config,
            "imaging": {"exposureUs": 60.0, "gainDb": 6.0}, "deviceSession": 1, "receivedCount": n})).collect::<Vec<_>>(),
        "state": "complete", "plcShotCount": group_count, "plcPlanVersion": 1, "deviceTriggers": null, "groupCount": group_count,
        "cameraFrames": frames, "incompleteGroups": incomplete,
        "missingFrames": missing.iter().map(|&(camera, seq)| json!({"camera": camera, "seq": seq})).collect::<Vec<_>>(),
        "lossAlarms": [], "alarms": [], "createdAt": 1, "endedAt": 2, "error": null, "failure": null, "simulated": true,
    })).unwrap()
}

pub(super) fn configs(doc: &RecipeDoc, config: &str) -> BTreeMap<String, Value> {
    doc.plan_cameras().into_iter().map(|camera| (camera, json!(config))).collect()
}

/// 测成的一帧，胶中心偏移 x（mm）；相机下标与 seq 由 measure_round 填。
pub(super) fn measured(x: f64) -> FrameMetrics {
    FrameMetrics { camera: 0, seq: 0, state: FrameState::Measured, error: None, width_min: Some(4.0), width_max: Some(4.0), width_mean: Some(4.0),
        widths: vec![Some(4.0)], spacing_mm: 1.0, gap_max_mm: 0.0, present_ratio: 1.0, x_mm: Some(x) }
}

/// 按加入标定的流程加一轮：核对、量每一帧（x 由 x_of(计划相机下标, seq) 给出）、记进候选。
fn add(w: &mut Workspace, id: &str, group_count: u64, standard: bool, x_of: impl Fn(usize, u64) -> f64) {
    let round = capture_round(id, &w.doc, group_count, DEVICE, &[]);
    check_round(&w.doc, &w.calibration.rounds, &round, &configs(&w.doc, DEVICE)).unwrap();
    let metrics = measure_round(&w.doc.plan_cameras(), &round, |camera, _, seq| Ok(measured(x_of(camera, seq))));
    let entry = calibration_round(&w.doc, &round, standard, &metrics);
    add_calibration_round(w, entry).unwrap();
}

#[test]
fn calibration_apply_writes_expected_frames_and_calib_but_leaves_tolerance_until_confirmed() {
    // 防的回归（规格 §2）：标定不写 N_exp / min / max / mean，或把建议容差直接写进配方；确认容差后才写入
    let mut w = burst_workspace(&[Straight]);
    for (i, n) in [20, 21, 20, 22, 20, 20, 21, 20, 20, 19].into_iter().enumerate() { add(&mut w, &format!("capture-{i}"), n, false, |_, _| 0.0); }
    let mut doc = w.doc.clone();
    apply_calibration(&mut doc, &w.calibration.rounds, &w.capture_review.rounds).unwrap();
    let calib = SegmentCalib { rounds: 10, min: 19, max: 22, mean: 20.3 };
    assert_eq!((doc.segments[0].expected_frames, doc.segments[0].calib.clone()), (Some(20), Some(calib.clone())));
    assert_eq!(doc.part_calib.as_ref().map(|p| (p.expected_total, p.calib.clone())), Some((20, calib)));
    assert_eq!(doc.segments[0].tolerance, 1, "建议容差不直接写进配方");
    w.doc = doc;
    let view = calibration_view(&w.doc, &w.calibration.rounds, &w.capture_review.rounds, None);
    assert_eq!(view.segments[0].outcome.as_ref().map(|o| o.suggested_tolerance), Some(2), "max(默认 1, 20 − 19, 22 − 20)");
    confirm_tolerance(&mut w, 1, 2).unwrap();
    assert_eq!(w.doc.segments[0].tolerance, 2);
}

#[test]
fn calibration_with_fewer_rounds_than_recipe_setting_is_rejected() {
    // 防的回归（规格 §2）：轮数少于配方设的标定轮数（可配）也能标定，或把标准件轮次算进轮数；失败时不能写一半
    for (name, normal, standard, accepted) in [("11 轮标定 + 3 轮标准件", 11, 3, false), ("12 轮标定", 12, 0, true)] {
        let mut w = burst_workspace(&[Straight]);
        w.doc.calibration_rounds = 12;
        for i in 0..normal { add(&mut w, &format!("capture-n{i}"), 20, false, |_, _| 0.0); }
        for i in 0..standard { add(&mut w, &format!("capture-s{i}"), 20, true, |_, _| 0.0); }
        let mut doc = w.doc.clone();
        let result = apply_calibration(&mut doc, &w.calibration.rounds, &w.capture_review.rounds);
        assert_eq!(result.is_ok(), accepted, "{name}：{result:?}");
        if accepted { assert_eq!(doc.expected_total(), Some(20), "{name}"); } else { assert_eq!(doc, w.doc, "{name}：配方不变"); }
    }
}

#[test]
fn multi_segment_calibration_uses_confirmed_manual_cuts_of_every_round_or_leaves_segments_pending() {
    // 防的回归（决策补充第二节、D-SB-21，轮次分段复核）：多段计划不看手动分段就把整件帧组数摊到各段；有标定轮次没确认分段（只是草稿）
    // 也逐段标定；逐段标定后 ΣN_exp 不等于各段之和；或多段计划拿不到整件 ΣN_exp 而无法发布
    let totals = [30, 31, 30, 30, 29, 30, 30, 31, 30, 30];
    for (name, confirm_all) in [("全部轮次已确认分段", true), ("最后一轮只保存了边界草稿", false)] {
        let mut w = burst_workspace(&[Straight, Corner, Straight]);
        for (i, n) in totals.into_iter().enumerate() {
            let id = format!("capture-{i}");
            let round = capture_round(&id, &w.doc, n, DEVICE, &[]);
            apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(vec![10, n - 8])).unwrap();
            if confirm_all || i + 1 < totals.len() { apply_capture_review(&mut w, &round, CaptureReviewEdit::Confirm).unwrap(); }
            add(&mut w, &id, n, false, |_, _| 0.0);
        }
        let mut doc = w.doc.clone();
        apply_calibration(&mut doc, &w.calibration.rounds, &w.capture_review.rounds).unwrap();
        assert_eq!(doc.part_calib.as_ref().map(|p| p.expected_total), Some(30), "{name}");
        let view = calibration_view(&doc, &w.calibration.rounds, &w.capture_review.rounds, None);
        if confirm_all {
            let frames: Vec<_> = doc.segments.iter().map(|s| (s.expected_frames, s.calib.as_ref().map(|c| (c.min, c.max)))).collect();
            assert_eq!(frames, [(Some(10), Some((10, 10))), (Some(12), Some((11, 13))), (Some(8), Some((8, 8)))], "{name}");
            assert_eq!(doc.expected_total(), Some(30), "{name}：ΣN_exp = 各段之和");
            assert_eq!(view.segments[1].outcome.as_ref().map(|o| o.suggested_tolerance), Some(1), "{name}：max(弯道默认, 12 − 11, 13 − 12)");
        } else {
            assert!(doc.segments.iter().all(|s| s.expected_frames.is_none() && s.calib.is_none()), "{name}：{:?}", doc.segments);
            assert_eq!(doc.expected_total(), Some(30), "{name}：凭整件均值仍可发布");
            assert!(view.segments.iter().all(|s| s.outcome.is_none() && s.pending.as_deref().is_some_and(|p| p.contains("capture-9"))), "{name}：{:?}", view.segments);
        }
    }
}

#[test]
fn standard_rounds_set_x_ref_for_straight_segment_and_refuse_corner() {
    // 防的回归（规格 §0/§1、D-SB-18）：x_ref 用了非标准件轮次、没去掉首尾帧、取错相机，ToEdge 也算出 x_ref，或弯道段得到 x_ref
    type Setup = fn(&mut RecipeDoc);
    // 标准件两轮：cam2 中间帧 0.2 / 0.4、首尾帧 5.0；cam1、cam3 恒 1.0。标定轮次恒 9.0（不能被用上）
    // （名称，段类型，配置，期望的 (x_ref, spread)，是否列出 x_ref 待定原因）
    let cases: [(&str, SegmentKind, Setup, Option<(f64, f64)>, bool); 4] = [
        ("直线段 AbsoluteX，参考相机 cam2", Straight,
            |d| d.segments[0].offset_ref = Some(OffsetRef { measure: OffsetMeasure::AbsoluteX, camera: "cam2".into(), x_ref: None, spread: None }), Some((0.3, 0.2)), false),
        ("直线段启用胶偏、未设参考 → 计划第一台相机", Straight, |_| {}, Some((1.0, 0.0)), false),
        ("直线段 ToEdge", Straight,
            |d| d.segments[0].offset_ref = Some(OffsetRef { measure: OffsetMeasure::ToEdge, camera: "cam2".into(), x_ref: None, spread: None }), None, true),
        ("弯道段", Corner, |_| {}, None, false),
    ];
    for (name, kind, setup, expected, pending) in cases {
        let mut w = burst_workspace(&[kind]);
        setup(&mut w.doc);
        for i in 0..10 { add(&mut w, &format!("capture-n{i}"), 20, false, |_, _| 9.0); }
        for (i, middle) in [0.2, 0.4].into_iter().enumerate() {
            add(&mut w, &format!("capture-s{i}"), 20, true, move |camera, seq| match (camera, seq) { (1, 1 | 20) => 5.0, (1, _) => middle, _ => 1.0 });
        }
        let mut doc = w.doc.clone();
        apply_calibration(&mut doc, &w.calibration.rounds, &w.capture_review.rounds).unwrap();
        let reference = doc.segments[0].offset_ref.as_ref().and_then(|r| r.x_ref.zip(r.spread));
        match (reference, expected) {
            (Some((x, spread)), Some((want_x, want_spread))) => assert!((x - want_x).abs() < 1e-9 && (spread - want_spread).abs() < 1e-9, "{name}：{x} ± {spread}"),
            (None, None) => {}
            other => panic!("{name}：{other:?}"),
        }
        if kind == Corner { assert!(doc.segments[0].offset_ref.is_none(), "{name}：弯道段没有胶偏参考"); }
        let view = calibration_view(&doc, &w.calibration.rounds, &w.capture_review.rounds, None);
        assert_eq!(view.segments[0].x_ref_pending.is_some(), pending, "{name}：{:?}", view.segments[0].x_ref_pending);
    }
}

#[test]
fn editing_expected_frames_directly_is_rejected_and_kind_change_clears_calibration() {
    // 防的回归（规格 §0 N_exp 不能手填、§1 段序列变化需重新标定、D-SB-18/20）：段列表或整份保存能手填 N_exp、标定统计、
    // 整件标定或 x_ref；段的类型序列变了旧标定还留着；只改容差也把标定清掉
    fn calibrated() -> Workspace {
        let mut w = burst_workspace(&[Straight]);
        for i in 0..10 { add(&mut w, &format!("capture-n{i}"), 20, false, |_, _| 0.0); }
        for i in 0..2 { add(&mut w, &format!("capture-s{i}"), 20, true, |_, _| 0.5); }
        let mut doc = w.doc.clone();
        apply_calibration(&mut doc, &w.calibration.rounds, &w.capture_review.rounds).unwrap();
        w.doc = doc;
        w
    }
    #[derive(Debug, PartialEq)]
    enum After { Rejected, Cleared, Kept, XRefCleared }
    type Entry = fn(&mut Workspace, RecipeDoc) -> Result<(), String>;
    let segments: Entry = |w, doc| update_segments(w, doc.segments);
    let whole: Entry = |w, doc| update_doc(w, doc);
    type Edit = fn(&mut RecipeDoc);
    let cases: [(&str, Entry, Edit, After); 8] = [
        ("段列表直接改 N_exp", segments, |d| d.segments[0].expected_frames = Some(25), After::Rejected),
        ("段列表直接改标定统计", segments, |d| d.segments[0].calib.as_mut().unwrap().max += 1, After::Rejected),
        ("段列表手填 x_ref", segments, |d| d.segments[0].offset_ref.as_mut().unwrap().x_ref = Some(1.5), After::Rejected),
        ("整份保存改整件标定", whole, |d| d.part_calib.as_mut().unwrap().expected_total = 25, After::Rejected),
        ("改成弯道段", segments, |d| {
            let checks = d.segment_defaults.corner.checks.clone();
            let segment = &mut d.segments[0];
            (segment.kind, segment.checks, segment.offset_ref) = (Corner, checks, None);
        }, After::Cleared),
        ("增加一段弯道", whole, |d| { let segment = new_segment(2, Corner, &d.segment_defaults); d.segments.push(segment); }, After::Cleared),
        ("只改帧数容差", segments, |d| d.segments[0].tolerance = 3, After::Kept),
        ("换胶偏参考相机", segments, |d| d.segments[0].offset_ref.as_mut().unwrap().camera = "cam3".into(), After::XRefCleared),
    ];
    for (name, entry, edit, after) in cases {
        let mut w = calibrated();
        let before = w.doc.clone();
        assert_eq!(before.segments[0].offset_ref.as_ref().and_then(|r| r.x_ref), Some(0.5), "前提：已标定 x_ref");
        let mut doc = w.doc.clone();
        edit(&mut doc);
        let result = entry(&mut w, doc);
        let segment: &PathSegment = &w.doc.segments[0];
        let x_ref = segment.offset_ref.as_ref().and_then(|r| r.x_ref);
        let state = match (result.is_ok(), segment.expected_frames, w.doc.part_calib.is_some(), x_ref) {
            (false, ..) => After::Rejected,
            (true, None, false, None) if w.doc.segments.iter().all(|s| s.calib.is_none()) => After::Cleared,
            (true, Some(20), true, Some(_)) => After::Kept,
            (true, Some(20), true, None) => After::XRefCleared,
            other => panic!("{name}：{other:?}"),
        };
        assert_eq!(state, after, "{name}：{result:?}");
        if after == After::Rejected { assert_eq!(w.doc, before, "{name}：拒绝时配方不变"); }
    }
}

#[test]
fn new_candidate_from_copied_burst_recipe_builds_without_its_calibration() {
    // 防的回归：新建候选清掉相机参数后，段配方过不了“检测相机要有曝光与增益”的校验，建不出候选；
    // 或复制别的段配方时把它的 N_exp、标定统计、整件标定与 x_ref 带进新候选（规格 §0 N_exp 只能由本配方的采集得出）
    let mut doc = crate::recipe::samples_burst();
    let calib = SegmentCalib { rounds: 10, min: 4, max: 4, mean: 4.0 };
    doc.part_calib = Some(crate::plan::PartCalib { expected_total: 4, calib: calib.clone() });
    doc.segments[0].calib = Some(calib);
    doc.segments[0].offset_ref = Some(OffsetRef { measure: OffsetMeasure::AbsoluteX, camera: "cam1".into(), x_ref: Some(0.2), spread: Some(0.1) });
    doc.build().unwrap();
    new_candidate_doc(&mut doc);
    doc.build().unwrap();
    let segment = &doc.segments[0];
    assert_eq!((segment.expected_frames, segment.calib.clone(), doc.part_calib.clone()), (None, None, None));
    assert_eq!(segment.offset_ref.as_ref().map(|r| (r.x_ref, r.spread)), Some((None, None)));
}

#[test]
fn calibration_round_writes_per_frame_features_and_boundaries_files() {
    // 防的回归（规格 §6 保存每帧特征与边界帧号，供分段算法）：缺帧或测量失败的帧不写进特征文件、顺序乱、相机写成下标，或分段结果没落盘
    let w = burst_workspace(&[Straight]);
    let cameras = w.doc.plan_cameras();
    // cam2 缺第 2 帧组；cam3 第 3 帧组测量失败
    let round = capture_round("capture-features", &w.doc, 3, DEVICE, &[(1, 2)]);
    let metrics = measure_round(&cameras, &round, |camera, _, seq| if (camera, seq) == (2, 3) { Err("lyFlow 运行失败".into()) } else { Ok(measured(seq as f64)) });
    let outcome = segment_round(&w.doc.segments, cameras.len(), round.group_count, &metrics);
    let dir = super::tests::ImportTestDir::new();
    write_features(&dir.0, &cameras, &metrics, &outcome).unwrap();
    let lines: Vec<Value> = std::fs::read_to_string(dir.0.join(FEATURES_FILE)).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let order: Vec<(String, u64)> = lines.iter().map(|l| (l["camera"].as_str().unwrap().to_string(), l["seq"].as_u64().unwrap())).collect();
    let expected: Vec<(String, u64)> = cameras.iter().flat_map(|c| (1..=3).map(move |seq| (c.clone(), seq))).collect();
    assert_eq!(order, expected, "每台相机每个帧组一行，按计划相机、帧组序号排列");
    let at = |camera: &str, seq: u64| lines.iter().find(|l| l["camera"] == camera && l["seq"] == seq).unwrap()["metrics"].clone();
    assert_eq!((at("cam1", 2)["state"].clone(), at("cam1", 2)["xMm"].clone()), (json!("measured"), json!(2.0)));
    assert_eq!((at("cam2", 2)["state"].clone(), at("cam2", 2)["xMm"].clone()), (json!("missing"), Value::Null));
    assert_eq!((at("cam3", 3)["state"].clone(), at("cam3", 3)["error"].clone()), (json!("error"), json!("lyFlow 运行失败")));
    let boundaries: Value = serde_json::from_str(&std::fs::read_to_string(dir.0.join(BOUNDARIES_FILE)).unwrap()).unwrap();
    assert_eq!(boundaries, json!({"kind": "unsegmented"}));
}

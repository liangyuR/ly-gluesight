use super::*;
use crate::plan::SegmentKind::{Corner, Straight};
use crate::workspace::burst_tests::{burst_workspace, capture_round, measured, DEVICE};

fn round(w: &Workspace, id: &str, count: u64) -> CaptureRoundV2 {
    capture_round(id, &w.doc, count, DEVICE, &[])
}

fn confirmed_template(w: &mut Workspace, round: &CaptureRoundV2, cuts: Vec<u64>) {
    w.teaching_piece = Some(crate::workspace::pieces_tests::piece(&round.round_id, 1));
    apply_capture_review(w, round, CaptureReviewEdit::Save(cuts)).unwrap();
    apply_capture_review(w, round, CaptureReviewEdit::Confirm).unwrap();
    apply_capture_review(w, round, CaptureReviewEdit::SetTemplate).unwrap();
}

#[test]
fn invalid_boundaries_leave_existing_review_unchanged() {
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let round = round(&w, "source", 10);
    apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(vec![3, 7])).unwrap();
    for cuts in [
        vec![],
        vec![3],
        vec![0, 7],
        vec![3, 10],
        vec![7, 3],
        vec![3, 3],
    ] {
        let before = state_value(&w);
        assert!(apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(cuts)).is_err());
        assert_eq!(state_value(&w), before);
    }
}

/// D-SB-44、RB-1：工作台一次保存——示教件边界已确认并成为模板；段的类型序列变化时其余已确认的件变“需复核”、
/// 各段 N_exp 清空，序列不变只改边界时其余件与标定不受影响；不是示教件的轮次不能保存为分段。
#[test]
fn template_round_save_confirms_template_and_invalidates_others_only_on_structure_change() {
    for structure_changed in [false, true] {
        let mut w = burst_workspace(&[Straight, Corner, Straight]);
        let template = round(&w, "template", 12);
        let other = round(&w, "other", 12);
        w.teaching_piece = Some(crate::workspace::pieces_tests::piece("template", 1));
        assert!(save_template_round(&w, &other, w.doc.segments.clone(), vec![4, 8]).is_err());
        apply_capture_review(&mut w, &other, CaptureReviewEdit::Save(vec![4, 8])).unwrap();
        apply_capture_review(&mut w, &other, CaptureReviewEdit::Confirm).unwrap();
        w.doc
            .segments
            .iter_mut()
            .for_each(|s| s.expected_frames = Some(4));
        let mut segments = w.doc.segments.clone();
        let cuts = if structure_changed {
            segments.push(crate::plan::new_segment(4, Corner, &w.doc.segment_defaults));
            vec![3, 6, 9]
        } else {
            vec![5, 8]
        };
        let next = save_template_round(&w, &template, segments, cuts.clone()).unwrap();
        let review = &next.capture_review.rounds["template"];
        assert_eq!(
            (&review.status, &review.cuts),
            (&ReviewStatus::Confirmed, &cuts),
            "{structure_changed}"
        );
        assert_eq!(
            next.capture_review
                .template
                .as_ref()
                .map(|t| t.round_id.as_str()),
            Some("template")
        );
        let other_status = &next.capture_review.rounds["other"].status;
        let expected = if structure_changed {
            ReviewStatus::Stale
        } else {
            ReviewStatus::Confirmed
        };
        assert_eq!(other_status, &expected, "{structure_changed}");
        assert_eq!(
            next.doc.segments[0].expected_frames.is_none(),
            structure_changed
        );
    }
}

#[test]
fn plan_change_then_revert_keeps_review_stale_and_template_invalid() {
    for product_changed in [false, true] {
        let mut w = burst_workspace(&[Straight, Corner, Straight]);
        let source = round(&w, "source", 10);
        confirmed_template(&mut w, &source, vec![3, 7]);
        let original = w.doc.clone();
        let mut changed = original.clone();
        if product_changed {
            changed.product_code += 1;
        } else {
            changed.segments[1].kind = Straight;
        }
        update_doc(&mut w, changed).unwrap();
        update_doc(&mut w, original).unwrap();
        assert_eq!(
            w.capture_review.rounds["source"].status,
            ReviewStatus::Stale
        );
        assert!(w.capture_review.template.is_none());
        assert!(apply_capture_review(&mut w, &source, CaptureReviewEdit::Confirm).is_err());
    }
}

/// 曝光增益只记录、不比较：旧版复核签名里带的曝光增益读入时去掉，已确认轮次仍按确认的边界给出各段帧组数。
#[test]
fn old_review_signature_with_exposure_or_gain_still_counts_as_confirmed() {
    let mut w = burst_workspace(&[Straight, Corner]);
    let source = round(&w, "source", 10);
    confirmed_template(&mut w, &source, vec![4]);
    let mut stored = serde_json::to_value(&w.capture_review).unwrap();
    let imaging = json!({"exposureUs": 60.0, "gainDb": 6.0});
    let cameras: serde_json::Map<String, Value> = w
        .doc
        .plan_cameras()
        .into_iter()
        .map(|c| (c, imaging.clone()))
        .collect();
    for path in ["/rounds/source", "/template"] {
        let signature = stored.pointer_mut(path).unwrap();
        signature["segmentSignature"]
            .as_array_mut()
            .unwrap()
            .push(json!(cameras));
        for camera in signature["captureSignature"].as_array_mut().unwrap() {
            camera.as_array_mut().unwrap().push(imaging.clone());
        }
    }
    w.capture_review = serde_json::from_value(stored).unwrap();
    assert_eq!(
        confirmed_segment_counts(&w.doc, &w.capture_review.rounds, "source", 10),
        Some(vec![4, 6])
    );
}

#[test]
fn exclusion_changes_image_statistics_without_compressing_counts_and_restore_remeasures() {
    let mut w = burst_workspace(&[Straight]);
    let source = round(&w, "source", 5);
    let measure = |_: usize, _: &str, seq: u64| Ok(measured(if seq == 3 { 100.0 } else { 1.0 }));
    let metrics = measure_calibration_round(&w, &source, measure);
    let entry = burst::calibration_round(&w.doc, &source, true, &metrics);
    assert_eq!(entry.x_means[0], Some(34.0));
    burst::add_calibration_round(&mut w, entry).unwrap();
    w.doc.segments[0].expected_frames = Some(5);
    apply_capture_review(
        &mut w,
        &source,
        CaptureReviewEdit::Exclude {
            start: 3,
            end: 3,
            excluded: true,
            reason: "反光".into(),
        },
    )
    .unwrap();
    assert!(w.calibration.rounds.is_empty());
    assert!(w.doc.segments[0].expected_frames.is_none());
    let metrics = measure_calibration_round(&w, &source, measure);
    let entry = burst::calibration_round(&w.doc, &source, true, &metrics);
    assert_eq!(entry.x_means[0], Some(1.0));
    assert_eq!(entry.group_count, 5);
    assert_eq!(
        metrics
            .iter()
            .filter(|m| m.camera == 0)
            .map(|m| m.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    apply_capture_review(
        &mut w,
        &source,
        CaptureReviewEdit::Exclude {
            start: 3,
            end: 3,
            excluded: false,
            reason: String::new(),
        },
    )
    .unwrap();
    let metrics = measure_calibration_round(&w, &source, measure);
    assert_eq!(
        burst::calibration_round(&w.doc, &source, true, &metrics).x_means[0],
        Some(34.0)
    );
}

#[test]
fn corridor_teaching_and_exclusion_cannot_reference_the_same_group() {
    let mut w = burst_workspace(&[Straight]);
    let source = round(&w, "source", 5);
    let camera = w.doc.plan_cameras()[0].clone();
    w.teaching_piece = Some(crate::workspace::pieces_tests::piece("source", 1));
    let teaching: burst::CameraTeaching = serde_json::from_value(json!({"camera":camera,"round":"source","seq":3,"segment":1,
        "frame":{"id":"image","view":1,"source":"capture:source#3","capturedAt":0,"size":[100,100],
            "camera":camera,"cameraTag":null,"calibTag":null,"geometryTag":null,"exposureUs":null,"gainDb":null,"historyId":null}})).unwrap();
    burst::apply_segment_teaching(&mut w, teaching.clone(), 5, None).unwrap();
    let exclude = || CaptureReviewEdit::Exclude {
        start: 3,
        end: 3,
        excluded: true,
        reason: "反光".into(),
    };
    assert!(apply_capture_review(&mut w, &source, exclude()).is_err());
    w.camera_teaching.clear();
    apply_capture_review(&mut w, &source, exclude()).unwrap();
    assert!(burst::apply_segment_teaching(&mut w, teaching, 5, None).is_err());
}

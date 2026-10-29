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

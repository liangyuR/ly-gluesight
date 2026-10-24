use super::*;
use crate::plan::SegmentKind::{Corner, Straight};
use crate::workspace::burst_tests::{burst_workspace, capture_round, measured, DEVICE};

fn round(w: &Workspace, id: &str, count: u64) -> CaptureRoundV2 {
    let mut round = capture_round(id, &w.doc, count, DEVICE, &[]);
    for camera in &mut round.cameras { camera.imaging = w.doc.cameras[&camera.camera_id]; }
    round
}

fn confirmed_template(w: &mut Workspace, round: &CaptureRoundV2, cuts: Vec<u64>) {
    apply_capture_review(w, round, CaptureReviewEdit::Save(cuts)).unwrap();
    apply_capture_review(w, round, CaptureReviewEdit::Confirm).unwrap();
    apply_capture_review(w, round, CaptureReviewEdit::SetTemplate).unwrap();
}

#[test]
fn invalid_boundaries_leave_existing_review_unchanged() {
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let round = round(&w, "source", 10);
    apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(vec![3, 7])).unwrap();
    for cuts in [vec![], vec![3], vec![0, 7], vec![3, 10], vec![7, 3], vec![3, 3]] {
        let before = state_value(&w);
        assert!(apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(cuts)).is_err());
        assert_eq!(state_value(&w), before);
    }
}

#[test]
fn template_requires_confirmation_and_application_preserves_exclusions_as_draft() {
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let source = round(&w, "source", 10);
    let target = round(&w, "target", 20);
    apply_capture_review(&mut w, &source, CaptureReviewEdit::Save(vec![3, 7])).unwrap();
    assert!(apply_capture_review(&mut w, &source, CaptureReviewEdit::SetTemplate).is_err());
    confirmed_template(&mut w, &source, vec![3, 7]);
    apply_capture_review(&mut w, &target, CaptureReviewEdit::Exclude { start: 5, end: 6, excluded: true, reason: "反光".into() }).unwrap();
    for (mode, cuts) in [(TemplateMode::Proportional, vec![6, 14]), (TemplateMode::Original, vec![3, 7])] {
        apply_capture_review(&mut w, &target, CaptureReviewEdit::ApplyTemplate(mode)).unwrap();
        let review = &w.capture_review.rounds["target"];
        assert_eq!(review.status, ReviewStatus::Draft);
        assert_eq!(review.cuts, cuts);
        assert_eq!(review.excluded_groups.iter().map(|g| g.seq).collect::<Vec<_>>(), vec![5, 6]);
        assert_eq!(review.group_count, 20);
    }
    apply_capture_review(&mut w, &source, CaptureReviewEdit::Save(vec![4, 7])).unwrap();
    assert!(w.capture_review.template.is_none());
}

#[test]
fn plan_change_then_revert_keeps_review_stale_and_template_invalid() {
    for product_changed in [false, true] {
        let mut w = burst_workspace(&[Straight, Corner, Straight]);
        let source = round(&w, "source", 10);
        confirmed_template(&mut w, &source, vec![3, 7]);
        let original = w.doc.clone();
        let mut changed = original.clone();
        if product_changed { changed.product_code += 1; }
        else { changed.segments[1].kind = Straight; }
        update_doc(&mut w, changed).unwrap();
        update_doc(&mut w, original).unwrap();
        assert_eq!(w.capture_review.rounds["source"].status, ReviewStatus::Stale);
        assert!(w.capture_review.template.is_none());
        assert!(apply_capture_review(&mut w, &source, CaptureReviewEdit::Confirm).is_err());
    }
}

#[test]
fn mismatched_capture_configuration_rejects_template_without_changing_target() {
    let mut w = burst_workspace(&[Straight, Corner]);
    let source = round(&w, "source", 10);
    confirmed_template(&mut w, &source, vec![4]);
    for imaging in [false, true] {
        let mut target = round(&w, "target", 10);
        if imaging { target.cameras[0].imaging.gain_db += 1.0; }
        else { target.cameras[0].camera_config = json!("another-device"); }
        let before = state_value(&w);
        assert!(apply_capture_review(&mut w, &target, CaptureReviewEdit::ApplyTemplate(TemplateMode::Original)).is_err());
        assert_eq!(state_value(&w), before);
    }
}

#[test]
fn reordered_capture_cameras_reuse_the_same_template() {
    let mut w = burst_workspace(&[Straight, Corner]);
    let source = round(&w, "source", 10);
    confirmed_template(&mut w, &source, vec![4]);
    let mut target = round(&w, "target", 10);
    target.cameras.reverse();
    apply_capture_review(&mut w, &target, CaptureReviewEdit::ApplyTemplate(TemplateMode::Original)).unwrap();
    assert_eq!(w.capture_review.rounds["target"].cuts, vec![4]);
}

#[test]
fn removing_an_applied_calibration_source_clears_derived_values_before_exclusion() {
    let mut w = burst_workspace(&[Straight]);
    let source = round(&w, "source", 5);
    let metrics = measure_calibration_round(&w, &source, |_, _, _| Ok(measured(1.0)));
    let entry = burst::calibration_round(&w.doc, &source, false, &metrics);
    burst::add_calibration_round(&mut w, entry).unwrap();
    w.doc.segments[0].expected_frames = Some(5);
    w.doc.segments[0].offset_ref = Some(crate::plan::OffsetRef {
        measure: crate::plan::OffsetMeasure::AbsoluteX, camera: w.doc.plan_cameras()[0].clone(), x_ref: Some(1.0), spread: Some(0.1),
    });
    burst::remove_calibration_round(&mut w, "source").unwrap();
    apply_capture_review(&mut w, &source, CaptureReviewEdit::Exclude { start: 3, end: 3, excluded: true, reason: "反光".into() }).unwrap();
    assert!(w.doc.segments[0].expected_frames.is_none());
    assert!(w.doc.segments[0].offset_ref.as_ref().unwrap().x_ref.is_none());
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
    apply_capture_review(&mut w, &source, CaptureReviewEdit::Exclude { start: 3, end: 3, excluded: true, reason: "反光".into() }).unwrap();
    assert!(w.calibration.rounds.is_empty());
    assert!(w.doc.segments[0].expected_frames.is_none());
    let metrics = measure_calibration_round(&w, &source, measure);
    let entry = burst::calibration_round(&w.doc, &source, true, &metrics);
    assert_eq!(entry.x_means[0], Some(1.0));
    assert_eq!(entry.group_count, 5);
    assert_eq!(metrics.iter().filter(|m| m.camera == 0).map(|m| m.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    apply_capture_review(&mut w, &source, CaptureReviewEdit::Exclude { start: 3, end: 3, excluded: false, reason: String::new() }).unwrap();
    let metrics = measure_calibration_round(&w, &source, measure);
    assert_eq!(burst::calibration_round(&w.doc, &source, true, &metrics).x_means[0], Some(34.0));
}

#[test]
fn corridor_teaching_and_exclusion_cannot_reference_the_same_group() {
    let mut w = burst_workspace(&[Straight]);
    let source = round(&w, "source", 5);
    let camera = w.doc.plan_cameras()[0].clone();
    let teaching: burst::CameraTeaching = serde_json::from_value(json!({"camera":camera,"round":"source","seq":3,
        "frame":{"id":"image","view":1,"source":"capture:source#3","capturedAt":0,"size":[100,100],
            "camera":camera,"cameraTag":null,"calibTag":null,"geometryTag":null,"exposureUs":null,"gainDb":null,"historyId":null}})).unwrap();
    let draft = || burst::CameraTeachDraft { corridor: vec![[1.0, 1.0], [50.0, 1.0]], mm_per_px: Some(0.1), detect: None };
    burst::apply_camera_teaching(&mut w, teaching.clone(), draft(), None).unwrap();
    let exclude = || CaptureReviewEdit::Exclude { start: 3, end: 3, excluded: true, reason: "反光".into() };
    assert!(apply_capture_review(&mut w, &source, exclude()).is_err());
    w.camera_teaching.clear();
    apply_capture_review(&mut w, &source, exclude()).unwrap();
    assert!(burst::apply_camera_teaching(&mut w, teaching, draft(), None).is_err());
}

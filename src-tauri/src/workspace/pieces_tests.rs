use super::burst::{calibration_round, measure_round, CameraTeaching};
use super::burst_tests::{burst_workspace, capture_round, configs, measured, DEVICE};
use super::capture_review::{apply_capture_review, CaptureReviewEdit};
use super::pieces::{
    measured_with_current_corridors, replace_teaching_piece, sync_pieces, TeachingPiece,
};
use super::*;
use crate::frame_align::{AlignedCut, ReviewReason};
use crate::plan::{
    OffsetMeasure, OffsetRef, PartCalib, SegmentCalib,
    SegmentKind::{Corner, Straight},
};
use crate::recipe::SegmentCorridor;
use crate::recipe_capture::CaptureRoundV2;

const GROUPS: u64 = 30;

pub(super) fn piece(round: &str, record: i64) -> TeachingPiece {
    TeachingPiece {
        round_id: round.into(),
        record_id: record,
        cycle_id: format!("cycle-{record}"),
        sn: format!("SN-{record}"),
        ts: record,
    }
}

/// 直线 / 弯道两段的候选，示教件 "teach"（边界 15 已确认）；`taught` 时每段每台相机都在示教件上选了示教帧、画了走廊。
fn with_teaching(taught: bool) -> (Workspace, CaptureRoundV2) {
    let mut w = burst_workspace(&[Straight, Corner]);
    let teach = capture_round("teach", &w.doc, GROUPS, DEVICE, &[]);
    w.teaching_piece = Some(piece("teach", 1));
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Save(vec![15])).unwrap();
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Confirm).unwrap();
    if taught {
        let corridor = crate::recipe::samples_burst().inspect[0].segments[0]
            .corridor
            .clone();
        for camera in w.doc.plan_cameras() {
            for segment in [1u32, 2] {
                w.camera_teaching.push(serde_json::from_value::<CameraTeaching>(json!({"camera": camera, "round": "teach",
                    "seq": u64::from(segment) * 15 - 5, "segment": segment,
                    "frame": {"id": format!("{camera}-{segment}"), "view": 1, "source": "", "capturedAt": 0, "size": [1000, 700],
                        "camera": camera, "cameraTag": null, "calibTag": null, "geometryTag": null, "exposureUs": null,
                        "gainDb": null, "historyId": null}})).unwrap());
            }
        }
        for inspect in &mut w.doc.inspect {
            inspect.segments = [1, 2]
                .map(|segment| SegmentCorridor {
                    segment,
                    corridor: corridor.clone(),
                })
                .to_vec();
        }
        w.doc.build().unwrap();
    }
    (w, teach)
}

fn aligned(cut: u64, reason: Option<ReviewReason>) -> AlignedCut {
    AlignedCut {
        cut,
        proportional: cut,
        sharpness: 2.0,
        reason,
        review: reason.is_some(),
    }
}

#[test]
fn replacing_teaching_piece_clears_image_derived_content_and_keeps_segment_structure() {
    // 防的回归（RB-2、RB-8）：换示教件后原示教件的边界、其余件的已确认边界、走廊与示教帧、标定件或 N_exp / 标定统计 /
    // partCalib / x_ref 还留着（新示教件图像与旧走廊、旧标定混用）；或把段的数量、类型、检测相机、容差等结构与配方级参数也清掉；
    // 同一件重复设置时照样清空
    let (mut w, _) = with_teaching(true);
    let other = capture_round("other", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Save(vec![14])).unwrap();
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Confirm).unwrap();
    let metrics = measure_round(&w.doc.plan_cameras(), &other, |_, _, _| Ok(measured(0.0)));
    w.calibration
        .rounds
        .push(calibration_round(&w.doc, &other, false, &metrics));
    let calib = SegmentCalib {
        rounds: 10,
        min: 14,
        max: 16,
        mean: 15.0,
    };
    w.doc.part_calib = Some(PartCalib {
        expected_total: 30,
        calib: calib.clone(),
    });
    for segment in &mut w.doc.segments {
        (segment.expected_frames, segment.calib, segment.tolerance) =
            (Some(15), Some(calib.clone()), 3);
    }
    w.doc.segments[0].offset_ref = Some(OffsetRef {
        measure: OffsetMeasure::AbsoluteX,
        camera: "cam1".into(),
        x_ref: Some(0.4),
        spread: Some(0.1),
        edge: None,
    });
    w.doc.calibration_rounds = 12;

    let before = w.clone();
    assert!(
        replace_teaching_piece(&mut w, piece("teach-again", 1)).is_err(),
        "同一件"
    );
    assert_eq!(
        state_value(&w),
        state_value(&before),
        "同一件被拒时候选不变"
    );

    replace_teaching_piece(&mut w, piece("new", 2)).unwrap();
    assert_eq!(w.teaching_piece, Some(piece("new", 2)));
    assert!(
        w.camera_teaching.is_empty() && w.doc.inspect.iter().all(|i| i.segments.is_empty()),
        "走廊与示教帧"
    );
    assert!(
        w.capture_review.rounds.is_empty() && w.capture_review.template.is_none(),
        "全部边界复核与模板"
    );
    assert!(
        w.calibration.rounds.is_empty() && w.doc.part_calib.is_none(),
        "标定件与整件标定"
    );
    assert!(
        w.doc
            .segments
            .iter()
            .all(|s| s.expected_frames.is_none() && s.calib.is_none()),
        "N_exp 与标定统计"
    );
    assert_eq!(
        w.doc.segments[0]
            .offset_ref
            .as_ref()
            .map(|r| (r.x_ref, r.spread)),
        Some((None, None)),
        "x_ref"
    );
    let structure = |doc: &RecipeDoc| {
        doc.segments
            .iter()
            .map(|s| {
                (
                    s.index,
                    s.kind,
                    s.tolerance,
                    s.cameras.clone(),
                    state_value(&s.checks),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(structure(&w.doc), structure(&before.doc), "段结构保留");
    assert_eq!(
        (w.doc.calibration_rounds, w.doc.plan_cameras()),
        (12, before.doc.plan_cameras()),
        "配方级参数保留"
    );
    assert!(w.revision > before.revision);
}

#[test]
fn sync_remeasures_pieces_whose_corridor_identity_changed_after_joining() {
    // 防的回归（PR #205 评审）：加入标定后改了当量、走廊或检测参数，同步仍跳过已加入的件，特征、起涂建议与 x_ref 沿用旧测量；
    // 弃用会删原图、示教件又不能弃用，已没有别的办法刷新
    let (w, teach) = with_teaching(true);
    let other = capture_round("other", &w.doc, GROUPS, DEVICE, &[]);
    let rounds = [teach, other];
    let sync = |w: &Workspace, x: f64| {
        sync_pieces(
            w,
            &rounds,
            &configs(&w.doc, DEVICE),
            |_| Ok(vec![aligned(16, None)]),
            |w, round| {
                Ok(measure_round(&w.doc.plan_cameras(), round, |_, _, _| {
                    Ok(measured(x))
                }))
            },
        )
    };
    let joined = sync(&w, 0.0).next;
    assert_eq!(joined.calibration.rounds.len(), 2, "前提：两件都已加入");
    let unchanged = sync(&joined, 1.0);
    assert!(unchanged.features.is_empty(), "测量输入没变时不重测");
    let mut changed = joined.clone();
    let scale = changed.doc.inspect[0].mm_per_px.unwrap_or(0.1);
    changed.doc.inspect[0].mm_per_px = Some(scale * 2.0);
    let outcome = sync(&changed, 1.0);
    let remeasured: Vec<_> = outcome
        .features
        .iter()
        .map(|(id, ..)| id.as_str())
        .collect();
    assert_eq!(remeasured, ["teach", "other"]);
    assert!(outcome
        .next
        .calibration
        .rounds
        .iter()
        .all(|r| measured_with_current_corridors(&outcome.next.doc, r)));
}

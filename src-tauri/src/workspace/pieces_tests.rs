use super::burst::{calibration_round, measure_round, CameraTeaching};
use super::burst_tests::{burst_workspace, capture_round, configs, measured, DEVICE};
use super::capture_review::{apply_capture_review, CaptureReviewEdit};
use super::pieces::{measure_teaching, replace_teaching_piece, teaching_measured, TeachingPiece};
use super::*;
use crate::plan::{
    OffsetMeasure, OffsetRef, PartCalib, SegmentCalib,
    SegmentKind::{self, Corner, Straight},
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

/// 按 kinds 建段的候选，示教件 "teach"（30 个帧组，每段 15 帧的边界已确认）；`taught` 时每段每台相机都在示教件上选了示教帧、画了走廊。
fn with_teaching(kinds: &[SegmentKind], taught: bool) -> (Workspace, CaptureRoundV2) {
    let mut w = burst_workspace(kinds);
    let groups = 15 * kinds.len() as u64;
    let teach = capture_round("teach", &w.doc, groups, DEVICE, &[]);
    w.teaching_piece = Some(piece("teach", 1));
    let cuts = (1..kinds.len() as u64).map(|k| 15 * k).collect();
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Save(cuts)).unwrap();
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Confirm).unwrap();
    if taught {
        let corridor = crate::recipe::samples_burst().inspect[0].segments[0]
            .corridor
            .clone();
        let segments: Vec<u32> = (1..=kinds.len() as u32).collect();
        for camera in w.doc.plan_cameras() {
            for &segment in &segments {
                w.camera_teaching.push(serde_json::from_value::<CameraTeaching>(json!({"camera": camera, "round": "teach",
                    "seq": u64::from(segment) * 15 - 5, "segment": segment,
                    "frame": {"id": format!("{camera}-{segment}"), "view": 1, "source": "", "capturedAt": 0, "size": [1000, 700],
                        "camera": camera, "cameraTag": null, "calibTag": null, "geometryTag": null, "exposureUs": null,
                        "gainDb": null, "historyId": null}})).unwrap());
            }
        }
        for inspect in &mut w.doc.inspect {
            inspect.segments = segments
                .iter()
                .map(|&segment| SegmentCorridor {
                    segment,
                    corridor: corridor.clone(),
                    sector_deg: None,
                    color_model: None,
                })
                .collect();
        }
        w.doc.build().unwrap();
    }
    (w, teach)
}

/// 按 kinds 建段、走廊已示教的候选，示教件每帧胶中心 x 都是 `x`，已测量。
pub(super) fn measured_teaching(kinds: &[SegmentKind], x: f64) -> Workspace {
    let (w, teach) = with_teaching(kinds, true);
    measure_with(&w, &teach, x).unwrap()
}

fn measure_with(w: &Workspace, teach: &CaptureRoundV2, x: f64) -> Result<Workspace, String> {
    measure_teaching(w, teach, &configs(&w.doc, DEVICE), |w, round| {
        Ok(measure_round(&w.doc.plan_cameras(), round, |_, _, _| {
            Ok(measured(x))
        }))
    })
    .map(|outcome| outcome.next)
}

#[test]
fn replacing_teaching_piece_keeps_corridors_and_clears_only_piece_derived_counts() {
    // 防的回归（RB-2、RB-8）：换示教件把走廊与示教帧全部清空，示教要从头重做；或原示教件的边界、其余件的已确认边界、
    // 示教件测量、N_exp / 标定统计 / partCalib / x_ref 还留着（新示教件的帧组与旧边界、旧测量混用）；或把段的数量、类型、
    // 检测相机、容差等结构与配方级参数也清掉；同一件重复设置时照样改动
    let (mut w, _) = with_teaching(&[Straight, Corner], true);
    let other = capture_round("other", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Save(vec![14])).unwrap();
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Confirm).unwrap();
    let metrics = measure_round(&w.doc.plan_cameras(), &other, |_, _, _| Ok(measured(0.0)));
    w.calibration
        .rounds
        .push(calibration_round(&w.doc, &other, &metrics));
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
    w.doc.boundary_guard = 3;

    let before = w.clone();
    assert!(
        replace_teaching_piece(&mut w, piece("teach-again", 1), GROUPS).is_err(),
        "同一件"
    );
    assert_eq!(
        state_value(&w),
        state_value(&before),
        "同一件被拒时候选不变"
    );

    // 新件 60 个帧组：不按帧组数换算，沿用原示教件的边界帧号（15）作为新件的边界草稿
    let carried = replace_teaching_piece(&mut w, piece("new", 2), 2 * GROUPS).unwrap();
    assert_eq!(carried, Some(vec![15]), "沿用原来的分段帧数");
    assert_eq!(w.teaching_piece, Some(piece("new", 2)));
    assert!(!before.camera_teaching.is_empty(), "前提：换件前已示教走廊");
    assert_eq!(
        w.camera_teaching.len(),
        before.camera_teaching.len(),
        "示教帧保留"
    );
    for (after, old) in w.camera_teaching.iter().zip(&before.camera_teaching) {
        assert_eq!(
            (
                state_value(&after.frame),
                &after.camera,
                after.segment,
                after.seq
            ),
            (state_value(&old.frame), &old.camera, old.segment, old.seq),
            "冻结的示教图、所属段与帧组号不变"
        );
        // 改指向新示教件：导出回传包、走廊试测与分段对应都按新件的轮次，旧轮次随后删除
        assert_eq!(after.round, "new");
    }
    assert_eq!(
        w.doc
            .inspect
            .iter()
            .map(|i| state_value(&i.segments))
            .collect::<Vec<_>>(),
        before
            .doc
            .inspect
            .iter()
            .map(|i| state_value(&i.segments))
            .collect::<Vec<_>>(),
        "走廊保留"
    );
    assert!(
        w.capture_review.rounds.is_empty() && w.capture_review.template.is_none(),
        "全部边界复核与模板"
    );
    assert!(
        w.calibration.rounds.is_empty() && w.doc.part_calib.is_none(),
        "示教件测量与整件标定"
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
        (w.doc.boundary_guard, w.doc.plan_cameras()),
        (3, before.doc.plan_cameras()),
        "配方级参数保留"
    );
    assert!(w.revision > before.revision);
}

#[test]
fn measuring_teaching_writes_x_ref_and_a_corridor_change_invalidates_it() {
    // 防的回归（RB-3，示教件兼作标准件；PR #205 评审）：测量示教件后单段配方的 x_ref 没写进配方；分段未确认或走廊没示教完也能测；
    // 测量后改了当量、走廊或检测参数，旧测量与 x_ref 还当作有效（发布时按过期的参考判胶偏）
    let (untaught, teach) = with_teaching(&[Straight], false);
    assert!(
        measure_with(&untaught, &teach, 0.5).is_err(),
        "走廊没示教完"
    );
    let w = measured_teaching(&[Straight], 0.5);
    assert!(teaching_measured(&w));
    let reference = w.doc.segments[0]
        .offset_ref
        .as_ref()
        .map(|r| (r.x_ref, r.spread));
    assert_eq!(reference, Some((Some(0.5), Some(0.0))));
    assert_eq!(w.doc.segments[0].expected_frames, Some(15));

    let mut changed = w.clone();
    let mut doc = changed.doc.clone();
    let scale = doc.inspect[0].mm_per_px.unwrap_or(0.1);
    doc.inspect[0].mm_per_px = Some(scale * 2.0);
    update_doc(&mut changed, doc).unwrap();
    assert!(!teaching_measured(&changed));
    assert_eq!(
        changed.doc.segments[0]
            .offset_ref
            .as_ref()
            .and_then(|r| r.x_ref),
        None
    );
    assert_eq!(
        changed.doc.segments[0].expected_frames,
        Some(15),
        "N_exp 不受走廊影响"
    );
}

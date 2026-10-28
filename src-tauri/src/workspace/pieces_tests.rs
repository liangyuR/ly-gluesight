use super::burst::{calibration_round, measure_round, CameraTeaching};
use super::burst_tests::{burst_workspace, capture_round, configs, measured, DEVICE};
use super::capture_review::{apply_capture_review, CaptureReviewEdit, ConfirmedBy, ReviewStatus};
use super::pieces::{
    discard_piece, measured_with_current_corridors, replace_teaching_piece, sync_pieces,
    TeachingPiece, LEGACY,
};
use super::progress::StepStatus;
use super::*;
use crate::frame_align::{AlignedCut, ReviewReason};
use crate::plan::{
    OffsetMeasure, OffsetRef, PartCalib, SegmentCalib,
    SegmentKind::{Corner, Straight},
};
use crate::recipe::SegmentCorridor;
use crate::recipe_capture::{CaptureRoundV2, RecipeCaptureHost};

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
fn teaching_piece_cannot_leave_calibration_alone_but_a_calibration_piece_is_discarded_with_its_results(
) {
    // 防的回归（RB-3）：示教件被单独移出标定（之后标定件不含示教件，示教件与标定口径分家）；弃用标定件后它的复核记录还在、
    // 或已应用的标定结果仍按含这一件的轮次保留
    let (mut w, teach) = with_teaching(true);
    let other = capture_round("other", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Save(vec![14])).unwrap();
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Confirm).unwrap();
    for round in [&teach, &other] {
        let metrics = measure_round(&w.doc.plan_cameras(), round, |_, _, _| Ok(measured(0.0)));
        w.calibration
            .rounds
            .push(calibration_round(&w.doc, round, false, &metrics));
    }
    w.doc.segments[0].expected_frames = Some(15);
    let before = state_value(&w);
    assert!(discard_piece(&mut w, "teach").is_err());
    assert_eq!(state_value(&w), before, "拒绝时候选不变");
    discard_piece(&mut w, "other").unwrap();
    assert_eq!(
        w.calibration
            .rounds
            .iter()
            .map(|r| r.round_id.as_str())
            .collect::<Vec<_>>(),
        ["teach"]
    );
    assert!(!w.capture_review.rounds.contains_key("other"));
    assert!(w.doc.segments[0].expected_frames.is_none());
}

#[test]
fn legacy_candidate_without_flow_rejects_every_edit() {
    // 防的回归（RB-5）：没有 flow 的旧流程候选还能打开编辑或发布；或新建的候选也被当成旧候选
    let w = burst_workspace(&[Straight]);
    assert_eq!(w.expect(w.revision), Ok(()), "新建候选");
    let mut stored = serde_json::to_value(&w).unwrap();
    stored.as_object_mut().unwrap().remove("flow");
    let legacy: Workspace = serde_json::from_value(stored).unwrap();
    assert_eq!(legacy.expect(legacy.revision), Err(LEGACY.to_string()));
}

#[test]
fn deleting_a_candidate_removes_only_its_rounds_and_legacy_delete_keeps_published_archive() {
    // 防的回归（RB-5）：删除候选后它导入的轮次还在（同编号新候选把它们当成标定件），或把别的配方的轮次一起删掉；
    // 删除旧流程候选连发布归档一起删掉，生产配方加载不到发布包
    let root = super::tests::ImportTestDir::new();
    let rounds = root.0.join("recipe-capture");
    let capture = RecipeCaptureHost::open(rounds.clone()).unwrap();
    for (round, recipe) in [
        ("capture-1-0", "A"),
        ("capture-2-0", "A"),
        ("capture-3-0", "B"),
    ] {
        std::fs::create_dir_all(rounds.join(round)).unwrap();
        std::fs::write(
            rounds.join(round).join("round.json"),
            json!({"format": 3, "recipeId": recipe}).to_string(),
        )
        .unwrap();
    }
    let mut deleted = capture.delete_recipe_rounds("A").unwrap();
    deleted.sort();
    assert_eq!(deleted, ["capture-1-0", "capture-2-0"]);
    assert!(rounds.join("capture-3-0").is_dir() && !rounds.join("capture-1-0").exists());

    let candidate = root.0.join("workspaces").join("A");
    std::fs::create_dir_all(candidate.join("published")).unwrap();
    std::fs::create_dir_all(candidate.join("images")).unwrap();
    std::fs::write(candidate.join("published").join("A-v1.json"), "{}").unwrap();
    std::fs::write(candidate.join("workspace.json"), "{}").unwrap();
    super::pieces::remove_legacy_dir(&candidate).unwrap();
    assert!(candidate.join("published").join("A-v1.json").is_file());
    assert!(!candidate.join("workspace.json").exists() && !candidate.join("images").exists());
}

#[test]
fn sync_confirms_and_adds_trusted_pieces_and_leaves_untrusted_for_review() {
    // 防的回归（RB-3、D-SB-51）：对齐不可信的件也被自动确认或加入标定；对齐可信的件不记“软件确认”；走廊示教没完成时
    // 也加入标定（按未示教的走廊量）；示教件不计入标定件或被当成标准件
    for taught in [true, false] {
        let (w, teach) = with_teaching(taught);
        let mut trusted = capture_round("trusted", &w.doc, GROUPS, DEVICE, &[]);
        trusted.origin = Some(crate::recipe_capture::RoundOrigin::History {
            cycle_id: "c-t".into(),
            record_id: 2,
            standard: true,
        });
        let doubtful = capture_round("doubtful", &w.doc, GROUPS, DEVICE, &[]);
        let outcome = sync_pieces(
            &w,
            &[teach.clone(), trusted.clone(), doubtful.clone()],
            &configs(&w.doc, DEVICE),
            |round| {
                Ok(vec![aligned(
                    16,
                    (round.round_id == "doubtful").then_some(ReviewReason::Deviates),
                )])
            },
            |w, round| {
                Ok(measure_round(&w.doc.plan_cameras(), round, |_, _, _| {
                    Ok(measured(0.0))
                }))
            },
        );
        let result: Vec<_> = outcome
            .pieces
            .iter()
            .map(|p| (p.round_id.as_str(), p.added))
            .collect();
        assert_eq!(
            result,
            [("teach", taught), ("trusted", taught), ("doubtful", false)],
            "{taught}"
        );
        let next = &outcome.next;
        let review = &next.capture_review.rounds["trusted"];
        assert_eq!(
            (&review.status, review.confirmed_by, &review.cuts),
            (&ReviewStatus::Confirmed, ConfirmedBy::Aligned, &vec![16]),
            "{taught}"
        );
        assert!(
            !next.capture_review.rounds.contains_key("doubtful"),
            "{taught}：不可信的不动"
        );
        assert!(
            outcome.pieces[2]
                .issue
                .as_deref()
                .is_some_and(|i| i.contains("待复核")),
            "{taught}"
        );
        if taught {
            let standard: Vec<_> = next
                .calibration
                .rounds
                .iter()
                .map(|r| (r.round_id.as_str(), r.standard))
                .collect();
            assert_eq!(standard, [("teach", false), ("trusted", true)]);
            assert_eq!(outcome.features.len(), 2);
        } else {
            assert!(next.calibration.rounds.is_empty());
            assert!(outcome.pieces[0]
                .issue
                .as_deref()
                .is_some_and(|i| i.contains("走廊示教未完成")));
        }
    }
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

#[test]
fn progress_requires_teaching_piece_then_its_confirmed_boundary_before_corridor() {
    // 防的回归（RB 步骤）：没有示教件也算完成第一步；示教件边界没确认（或段结构改了已失效）也算分段完成，走廊示教提前开放
    let progress = |w: &Workspace| {
        let p = super::burst::progress_v2(w, &super::burst::camera_teaching_views(w), None);
        (
            p.steps[0].status,
            p.steps[1].status,
            p.steps[2].status,
            p.next.map(|n| n.target),
        )
    };
    let (mut w, teach) = with_teaching(false);
    assert_eq!(
        progress(&w),
        (
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::NotStarted,
            Some(super::burst::TargetV2::Corridor {
                camera: "cam1".into(),
                segment: Some(1)
            })
        )
    );
    let mut segments = w.doc.segments.clone();
    segments[1].kind = Straight;
    super::burst::update_segments(&mut w, segments).unwrap();
    assert_eq!(
        progress(&w),
        (
            StepStatus::Done,
            StepStatus::NotStarted,
            StepStatus::NotStarted,
            Some(super::burst::TargetV2::Segments)
        )
    );
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Save(vec![15])).unwrap();
    assert_eq!(progress(&w).1, StepStatus::NotStarted, "只保存未确认");
    w.teaching_piece = None;
    assert_eq!(
        progress(&w),
        (
            StepStatus::NotStarted,
            StepStatus::NotStarted,
            StepStatus::NotStarted,
            Some(super::burst::TargetV2::Capture)
        )
    );
}

#[test]
fn teaching_piece_is_written_only_into_the_release_doc_and_recipes_without_it_still_load() {
    // 防的回归（RB-7）：发布的正式版不带示教件来源，或没有示教件也能生成正式版；候选文档从前端或生产配方带进示教件来源；
    // 没有这个字段的旧生产配方（schema 仍为 8）加载失败
    let (mut w, _) = with_teaching(false);
    let release = formal_release_doc(&w).unwrap();
    assert_eq!(release.teaching_piece, Some(piece("teach", 1).recipe_ref()));
    let mut edited = release.clone();
    edited.stage = w.doc.stage;
    edited.teaching_id = w.doc.teaching_id.clone();
    update_doc(&mut w, edited).unwrap();
    assert_eq!(w.doc.teaching_piece, None, "候选文档恒为空");
    w.teaching_piece = None;
    assert!(formal_release_doc(&w).is_err());
    let old = serde_json::to_value(crate::recipe::samples_burst()).unwrap();
    assert!(old.get("teachingPiece").is_none(), "没有示教件时不写出");
    let loaded: RecipeDoc = serde_json::from_value(old).unwrap();
    assert!(loaded.build().is_ok() && loaded.teaching_piece.is_none());
}

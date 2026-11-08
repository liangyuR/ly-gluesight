use super::burst::{apply_segment_teaching, CameraTeaching};
use super::burst_tests::{burst_workspace, capture_round, DEVICE};
use super::capture_review::{apply_capture_review, CaptureReviewEdit};
use super::pass_align::{
    accept_carry, align, alignment_check, edit_alignment, replace_by_passes, AlignInput,
    PassChange, PassLink,
};
use super::pieces_tests::piece;
use super::*;
use crate::plan::SegmentKind::{Corner, Straight};
use crate::recipe::SegmentCorridor;

/// 合成的逐笔图像距离：笔的标签相同为 0.02（同一笔），标签相同但带 `'` 为 0.4（同一笔改了轨迹），其余 0.9。
fn distances(old: &[&str], new: &[&str]) -> Vec<Vec<Option<f32>>> {
    old.iter()
        .map(|a| {
            new.iter()
                .map(|b| {
                    Some(if a == b {
                        0.02
                    } else if b.trim_end_matches('\'') == *a {
                        0.4
                    } else {
                        0.9
                    })
                })
                .collect()
        })
        .collect()
}

fn summary(rows: &[super::pass_align::PassPair]) -> Vec<(Option<u32>, Option<u32>, PassChange)> {
    rows.iter().map(|r| (r.old, r.new, r.change)).collect()
}

#[test]
fn auto_alignment_finds_inserted_removed_and_changed_passes() {
    use PassChange::*;
    let cases: [(
        &[&str],
        &[&str],
        &[u32],
        &[u32],
        Vec<(Option<u32>, Option<u32>, PassChange)>,
    ); 3] = [
        (
            &["a", "b", "c", "d"],
            &["a", "b", "x", "c", "d"],
            &[10, 10, 10, 10],
            &[10, 10, 6, 10, 10],
            vec![
                (Some(0), Some(0), Same),
                (Some(1), Some(1), Same),
                (None, Some(2), Added),
                (Some(2), Some(3), Same),
                (Some(3), Some(4), Same),
            ],
        ),
        (
            &["a", "b", "c", "d"],
            &["a", "c", "d"],
            &[10, 10, 10, 10],
            &[10, 10, 10],
            vec![
                (Some(0), Some(0), Same),
                (Some(1), None, Removed),
                (Some(2), Some(1), Same),
                (Some(3), Some(2), Same),
            ],
        ),
        (
            &["a", "b", "c", "d"],
            &["a", "b'", "c'", "d"],
            &[10, 10, 10, 10],
            &[10, 14, 10, 10],
            vec![
                (Some(0), Some(0), Same),
                (Some(1), Some(1), Changed),
                (Some(2), Some(2), Changed),
                (Some(3), Some(3), Same),
            ],
        ),
    ];
    for (old, new, old_passes, new_passes, expected) in cases {
        let rows = align(old_passes, new_passes, &distances(old, new));
        assert_eq!(summary(&rows), expected, "{old:?} → {new:?}");
    }
}

fn teaching(camera: &str, round: &str, seq: u64, segment: u32) -> CameraTeaching {
    serde_json::from_value(json!({"camera": camera, "round": round, "seq": seq, "segment": segment,
        "frame": {"id": format!("{camera}-{segment}"), "view": 1, "source": "", "capturedAt": 0, "size": [1000, 700],
            "camera": camera, "cameraTag": null, "calibTag": null, "geometryTag": null, "exposureUs": null,
            "gainDb": null, "historyId": null}}))
    .unwrap()
}

/// 每笔一段（cuts 在笔末），示教件 "teach" 的笔结构为 `passes`；每段每台相机都有示教帧（笔内第 5 帧）与走廊，颜色模型标 `m<段号>`。
fn taught(passes: &[u32], cuts: &[u64]) -> Workspace {
    let kinds: Vec<_> = (0..=cuts.len())
        .map(|i| if i % 2 == 0 { Straight } else { Corner })
        .collect();
    let mut w = burst_workspace(&kinds);
    let mut old = piece("teach", 1);
    old.passes = passes.to_vec();
    w.teaching_piece = Some(old);
    let corridor = crate::recipe::samples_burst().inspect[0].segments[0]
        .corridor
        .clone();
    let starts: Vec<u64> = std::iter::once(0).chain(cuts.iter().copied()).collect();
    for inspect in &mut w.doc.inspect {
        inspect.segments = (1..=kinds.len() as u32)
            .map(|segment| SegmentCorridor {
                segment,
                corridor: corridor.clone(),
                sector_deg: None,
                color_model: Some(format!("m{segment}")),
            })
            .collect();
        for (k, start) in starts.iter().enumerate() {
            w.camera_teaching
                .push(teaching(&inspect.camera, "teach", start + 5, k as u32 + 1));
        }
    }
    w.doc.build().unwrap();
    w
}

fn replace(
    w: &mut Workspace,
    old: &[&str],
    new: &[&str],
    new_passes: &[u32],
    cuts: &[u64],
) -> Vec<u64> {
    let mut next = piece("new", 2);
    next.passes = new_passes.to_vec();
    let input = AlignInput {
        old_piece: w.teaching_piece.clone().unwrap(),
        cuts: cuts.to_vec(),
        distances: distances(old, new),
    };
    replace_by_passes(w, next, input).unwrap()
}

fn color_model(w: &Workspace, segment: u32) -> Option<String> {
    w.doc.inspect[0]
        .segments
        .iter()
        .find(|c| c.segment == segment)
        .and_then(|c| c.color_model.clone())
}

fn change(w: &Workspace, segment: u32) -> Option<PassChange> {
    let id = w.doc.segments[segment as usize - 1].id.clone().unwrap();
    w.pass_alignment.as_ref().unwrap().carry[&id].change
}

const TEN: [&str; 10] = ["p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9", "p10"];
const TEN_CUTS: [u64; 9] = [10, 20, 30, 40, 50, 60, 70, 80, 90];

/// RB-11 验收场景：10 笔改第 5、6 笔，又在第 6 笔后加一笔。
fn changed_program() -> (Workspace, Vec<u64>) {
    let mut w = taught(&[10; 10], &TEN_CUTS);
    let new = [
        "p1", "p2", "p3", "p4", "p5'", "p6'", "x", "p7", "p8", "p9", "p10",
    ];
    let cuts = replace(
        &mut w,
        &TEN,
        &new,
        &[10, 10, 10, 10, 14, 8, 6, 10, 10, 10, 10],
        &TEN_CUTS,
    );
    (w, cuts)
}

#[test]
fn changing_two_passes_and_adding_one_keeps_teaching_of_the_other_passes_on_their_segments() {
    let (w, cuts) = changed_program();
    assert_eq!(cuts, vec![10, 20, 30, 40, 54, 62, 68, 78, 88, 98]);
    assert_eq!(w.doc.segments.len(), 11);
    // 不变的笔：走廊（颜色模型）跟着段走到新段号，示教帧按笔内位置落到新件
    for (new, old) in [(1, 1), (4, 4), (8, 7), (11, 10)] {
        assert_eq!(color_model(&w, new), Some(format!("m{old}")));
        assert_eq!(change(&w, new), Some(PassChange::Same));
    }
    let frame = w
        .camera_teaching
        .iter()
        .find(|t| t.camera == "cam1" && t.segment == Some(8))
        .unwrap();
    assert_eq!((frame.round.as_str(), frame.seq), ("new", 73));
    // 改动的笔保留旧走廊作参考，标改动；新增的笔新建一段，没有走廊
    assert_eq!(color_model(&w, 5), Some("m5".into()));
    assert_eq!(change(&w, 5), Some(PassChange::Changed));
    assert_eq!(change(&w, 6), Some(PassChange::Changed));
    assert_eq!(color_model(&w, 7), None);
    assert_eq!(change(&w, 7), Some(PassChange::Added));
}

#[test]
fn a_pass_added_inside_a_segment_joins_that_segment_as_changed() {
    let mut w = taught(&[10, 10, 10], &[20]);
    let cuts = replace(
        &mut w,
        &["a", "b", "c"],
        &["a", "x", "b", "c"],
        &[10, 4, 10, 10],
        &[20],
    );
    assert_eq!(cuts, vec![24]);
    assert_eq!(change(&w, 1), Some(PassChange::Changed));
    assert_eq!(change(&w, 2), Some(PassChange::Same));
}

#[test]
fn shortening_a_changed_pass_keeps_every_segment_in_it() {
    let mut w = taught(&[100], &[1]);
    let cuts = replace(&mut w, &["a"], &["a'"], &[50], &[1]);
    assert_eq!(cuts, vec![1]);
    assert_eq!(color_model(&w, 1), Some("m1".into()));
    assert_eq!(color_model(&w, 2), Some("m2".into()));
}

#[test]
fn replacing_still_works_when_the_only_detecting_segment_has_no_counterpart() {
    let mut w = taught(&[10, 10], &[10]);
    w.doc.segments[0].skip = true;
    for inspect in &mut w.doc.inspect {
        inspect.segments.retain(|c| c.segment != 1);
    }
    w.camera_teaching.retain(|t| t.segment != Some(1));
    w.doc.build().unwrap();
    let cuts = replace(&mut w, &["a", "b"], &["x"], &[100], &[10]);
    assert!(cuts.is_empty());
    assert!(!w.doc.segments[0].skip);
    assert_eq!(change(&w, 1), Some(PassChange::Added));
}

#[test]
fn removing_a_pass_drops_only_the_segments_inside_it() {
    let mut w = taught(&[10, 10, 10], &[10, 20]);
    let cuts = replace(&mut w, &["a", "b", "c"], &["a", "c"], &[10, 10], &[10, 20]);
    assert_eq!(cuts, vec![10]);
    assert_eq!(color_model(&w, 2), Some("m3".into()));
    let removed = &w.pass_alignment.as_ref().unwrap().removed;
    assert_eq!(removed.iter().map(|r| r.index).collect::<Vec<_>>(), vec![2]);
}

fn confirm(w: &mut Workspace) {
    let links: Vec<PassLink> = w
        .pass_alignment
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .filter_map(|r| {
            Some(PassLink {
                old: r.old?,
                new: r.new?,
                changed: r.change == PassChange::Changed,
            })
        })
        .collect();
    edit_alignment(w, &links, true).unwrap();
}

#[test]
fn publishing_waits_for_reteaching_changed_and_added_segments_and_accepting_carried_ones() {
    let (mut w, _) = changed_program();
    assert!(!alignment_check(&w).unwrap().passed);
    confirm(&mut w);
    let check = alignment_check(&w).unwrap();
    assert!(!check.passed);
    assert!(
        check.detail.contains("第 5 段所在笔改动"),
        "{}",
        check.detail
    );
    assert!(check.detail.contains("第 7 段为新增"), "{}", check.detail);
    // 在新示教件上确认分段，再给改动与新增的段逐台相机选示教帧（走公开的示教操作）
    let (cuts, groups) = (vec![10, 20, 30, 40, 54, 62, 68, 78, 88, 98], 108);
    let round = capture_round("new", &w.doc, groups, DEVICE, &[]);
    apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(cuts.clone())).unwrap();
    apply_capture_review(&mut w, &round, CaptureReviewEdit::Confirm).unwrap();
    for segment in [5u32, 6, 7] {
        let seq = cuts[segment as usize - 2] + 1;
        for camera in w.doc.plan_cameras() {
            apply_segment_teaching(&mut w, teaching(&camera, "new", seq, segment), groups, None)
                .unwrap();
        }
    }
    assert!(!alignment_check(&w).unwrap().passed, "沿用的段还没确认");
    for segment in [1, 2, 3, 4, 8, 9, 10, 11] {
        accept_carry(&mut w, segment, true).unwrap();
    }
    assert!(alignment_check(&w).unwrap().passed);
}

#[test]
fn segment_edits_sent_without_ids_keep_the_carry_state() {
    let (mut w, _) = changed_program();
    confirm(&mut w);
    let ids: Vec<_> = w.doc.segments.iter().map(|s| s.id.clone()).collect();
    let mut doc = w.doc.clone();
    for segment in &mut doc.segments {
        segment.id = None;
    }
    doc.segments[0].tolerance += 1;
    super::update_doc(&mut w, doc).unwrap();
    assert_eq!(
        w.doc
            .segments
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(change(&w, 1), Some(PassChange::Same));
}

#[test]
fn segment_ids_stay_out_of_the_published_recipe() {
    let (w, _) = changed_program();
    let doc = super::formal_release_doc(&w, 2).unwrap();
    assert!(doc.segments.iter().all(|s| s.id.is_none()));
}

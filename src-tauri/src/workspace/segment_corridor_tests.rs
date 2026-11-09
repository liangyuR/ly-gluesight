use super::burst::{apply_segment_teaching, CameraTeaching};
use super::burst_tests::{burst_workspace, capture_round, DEVICE};
use super::capture_review::{apply_capture_review, CaptureReviewEdit};
use super::*;
use crate::plan::SegmentKind::{Corner, Straight};
use crate::recipe::SegmentCorridor;

const GROUPS: u64 = 30;

fn teaching(camera: &str, round: &str, seq: u64, segment: u32, size: [u32; 2]) -> CameraTeaching {
    serde_json::from_value(json!({"camera": camera, "round": round, "seq": seq, "segment": segment,
        "frame": {"id": format!("{camera}-{round}-{seq}"), "view": 1, "source": "", "capturedAt": 0, "size": size,
            "camera": camera, "cameraTag": null, "calibTag": null, "geometryTag": null, "exposureUs": null, "gainDb": null, "historyId": null}})).unwrap()
}

/// 直线 / 弯道 / 直线三段，第 2 段只由第一台相机检测；还没有走廊与示教帧；示教件 “confirmed” 与另一件 “other” 边界
/// 10/20 已确认，“draft” 只存了草稿。
fn workspace() -> (Workspace, Vec<String>) {
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let cameras = w.doc.plan_cameras();
    let mut doc = w.doc.clone();
    doc.segments[1].cameras = vec![cameras[0].clone()];
    update_doc(&mut w, doc).unwrap();
    w.teaching_piece = Some(super::pieces_tests::piece("confirmed", 1));
    let confirmed = capture_round("confirmed", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &confirmed, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    apply_capture_review(&mut w, &confirmed, CaptureReviewEdit::Confirm).unwrap();
    let other = capture_round("other", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    apply_capture_review(&mut w, &other, CaptureReviewEdit::Confirm).unwrap();
    let draft = capture_round("draft", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &draft, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    (w, cameras)
}

/// 在已确认轮次第 `segment` 段中间选示教帧（100×100）并保存走廊。
fn with_corridor(w: &mut Workspace, camera: &str, segment: u32, corridor: Vec<[f32; 2]>) {
    apply_segment_teaching(
        w,
        teaching(
            camera,
            "confirmed",
            u64::from(segment) * 10 - 5,
            segment,
            [100, 100],
        ),
        GROUPS,
        None,
    )
    .unwrap();
    let mut doc = w.doc.clone();
    doc.inspect
        .iter_mut()
        .find(|i| i.camera == camera)
        .unwrap()
        .segments
        .push(SegmentCorridor {
            segment,
            corridor,
            sector_deg: None,
            color_model: None,
            measure: None,
        });
    update_doc(w, doc).unwrap();
}

#[test]
fn segment_teaching_frame_must_lie_in_that_segment_of_a_confirmed_round() {
    // 防的回归（D-SB-42/45、RB-1）：段走廊在别的段、示教件以外的件或不检测该段的相机上示教；同一台相机各段示教帧尺寸不一
    // （生产按相机一个尺寸建测量图）
    let cases: [(&str, usize, u32, &str, u64, [u32; 2], bool); 6] = [
        ("段内帧组", 0, 2, "confirmed", 15, [100, 100], true),
        ("段首帧组", 0, 2, "confirmed", 11, [100, 100], true),
        ("帧组落在下一段", 0, 2, "confirmed", 21, [100, 100], false),
        ("示教帧不属于示教件", 0, 2, "other", 15, [100, 100], false),
        (
            "不是这一段的检测相机",
            1,
            2,
            "confirmed",
            15,
            [100, 100],
            false,
        ),
        (
            "与这台相机第 1 段的示教帧尺寸不同",
            0,
            2,
            "confirmed",
            15,
            [200, 100],
            false,
        ),
    ];
    for (name, camera, segment, round, seq, size, ok) in cases {
        let (mut w, cameras) = workspace();
        apply_segment_teaching(
            &mut w,
            teaching(&cameras[camera], "confirmed", 5, 1, [100, 100]),
            GROUPS,
            None,
        )
        .unwrap();
        let result = apply_segment_teaching(
            &mut w,
            teaching(&cameras[camera], round, seq, segment, size),
            GROUPS,
            None,
        );
        assert_eq!(result.is_ok(), ok, "{name}: {result:?}");
        assert_eq!(
            w.camera_teaching.iter().any(|t| t.segment == Some(segment)),
            ok,
            "{name}"
        );
    }
}

#[test]
fn saving_the_segmentation_keeps_corridors_and_params_of_segments_that_did_not_change() {
    // 防的回归（D-SB-52）：工作台保存分段时只要段的类型序列变了，没动的段的走廊与示教帧也全部清空；段参数按位置对应，
    // 插入或删除边界后错位到相邻段；只挪边界时示教帧已不在本段，走廊却还留着；
    // D-SB-53：段设为不检测后它的走廊还留着、保存后还提示它待示教
    let (base, cameras) = workspace();
    let corner_cameras = vec![cameras[0].clone()];
    type Case<'a> = (
        &'a str,
        &'a [crate::plan::SegmentKind],
        &'a [u64],
        &'a [u32],
        &'a [(usize, u32)],
        [Option<f64>; 4],
        &'a [&'a str],
    );
    let cases: [Case; 5] = [
        (
            "第 2 段设为不检测",
            &[Straight, Corner, Straight],
            &[10, 20],
            &[2],
            &[(0, 1), (1, 3)],
            [Some(100.0), Some(40.0), Some(80.0), None],
            &["原第 2 段（弯道）· "],
        ),
        (
            "第 3 段中间加边界",
            &[Straight, Corner, Straight, Straight],
            &[10, 20, 25],
            &[],
            &[(0, 1), (0, 2), (1, 3)],
            [Some(100.0), Some(40.0), None, None],
            &[],
        ),
        (
            "第 1 段中间加边界",
            &[Straight, Straight, Corner, Straight],
            &[5, 10, 20],
            &[],
            &[(0, 1), (0, 3), (1, 4)],
            [None, None, Some(40.0), Some(80.0)],
            &[],
        ),
        (
            "第 2 段改成直线",
            &[Straight, Straight, Straight],
            &[10, 20],
            &[],
            &[(0, 1), (1, 3)],
            [Some(100.0), None, Some(80.0), None],
            &["原第 2 段（弯道）· "],
        ),
        (
            "边界 20 挪到 14，第 2 段的示教帧落进第 3 段",
            &[Straight, Corner, Straight],
            &[10, 14],
            &[],
            &[(0, 1), (1, 3)],
            [Some(100.0), None, None, None],
            &["原第 2 段（弯道）· "],
        ),
    ];
    for (name, kinds, cuts, skipped, corridors, lengths, dropped) in cases {
        let mut w = base.clone();
        with_corridor(&mut w, &cameras[0], 1, vec![[1.0, 1.0], [50.0, 1.0]]);
        with_corridor(&mut w, &cameras[0], 2, vec![[1.0, 1.0], [8.0, 1.0]]);
        with_corridor(&mut w, &cameras[1], 3, vec![[1.0, 1.0], [50.0, 1.0]]);
        let mut doc = w.doc.clone();
        for (segment, length) in doc.segments.iter_mut().zip([100.0, 40.0, 80.0]) {
            segment.length_mm = Some(length);
        }
        update_doc(&mut w, doc).unwrap();
        let round = capture_round("confirmed", &w.doc, GROUPS, DEVICE, &[]);
        let incoming = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| crate::plan::PathSegment {
                cameras: if kind == Corner {
                    corner_cameras.clone()
                } else {
                    Vec::new()
                },
                skip: skipped.contains(&(i as u32 + 1)),
                ..crate::plan::new_segment(i as u32 + 1, kind, &w.doc.segment_defaults)
            })
            .collect();
        let next = super::capture_review::save_template_round(&w, &round, incoming, cuts.to_vec())
            .unwrap();
        let mut kept: Vec<(usize, u32)> = Vec::new();
        for (c, camera) in cameras.iter().enumerate() {
            for segment in &next.doc.segments {
                let corridor =
                    next.doc.inspect.iter().any(|i| {
                        i.camera == *camera && i.segment_corridor(segment.index).is_some()
                    });
                let frame = next
                    .camera_teaching
                    .iter()
                    .any(|t| t.camera == *camera && t.segment == Some(segment.index));
                assert_eq!(corridor, frame, "{name}：走廊与示教帧要一起留或一起去掉");
                if corridor {
                    kept.push((c, segment.index));
                }
            }
        }
        assert_eq!(kept, corridors.to_vec(), "{name}：留下的走廊");
        let got: Vec<Option<f64>> = (0..4)
            .map(|i| next.doc.segments.get(i).and_then(|s| s.length_mm))
            .collect();
        assert_eq!(got, lengths.to_vec(), "{name}：段长");
        let impact = super::capture_review::save_impact(&w, &next);
        let expected: Vec<String> = dropped
            .iter()
            .map(|d| format!("{d}{}", cameras[0]))
            .collect();
        assert_eq!(impact.dropped, expected, "{name}：确认弹窗列出的清除项");
        for index in skipped {
            let prefix = format!("第 {index} 段");
            assert!(
                !impact.untaught.iter().any(|u| u.starts_with(&prefix)),
                "{name}：不检测的段不该待示教"
            );
        }
    }
}

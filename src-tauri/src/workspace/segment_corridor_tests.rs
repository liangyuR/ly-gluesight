use super::burst::{
    apply_camera_teaching, apply_segment_teaching, camera_teaching_views, corridor_segments,
    update_segments, CameraTeachDraft, CameraTeaching,
};
use super::burst_tests::{burst_workspace, capture_round, DEVICE};
use super::capture_review::{apply_capture_review, CaptureReviewEdit};
use super::*;
use crate::plan::SegmentKind::{Corner, Straight};
use crate::recipe::SegmentCorridor;

const GROUPS: u64 = 30;

fn teaching(camera: &str, round: &str, seq: u64, segment: Option<u32>) -> CameraTeaching {
    serde_json::from_value(json!({"camera": camera, "round": round, "seq": seq, "segment": segment,
        "frame": {"id": format!("{camera}-{round}-{seq}"), "view": 1, "source": "", "capturedAt": 0, "size": [100, 100],
            "camera": camera, "cameraTag": null, "calibTag": null, "geometryTag": null, "exposureUs": null, "gainDb": null, "historyId": null}})).unwrap()
}

/// 直线 / 弯道 / 直线三段，第 2 段只用第一台相机检测；各相机已示教默认走廊；“confirmed” 轮次边界 10/20 已确认，“draft” 只存了草稿。
fn workspace() -> (Workspace, Vec<String>) {
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let cameras = w.doc.plan_cameras();
    let mut doc = w.doc.clone();
    doc.segments[1].cameras = vec![cameras[0].clone()];
    update_doc(&mut w, doc).unwrap();
    for camera in &cameras {
        let draft = CameraTeachDraft {
            corridor: vec![[1.0, 1.0], [50.0, 1.0]],
            mm_per_px: Some(0.1),
            detect: None,
        };
        apply_camera_teaching(&mut w, teaching(camera, "default", 1, None), draft, None).unwrap();
    }
    let confirmed = capture_round("confirmed", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &confirmed, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    apply_capture_review(&mut w, &confirmed, CaptureReviewEdit::Confirm).unwrap();
    let draft = capture_round("draft", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &draft, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    (w, cameras)
}

fn with_segment_corridor(w: &mut Workspace, camera: &str, segment: u32, corridor: Vec<[f32; 2]>) {
    apply_segment_teaching(
        w,
        teaching(
            camera,
            "confirmed",
            u64::from(segment) * 10 - 5,
            Some(segment),
        ),
        GROUPS,
    )
    .unwrap();
    let mut doc = w.doc.clone();
    doc.inspect
        .iter_mut()
        .find(|i| i.camera == camera)
        .unwrap()
        .segments
        .push(SegmentCorridor { segment, corridor });
    update_doc(w, doc).unwrap();
}

#[test]
fn segment_teaching_frame_must_lie_in_that_segment_of_a_confirmed_round() {
    // 防的回归（D-SB-42）：段走廊在别的段、未确认分段的轮次或不检测该段的相机上示教，或没有默认走廊（缺像素当量与检测参数）就示教段走廊
    let cases: [(&str, usize, u32, &str, u64, bool, bool); 6] = [
        ("段内帧组", 0, 2, "confirmed", 15, true, true),
        ("段首帧组", 0, 2, "confirmed", 11, true, true),
        ("帧组落在下一段", 0, 2, "confirmed", 21, true, false),
        ("示教轮次只存了边界草稿", 0, 2, "draft", 15, true, false),
        ("不是这一段的检测相机", 1, 2, "confirmed", 15, true, false),
        (
            "这台相机还没有默认走廊的示教帧",
            0,
            2,
            "confirmed",
            15,
            false,
            false,
        ),
    ];
    for (name, camera, segment, round, seq, has_default, ok) in cases {
        let (mut w, cameras) = workspace();
        if !has_default {
            w.camera_teaching.retain(|t| t.camera != cameras[camera]);
        }
        let result = apply_segment_teaching(
            &mut w,
            teaching(&cameras[camera], round, seq, Some(segment)),
            GROUPS,
        );
        assert_eq!(result.is_ok(), ok, "{name}: {result:?}");
        let taught = w.camera_teaching.iter().any(|t| t.segment == Some(segment));
        assert_eq!(taught, ok, "{name}");
    }
}

#[test]
fn calibration_measures_frames_of_a_segment_with_its_saved_corridor_and_others_with_the_default() {
    // 防的回归（D-SB-42）：标定不按已确认边界选段走廊、把段走廊用到相邻段或别的相机、用了没画完 / 超出示教帧的段走廊，
    // 或在未确认分段的轮次上按草稿边界选走廊
    let (mut w, cameras) = workspace();
    with_segment_corridor(&mut w, &cameras[0], 2, vec![[1.0, 1.0], [50.0, 1.0]]);
    with_segment_corridor(&mut w, &cameras[0], 3, vec![[1.0, 1.0], [500.0, 1.0]]);
    let choice = corridor_segments(&w, "confirmed", GROUPS);
    let expected: Vec<Option<u32>> = (1..=GROUPS)
        .map(|seq| (11..=20).contains(&seq).then_some(2))
        .collect();
    assert_eq!(choice[0], expected);
    assert!(choice[1..].iter().flatten().all(Option::is_none));
    assert!(corridor_segments(&w, "draft", GROUPS)
        .iter()
        .flatten()
        .all(Option::is_none));
}

#[test]
fn segment_structure_change_drops_segment_corridors_and_their_frames_but_keeps_defaults() {
    // 防的回归（D-SB-42）：段的类型序列变了，旧段号上的段走廊被套到新的段上；只改参数也清掉段走廊；
    // 或保存了没有示教帧的段走廊
    let edits: [(&str, fn(&mut Vec<crate::plan::PathSegment>), bool); 3] = [
        ("改段类型", |s| s[1].kind = Straight, true),
        (
            "删一段",
            |s| {
                s.pop();
            },
            true,
        ),
        ("只改容差", |s| s[1].tolerance += 1, false),
    ];
    for (name, edit, dropped) in edits {
        let (mut w, cameras) = workspace();
        with_segment_corridor(&mut w, &cameras[0], 2, vec![[1.0, 1.0], [50.0, 1.0]]);
        let mut segments = w.doc.segments.clone();
        edit(&mut segments);
        update_segments(&mut w, segments).unwrap();
        assert_eq!(w.doc.inspect[0].segments.is_empty(), dropped, "{name}");
        assert_eq!(
            w.camera_teaching.iter().any(|t| t.segment.is_some()),
            !dropped,
            "{name}"
        );
        assert_eq!(
            w.camera_teaching
                .iter()
                .filter(|t| t.segment.is_none())
                .count(),
            cameras.len(),
            "{name}"
        );
    }
    let (mut w, cameras) = workspace();
    let mut doc = w.doc.clone();
    doc.inspect
        .iter_mut()
        .find(|i| i.camera == cameras[2])
        .unwrap()
        .segments
        .push(SegmentCorridor {
            segment: 3,
            corridor: vec![[1.0, 1.0], [50.0, 1.0]],
        });
    assert!(update_doc(&mut w, doc).is_err(), "没有段示教帧的段走廊");
}

#[test]
fn camera_whose_detected_segments_all_have_corridors_is_taught_without_default_corridor() {
    // 防的回归（D-SB-42）：相机检测的各段都画了段走廊、默认走廊清空后，走廊示教仍判“待示教走廊”，标定前被迫再画默认走廊；
    // 或只画了部分段就算完成
    for (name, segments, ready) in [
        ("检测的两段都有段走廊", &[1, 3][..], true),
        ("只画了一段", &[1][..], false),
    ] {
        let (mut w, cameras) = workspace();
        for &segment in segments {
            with_segment_corridor(&mut w, &cameras[1], segment, vec![[1.0, 1.0], [50.0, 1.0]]);
        }
        let mut doc = w.doc.clone();
        doc.inspect
            .iter_mut()
            .find(|i| i.camera == cameras[1])
            .unwrap()
            .corridor
            .clear();
        update_doc(&mut w, doc).unwrap();
        let views = camera_teaching_views(&w);
        let view = views.iter().find(|v| v.camera == cameras[1]).unwrap();
        assert!(!view.saved, "{name}");
        assert_eq!(view.ready(), ready, "{name}");
    }
}

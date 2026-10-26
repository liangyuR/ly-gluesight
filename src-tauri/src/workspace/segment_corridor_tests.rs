use super::burst::{
    apply_segment_teaching, calibration_corridors, camera_teaching_views, update_segments,
    CameraTeaching,
};
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

/// 直线 / 弯道 / 直线三段，第 2 段只由第一台相机检测；还没有走廊与示教帧；“confirmed” 轮次边界 10/20 已确认，“draft” 只存了草稿。
fn workspace() -> (Workspace, Vec<String>) {
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let cameras = w.doc.plan_cameras();
    let mut doc = w.doc.clone();
    doc.segments[1].cameras = vec![cameras[0].clone()];
    update_doc(&mut w, doc).unwrap();
    let confirmed = capture_round("confirmed", &w.doc, GROUPS, DEVICE, &[]);
    apply_capture_review(&mut w, &confirmed, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    apply_capture_review(&mut w, &confirmed, CaptureReviewEdit::Confirm).unwrap();
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
        .push(SegmentCorridor { segment, corridor });
    update_doc(w, doc).unwrap();
}

#[test]
fn segment_teaching_frame_must_lie_in_that_segment_of_a_confirmed_round() {
    // 防的回归（D-SB-42/45）：段走廊在别的段、未确认分段的轮次或不检测该段的相机上示教；同一台相机各段示教帧尺寸不一
    // （生产按相机一个尺寸建测量图）
    let cases: [(&str, usize, u32, &str, u64, [u32; 2], bool); 6] = [
        ("段内帧组", 0, 2, "confirmed", 15, [100, 100], true),
        ("段首帧组", 0, 2, "confirmed", 11, [100, 100], true),
        ("帧组落在下一段", 0, 2, "confirmed", 21, [100, 100], false),
        (
            "示教轮次只存了边界草稿",
            0,
            2,
            "draft",
            15,
            [100, 100],
            false,
        ),
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
fn calibration_measures_each_frame_only_with_its_segment_detecting_cameras() {
    // 防的回归（D-SB-45）：标定不按已确认边界归段、让不检测本段的相机也去量（没有它在这段的走廊，只会量出检测不完整），
    // 或多段配方在只存了边界草稿的轮次上按草稿归段
    let (w, cameras) = workspace();
    let choice = calibration_corridors(&w, "confirmed", GROUPS).unwrap();
    let owner = |seq: u64| {
        if seq <= 10 {
            1
        } else if seq <= 20 {
            2
        } else {
            3
        }
    };
    let expected = |camera: usize| {
        (1..=GROUPS)
            .map(|seq| (camera == 0 || owner(seq) != 2).then_some(owner(seq)))
            .collect::<Vec<_>>()
    };
    assert_eq!(choice, (0..cameras.len()).map(expected).collect::<Vec<_>>());
    assert!(calibration_corridors(&w, "draft", GROUPS).is_err());
    let single = burst_workspace(&[Straight]);
    assert!(
        calibration_corridors(&single, "draft", GROUPS)
            .unwrap()
            .iter()
            .flatten()
            .all(|s| *s == Some(1)),
        "单段配方不需要确认分段"
    );
}

#[test]
fn segment_structure_or_camera_change_drops_only_the_affected_corridors_and_frames() {
    // 防的回归（D-SB-45）：段的类型序列变了，旧段号上的走廊被套到新的段上；某段去掉一台检测相机后，它在这段的走廊与示教帧
    // 还留着（发布检查、测量图都会去找它）；只改参数也清掉走廊；或保存了没有示教帧的走廊
    type Edit = fn(&mut Vec<crate::plan::PathSegment>, &[String]);
    let edits: [(&str, Edit, bool, bool); 4] = [
        ("改段类型", |s, _| s[1].kind = Straight, false, false),
        (
            "删一段",
            |s, _| {
                s.pop();
            },
            false,
            false,
        ),
        (
            "第 3 段去掉第二台相机",
            |s, c| s[2].cameras = vec![c[0].clone(), c[2].clone()],
            true,
            false,
        ),
        ("只改容差", |s, _| s[1].tolerance += 1, true, true),
    ];
    for (name, edit, first_kept, second_kept) in edits {
        let (mut w, cameras) = workspace();
        with_corridor(&mut w, &cameras[0], 2, vec![[1.0, 1.0], [50.0, 1.0]]);
        with_corridor(&mut w, &cameras[1], 3, vec![[1.0, 1.0], [50.0, 1.0]]);
        let mut segments = w.doc.segments.clone();
        edit(&mut segments, &cameras);
        update_segments(&mut w, segments).unwrap();
        let kept = |camera: &str, segment: u32| {
            let corridor = w
                .doc
                .inspect
                .iter()
                .any(|i| i.camera == camera && i.segment_corridor(segment).is_some());
            let frame = w
                .camera_teaching
                .iter()
                .any(|t| t.camera == camera && t.segment == Some(segment));
            assert_eq!(corridor, frame, "{name}：走廊与示教帧要一起留或一起去掉");
            corridor
        };
        assert_eq!(
            (kept(&cameras[0], 2), kept(&cameras[1], 3)),
            (first_kept, second_kept),
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
    assert!(update_doc(&mut w, doc).is_err(), "没有示教帧的走廊");
}

#[test]
fn camera_is_taught_only_when_every_segment_it_detects_has_a_corridor() {
    // 防的回归（D-SB-45）：还要求相机级的默认走廊才算示教完成；或只画了部分段、或缺像素当量就算完成
    let cases: [(&str, &[u32], bool, bool); 3] = [
        ("检测的两段都有走廊", &[1, 3], true, true),
        ("只画了一段", &[1], true, false),
        ("两段都有走廊但缺像素当量", &[1, 3], false, false),
    ];
    for (name, segments, scaled, ready) in cases {
        let (mut w, cameras) = workspace();
        for &segment in segments {
            with_corridor(&mut w, &cameras[1], segment, vec![[1.0, 1.0], [50.0, 1.0]]);
        }
        if !scaled {
            w.doc
                .inspect
                .iter_mut()
                .find(|i| i.camera == cameras[1])
                .unwrap()
                .mm_per_px = None;
        }
        let views = camera_teaching_views(&w);
        assert_eq!(
            views.iter().find(|v| v.camera == cameras[1]).unwrap().ready,
            ready,
            "{name}"
        );
    }
}

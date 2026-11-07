use super::burst::update_segments;
use super::*;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::plan::{
    new_segment, PartCalib, PathSegment, SegmentCalib,
    SegmentKind::{self, Corner, Straight},
};
use crate::recipe_capture::CaptureRoundV2;

pub(super) const DEVICE: &str = "device-a";

/// 段配方候选：三台模拟相机 cam1–cam3（都检测每一段、有像素当量），段按 kinds 新建（N_exp 待标定、不开逐帧定向），还没有走廊与示教帧。
pub(super) fn burst_workspace(kinds: &[SegmentKind]) -> Workspace {
    let mut doc = crate::recipe::samples_burst();
    doc.segments = kinds
        .iter()
        .enumerate()
        .map(|(i, &kind)| PathSegment {
            heading: None,
            ..new_segment(i as u32 + 1, kind, &doc.segment_defaults)
        })
        .collect();
    doc.inspect
        .iter_mut()
        .for_each(|inspect| inspect.segments.clear());
    doc.build().unwrap();
    Workspace::new(doc, None)
}

/// 一轮完整的段配方示教采集：计划相机都在，采集时设备配置为 config；missing 为 (本轮相机下标, seq) 的缺帧。
pub(super) fn capture_round(
    id: &str,
    doc: &RecipeDoc,
    group_count: u64,
    config: &str,
    missing: &[(usize, u64)],
) -> CaptureRoundV2 {
    let mut missing = missing.to_vec();
    missing.sort_by_key(|&(camera, seq)| (seq, camera));
    let frames: Vec<u64> = (0..3)
        .map(|camera| group_count - missing.iter().filter(|m| m.0 == camera).count() as u64)
        .collect();
    let mut incomplete: Vec<u64> = missing.iter().map(|m| m.1).collect();
    incomplete.dedup();
    serde_json::from_value(json!({
        "format": 2, "roundId": id, "recipeId": doc.id,
        "cameras": doc.plan_cameras().iter().zip(&frames).map(|(camera, n)| json!({"cameraId": camera, "cameraConfig": config,
            "imaging": {"exposureUs": 60.0, "gainDb": 6.0}, "deviceSession": 1, "receivedCount": n})).collect::<Vec<_>>(),
        "state": "complete", "plcShotCount": group_count, "plcPlanVersion": 1, "deviceTriggers": null, "groupCount": group_count,
        "cameraFrames": frames, "incompleteGroups": incomplete,
        "missingFrames": missing.iter().map(|&(camera, seq)| json!({"camera": camera, "seq": seq})).collect::<Vec<_>>(),
        "lossAlarms": [], "alarms": [], "createdAt": 1, "endedAt": 2, "error": null, "failure": null, "simulated": true,
    })).unwrap()
}

pub(super) fn configs(doc: &RecipeDoc, config: &str) -> BTreeMap<String, Value> {
    doc.plan_cameras()
        .into_iter()
        .map(|camera| (camera, json!(config)))
        .collect()
}

/// 测成的一帧，胶中心偏移 x（mm）；相机下标与 seq 由 measure_round 填。
pub(super) fn measured(x: f64) -> FrameMetrics {
    FrameMetrics {
        camera: 0,
        seq: 0,
        state: FrameState::Measured,
        error: None,
        width_min: Some(4.0),
        width_max: Some(4.0),
        width_mean: Some(4.0),
        widths: vec![Some(4.0)],
        spacing_mm: 1.0,
        gap_max_mm: 0.0,
        present_ratio: 1.0,
        x_mm: Some(x),
        corridor: None,
        heading_deg: None,
        heading_fallback: false,
        fallback_reason: None,
        edge_mm: None,
    }
}

#[test]
fn teaching_cuts_set_each_segment_n_exp_and_an_unconfirmed_draft_clears_them() {
    // 防的回归（RB-3）：各段 N_exp 不取示教件已确认分段的帧组数（或还要凑标定件才有）；ΣN_exp 不等于各段之和；
    // 示教件边界改成草稿后旧 N_exp 还留着，生产按过期的边界比例分段
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    let teach = capture_round("teach", &w.doc, 30, DEVICE, &[]);
    w.teaching_piece = Some(super::pieces_tests::piece("teach", 1));
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Save(vec![10, 22])).unwrap();
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Confirm).unwrap();
    let frames: Vec<_> = w.doc.segments.iter().map(|s| s.expected_frames).collect();
    assert_eq!(frames, [Some(10), Some(12), Some(8)]);
    assert_eq!(w.doc.expected_total(), Some(30));
    apply_capture_review(&mut w, &teach, CaptureReviewEdit::Save(vec![11, 22])).unwrap();
    assert!(
        w.doc
            .segments
            .iter()
            .all(|s| s.expected_frames.is_none() && s.calib.is_none()),
        "{:?}",
        w.doc.segments
    );
    assert_eq!(w.doc.expected_total(), None);
}

#[test]
fn saving_expected_frames_or_x_ref_is_ignored_and_kind_change_clears_them() {
    // 防的回归（规格 §0 N_exp 不能手填、§1 段序列变化需重新分段、D-SB-18/20、RB-3）：段列表或整份保存能手填 N_exp、标定统计、
    // 整件标定或 x_ref；界面拿着旧值保存被拒；段的类型序列变了旧 N_exp 还留着；只改容差也把 N_exp 或 x_ref 清掉
    fn calibrated() -> Workspace {
        super::pieces_tests::measured_teaching(&[Straight], 0.5)
    }
    #[derive(Debug, PartialEq)]
    enum After {
        Ignored,
        Cleared,
        Kept,
        XRefCleared,
    }
    type Entry = fn(&mut Workspace, RecipeDoc) -> Result<(), String>;
    let segments: Entry = |w, doc| update_segments(w, doc.segments);
    let whole: Entry = |w, doc| update_doc(w, doc);
    type Edit = fn(&mut RecipeDoc);
    let cases: [(&str, Entry, Edit, After); 8] = [
        (
            "段列表直接改 N_exp",
            segments,
            |d| d.segments[0].expected_frames = Some(25),
            After::Ignored,
        ),
        (
            "段列表直接改标定统计",
            segments,
            |d| d.segments[0].calib.as_mut().unwrap().max += 1,
            After::Ignored,
        ),
        (
            "段列表手填 x_ref",
            segments,
            |d| d.segments[0].offset_ref.as_mut().unwrap().x_ref = Some(1.5),
            After::Ignored,
        ),
        (
            "整份保存写整件标定",
            whole,
            |d| {
                let calib = SegmentCalib {
                    rounds: 1,
                    min: 25,
                    max: 25,
                    mean: 25.0,
                };
                d.part_calib = Some(PartCalib {
                    expected_total: 25,
                    calib,
                });
            },
            After::Ignored,
        ),
        (
            "改成弯道段",
            segments,
            |d| {
                let checks = d.segment_defaults.corner.checks.clone();
                let segment = &mut d.segments[0];
                (segment.kind, segment.checks, segment.offset_ref) = (Corner, checks, None);
            },
            After::Cleared,
        ),
        (
            "增加一段弯道",
            whole,
            |d| {
                let segment = new_segment(2, Corner, &d.segment_defaults);
                d.segments.push(segment);
            },
            After::Cleared,
        ),
        (
            "只改帧数容差",
            segments,
            |d| d.segments[0].tolerance = 3,
            After::Kept,
        ),
        (
            "换胶偏参考相机",
            segments,
            |d| d.segments[0].offset_ref.as_mut().unwrap().camera = "cam3".into(),
            After::XRefCleared,
        ),
    ];
    for (name, entry, edit, after) in cases {
        let mut w = calibrated();
        let before = w.doc.clone();
        assert_eq!(
            before.segments[0].offset_ref.as_ref().and_then(|r| r.x_ref),
            Some(0.5),
            "前提：示教件已测出 x_ref"
        );
        let mut doc = w.doc.clone();
        edit(&mut doc);
        let result = entry(&mut w, doc);
        let segment: &PathSegment = &w.doc.segments[0];
        let x_ref = segment.offset_ref.as_ref().and_then(|r| r.x_ref);
        assert!(result.is_ok(), "{name}：{result:?}");
        let state = match (segment.expected_frames, x_ref) {
            _ if w.doc == before => After::Ignored,
            (None, None) if w.doc.segments.iter().all(|s| s.calib.is_none()) => After::Cleared,
            (Some(15), Some(0.5)) => After::Kept,
            (Some(15), None) => After::XRefCleared,
            other => panic!("{name}：{other:?}"),
        };
        assert_eq!(state, after, "{name}");
    }
}

#[test]
fn candidate_cameras_take_frames_by_camera_id_and_absent_cameras_have_no_frames() {
    // 防的回归（D-SB-58）：任意候选复测时按下标取记录的原图，候选相机顺序与记录不同就读到别的相机；记录里没有的相机被当成有原图
    use crate::store::FrameRecord;
    let frame = |camera: usize, seq: u64| FrameRecord {
        camera,
        seq,
        status: FrameState::Measured,
        frame_counter: Some(seq),
        trigger_counter: None,
        region: None,
        metrics: None,
        raw_file: Some(format!("c{camera}-{seq}")),
    };
    let frames: Vec<FrameRecord> = (1..=2)
        .flat_map(|seq| (0..3).map(move |c| frame(c, seq)))
        .collect();
    // 记录的相机 [cam1, cam2, cam3]，候选的 [cam3, cam1, camX]
    let mapped = super::burst_history::candidate_frames(&[Some(2), Some(0), None], &frames);
    let of = |camera: usize| {
        mapped
            .iter()
            .filter(|f| f.camera == camera)
            .map(|f| (f.seq, f.status, f.raw_file.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        of(0),
        vec![
            (1, FrameState::Measured, Some("c2-1".into())),
            (2, FrameState::Measured, Some("c2-2".into()))
        ]
    );
    assert_eq!(
        of(1),
        vec![
            (1, FrameState::Measured, Some("c0-1".into())),
            (2, FrameState::Measured, Some("c0-2".into()))
        ]
    );
    assert_eq!(
        of(2),
        vec![
            (1, FrameState::Missing, None),
            (2, FrameState::Missing, None)
        ]
    );
}

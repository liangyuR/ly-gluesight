use super::*;

use crate::recipe::RecipeDoc;
use crate::segmentation::Region;
use crate::store::burst_fixture;

/// 段配方：直线段 + 弯道段（N_exp 待分段算法），整件标定 6 个帧组。
fn burst_doc() -> RecipeDoc {
    let mut doc = crate::recipe::samples_burst();
    let defaults = doc.segment_defaults.clone();
    doc.segments = vec![crate::plan::new_segment(1, SegmentKind::Straight, &defaults), crate::plan::new_segment(2, SegmentKind::Corner, &defaults)];
    doc.part_calib = Some(crate::plan::PartCalib { expected_total: 6, calib: crate::plan::SegmentCalib { rounds: 10, min: 5, max: 7, mean: 6.0 } });
    doc
}

/// 一件段配方记录：直线区域 seq 1–3、弯道区域 seq 4–6；相机 1 第 5 帧整帧胶宽 6.5 mm（超出默认上限 5.5 mm），
/// 相机 3 第 2 帧缺帧（检测不完整）。原结论按配方算出，为胶宽 NG。
fn burst_row(recipe: &Recipe) -> (StoredMeasurement, Vec<FrameRecord>) {
    let mut frames = burst_fixture::frames(6, &[(2, 2)]);
    frames.iter_mut().find(|frame| (frame.camera, frame.seq) == (0, 5)).unwrap().metrics = Some(burst_fixture::measured(0, 5, 6.5));
    let regions = vec![
        Region { kind: RegionKind::Straight { segment: 1 }, first_seq: 1, last_seq: 3 },
        Region { kind: RegionKind::Corner { segment: 2 }, first_seq: 4, last_seq: 6 },
    ];
    let (_, burst) = burst_fixture::record(recipe, frames, Some(regions), Some(6));
    let row = StoredMeasurement {
        id: 7,
        ts: 1234,
        sn: "SN-B7".into(),
        recipe_id: Some(recipe.id.clone()),
        recipe_revision: Some(recipe.revision_id.clone()),
        verdict: burst.judgement.verdict,
        burst: Some(burst.judgement),
    };
    (row, burst.frames)
}

/// 规格 §1 计划身份：段序列 / 类型 / 预期帧数、相机绑定变了，已存指标的区域口径就变了，用当前候选重判时跳过并记 LayoutChanged；
/// 走廊变了每帧指标要原图复测，同样跳过。容差、限值变化沿用计划身份，可以直接重判。
#[test]
fn rejudge_skips_when_plan_identity_changed_but_allows_tolerance_only_change() {
    type Edit = fn(&mut RecipeDoc);
    let cases: [(&str, Edit, Option<Verdict>); 6] = [
        ("弯道段改为直线段", |doc| doc.segments[1] = crate::plan::new_segment(2, SegmentKind::Straight, &doc.segment_defaults.clone()), None),
        ("相机顺序", |doc| doc.inspect.swap(0, 1), None),
        ("整件重新标定为 7 个帧组", |doc| doc.part_calib.as_mut().unwrap().expected_total = 7, None),
        ("相机 1 的走廊", |doc| doc.inspect[0].corridor[0][0] += 5.0, None),
        ("弯道段帧数容差", |doc| doc.segments[1].tolerance = 3, Some(Verdict::NgWidth)),
        ("弯道段胶宽上允差", |doc| doc.segments[1].checks.width.tol_upper = 3.0, Some(Verdict::Ok)),
    ];
    let original = Arc::new(burst_doc().build().unwrap());
    let request = RejudgeRequest { use_current_recipe: true, ..Default::default() };
    for (case, edit, rejudged) in cases {
        let mut doc = burst_doc();
        edit(&mut doc);
        let candidate = Arc::new(doc.build().unwrap());
        let (row, frames) = burst_row(&original);
        let result = rejudge_rows(vec![row], &request, |_| Ok(Some(original.clone())), |_| Some(candidate.clone()), |_| Ok(frames.clone())).unwrap();
        match rejudged {
            None => assert_eq!(result.skip_reasons.iter().map(|skip| skip.kind).collect::<Vec<_>>(), [SkipKind::LayoutChanged], "{case}"),
            Some(verdict) => assert_eq!(result.matrix.iter().map(|cell| (cell.from, cell.to)).collect::<Vec<_>>(), [(Verdict::NgWidth, verdict)], "{case}"),
        }
    }
}

use super::*;

fn recipe() -> Recipe {
    let mut recipe = crate::recipe::samples().remove(1).build().unwrap();
    recipe.filter_window = 1;
    for segment in &mut recipe.segments {
        segment.width = Some(JudgeParams { nominal: 4.0, tol_upper: 0.5, tol_lower: 0.5, abs_min: 0.0, abs_max: 100.0, max_excursion_len: 0.0 });
    }
    recipe
}

fn row(recipe: &Recipe, id: i64) -> StoredMeasurement {
    let table = vec![crate::judge::PointState::Measured { d: Some(0.0), w: 6.0 }; recipe.point_count()];
    StoredMeasurement {
        id,
        ts: 1234,
        sn: "SN-42".into(),
        recipe_id: Some(recipe.id.clone()),
        recipe_revision: Some(recipe.revision_id.clone()),
        verdict: judge::judge(recipe, &table).verdict,
        table,
        burst: None,
    }
}

/// 旧拍照点记录重判不读段配方帧记录。
fn no_frames(_: i64) -> Result<Vec<FrameRecord>, String> {
    panic!("旧拍照点记录不读帧记录")
}

#[test]
fn missing_or_unidentified_snapshot_never_falls_back_to_current_recipe() {
    let path = std::env::temp_dir().join(format!("gluesight-history-snapshot-{}-{}.sqlite", std::process::id(), ly_plc::now_ms()));
    let store = Store::open(&path).unwrap();
    assert!(record_recipe(&store, Some("missing"), || panic!("不得读取当前配方")).unwrap().is_none());
    assert!(record_recipe(&store, None, || panic!("无身份历史不得读取当前配方")).unwrap().is_none());
    drop(store);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn current_recipe_with_same_point_count_requires_same_complete_layout() {
    let original = Arc::new(recipe());
    let mut variants = Vec::new();
    let mut candidate = (*original).clone();
    candidate.shots[0].id = "other-shot".into();
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.shots[0].pose_id = "other-pose".into();
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.shots[0].camera = "other-camera".into();
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.shots[0].view = 2;
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.points.x[0] += 1.0;
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.points.k.swap(0, original.segments[1].first);
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.shots[0].mm_per_px = candidate.shots[0].mm_per_px.map(|m| m * 2.0);
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.shots[0].calib = Some("other-calibration".into());
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.shots[0].path[0][0] += 1.0;
    variants.push(candidate);
    let mut candidate = (*original).clone();
    candidate.spacing *= 2.0;
    variants.push(candidate);
    let request = RejudgeRequest { use_current_recipe: true, ..Default::default() };
    for candidate in variants {
        assert_eq!(candidate.point_count(), original.point_count());
        let result = rejudge_rows(vec![row(&original, 1)], &request, |_| Ok(Some(original.clone())), |_| Some(Arc::new(candidate.clone())), no_frames).unwrap();
        assert_eq!((result.total, result.skipped), (0, 1));
        assert_eq!(result.skip_reasons[0].kind, SkipKind::LayoutChanged);
        assert!(result.changes.is_empty());
    }
    let mut other_revision = (*original).clone();
    other_revision.version += 1;
    other_revision.revision_id = format!("{}-v{}", other_revision.id, other_revision.version);
    other_revision.shots[0].view = 2;
    let other_revision = Arc::new(other_revision);
    let result = rejudge_rows(vec![row(&original, 1), row(&other_revision, 2)], &request, |revision_id| {
        Ok(Some(if revision_id == original.revision_id { original.clone() } else { other_revision.clone() }))
    }, |_| Some(original.clone()), no_frames).unwrap();
    assert_eq!((result.total, result.skipped), (1, 1));
    assert_eq!(result.skip_reasons[0].id, 2);
}

#[test]
fn threshold_only_changes_are_rejudged_without_altering_original_result() {
    let original = Arc::new(recipe());
    let mut candidate = (*original).clone();
    candidate.version += 1;
    candidate.revision_id = format!("{}-v{}", candidate.id, candidate.version);
    candidate.filter_window = 3;
    for segment in &mut candidate.segments { segment.width.as_mut().unwrap().tol_upper = 3.0; }
    let request = RejudgeRequest { use_current_recipe: true, ..Default::default() };
    let result = rejudge_rows(vec![row(&original, 1)], &request, |_| Ok(Some(original.clone())), |_| Some(Arc::new(candidate.clone())), no_frames).unwrap();
    assert_eq!((result.total, result.skipped), (1, 0));
    assert_eq!((result.changes[0].from, result.changes[0].to), (Verdict::NgWidth, Verdict::Ok));
    assert_eq!(row(&original, 2).verdict, Verdict::NgWidth);
    let request = RejudgeRequest { overrides: Overrides { width: KindOverride { tol_upper: Some(3.0), ..Default::default() }, ..Default::default() }, ..Default::default() };
    let result = rejudge_rows(vec![row(&original, 1)], &request, |_| Ok(Some(original.clone())), |_| panic!("原配方重判不读当前配方"), no_frames).unwrap();
    assert_eq!(result.changes[0].to, Verdict::Ok);
}

#[test]
fn unavailable_or_inconsistent_original_inputs_are_explicitly_skipped() {
    let original = Arc::new(recipe());
    let mut no_table = row(&original, 1);
    no_table.table.clear();
    let mut err = row(&original, 2);
    err.verdict = Verdict::ErrInspect;
    let mut no_revision = row(&original, 3);
    no_revision.recipe_revision = None;
    let mut bad_layout = row(&original, 4);
    bad_layout.table.pop();
    let result = rejudge_rows(vec![no_table, err, no_revision, bad_layout], &RejudgeRequest::default(), |_| Ok(Some(original.clone())), |_| None, no_frames).unwrap();
    assert_eq!((result.total, result.skipped), (0, 4));
    assert_eq!(result.skip_reasons.iter().map(|skip| (skip.id, skip.kind)).collect::<Vec<_>>(),
        [(1, SkipKind::NoTable), (2, SkipKind::ErrResult), (3, SkipKind::NoRevision), (4, SkipKind::LayoutMismatch)]);
    let result = rejudge_rows(vec![row(&original, 5)], &RejudgeRequest { use_current_recipe: true, ..Default::default() }, |_| Ok(None), |_| Some(original.clone()), no_frames).unwrap();
    assert_eq!(result.skip_reasons[0].kind, SkipKind::SnapshotMissing, "快照缺失不能退回当前配方");
    assert_eq!(rejudge_rows(vec![row(&original, 6)], &RejudgeRequest::default(), |_| Err("数据库读取失败".into()), |_| None, no_frames).unwrap_err(), "数据库读取失败");
}

#[test]
fn fixture_recipe_with_the_same_template_source_rejudges_a_nominal_only_change() {
    // 防的回归（定稿 §7“只改名义 → 历史可重判”）：含特征定位的配方一律拒绝复用（G2 的权宜）。
    // 来源换了由测量布局拦下，见 workspace 的测量布局用例
    let mut doc = crate::recipe::samples().remove(1);
    doc.shots[0].fixture = Some(crate::recipe::FixtureSpec { rect: [100, 100, 32, 32], source: "capture-1-0-v1".into(), search_radius_px: 20.0,
        angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 });
    let original = Arc::new(doc.build().unwrap());
    let table = vec![crate::judge::PointState::Measured { d: Some(0.0), w: 4.0 }; original.point_count()];
    let stored = StoredMeasurement { table: table.clone(), verdict: judge::judge(&original, &table).verdict, ..row(&original, 1) };
    let mut next = doc.clone();
    next.version += 1;
    next.limits.position.as_mut().unwrap().nominal = 3.0;
    next.limits.gauge_ref = Some("GA-0315".into());
    let renominal = Arc::new(next.build().unwrap());
    let request = RejudgeRequest { use_current_recipe: true, ..Default::default() };
    let result = rejudge_rows(vec![stored], &request, |_| Ok(Some(original.clone())), |_| Some(renominal.clone()), no_frames).unwrap();
    assert_eq!((result.total, result.skipped), (1, 0));
    assert_eq!((result.changes[0].from, result.changes[0].to), (Verdict::Ok, Verdict::NgPosition));
}

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
        table: Vec::new(),
        burst: Some(burst.judgement),
    };
    (row, burst.frames)
}

/// 规格 §1：允差按段类型——直线段的覆盖值不作用于弯道区域；§0：检测不完整等标记与阈值无关，重判后照常带出。
#[test]
fn rejudge_burst_part_with_kind_override_keeps_incomplete_flag() {
    let recipe = Arc::new(burst_doc().build().unwrap());
    assert_eq!(burst_row(&recipe).0.verdict, Verdict::NgWidth, "夹具：弯道区域一帧胶宽超差");
    let widen = KindOverride { tol_upper: Some(3.0), ..Default::default() };
    for (case, overrides, changed_to) in [
        ("只放宽直线段", Overrides { straight: widen.clone(), ..Default::default() }, None),
        ("放宽弯道段", Overrides { corner: widen.clone(), ..Default::default() }, Some(Verdict::Ok)),
    ] {
        let (row, frames) = burst_row(&recipe);
        let request = RejudgeRequest { overrides, ..Default::default() };
        let result = rejudge_rows(vec![row], &request, |_| Ok(Some(recipe.clone())), |_| panic!("原配方重判不读当前配方"), |_| Ok(frames.clone())).unwrap();
        assert_eq!((result.total, result.skipped), (1, 0), "{case}");
        assert_eq!(result.changes.first().map(|change| change.to), changed_to, "{case}");
        if changed_to.is_some() {
            assert!(result.changes[0].flags.as_ref().is_some_and(|flags| flags.incomplete), "{case}：缺帧标记与阈值无关");
        }
    }
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

/// 导出契约：段配方记录在 CSV 末尾带 ΣN_exp、Σtol、各相机帧数、deviceTriggers 与整件标记；旧记录这几列留空，列数与表头一致。
#[test]
fn csv_export_of_burst_part_contains_expected_total_camera_frames_and_flags_columns() {
    let path = std::env::temp_dir().join(format!("gluesight-history-csv-{}-{}.sqlite", std::process::id(), ly_plc::now_ms()));
    let store = Store::open(&path).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, burst) = burst_fixture::record(&recipe, burst_fixture::frames(4, &[(1, 2)]), None, Some(5));
    fn part<'a>(recipe: Option<&'a Recipe>, judgement: &'a judge::Judgement, cycle: &'a str, burst: Option<&'a crate::store::BurstRecord>,
        delivery: &'a crate::store::PlcDelivery) -> crate::store::PartRecord<'a> {
        let frames = burst.map_or(0, |burst| burst.judgement.group_count as usize);
        crate::store::PartRecord {
            ts: 1234, sn: "SN-CSV", recipe, judgement, drain_ms: None, frames: &[], frames_expected: frames, frames_received: frames, triggers: 5,
            table: None, software_version: "csv-test", cycle_id: Some(cycle), bundle_id: None, delivery, shots: &[],
            source: crate::store::RecordSource::Real, sample: None, burst,
        }
    }
    let delivery = crate::store::PlcDelivery::default();
    let legacy = judge::Judgement::error(1, "旧记录");
    store.insert(&part(Some(&recipe), &judgement, "burst", Some(&burst), &delivery)).unwrap();
    store.insert(&part(None, &legacy, "legacy", None, &delivery)).unwrap();
    let header: Vec<&str> = CSV_HEADER.trim_start_matches('\u{feff}').trim_end().split(',').collect();
    let tail = |summary: &PartSummary| {
        let row = csv_row(summary);
        let cells: Vec<&str> = row.trim_end().split(',').collect();
        assert_eq!(cells.len(), header.len(), "{row}");
        cells[cells.len() - 5..].iter().map(|cell| cell.to_string()).collect::<Vec<_>>()
    };
    assert_eq!(header[header.len() - 5..], ["预期帧组数(ΣN_exp)", "帧组容差(Σtol)", "各相机帧数", "deviceTriggers", "标记"]);
    let page = store.query(&HistoryQuery::default()).unwrap();
    let by_cycle = |cycle: &str| page.items.iter().find(|item| item.cycle_id.as_deref() == Some(cycle)).unwrap();
    assert_eq!(tail(by_cycle("burst")), ["4", "1", "4/3/4", "5", "\"检测不完整、胶偏未判、含未分段区域\""]);
    assert_eq!(tail(by_cycle("legacy")), ["", "", "", "", "\"\""]);
    drop(store);
    let _ = std::fs::remove_file(&path);
}

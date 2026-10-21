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
    }
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
        let result = rejudge_rows(vec![row(&original, 1)], &request, |_| Ok(Some(original.clone())), |_| Some(Arc::new(candidate.clone()))).unwrap();
        assert_eq!((result.total, result.skipped), (0, 1));
        assert!(result.skip_reasons[0].reason.contains("需要使用原图复测"));
        assert!(result.changes.is_empty());
    }
    let mut other_revision = (*original).clone();
    other_revision.version += 1;
    other_revision.revision_id = format!("{}-v{}", other_revision.id, other_revision.version);
    other_revision.shots[0].view = 2;
    let other_revision = Arc::new(other_revision);
    let result = rejudge_rows(vec![row(&original, 1), row(&other_revision, 2)], &request, |revision_id| {
        Ok(Some(if revision_id == original.revision_id { original.clone() } else { other_revision.clone() }))
    }, |_| Some(original.clone())).unwrap();
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
    let result = rejudge_rows(vec![row(&original, 1)], &request, |_| Ok(Some(original.clone())), |_| Some(Arc::new(candidate.clone()))).unwrap();
    assert_eq!((result.total, result.skipped), (1, 0));
    assert_eq!((result.changes[0].from, result.changes[0].to), (Verdict::NgWidth, Verdict::Ok));
    assert_eq!(row(&original, 2).verdict, Verdict::NgWidth);
    let request = RejudgeRequest { overrides: Overrides { width: KindOverride { tol_upper: Some(3.0), ..Default::default() }, ..Default::default() }, ..Default::default() };
    let result = rejudge_rows(vec![row(&original, 1)], &request, |_| Ok(Some(original.clone())), |_| panic!("原配方重判不读当前配方")).unwrap();
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
    let result = rejudge_rows(vec![no_table, err, no_revision, bad_layout], &RejudgeRequest::default(), |_| Ok(Some(original.clone())), |_| None).unwrap();
    assert_eq!((result.total, result.skipped, result.skip_reasons.len()), (0, 4, 4));
    let result = rejudge_rows(vec![row(&original, 5)], &RejudgeRequest { use_current_recipe: true, ..Default::default() }, |_| Ok(None), |_| Some(original.clone())).unwrap();
    assert_eq!(result.skip_reasons[0].reason, "原配方快照缺失，无法核对测点布局");
    assert_eq!(rejudge_rows(vec![row(&original, 6)], &RejudgeRequest::default(), |_| Err("数据库读取失败".into()), |_| None).unwrap_err(), "数据库读取失败");
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
    let result = rejudge_rows(vec![stored], &request, |_| Ok(Some(original.clone())), |_| Some(renominal.clone())).unwrap();
    assert_eq!((result.total, result.skipped), (1, 0));
    assert_eq!((result.changes[0].from, result.changes[0].to), (Verdict::Ok, Verdict::NgPosition));
}

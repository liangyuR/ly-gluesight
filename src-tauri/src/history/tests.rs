use super::*;
use crate::store::{PlcDelivery, PlcDeliveryState};

fn recipe() -> Recipe {
    let mut recipe = crate::recipe::samples().remove(1).build().unwrap();
    recipe.filter_window = 1;
    for segment in &mut recipe.segments {
        segment.width = Some(JudgeParams { nominal: 4.0, tol_upper: 0.5, tol_lower: 0.5, abs_min: 0.0, abs_max: 100.0, max_excursion_len: 0.0 });
    }
    recipe
}

fn row(recipe: &Recipe, id: i64) -> StoredMeasurement {
    let table = vec![crate::judge::PointState::Measured { d: 0.0, w: 6.0 }; recipe.point_count()];
    StoredMeasurement {
        id,
        ts: 1234,
        sn: 42,
        recipe_id: Some(recipe.id.clone()),
        recipe_revision: Some(recipe.revision_id.clone()),
        verdict: judge::judge(recipe, &table).verdict,
        table,
        cycle_id: Some(format!("cycle-{id}")),
        bundle_id: Some("bundle-id".into()),
        delivery: PlcDelivery::default(),
    }
}

#[test]
fn a_named_missing_snapshot_never_falls_back_to_current_recipe() {
    let path = std::env::temp_dir().join(format!("gluesight-history-snapshot-{}-{}.sqlite", std::process::id(), ly_plc::now_ms()));
    let store = Store::open(&path).unwrap();
    let current = Arc::new(recipe());
    assert!(record_recipe(&store, Some("missing"), || panic!("不得读取当前配方")).unwrap().is_none());
    assert_eq!(record_recipe(&store, None, || Some(current.clone())).unwrap().unwrap().revision_id, current.revision_id);
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
fn each_original_snapshot_is_checked_even_with_the_same_recipe_id() {
    let first = Arc::new(recipe());
    let mut second = (*first).clone();
    second.version += 1;
    second.revision_id = format!("{}-v{}", second.id, second.version);
    second.shots[0].view = 2;
    let second = Arc::new(second);
    let result = rejudge_rows(vec![row(&first, 1), row(&second, 2)], &RejudgeRequest { use_current_recipe: true, ..Default::default() }, |revision_id| {
        Ok(Some(if revision_id == first.revision_id { first.clone() } else { second.clone() }))
    }, |_| Some(first.clone())).unwrap();
    assert_eq!((result.total, result.skipped), (1, 1));
    assert_eq!(result.skip_reasons[0].id, 2);
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
fn csv_preserves_full_identity_and_delivery_with_escaped_text() {
    let summary = PartSummary {
        id: 1,
        ts: 1234,
        sn: 42,
        recipe_id: Some("recipe-id".into()),
        recipe_version: Some(7),
        recipe_revision: Some("recipe-id-v7".into()),
        trigger_mode: Some("fly".into()),
        verdict: Verdict::Ok,
        plc_code: 1,
        fault_code: 0,
        reason: "包含,逗号与\"引号\"\n换行".into(),
        drain_ms: Some(10),
        frames_expected: 4,
        frames_received: 4,
        retest_of: None,
        cycle_id: Some("cycle-id".into()),
        bundle_id: Some("full-frozen-bundle-id".into()),
        delivery: PlcDelivery { state: PlcDeliveryState::Acknowledged, updated_at: 2345, message: Some("PLC \"已确认\"".into()) },
    };
    let csv = csv_row(&summary);
    assert!(csv.contains("\"recipe-id-v7\""));
    assert!(csv.contains("\"包含,逗号与\"\"引号\"\"\n换行\""));
    assert!(csv.contains(",\"cycle-id\",\"full-frozen-bundle-id\",acknowledged,"));
    assert!(csv.ends_with(",\"PLC \"\"已确认\"\"\"\r\n"));
}

use super::*;

#[test]
fn offline_validated_release_runs_only_in_offline_mode() {
    let cases = [
        ("真机 + 离线验证版本 → 拒绝", ValidationSite::Offline, false, false),
        ("离线 + 离线验证版本 → 可运行", ValidationSite::Offline, true, true),
        ("真机 + 真机验证版本 → 可运行", ValidationSite::Real, false, true),
    ];
    for (name, site, offline, admitted) in cases {
        assert_eq!(site_admits(site, offline).is_ok(), admitted, "{name}");
    }
}

/// 正常模式生产准入：采集版没有发布包、没示教标定，必须拒收（cycle 回 90/95）；正式版照常往下走。
#[test]
fn collect_recipe_is_refused_by_normal_production_admission() {
    let mut doc = crate::recipe::samples_burst();
    assert_eq!(stage_admits(&doc.build().unwrap()), Ok(()));
    doc.stage = crate::recipe::RecipeStage::Collect;
    assert_eq!(
        stage_admits(&doc.build().unwrap()),
        Err(COLLECT_ONLY.to_string())
    );
}

use super::*;

#[test]
fn graphs_preserve_each_shot_and_only_skip_explicitly_skipped_shots() {
    let mut doc = crate::recipe::samples().remove(1);
    doc.shots[1].skip = true;
    doc.shots[2].view = 3;
    for shot in &mut doc.shots { shot.mm_per_px = Some(0.112); }
    let graph = graphs(&doc.build().unwrap()).unwrap();
    assert_eq!(graph["shots"].as_array().unwrap().len(), 3);
    assert_eq!(graph["shots"][1]["k"], 2);
    assert!(graph["shots"][0]["graph"].is_object());
    assert_eq!(graph["shots"][1]["camera"], "cam1");
    assert_eq!(graph["shots"][1]["view"], 3);
    let stored: Value = serde_json::from_slice(&serde_json::to_vec(&graph).unwrap()).unwrap();
    let restored: crate::recipe::RecipeDoc = serde_json::from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(stored, graphs(&restored.build().unwrap()).unwrap());
    doc.shots[0].path.clear();
    assert!(graphs(&doc.build().unwrap()).unwrap_err().contains("尚未示教"));
}

#[test]
fn release_runs_on_any_engine_of_the_same_series_only() {
    let cases = [
        ("1.1.0:4096:17", "1.1.0", true),
        ("1.1.0:4096:17", "1.1.7", true),
        ("1.1.3:4096:17", "1.1.0", true),
        ("1.1.0:4096:17", "1.2.0", false),
        ("1.1.0:4096:17", "2.1.0", false),
        ("bad:4096:17", "1.1.0", false),
    ];
    for (published, loaded, runs) in cases {
        assert_eq!(engine_version_matches(published, loaded), runs, "{published} → {loaded}");
    }
}

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

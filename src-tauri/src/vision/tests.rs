use super::*;

#[test]
fn invalid_frame_is_rejected_before_ffi() {
    assert!(image_input(&FrameImage::new(0, 1, vec![])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3, 4, 5])).is_err());
}

#[test]
fn calibration_keys_are_distinct_for_each_view_of_one_device() {
    assert_eq!(station_view_key("device1", 1).unwrap(), "device1-v1");
    assert_eq!(station_view_key("device1", 3).unwrap(), "device1-v3");
    assert!(station_view_key("device1", 0).is_err());
    assert!(station_view_key("device1", 4).is_err());
}

#[test]
fn unloaded_core_status_reuses_the_failure_within_backoff_and_reloads_after_it() {
    let dir = std::env::temp_dir().join(format!("gluesight-core-backoff-{}-{}", std::process::id(), ly_plc::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let core = dir.join("lyflow_core.dll");
    let runtime = || Some(dir.to_str().unwrap().to_string());
    let host = VisionHost::default();
    let now = Instant::now();
    let first = host.status_at(runtime(), now);
    std::fs::write(&core, b"not a dll").unwrap();
    let within = host.status_at(runtime(), now);
    let after = host.status_at(runtime(), now + STATUS_RELOAD_BACKOFF + Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!first.loaded && !within.loaded && !after.loaded);
    assert_eq!(within.message, first.message);
    assert_ne!(after.message, first.message);
}

#[test]
fn runtime_outside_the_locked_series_or_not_a_formal_build_is_refused() {
    let dir = std::env::temp_dir().join(format!("gluesight-runtime-descriptor-{}-{}", std::process::id(), ly_plc::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let descriptor = |engine: &str, abi: &str, arch: &str, local: bool| std::fs::write(dir.join("lyflow-runtime.json"),
        json!({"engineVersion": engine, "abiVersion": abi, "architecture": arch, "localValidationOnly": local}).to_string()).unwrap();
    let missing = check_runtime(&dir, "1.1");
    let mut refused = Vec::new();
    for (engine, abi, arch, local) in [("1.2.0", "1.2.0", "x64", false), ("1.1.3", "1.2", "x64", false), ("1.1.3", "1.1", "x86", false), ("1.1.3", "1.1", "x64", true)] {
        descriptor(engine, abi, arch, local);
        refused.push(check_runtime(&dir, "1.1").is_err());
    }
    descriptor("1.1.7", "1.1", "x64", false);
    let patch = check_runtime(&dir, "1.1");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(missing.unwrap_err().contains("缺少描述文件"));
    assert_eq!(refused, [true; 4]);
    assert!(patch.is_ok());
}

#[test]
fn operator_manifest_without_locate_or_without_template_pad_is_refused_at_the_matching_layer() {
    // 防的回归：必需算子清单漏掉 glue.locate（加载了不能定位的核心库）；或 1.0.0 的 glue.locate 冒充带外圈模板的版本
    let op = |id: &str, params: &[&str]| json!({"id": id, "params": params.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
        "outputs": [{"name": "pose"}, {"name": "score"}]});
    let base: Vec<Value> = REQUIRED_OPERATORS.split(',').filter(|id| *id != "glue.locate").map(|id| op(id, &[])).collect();
    let with = |locate: Option<Value>| json!({"operators": base.iter().cloned().chain(locate).collect::<Vec<_>>()});
    let released = op("glue.locate", &["template", "anchor", "searchRadius", "angleRange", "angleStep", "minScore", "maxPeakRatio"]);
    let pad = op("glue.locate", &["template", "anchor", "searchRadius", "angleRange", "angleStep", "minScore", "maxPeakRatio", "templatePad"]);
    // （清单，加载是否通过，locate 是否可用）
    for (name, manifest, loads, locates) in [
        ("没有 glue.locate", with(None), false, false),
        ("glue.locate 没有 templatePad", with(Some(released)), true, false),
        ("lyFlow#32 之后的 glue.locate", with(Some(pad)), true, true),
    ] {
        assert_eq!(check_operators(&manifest).is_ok(), loads, "{name}");
        assert_eq!(locate_support(&manifest).is_ok(), locates, "{name}");
    }
}

#[test]
fn every_operator_in_measurement_graphs_is_a_required_operator() {
    // 防的回归：测量图用了 requiredOperators（runtime.json）之外的算子，打包与加载时都查不出，换运行库后生产才失败
    // 样例配方原样一份、每个拍照点都带特征定位再一份，后者的图里必须真的有 n_locate，否则定位分支没被检查到
    let required: Vec<&str> = REQUIRED_OPERATORS.split(',').collect();
    let fixture = crate::recipe::FixtureSpec { rect: [100, 100, 32, 32], source: "capture-1-0-v1".into(), search_radius_px: 20.0, angle_range_deg: 0.0, min_score: 0.6,
        max_peak_ratio: 1.0, max_end_shift_mm: None };
    let mut located_nodes = 0;
    for sample in crate::recipe::samples() {
        let mut located = sample.clone();
        for shot in &mut located.shots {
            shot.fixture = Some(fixture.clone());
        }
        for doc in [sample, located] {
            let recipe = doc.build().unwrap();
            for k in 0..recipe.shot_count() {
                let graph = build_taught_graph(&recipe, k).unwrap();
                for node in graph["nodes"].as_array().unwrap() {
                    assert!(required.contains(&node["op"].as_str().unwrap()), "{}", node["op"]);
                    located_nodes += usize::from(node["id"] == "n_locate");
                }
            }
        }
    }
    assert!(located_nodes > 0, "带 fixture 的样例图里没有 n_locate");
}

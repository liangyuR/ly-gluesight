use super::*;

#[test]
fn invalid_frame_is_rejected_before_ffi() {
    assert!(image_input(&FrameImage::new(0, 1, vec![])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3, 4, 5])).is_err());
}

#[test]
fn device_key_and_each_view_key_of_one_device_never_collide() {
    // 防的回归：段配方的设备标定键 {device}（D-SB-30）与旧配方的 {device}-v{view} 落到同一个文件，标定一方就改掉另一方；
    // 或图编号越界、设备编号能拼出路径（标定文件名 plane_calib_{键}.json）
    let device = station_key("device1").unwrap();
    let views: Vec<String> = (1..=3).map(|v| station_view_key("device1", v).unwrap()).collect();
    assert_eq!(views, ["device1-v1", "device1-v2", "device1-v3"]);
    assert!(!views.contains(&device));
    assert!(station_view_key("device1", 0).is_err());
    assert!(station_view_key("device1", 4).is_err());
    for bad in ["", "../device1", "device 1"] {
        assert!(station_key(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn calibration_page_view_on_single_view_camera_also_writes_the_key_burst_recipe_reads() {
    // 防的回归（D-SB-30 前后端契约）：标定页（CalibPanel / FlyshotCalibrationPage）总带图编号、单视角相机为 1，标定只存成
    // 旧配方的 {device}-v1，段配方按 {device} 读不到——走廊示教的当量缺省与发布包标定 source 恒为空；或三目拼接相机的
    // 图 N 写进段配方的设备键；第一个键是标定页读回的键，必须仍是 {device}-v{view}
    use crate::camera::CameraConfig;
    let single = CameraConfig { id: "cam1".into(), ..CameraConfig::default() };
    let triple = CameraConfig { id: "cam2".into(), view_count: 3, ..CameraConfig::default() };
    let burst_reads = camera_calib_key(&CameraInspect { camera: "cam1".into(), corridor: Vec::new(), mm_per_px: None, calib: None, detect: None }).unwrap();
    // （相机，标定页传的图编号，要写的键：第一个也是读回的键）
    let cases = [(&single, 1, vec!["cam1-v1".to_string(), burst_reads]), (&triple, 1, vec!["cam2-v1".into()]), (&triple, 2, vec!["cam2-v2".into()])];
    for (config, view, keys) in cases {
        assert_eq!(calib_keys(config, Some(view)).unwrap(), keys, "{} 图 {view}", config.id);
    }
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
    assert!(missing.is_err());
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
        max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 };
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

#[test]
fn runtime_without_padded_template_support_refuses_only_recipes_using_fixture() {
    // 防的回归：已发布的 1.1.4 运行库（locate 1.0.0）挡住了没有特征定位的配方的发布与生产加载（现场打包停摆）；
    // 或含特征定位的配方在它上面照常发布、加载，到生产每帧才失败
    let unsupported: Result<(), String> = Err("glue.locate 不支持 templatePad".into());
    let plain = crate::recipe::samples().remove(1).build().unwrap();
    let mut located = crate::recipe::samples().remove(1);
    located.shots[0].fixture = Some(crate::recipe::FixtureSpec { rect: [100, 100, 32, 32], source: "capture-1-0-v1".into(),
        search_radius_px: 20.0, angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 });
    let located = located.build().unwrap();
    assert!(require_locate(&plain, &unsupported).is_ok());
    assert!(require_locate(&located, &unsupported).unwrap_err().contains("templatePad"));
    assert!(require_locate(&located, &Ok(())).is_ok());
}

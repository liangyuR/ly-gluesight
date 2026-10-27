use super::*;

#[test]
fn calibration_page_view_on_single_view_camera_also_writes_the_key_burst_recipe_reads() {
    // 防的回归（D-SB-30 前后端契约）：标定页（CalibPanel / FlyshotCalibrationPage）总带图编号、单视角相机为 1，标定只存成
    // 旧配方的 {device}-v1，段配方按设备标定读不到——走廊示教的当量缺省与发布包标定 source 恒为空；或三目拼接相机的
    // 图 N 写进段配方的设备标定；第一个键是标定页读回的键，必须仍是 {device}-v{view}
    use crate::camera::CameraConfig;
    let single = CameraConfig {
        id: "cam1".into(),
        ..CameraConfig::default()
    };
    let triple = CameraConfig {
        id: "cam2".into(),
        view_count: 3,
        ..CameraConfig::default()
    };
    let burst_reads = camera_calib_key(&CameraInspect {
        camera: "cam1".into(),
        mm_per_px: None,
        calib: None,
        detect: None,
        segments: Vec::new(),
        window: None,
        nozzle: None,
    })
    .unwrap();
    let station = |key: &str| CalibKey::Station(key.into());
    // （相机，标定页传的图编号，要写的键：第一个也是读回的键）
    let cases = [(&single, 1, vec![station("cam1-v1"), burst_reads]), (&triple, 1, vec![station("cam2-v1")]), (&triple, 2, vec![station("cam2-v2")])];
    for (config, view, keys) in cases {
        assert_eq!(calib_keys(config, Some(view)).unwrap(), keys, "{} 图 {view}", config.id);
    }
}

#[test]
fn latest_frame_calibration_stores_project_relative_image() {
    let root = std::env::temp_dir().join(format!(
        "calibration-latest-{}-{}",
        std::process::id(),
        ly_plc::now_ms()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("calib/device/plane_calib_cam1.json");
    let image = FrameImage::new(2, 1, vec![10, 20]);
    save_calibration(
        &root,
        std::slice::from_ref(&path),
        json!({"mmPerPx": 0.1}),
        &image,
        None,
        "cam1",
        1,
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let relative = doc["samplePath"].as_str().unwrap();
    assert_eq!(
        relative,
        format!("calib/images/{}.pgm", doc["sampleId"].as_str().unwrap())
    );
    assert_eq!(
        crate::image_file::load(&root.join(relative))
            .unwrap()
            .pixels,
        image.pixels
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_calibration_image_write_keeps_previous_result() {
    let root = std::env::temp_dir().join(format!(
        "calibration-failed-{}-{}",
        std::process::id(),
        ly_plc::now_ms()
    ));
    std::fs::create_dir_all(root.join("calib/images")).unwrap();
    let path = root.join("calib/plane_calib_cam1-v1.json");
    std::fs::write(&path, "previous").unwrap();
    std::fs::write(root.join("calib/images/sample.pgm"), b"different image").unwrap();
    let image = FrameImage::new(2, 1, vec![10, 20]);
    assert!(save_calibration(
        &root,
        std::slice::from_ref(&path),
        json!({}),
        &image,
        Some("sample"),
        "cam1",
        1
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "previous");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recalibration_with_same_frozen_image_updates_result_without_changing_image() {
    let root = std::env::temp_dir().join(format!(
        "calibration-repeat-{}-{}",
        std::process::id(),
        ly_plc::now_ms()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("calib/plane_calib_cam1-v1.json");
    let image = FrameImage::new(2, 1, vec![10, 20]);
    for mm_per_px in [0.1, 0.2] {
        save_calibration(
            &root,
            std::slice::from_ref(&path),
            json!({"mmPerPx": mm_per_px}),
            &image,
            Some("sample"),
            "cam1",
            1,
        )
        .unwrap();
    }
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(doc["data"]["mmPerPx"], json!(0.2));
    assert_eq!(
        crate::image_file::load(&root.join(doc["samplePath"].as_str().unwrap()))
            .unwrap()
            .pixels,
        image.pixels
    );
    std::fs::remove_dir_all(root).unwrap();
}

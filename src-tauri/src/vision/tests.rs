use super::*;

#[test]
fn saved_calibration_sample_reads_back_image_and_corners_and_rejects_unsafe_sample_id() {
    let root = std::env::temp_dir().join(format!(
        "calibration-sample-{}-{}",
        std::process::id(),
        ly_plc::now_ms()
    ));
    let path = root.join("calib/plane_calib_cam1-v1.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let image = FrameImage::new(2, 1, vec![10, 20]);
    let corners = [[1.5, 0.5], [0.5, 0.25]];
    save_calibration(
        &root,
        std::slice::from_ref(&path),
        json!({"mmPerPx": 0.05}),
        &corners,
        &image,
        Some("sample"),
        "cam1",
        1,
    )
    .unwrap();

    let info = calib_info(&path).unwrap();
    assert_eq!(
        (info.sample_id.as_deref(), info.corners.as_deref()),
        (Some("sample"), Some(&corners[..]))
    );
    let (id, restored, _) = calib_sample(&root, &path).unwrap().unwrap();
    assert_eq!(
        (
            id.as_str(),
            restored.width,
            restored.height,
            restored.pixels.to_vec()
        ),
        ("sample", 2, 1, vec![10, 20])
    );

    std::fs::write(&path, json!({"sampleId": "../sample"}).to_string()).unwrap();
    assert!(calib_sample(&root, &path).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

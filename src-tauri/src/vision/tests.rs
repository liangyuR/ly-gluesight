use super::*;

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

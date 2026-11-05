use super::*;

#[test]
fn color_frame_enters_lyflow_as_three_channels_with_full_buffer() {
    // 防的回归：注入 lyFlow 时通道数写死 1 或长度按 宽 × 高 校验，彩色帧被拒或只送前三分之一的字节，
    // lyFlow 把 RGB 交错当灰度量出错位的胶边
    let rgb: Vec<u8> = (0..2 * 2 * 3).collect();
    let input = image_input(&FrameImage::rgb(2, 2, rgb.clone())).unwrap();
    assert_eq!((input.channels, input.pixels), (3, rgb.clone()));
    assert_eq!(
        image_input(&FrameImage::new(2, 2, vec![1; 4]))
            .unwrap()
            .channels,
        1
    );
    assert!(
        image_input(&FrameImage::rgb(2, 2, rgb[..4].to_vec())).is_err(),
        "RGB 缓冲按 宽 × 高 截断"
    );
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
        &[],
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

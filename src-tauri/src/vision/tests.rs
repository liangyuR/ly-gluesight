use super::*;

#[test]
fn configured_core_path_takes_priority_without_resolving_install_directory() {
    let actual = resolve_core_path(Some("  D:/custom/lyflow_core.dll  "), || panic!("explicit path must win")).unwrap();
    let expected = if cfg!(windows) { r"D:\custom\lyflow_core.dll" } else { "D:/custom/lyflow_core.dll" };
    assert_eq!(actual, Some(PathBuf::from(expected)));
}

#[cfg(windows)]
#[test]
fn empty_core_settings_resolve_relative_to_executable() {
    let exe = PathBuf::from(r"D:\安装目录\GlueSight\GlueSight.exe");
    let expected = PathBuf::from(r"D:\安装目录\GlueSight\runtime\lyflow\lyflow_core.dll");
    for setting in [None, Some(""), Some(" \t ")] {
        assert_eq!(resolve_core_path(setting, || Ok(exe.clone())).unwrap(), Some(expected.clone()));
    }
    assert!(resolve_core_path(None, || Err(std::io::Error::other("unavailable"))).unwrap_err().contains("无法确定内置 lyFlow"));
}

#[cfg(not(windows))]
#[test]
fn other_platforms_do_not_use_windows_bundle() {
    assert_eq!(resolve_core_path(None, || panic!("no Windows bundle")).unwrap(), None);
}

#[test]
fn failed_explicit_core_path_is_reported_without_falling_back() {
    let host = VisionHost::default();
    let path = std::env::temp_dir().join(format!("gluesight-missing-core-{}", std::process::id())).join("lyflow_core.dll");
    let status = host.status(Some(&format!("  {}  ", path.display())));
    assert!(!status.loaded);
    assert_eq!(status.path, Some(path.display().to_string()));
    assert!(!status.message.is_empty());
}

#[test]
fn missing_bundled_core_reports_installation_repair() {
    let host = VisionHost::default();
    let path = std::env::temp_dir().join(format!("gluesight-missing-bundle-{}", std::process::id())).join("lyflow_core.dll");
    assert!(host.engine_at(&path, true).is_none());
    let error = host.error.lock().unwrap().clone().unwrap();
    assert!(error.contains("安装目录缺少内置 lyFlow 核心库"));
    assert!(error.contains(&path.display().to_string()));
}

#[test]
fn missing_pack_is_reported_before_running() {
    let error = check_operators(&json!({"operators": [{"id": "io.load_image"}]})).unwrap_err();
    assert!(error.contains("glue.taught_path"));
    assert!(error.contains("image.board_calib"));
    assert!(check_operators(&json!({"operators": [
        {"id": "io.load_image"}, {"id": "image.board_calib"}, {"id": "image.load_calib"},
        {"id": "glue.taught_path"}, {"id": "glue.bead_width"}
    ]})).is_ok());
    let old = json!({"operators": [
        {"id": "io.load_image"}, {"id": "image.board_calib"}, {"id": "image.load_calib"},
        {"id": "glue.locate"}, {"id": "glue.station_calipers"}, {"id": "glue.bead_width"}
    ]});
    assert!(check_operators(&old).unwrap_err().contains("旧版飞拍核心库"));
}

#[test]
fn invalid_frame_is_rejected_before_ffi() {
    assert!(image_input(&FrameImage::new(0, 1, vec![])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3, 4, 5])).is_err());
}

fn native_engine() -> Engine {
    let path = std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL to a core built with LYFLOW_PACKS=glue");
    Engine::load(Path::new(&path)).expect("The real image/glue core must load")
}

fn fixture_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../output/engine-tests").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save_pgm(path: &Path, width: u32, height: u32, pixels: &[u8]) {
    let mut data = format!("P5\n{width} {height}\n255\n").into_bytes();
    data.extend_from_slice(pixels);
    std::fs::write(path, data).unwrap();
}

#[test]
#[ignore = "requires a real core DLL with std-image and glue"]
fn native_board_calibration_uses_full_pixels_and_new_frames_do_not_reuse_old_board() {
    let engine = native_engine();
    let (width, height) = (640, 480);
    let mut pixels = vec![220; width * height];
    for y in 0..280 {
        for x in 0..400 {
            pixels[(y + 80) * width + x + 80] = if (x / 40 + y / 40) % 2 == 0 { 30 } else { 220 };
        }
    }
    let board = FrameImage::new(width as u32, height as u32, pixels);
    save_pgm(&fixture_dir("board").join("board.pgm"), board.width, board.height, &board.pixels);
    let graph = json!({"schemaVersion": 1, "id": "01TUJIAOBOARDCALIBTEST0000", "nodes": [
        {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
        {"id": "n_calib", "op": "image.board_calib", "params": {"pattern": [9, 6], "square": 5}}
    ], "edges": [{"id": "e0", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_calib", "port": "image"}}],
       "outputs": {"calib": {"node": "n_calib", "port": "calib"}}}).to_string();
    let calibrated = engine.run(&graph, "host-native-board", "", &board, &json!({})).unwrap();
    assert_eq!(calibrated.status(), "ok", "{}", calibrated.failure());
    let data = calibrated.record("calib").unwrap();
    assert_eq!(data["unit"], "mm");
    assert!((data["mmPerPx"].as_f64().unwrap() - 0.125).abs() < 0.005);
    assert!(data["rms"].as_f64().unwrap() < 0.1);
    assert_eq!(calibrated.summary["nodes"]["n_load"]["provided"], true);
    let blank = FrameImage::new(width as u32, height as u32, vec![220; width * height]);
    let result = engine.run(&graph, "host-native-no-board", "", &blank, &json!({})).unwrap();
    assert_eq!(result.status(), "failed");
    assert!(result.failure().contains("no_board"), "{}", result.failure());
    assert!(result.record("calib").is_none());
}

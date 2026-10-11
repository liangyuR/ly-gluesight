use super::*;

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

#[test]
fn calibration_keys_are_distinct_for_each_view_of_one_device() {
    assert_eq!(station_view_key("device1", 1).unwrap(), "device1-v1");
    assert_eq!(station_view_key("device1", 3).unwrap(), "device1-v3");
    assert!(station_view_key("device1", 0).is_err());
    assert!(station_view_key("device1", 4).is_err());
}

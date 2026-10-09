use super::*;
use crate::sim::Scenario;

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

fn job() -> Job {
    Job { run_id: 0, sn: 1, k: 0, cam: 0, recipe: crate::recipe::builtin()[0].clone(),
        scenario: Scenario::Normal, image: None }
}

fn table(job: &Job) -> StationMeasure {
    let ids: Vec<_> = job.recipe.owned_points(0).map(|j| json!(j)).collect();
    let n = ids.len();
    StationMeasure { unit: "mm".into(), ids, status: vec!["ok".into(); n], inner_center: vec![Some(0.75); n],
        width: vec![Some(1.2); n], point: vec![[50.0, 60.0]; n] }
}

fn pose() -> Pose { Pose { ok: true, score: 0.99 } }

#[test]
fn missing_pack_is_reported_before_running() {
    let error = check_operators(&json!({"operators": [{"id": "io.load_image"}]})).unwrap_err();
    assert!(error.contains("glue.locate"));
    assert!(error.contains("image.board_calib"));
    assert!(check_operators(&json!({"operators": [
        {"id": "io.load_image"}, {"id": "image.board_calib"}, {"id": "image.load_calib"},
        {"id": "glue.locate"}, {"id": "glue.station_calipers"}
    ]})).is_ok());
}

#[test]
fn invalid_frame_is_rejected_before_ffi() {
    assert!(image_input(&FrameImage::new(0, 1, vec![])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3, 4, 5])).is_err());
}

#[test]
fn flyshot_preserves_width_and_pixel_positions_for_judging_and_overlay() {
    let job = job();
    let result = table(&job).into_measured(&job, pose()).unwrap();
    assert_eq!(result.w[0], 1.2);
    assert_eq!(result.d[0], 0.75);
    assert_eq!(result.px[0], [50.0, 60.0]);
    assert!(result.st.iter().all(|&st| st == ST_OK));
}

#[test]
fn malformed_or_foreign_point_tables_fail_without_panicking() {
    let job = job();
    let mut short = table(&job);
    short.status.pop();
    assert!(short.into_measured(&job, pose()).is_err());
    let mut duplicate = table(&job);
    duplicate.ids[1] = duplicate.ids[0].clone();
    assert!(duplicate.into_measured(&job, pose()).is_err());
    let mut foreign = table(&job);
    foreign.ids[0] = json!(u64::MAX);
    assert!(foreign.into_measured(&job, pose()).is_err());
    let mut incomplete = table(&job);
    incomplete.ids.pop(); incomplete.status.pop(); incomplete.inner_center.pop(); incomplete.width.pop(); incomplete.point.pop();
    assert!(incomplete.into_measured(&job, pose()).is_err());
}

#[test]
fn missing_width_and_failed_pose_cannot_become_valid_measurements() {
    let job = job();
    let mut data = table(&job);
    data.width[0] = None;
    data.status[1] = "no_bead".into();
    data.status[2] = "incomplete_bead".into();
    let result = data.into_measured(&job, pose()).unwrap();
    assert_eq!(&result.st[..3], &[ST_INVALID, ST_GAP, ST_INVALID]);
    let result = table(&job).into_measured(&job, Pose { ok: false, score: 0.0 }).unwrap();
    assert!(result.st.iter().all(|&st| st == ST_INVALID));
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
#[ignore = "requires a real core DLL with std-image and glue"]
fn native_flyshot_injection_measures_mm_and_a_blank_frame_cannot_reuse_the_part() {
    let engine = native_engine();
    let dir = fixture_dir("flyshot");
    let (width, height) = (900u32, 700u32);
    let mut pixels = vec![186u8; (width * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let (x, y) = (x as f64, y as f64);
            let inside = x >= 120.0 && x <= 780.0 && y >= 100.0 && y <= 330.0
                && (x - x.clamp(180.0, 720.0)).hypot(y - y.clamp(160.0, 270.0)) <= 60.0;
            let hole = (x - 220.0).hypot(y - 460.0) <= 18.0 || (x - 660.0).hypot(y - 480.0) <= 18.0;
            let bead = x >= 180.0 && x <= 720.0 && (y - 350.0).abs() <= 8.0;
            pixels[y as usize * width as usize + x as usize] = if inside { 35 } else if hole { 50 } else if bead { 70 } else { 186 };
        }
    }
    let image = FrameImage::new(width, height, pixels);
    save_pgm(&dir.join("part.pgm"), width, height, &image.pixels);
    let mut template = Vec::new();
    for y in 250..510usize { template.extend_from_slice(&image.pixels[y * 900 + 590..y * 900 + 810]); }
    save_pgm(&dir.join("template.pgm"), 220, 260, &template);
    let xs: Vec<_> = (200..=700).step_by(4).collect();
    std::fs::write(dir.join("stations.json"), json!({"points": xs.iter().map(|&x| [x, 350]).collect::<Vec<_>>(),
        "normals": vec![[0, 1]; xs.len()], "ids": (0..xs.len()).collect::<Vec<_>>()}).to_string()).unwrap();
    std::fs::write(dir.join("calib.json"), json!({"kind": "Record", "type": "image.PlaneCalib",
        "data": {"H": [0.05, 0, 0, 0, 0.05, 0, 0, 0, 1], "unit": "mm"}}).to_string()).unwrap();
    let params = json!({"template": "template.pgm", "anchor": [590, 250], "stations": "stations.json", "calib": "calib.json"});
    for (run_id, frame, present) in [("host-native-part", &image, true),
        ("host-native-empty", &FrameImage::new(width, height, vec![186; (width * height) as usize]), false),
        ("host-native-part-again", &image, true)] {
        let result = engine.run(FLYSHOT_GRAPH, run_id, &dir.to_string_lossy(), frame, &params).unwrap();
        assert_eq!(result.status(), "ok", "{}", result.failure());
        assert_eq!(result.record("pose").unwrap()["ok"], present);
        let measured = result.record("measure").unwrap();
        assert_eq!(measured["unit"], "mm");
        assert_eq!(measured["ids"].as_array().unwrap().len(), xs.len());
        if present {
            assert!((measured["innerCenter"][0].as_f64().unwrap() - 1.0).abs() < 0.03);
            assert!((measured["width"][0].as_f64().unwrap() - 0.8).abs() < 0.1);
        } else {
            assert!(measured["status"].as_array().unwrap().iter().all(|s| s == "pose_fail"));
        }
    }
}

#[test]
#[ignore = "requires a real core DLL with std-image and glue"]
fn native_simulated_gap_survives_caliper_averaging_and_merges_across_frames() {
    let engine = native_engine();
    let dir = fixture_dir("simulated-cross-frame-gap");
    let recipe = Arc::new(crate::recipe::samples().remove(1).build().unwrap());
    let assets = crate::simimage::teach(&recipe, &dir).unwrap();
    let mut table = vec![crate::judge::PointState::Pending; recipe.point_count()];
    let mut frames_with_gap = Vec::new();
    for k in 0..recipe.shot_count() {
        let image = crate::simimage::render(&recipe, k, Scenario::Gap,
            crate::simimage::PoseError { dx: 0.25, dy: -0.3, deg: 0.12 }, 42 + k as u64);
        let shot = &assets.shots[k];
        let result = engine.run(FLYSHOT_GRAPH, &format!("simulated-gap-{k}"), "", &image,
            &json!({"template":shot.template, "anchor":shot.anchor, "stations":shot.stations, "calib":shot.calib})).unwrap();
        assert_eq!(result.status(), "ok", "{}", result.failure());
        let measured = serde_json::from_value::<StationMeasure>(result.record("measure").unwrap().clone()).unwrap()
            .into_measured(&Job { run_id: 0, sn: 1, k, cam: 0, recipe: recipe.clone(), scenario: Scenario::Gap,
                image: None },
                serde_json::from_value(result.record("pose").unwrap().clone()).unwrap()).unwrap();
        if measured.st.contains(&ST_GAP) { frames_with_gap.push(k as u8); }
        for (i, &j) in measured.idx.iter().enumerate() {
            table[j as usize] = match measured.st[i] {
                ST_OK => crate::judge::PointState::Measured { d: measured.d[i], w: measured.w[i] },
                ST_GAP => crate::judge::PointState::Gap,
                _ => crate::judge::PointState::Invalid,
            };
        }
    }
    assert_eq!(frames_with_gap, [1, 2]);
    let judgement = crate::judge::judge(&recipe, &table);
    assert_eq!(judgement.verdict, crate::judge::Verdict::NgGap, "{}", judgement.reason);
    assert!(judgement.gaps.iter().any(|gap| gap.frames == [1, 2]));
}

use super::*;
use crate::recipe::{DetectParams, RecipeDoc};

fn document() -> RecipeDoc {
    let mut doc = crate::recipe::samples().remove(0);
    doc.shots.truncate(2);
    for shot in &mut doc.shots {
        shot.path = vec![[40.0, 80.0], [200.0, 80.0]];
        shot.mm_per_px = Some(0.25);
    }
    doc.spacing = 1.0;
    doc.detect = DetectParams { search_mm: 8.0, polarity: Polarity::Dark, width_range: [2.0, 6.0] };
    doc
}

fn result(recipe: &Recipe, k: usize, gaps: &[usize]) -> RunResult {
    let p = plan(recipe, k).unwrap();
    let n = p.idx.len();
    let mut present = vec![true; n];
    let mut lo = vec![json!(-6.0); n];
    let mut hi = vec![json!(10.0); n];
    let mut width = vec![json!(16.0); n];
    for &i in gaps {
        present[i] = false;
        lo[i] = Value::Null;
        hi[i] = Value::Null;
        width[i] = Value::Null;
    }
    let center: Vec<_> = p.idx.iter().map(|&j| [recipe.points.x[j], recipe.points.y[j]]).collect();
    let s: Vec<_> = (0..n).map(|i| i as f64 * p.step).collect();
    let yes = present.iter().filter(|&&x| x).count();
    RunResult { summary: json!({"status": "ok"}), outputs: json!({
        "stations": {"value": {"data": {"count":n, "unit":"px", "s":s, "center":center,
            "normal":vec![[0.0,1.0];n], "present":present, "lo":lo, "hi":hi, "widthPx":width}}},
        "beadInfo": {"value": {"data": {"unit":"px", "lineSource":"taught", "pathOk":true,
            "stations":n, "present":yes, "coverage":yes as f64 / n as f64}}}
    }) }
}

#[test]
fn taught_graph_preserves_search_half_width_units_polarity_and_per_shot_override() {
    let mut doc = document();
    doc.shots[1].detect = Some(DetectParams { search_mm: 10.0, polarity: Polarity::Light, width_range: [2.0, 8.0] });
    let recipe = doc.build().unwrap();
    let graph = build_taught_graph(&recipe, 1).unwrap();
    let path = &graph["nodes"][1]["params"];
    let width = &graph["nodes"][2]["params"];
    assert_eq!(path["polarity"], "bright");
    assert_eq!(path["tolerance"], 24.0);
    assert_eq!(path["widthMax"], 32.0);
    assert_eq!(path["zone"], json!([0.0, 0.0]));
    assert_eq!(serde_json::from_str::<Value>(path["points"].as_str().unwrap()).unwrap(), json!(doc.shots[1].path));
    assert_eq!(width["searchHalf"], 40.0);
    assert_eq!(width["stationStep"], 4.0);
    assert_eq!(width["widthRange"], json!([8.0, 32.0]));
    assert_eq!(graph["outputs"]["stations"]["port"], "bead.stations");
    assert!(graph["nodes"].as_array().unwrap().iter().all(|n| n["op"] != "glue.judge"));
    assert_eq!(build_taught_graph(&recipe, 0).unwrap()["nodes"][1]["params"]["polarity"], "dark");
    for sample in crate::recipe::samples() {
        let recipe = sample.build().unwrap();
        for k in 0..recipe.shot_count() {
            let graph = build_taught_graph(&recipe, k).unwrap();
            let saved: Value = serde_json::from_str(&graph.to_string()).unwrap();
            assert_eq!(saved, graph);
        }
    }
}

#[test]
fn taught_graph_rejects_unsupported_pixel_parameters_and_bad_geometry() {
    let original = document().build().unwrap();
    let cases: Vec<(Box<dyn Fn(&mut Recipe)>, &str)> = vec![
        (Box::new(|r| r.shots[0].skip = true), "不检"),
        (Box::new(|r| r.shots[0].path.clear()), "尚未示教"),
        (Box::new(|r| r.shots[0].mm_per_px = Some(f32::NAN)), "像素当量"),
        (Box::new(|r| r.shots[0].path[0][0] = f32::INFINITY), "有限数"),
        (Box::new(|r| r.shots[0].path[1] = [47.0, 80.0]), "8 px"),
        (Box::new(|r| r.spacing = 0.1), "1 px"),
        (Box::new(|r| r.detect.search_mm = 0.4), "2 px"),
        (Box::new(|r| r.detect.width_range = [0.1, 0.5]), "4–400 px"),
        (Box::new(|r| { r.detect.width_range = [1.0, 101.0]; r.detect.search_mm = 150.0; }), "4–400 px"),
        (Box::new(|r| r.detect.search_mm = 104.0), "0–400 px"),
        (Box::new(|r| r.detect.width_range = [7.0, 6.0]), "胶宽范围"),
        (Box::new(|r| r.points.x[0] += 1.0), "测点表"),
    ];
    for (change, message) in cases {
        let mut recipe = original.clone();
        change(&mut recipe);
        let error = build_taught_graph(&recipe, 0).unwrap_err();
        assert!(error.contains(message), "{error}; expected {message}");
    }
    assert!(build_taught_graph(&original, 99).unwrap_err().contains("不存在"));
}

#[test]
fn taught_graph_does_not_fill_or_discard_a_rounding_mismatched_terminal_station() {
    let mut doc = document();
    doc.shots[0].path[1][0] = 199.999;
    let recipe = doc.build().unwrap();
    assert_eq!(recipe.owned_points(0).count(), 41);
    assert!(build_taught_graph(&recipe, 0).unwrap_err().contains("末站"));
    doc.shots[0].path[1][0] = 199.5;
    let recipe = doc.build().unwrap();
    assert_eq!(recipe.owned_points(0).count(), 40);
    assert!(build_taught_graph(&recipe, 0).is_ok());
}

#[test]
fn station_output_maps_global_ids_signed_offsets_and_gaps_without_judging() {
    let recipe = document().build().unwrap();
    let p = plan(&recipe, 1).unwrap();
    let m = parse_result(&recipe, 1, &p, &result(&recipe, 1, &[5, 6])).unwrap();
    assert_eq!(m.idx, (41..82).collect::<Vec<_>>());
    assert_eq!(m.d[0], 0.5);
    assert_eq!(m.w[0], 4.0);
    assert_eq!(m.px[0], [40.0, 82.0]);
    assert_eq!(m.st[5], ST_GAP);
    assert!(m.d[5].is_nan() && m.w[5].is_nan());
    assert_eq!(m.px[5], [60.0, 80.0]);
    assert!((m.coverage - 39.0 / 41.0).abs() < 1e-6);
    let record = m.record();
    assert_eq!(record["unit"], "mm");
    assert_eq!(record["status"][5], "no_bead");
    assert!(record["width"][5].is_null() && record["innerCenter"][5].is_null());
    assert!(record.get("passed").is_none());
}

#[test]
fn station_output_rejects_incomplete_mismatched_and_malformed_results() {
    let recipe = document().build().unwrap();
    let p = plan(&recipe, 0).unwrap();
    for (pointer, value) in [
        ("/summary/status", json!("degraded")),
        ("/outputs/stations/value/data/unit", json!("mm")),
        ("/outputs/beadInfo/value/data/lineSource", json!("fit")),
        ("/outputs/beadInfo/value/data/pathOk", json!(false)),
        ("/outputs/stations/value/data/count", json!(40)),
        ("/outputs/stations/value/data/lo", json!([])),
        ("/outputs/stations/value/data/s/1", json!(0)),
        ("/outputs/stations/value/data/center/0", json!([0, 0])),
        ("/outputs/stations/value/data/normal/0", json!([0, 0])),
        ("/outputs/stations/value/data/present/0", json!(1)),
        ("/outputs/stations/value/data/lo/0", Value::Null),
        ("/outputs/stations/value/data/hi/0", json!(-10)),
        ("/outputs/stations/value/data/widthPx/0", json!(1)),
        ("/outputs/beadInfo/value/data/coverage", json!(0.1)),
        ("/outputs/beadInfo/value/data/present", json!(0)),
    ] {
        let good = result(&recipe, 0, &[]);
        let mut raw = json!({"summary": good.summary, "outputs": good.outputs});
        *raw.pointer_mut(pointer).unwrap() = value;
        let bad = RunResult { summary: raw["summary"].clone(), outputs: raw["outputs"].clone() };
        assert!(parse_result(&recipe, 0, &p, &bad).is_err(), "accepted {pointer}");
    }
    let mut bad = result(&recipe, 0, &[1]);
    bad.outputs["stations"]["value"]["data"]["widthPx"][1] = json!(4);
    assert!(parse_result(&recipe, 0, &p, &bad).unwrap_err().contains("无胶站"));
}

#[test]
fn empty_bead_never_becomes_a_successful_measurement_even_if_all_gaps_are_allowed() {
    let mut recipe = document().build().unwrap();
    recipe.segments[0].max_gap_len = 1000.0;
    let p = plan(&recipe, 0).unwrap();
    let all: Vec<_> = (0..p.idx.len()).collect();
    let error = parse_result(&recipe, 0, &p, &result(&recipe, 0, &all)).unwrap_err();
    assert!(error.contains("没找到胶"));
}

fn bead_image(bright: bool, gap: bool, blank: bool, center_y: i32) -> FrameImage {
    let (width, height) = (256, 160);
    let mut pixels = vec![if bright { 28 } else { 220 }; width * height];
    if !blank {
        for y in center_y - 8..center_y + 8 {
            for x in 16..240 {
                if gap && (104..136).contains(&x) { continue; }
                pixels[y as usize * width + x] = if bright { 220 } else { 28 };
            }
        }
    }
    FrameImage::new(width as u32, height as u32, pixels)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:016x}", bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3)))
}

#[test]
#[ignore = "requires LYFLOW_CORE_DLL with glue.taught_path; writes synthetic evidence to GLUESIGHT_VISION_TEST_DIR or system temp"]
fn native_taught_measurement_uses_pixels_for_width_offset_gaps_polarity_and_fresh_frames() {
    let dll = std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL to the taught-path core");
    let engine = Engine::load(std::path::Path::new(&dll)).unwrap();
    let dir = std::env::var_os("GLUESIGHT_VISION_TEST_DIR").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gluesight-taught-native").join(std::process::id().to_string()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut evidence = Vec::new();
    for (name, bright, gap, blank, center_y) in [
        ("dark-full", false, false, false, 82),
        ("dark-gap", false, true, false, 82),
        ("blank-after-full", false, false, true, 82),
        ("bright-full", true, false, false, 82),
        ("shifted-next-frame", false, false, false, 88),
    ] {
        let mut doc = document();
        doc.detect.polarity = if bright { Polarity::Light } else { Polarity::Dark };
        let recipe = doc.build().unwrap();
        let image = bead_image(bright, gap, blank, center_y);
        let mut pgm = format!("P5\n{} {}\n255\n", image.width, image.height).into_bytes();
        pgm.extend_from_slice(&image.pixels);
        std::fs::write(dir.join(format!("{name}.pgm")), &pgm).unwrap();
        let graph = build_taught_graph(&recipe, 0).unwrap();
        std::fs::write(dir.join(format!("{name}.lyflow.json")), graph.to_string()).unwrap();
        let run = measure_shot_with_graph(&engine, &recipe, 0, &image, &format!("native-taught-{name}"), "", &graph);
        if blank {
            let error = run.unwrap_err();
            assert!(error.contains("没找到胶"), "{error}");
            evidence.push(json!({"name":name,"fixtureFnv1a64":hash(&pgm),"graphFnv1a64":hash(graph.to_string().as_bytes()),"error":error}));
        } else {
            let m = run.unwrap();
            assert_eq!(m.idx.len(), 41);
            let gaps = m.st.iter().filter(|&&s| s == ST_GAP).count();
            assert_eq!(gaps > 0, gap, "{name}: {gaps}");
            if gap { assert!(gaps >= 6, "{gaps}"); }
            for i in 0..m.idx.len() {
                if m.st[i] != ST_OK { continue; }
                assert!((m.w[i] - 4.0).abs() < 0.4, "{name}: {}", m.w[i]);
                let expected = (center_y as f32 - 80.5) * 0.25;
                assert!((m.d[i] - expected).abs() < 0.3, "{name}: {} != {expected}", m.d[i]);
            }
            evidence.push(json!({"name":name,"fixtureFnv1a64":hash(&pgm),"graphFnv1a64":hash(graph.to_string().as_bytes()),"measurement":m}));
        }
    }
    let mut doc = document();
    doc.shots[0].path.reverse();
    let recipe = doc.build().unwrap();
    let reversed = measure_shot(&engine, &recipe, 0, &bead_image(false, false, false, 82), "native-taught-reversed", "").unwrap();
    assert!(reversed.d.iter().all(|&d| d < 0.0));
    let mut altered = build_taught_graph(&recipe, 0).unwrap();
    altered["nodes"][2]["params"]["stationStep"] = json!(8);
    assert!(measure_shot_with_graph(&engine, &recipe, 0, &bead_image(false, false, false, 82), "native-taught-altered", "", &altered).unwrap_err().contains("发布包算法图"));
    let report = json!({"source":"synthetic full-resolution pixels; not field accuracy acceptance", "engine":engine.path,
        "engineFnv1a64":hash(&std::fs::read(&dll).unwrap()), "version":engine.version, "cases":evidence});
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("Native taught-path evidence: {}", dir.display());
}

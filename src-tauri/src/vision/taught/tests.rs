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
fn station_output_maps_global_ids_signed_offsets_and_gaps_without_judging() {
    let recipe = document().build().unwrap();
    let p = plan(&recipe, 1).unwrap();
    let m = parse_result(&recipe, &p, &result(&recipe, 1, &[5, 6])).unwrap();
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
    assert_eq!(record["edges"][0], json!([[40.0, 74.0], [40.0, 90.0]]));
    assert!(record["edges"][5].is_null());
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
        assert!(parse_result(&recipe, &p, &bad).is_err(), "accepted {pointer}");
    }
    let mut bad = result(&recipe, 0, &[1]);
    bad.outputs["stations"]["value"]["data"]["widthPx"][1] = json!(4);
    assert!(parse_result(&recipe, &p, &bad).unwrap_err().contains("无胶站"));
}

#[test]
fn multiview_failure_is_local_but_missing_image_is_acquisition_failure() {
    let mut doc = crate::recipe::samples().remove(0);
    doc.shots.truncate(1);
    let shot = &mut doc.shots[0];
    shot.views = [1, 2].into_iter().map(|view| crate::recipe::ShotViewSpec { view, enabled: true,
        path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None }).collect();
    let recipe = doc.build().unwrap();
    let images = vec![(1, std::sync::Arc::new(FrameImage::new(1, 1, vec![0]))), (2, std::sync::Arc::new(FrameImage::new(1, 1, vec![0])))];
    let result = measure_views_with(&recipe, 0, &images, |view, _| {
        if view == 1 { return Err("图 1 引擎失败".into()); }
        let idx: Vec<_> = recipe.view_points(0, view).collect();
        Ok(ShotMeasurement { idx: idx.iter().map(|&j| j as u32).collect(), d: vec![0.0; idx.len()], w: vec![4.0; idx.len()],
            st: vec![ST_OK; idx.len()], px: idx.iter().map(|&j| [recipe.points.x[j], recipe.points.y[j]]).collect(),
            edges: idx.iter().map(|&j| Some([[recipe.points.x[j], recipe.points.y[j] - 2.0], [recipe.points.x[j], recipe.points.y[j] + 2.0]])).collect(), coverage: 1.0, ms: 1 })
    }).unwrap();
    assert_eq!(result.views.len(), 2);
    assert!(result.views[0].error.is_some());
    let mut table = vec![crate::judge::PointState::Pending; recipe.point_count()];
    for (i, &j) in result.reading.idx.iter().enumerate() {
        table[j as usize] = if result.reading.st[i] == ST_OK { crate::judge::PointState::Measured { d: 0.0, w: 4.0 } } else { crate::judge::PointState::Invalid };
    }
    assert_eq!(crate::judge::judge(&recipe, &table).verdict, crate::judge::Verdict::Ok);
    assert!(measure_views_with(&recipe, 0, &images[..1], |_, _| panic!("must reject before measurement")).is_err());
}

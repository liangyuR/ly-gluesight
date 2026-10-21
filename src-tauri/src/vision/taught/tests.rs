use super::*;
use crate::measure::ST_INVALID;
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
            "normal":vec![[0.0,1.0];n], "present":present, "lo":lo, "hi":hi, "widthPx":width,
            "candidateLo": vec![Value::Null;n], "candidateHi": vec![Value::Null;n], "candidateWidthPx": vec![Value::Null;n]}}},
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
fn station_output_maps_global_ids_signed_offsets_and_rejected_candidates_without_changing_gap_judgement() {
    let mut doc = document();
    doc.limits.max_gap_len = 0.0;
    let recipe = doc.build().unwrap();
    let p = plan(&recipe, 1).unwrap();
    let mut output = result(&recipe, 1, &[5, 6]);
    let raw = &mut output.outputs["stations"]["value"]["data"];
    raw["candidateLo"][5] = json!(-10.0);
    raw["candidateHi"][5] = json!(18.0);
    raw["candidateWidthPx"][5] = json!(28.0);
    let m = parse_result(&recipe, &p, &output).unwrap();
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
    assert_eq!(record["candidateWidth"][5], 7.0);
    assert_eq!(record["candidateEdges"][5], json!([[60.0, 70.0], [60.0, 98.0]]));
    assert!(record["candidateWidth"][6].is_null() && record["candidateEdges"][6].is_null());
    assert!(record.get("passed").is_none());
    let baseline = parse_result(&recipe, &p, &result(&recipe, 1, &[5, 6])).unwrap();
    let judged = |reading: &ShotMeasurement| {
        let mut table = vec![crate::judge::PointState::Measured { d: Some(0.0), w: 4.0 }; recipe.point_count()];
        for (i, &j) in reading.idx.iter().enumerate() {
            table[j as usize] = if reading.st[i] == ST_OK {
                crate::judge::PointState::Measured { d: Some(reading.d[i]), w: reading.w[i] }
            } else { crate::judge::PointState::Gap };
        }
        serde_json::to_value(crate::judge::judge(&recipe, &table)).unwrap()
    };
    assert_eq!(judged(&m)["verdict"], "NG_GAP");
    assert_eq!(judged(&m), judged(&baseline));
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
        ("/outputs/stations/value/data/candidateLo", json!([])),
        ("/outputs/stations/value/data/candidateHi/0", json!(10)),
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
        path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None, fixture: None }).collect();
    let recipe = doc.build().unwrap();
    let images = vec![(1, std::sync::Arc::new(FrameImage::new(1, 1, vec![0]))), (2, std::sync::Arc::new(FrameImage::new(1, 1, vec![0])))];
    let result = measure_views_with(&recipe, 0, &images, |view, _| {
        if view == 1 { return Err("图 1 引擎失败".into()); }
        let idx: Vec<_> = recipe.view_points(0, view).collect();
        Ok(ShotMeasurement { idx: idx.iter().map(|&j| j as u32).collect(), d: vec![0.0; idx.len()], w: vec![4.0; idx.len()],
            st: vec![ST_OK; idx.len()], px: idx.iter().map(|&j| [recipe.points.x[j], recipe.points.y[j]]).collect(),
            edges: idx.iter().map(|&j| Some([[recipe.points.x[j], recipe.points.y[j] - 2.0], [recipe.points.x[j], recipe.points.y[j] + 2.0]])).collect(), candidate_width: vec![None; idx.len()], candidate_edges: vec![None; idx.len()], coverage: 1.0, ms: 1,
            pose: None, reference_failed: None })
    }).unwrap();
    assert_eq!(result.views.len(), 2);
    assert!(result.views[0].error.is_some());
    let mut table = vec![crate::judge::PointState::Pending; recipe.point_count()];
    for (i, &j) in result.reading.idx.iter().enumerate() {
        table[j as usize] = if result.reading.st[i] == ST_OK { crate::judge::PointState::Measured { d: Some(0.0), w: 4.0 } } else { crate::judge::PointState::Invalid };
    }
    assert_eq!(crate::judge::judge(&recipe, &table).verdict, crate::judge::Verdict::Ok);
    assert!(measure_views_with(&recipe, 0, &images[..1], |_, _| panic!("must reject before measurement")).is_err());
}

fn fixture_spec() -> FixtureSpec {
    FixtureSpec { rect: [100, 100, 32, 32], search_radius_px: 20.0, angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None }
}

/// document() 的拍照点 0（41 站、站距 4 px = 1 mm、法向 (0, 1)）带上特征定位。
fn located(f: FixtureSpec) -> Recipe {
    let mut doc = document();
    doc.shots[0].fixture = Some(f);
    doc.build().unwrap()
}

/// glue.locate 1.1.0 的 pose 记录：模板中心相对示教位置平移 offset（px），角度 0。
fn pose(f: &FixtureSpec, ok: bool, offset: [f64; 2], observable: bool) -> Value {
    let c = f.teach_center();
    json!({"ok": ok, "reason": if ok { "" } else { "low_score" }, "message": "", "score": if ok { 0.95 } else { 0.3 }, "peakRatio": 0.4,
        "angle": 0.0, "angleObservable": observable, "center": [c[0] + offset[0], c[1] + offset[1]], "teachCenter": c, "offset": offset,
        "templateSize": [f.rect[2], f.rect[3]], "minScore": f.min_score, "maxPeakRatio": f.max_peak_ratio})
}

fn with_pose(mut output: RunResult, pose: Value) -> RunResult {
    output.outputs["pose"] = json!({"value": {"data": pose}});
    output
}

fn job(recipe: &Recipe) -> Job {
    Job { cycle_id: "cycle-1".into(), shot_id: recipe.shots[0].id.clone(), camera: recipe.shots[0].camera.clone(), bundle_id: None,
        production: None, candidate: None, submitted_at: Instant::now(), timeout: std::time::Duration::from_secs(1), image_measurement: true,
        sn: String::new(), k: 0, cam: 0, recipe: Arc::new(recipe.clone()), scenario: crate::offline::Scenario::Normal, image: None, images: Vec::new() }
}

#[test]
fn accepted_pose_measures_offset_from_moved_centerline_along_rotated_normal() {
    // 防的回归：d 仍相对静态示教线、正向取反、或漏乘 mm/px。
    // 位姿把示教线沿法向 (0, 1) 搬 +1 px，胶中心在静态线 +2 px：d' = +1 px = +0.25 mm（静态是 +0.5，符号反了是 −0.25）
    let f = fixture_spec();
    let recipe = located(f.clone());
    let p = plan(&recipe, 0).unwrap();
    let m = parse_result(&recipe, &p, &with_pose(result(&recipe, 0, &[]), pose(&f, true, [0.0, 1.0], true))).unwrap();
    for i in 2..=38 {
        assert_eq!(m.st[i], ST_OK, "站 {i}");
        assert!((m.d[i] - 0.25).abs() < 1e-5, "站 {i}：{}", m.d[i]);
    }
    assert!(m.pose.as_ref().is_some_and(|p| p.accepted) && m.reference_failed.is_none());
}

#[test]
fn rejected_pose_keeps_widths_marks_no_reference_and_reports_the_view() {
    // 防的回归：定位失败时丢了胶宽或改掉断胶站、失败在帧上不可见、多视图契约把整个视图判成测不了；
    // 开了转角却看不出角度时悄悄降级成只补平移（定稿 §10 决定二）
    for (name, angle_range, ok, observable) in [("定位失败", 0.0, false, true), ("开转角看不出角度", 3.0, true, false)] {
        let mut doc = document();
        doc.shots.truncate(1);
        let mut f = fixture_spec();
        f.angle_range_deg = angle_range;
        let shot = &mut doc.shots[0];
        shot.views = [1, 2].into_iter().map(|view| crate::recipe::ShotViewSpec { view, enabled: true, path: shot.path.clone(),
            mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None, fixture: (view == 1).then(|| f.clone()) }).collect();
        let recipe = doc.build().unwrap();
        let images: Vec<_> = [1, 2].into_iter().map(|view| (view, Arc::new(FrameImage::new(1, 1, vec![0])))).collect();
        let multi = measure_views_with(&recipe, 0, &images, |view, _| {
            let projected = recipe.for_view(0, view)?;
            let raw = result(&projected, 0, &[5, 6]);
            let output = if view == 1 { with_pose(raw, pose(&f, ok, [0.0, 0.0], observable)) } else { raw };
            parse_result(&projected, &plan(&projected, 0)?, &output)
        }).unwrap();
        assert!(multi.views.iter().all(|v| v.error.is_none()), "{name}");
        let measured = multi.into_measured(&job(&recipe));
        assert_eq!(measured.reference_failed, [ReferenceFailure { view: 1, reason: REFERENCE_LOCATE_FAILED.into() }], "{name}");
        assert!((measured.score - 78.0 / 82.0).abs() < 1e-6, "{name}：有胶比例 {}", measured.score);
        for i in 0..82 {
            let want = match i { 5 | 6 | 46 | 47 => ST_GAP, 0..41 => ST_NO_REF, _ => ST_OK };
            assert_eq!(measured.st[i], want, "{name} 站 {i}");
            assert_ne!(measured.st[i], ST_INVALID);
            if want != ST_GAP { assert_eq!(measured.w[i], 4.0, "{name} 站 {i}"); }
        }
    }
}

#[test]
fn tangential_shift_excludes_each_end_by_shift_plus_one_spacing() {
    // 防的回归（决定一）：两端排除长度不是 max(0, |Δ| − m) + 1 个站距；排除区里原本断胶的站还当断胶判；排除动了中段的 d
    let f = fixture_spec();
    let recipe = located(f.clone());
    let p = plan(&recipe, 0).unwrap();
    // 41 站、站距 4 px（1 mm）；位姿沿胶路切向 +10 px（Δ = 2.5 mm）：两端各排除 3.5 mm，即 4 站
    let m = parse_result(&recipe, &p, &with_pose(result(&recipe, 0, &[2, 20, 38]), pose(&f, true, [10.0, 0.0], true))).unwrap();
    let excluded: Vec<usize> = (0..41).filter(|&i| m.st[i] == ST_OUT_OF_RANGE).collect();
    assert_eq!(excluded, [0, 1, 2, 3, 37, 38, 39, 40]);
    assert!(excluded.iter().all(|&i| m.d[i].is_nan()));
    for i in [2, 38] { assert!(m.w[i].is_nan() && m.edges[i].is_none(), "站 {i} 原本断胶"); }
    assert!(m.w[0] == 4.0 && m.edges[0].is_some(), "有胶站的胶宽与胶边留着");
    assert_eq!(m.st[20], ST_GAP);
    for i in (4..=36).filter(|&i| i != 20) {
        assert!(m.st[i] == ST_OK && (m.d[i] - 0.5).abs() < 1e-5, "站 {i}：{} {}", m.st[i], m.d[i]);
    }
    let pose = m.pose.unwrap();
    let close = |v: Option<[f32; 2]>, want: f32| v.is_some_and(|v| v.iter().all(|x| (x - want).abs() < 1e-5));
    assert!(close(pose.end_shift_mm, 2.5) && close(pose.excluded_mm, 3.5), "{pose:?}");
    assert!(m.reference_failed.is_none());
}

#[test]
fn end_shift_beyond_limit_drops_reference_after_excluding_ends() {
    // 防的回归：护栏按排除长度而不是 |Δ| 判、缺省上限没按临时默认公式算、或护栏先于两端排除（后沿的假断胶会盖过 ERR 92）
    let mut explicit = fixture_spec();
    explicit.max_end_shift_mm = Some(2.0);
    // 缺省上限：searchRadius 20 px、0.25 mm/px、只找平移 → √2·18·0.25 ≈ 6.36 mm
    let limit = std::f64::consts::SQRT_2 * 18.0 * 0.25;
    // （fixture，切向搬移 mm，是否触发护栏）
    for (f, shift, fires) in [(explicit, 2.5, true), (fixture_spec(), -(limit + 0.1), true), (fixture_spec(), limit - 0.1, false)] {
        let recipe = located(f.clone());
        let p = plan(&recipe, 0).unwrap();
        let m = parse_result(&recipe, &p, &with_pose(result(&recipe, 0, &[]), pose(&f, true, [shift / 0.25, 0.0], true))).unwrap();
        let ends = (shift.abs() + 1.0 + 1e-6).floor() as usize + 1;
        for i in 0..41 {
            let want = if i < ends || 40 - i < ends { ST_OUT_OF_RANGE } else if fires { ST_NO_REF } else { ST_OK };
            assert_eq!(m.st[i], want, "Δ {shift} 站 {i}");
            assert_eq!(m.w[i], 4.0, "Δ {shift} 站 {i}");
        }
        assert_eq!(m.reference_failed.as_deref(), fires.then_some(REFERENCE_END_SHIFT), "Δ {shift}");
    }
}

#[test]
fn fixture_graph_follows_lyflow_template_contract_and_run_requires_template_dir() {
    // 防的回归：base_dir 为空或相对时 lyFlow 按进程当前目录找 template.pgm，悄悄打开别处的文件；
    // 图里的模板名、外圈、anchor 与 lyFlow#32 的契约对不上；胶路太短时恒等位姿下整段超出检测范围、每帧 ERR 92
    let f = fixture_spec();
    let recipe = located(f.clone());
    let graph = build_taught_graph(&recipe, 0).unwrap();
    let locate = graph["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "n_locate").unwrap();
    let params = &locate["params"];
    assert_eq!(locate["op"], "glue.locate");
    assert_eq!(params["template"], "template.pgm");
    assert_eq!(params["templatePad"], 4);
    assert_eq!(params["anchor"], json!([100.0, 100.0]));
    assert_eq!(params["angleRange"], 0.0);
    assert_eq!(graph["outputs"]["pose"], json!({"node": "n_locate", "port": "pose"}));
    assert!(graph["edges"].as_array().unwrap().iter().any(|e| e["from"]["node"] == "n_load" && e["to"]["node"] == "n_locate"));
    for base in ["", "fixtures/P1"] {
        assert!(plan_run(&recipe, 0, base).err().unwrap().contains("模板目录"), "{base:?}");
    }
    let dir = std::env::temp_dir().join(format!("gs-plan-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = plan_run(&recipe, 0, dir.to_str().unwrap()).err();
    std::fs::write(dir.join("template.pgm"), b"P5\n1 1\n255\n\0").unwrap();
    let present = plan_run(&recipe, 0, dir.to_str().unwrap()).is_ok();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(missing.unwrap().contains("模板缺失"));
    assert!(present);
    let mut short = document();
    short.shots[0].path = vec![[40.0, 80.0], [52.0, 80.0]];
    short.shots[0].fixture = Some(f);
    assert!(build_taught_graph(&short.build().unwrap(), 0).unwrap_err().contains("胶路太短"));
}

#[test]
fn locate_output_breaking_the_contract_is_rejected() {
    // 防的回归：有 fixture 却没跑定位；1.0.0 的 glue.locate 不认外圈、把整个文件当模板（templateSize = w + 8）还照用位姿；
    // 旧算子不报 angleObservable 时把位姿当成看得出角度
    let f = fixture_spec();
    let recipe = located(f.clone());
    let p = plan(&recipe, 0).unwrap();
    let mut cases = vec![("没有 pose 输出", result(&recipe, 0, &[]))];
    for ok in [true, false] {
        let mut padded = pose(&f, ok, [0.0, 0.0], true);
        padded["templateSize"] = json!([40, 40]);
        cases.push(("模板尺寸含外圈", with_pose(result(&recipe, 0, &[]), padded)));
    }
    let mut old = pose(&f, true, [0.0, 0.0], true);
    old.as_object_mut().unwrap().remove("angleObservable");
    cases.push(("缺 angleObservable", with_pose(result(&recipe, 0, &[]), old)));
    for (name, output) in cases {
        assert!(parse_result(&recipe, &p, &output).is_err(), "{name}");
    }
}

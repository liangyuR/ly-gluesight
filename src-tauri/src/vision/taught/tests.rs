use super::*;
use crate::recipe::{DetectParams, HeadingWindow, RecipeDoc};

const FRAME: [u32; 2] = [400, 300];

/// 段配方：三台相机的走廊都沿 y = 80 从 x = 40 到 200；0.25 mm/px、站距 1 mm（4 px）→ 41 站，
/// 搜索半宽 32 px、胶宽 8–24 px → 中心偏移容差 20 px。
fn burst_doc() -> RecipeDoc {
    let mut doc = crate::recipe::samples_burst();
    doc.spacing = 1.0;
    doc.detect = DetectParams { search_mm: 8.0, polarity: Polarity::Dark, width_range: [2.0, 6.0] };
    for inspect in &mut doc.inspect {
        inspect.segments = vec![crate::recipe::SegmentCorridor {
            segment: 1,
            corridor: vec![[40.0, 80.0], [200.0, 80.0]],
            sector_deg: None,
            color_model: None,
        }];
        inspect.mm_per_px = Some(0.25);
    }
    doc
}

fn corridor_of(doc: RecipeDoc, camera: usize) -> CorridorGraph {
    let recipe = doc.build().unwrap();
    plan_corridor(&recipe.inspect[camera], 1, &recipe, FRAME).unwrap()
}

/// 一个暗段（沿法向的胶中心偏移与胶宽，px）：峰值 60、局部亮度 120、陡度 0.8、两边完整。
fn dark_run(mid: f64, width: f64) -> Value {
    json!({"lo": mid - width / 2.0, "hi": mid + width / 2.0, "width": width, "mid": mid, "peak": 60.0, "parts": 1,
        "loComplete": true, "hiComplete": true, "edgeLo": 10.0, "edgeHi": 10.0, "depth": 12.0, "sharpness": 0.8, "level": 120.0})
}

/// 沿 y = 80 每 4 px 一站（法向 (0, 1)）的 `glue.LateralRuns`：beads[i] = Some((胶中心偏移, 胶宽)) 的站有一个暗段。
fn lateral(beads: &[Option<(f64, f64)>]) -> Value {
    let stations: Vec<Value> = beads.iter().enumerate().map(|(i, b)| json!({
        "s": i as f64 * 4.0, "center": [40.0 + 4.0 * i as f64, 80.0], "normal": [0.0, 1.0], "validFraction": 1.0,
        "runs": b.map(|(x, w)| vec![dark_run(x, w)]).unwrap_or_default(),
    })).collect();
    json!({"count": beads.len(), "stationStep": 4.0, "searchHalf": 32.0, "split": "half_max", "stations": stations})
}

fn record(data: Value) -> Value {
    json!({"value": {"data": data}})
}

fn corridor_output(beads: &[Option<(f64, f64)>]) -> RunResult {
    let runs = lateral(beads);
    RunResult {
        summary: json!({"status": "ok"}),
        outputs: json!({"runs": record(runs.clone()), "split": record(runs)}),
    }
}

/// 41 站：absent 里的站无胶，其余胶宽 16 px（4 mm）、胶中心偏 +2 px；exits 里的 (站, 偏移) 让胶中心在前后 15 站里逐渐偏到那里。
fn beads(
    absent: &[std::ops::RangeInclusive<usize>],
    exits: &[(usize, f64)],
) -> Vec<Option<(f64, f64)>> {
    (0..41)
        .map(|i| {
            if absent.iter().any(|r| r.contains(&i)) {
                return None;
            }
            let x = exits
                .iter()
                .map(|&(k, v)| 2.0 + (v - 2.0) * (1.0 - i.abs_diff(k) as f64 / 15.0).max(0.0))
                .fold(2.0, |a: f64, b| {
                    if (b - 2.0).abs() > (a - 2.0).abs() {
                        b
                    } else {
                        a
                    }
                });
            Some((x, 16.0))
        })
        .collect()
}

fn measure(
    c: &CorridorGraph,
    size: [u32; 2],
    output: RunResult,
) -> Result<FrameMeasurement, String> {
    let image = FrameImage::new(size[0], size[1], vec![0; (size[0] * size[1]) as usize]);
    measure_frame_with(c, &image, 7, |_, _| Ok(output))
}

#[test]
fn frame_whose_color_mode_differs_from_the_taught_frame_fails_before_measuring() {
    // 防的回归：相机图像模式改了（灰度示教、彩色生产或反过来），lyFlow 照样能把 RGB 转灰度量，结果却与示教口径不可比，
    // 却不报错地给出判定
    let doc = burst_doc();
    let recipe = doc.build().unwrap();
    let gray_taught = CorridorSet::build(&recipe, &[FRAME; 3])
        .unwrap()
        .with_channels(&[1, 1, 1]);
    let FrameGraph::Corridor(c) = gray_taught.get(0, 1).unwrap() else {
        panic!("应是示教走廊")
    };
    let n = (FRAME[0] * FRAME[1]) as usize;
    let color = FrameImage::rgb(FRAME[0], FRAME[1], vec![0; n * 3]);
    let ran = std::cell::Cell::new(false);
    let result = measure_frame_with(c, &color, 7, |_, _| {
        ran.set(true);
        Ok(corridor_output(&beads(&[], &[])))
    });
    assert!(result.is_err() && !ran.get(), "彩色帧量了灰度示教的走廊");
    let gray = FrameImage::new(FRAME[0], FRAME[1], vec![0; n]);
    assert!(measure_frame_with(c, &gray, 7, |_, _| Ok(corridor_output(&beads(&[], &[])))).is_ok());
}

/// 胶色卡尺一站的内容：胶（法向 −6..10 px）、被高光盖住、或什么都没有。
#[derive(Clone, Copy)]
enum ColorStation {
    Bead,
    Highlight,
    Bare,
}

/// 41 站、站距 4 px 的胶色卡尺输出（合并前后同一份）。
fn color_output(at: impl Fn(usize) -> ColorStation) -> RunResult {
    let bead = json!({"lo": -6.0, "hi": 10.0, "width": 16.0, "mid": 2.0, "peak": 0.95, "parts": 1, "loComplete": true,
        "hiComplete": true, "loCause": "edge", "hiCause": "edge", "meanScore": 0.9, "unknownWidth": 0.0,
        "edgeLo": null, "edgeHi": null, "depth": null, "sharpness": null, "level": 1.0});
    let stations: Vec<Value> = (0..41)
        .map(|i| {
            let (runs, unknown, info) = match at(i) {
                ColorStation::Bead => (json!([bead]), json!([]), 1.0),
                ColorStation::Highlight => (json!([]), json!([{"lo": -9.0, "hi": 13.0, "cause": "highlight"}]), 0.3),
                ColorStation::Bare => (json!([]), json!([]), 1.0),
            };
            json!({"s": i as f64 * 4.0, "center": [40.0 + 4.0 * i as f64, 80.0], "normal": [0.0, 1.0],
                "validFraction": 1.0, "infoFraction": info, "unknownSpans": unknown, "runs": runs})
        })
        .collect();
    let runs = json!({"count": 41, "stationStep": 4.0, "searchHalf": 32.0, "split": "half_max", "source": "color", "stations": stations});
    RunResult {
        summary: json!({"status": "ok"}),
        outputs: json!({"runs": record(runs.clone()), "split": record(runs)}),
    }
}

/// burst_doc 第 1 段 cam1 取了色之后的示教走廊。
fn color_corridor() -> CorridorGraph {
    let mut doc = burst_doc();
    doc.inspect[0].segments[0].color_model = Some(r#"{"version":1}"#.into());
    let recipe = doc.build().unwrap();
    let set = CorridorSet::build(&recipe, &[FRAME; 3]).unwrap();
    let Some(FrameGraph::Corridor(c)) = set.get(0, 1) else {
        panic!("应是示教走廊")
    };
    c.clone()
}

#[test]
fn segment_with_color_model_measures_score_runs_with_color_graph_and_rejects_gray_frames() {
    // 防的回归：取了色的段仍按灰度响应图量（粉胶在铸件和铝面上明暗相反，灰度测不全）；胶色卡尺的段
    // （没有陡度，对比度按得分、局部亮度恒为 1）被灰度门限判成无胶；或灰度帧被当成胶色量
    let mut doc = burst_doc();
    doc.inspect[0].segments[0].color_model = Some(r#"{"version":1}"#.into());
    let recipe = doc.build().unwrap();
    let set = CorridorSet::build(&recipe, &[FRAME; 3]).unwrap();
    let (FrameGraph::Corridor(c), FrameGraph::Corridor(gray_graph)) =
        (set.get(0, 1).unwrap(), set.get(1, 1).unwrap())
    else {
        panic!("应是示教走廊")
    };
    assert_eq!(
        (c.template.id, gray_graph.template.id),
        ("corridor_color", "corridor")
    );
    assert_eq!(c.params["colorModel"], json!(r#"{"version":1}"#));
    let output = || color_output(|_| ColorStation::Bead);
    let n = (FRAME[0] * FRAME[1]) as usize;
    let color = FrameImage::rgb(FRAME[0], FRAME[1], vec![0; n * 3]);
    let m = measure_frame_with(c, &color, 7, |run, _| {
        assert_eq!(run.template.id, "corridor_color");
        Ok(output())
    })
    .unwrap()
    .metrics;
    assert_eq!(
        (m.present_ratio, m.gap_max_mm),
        (1.0, 0.0),
        "胶色卡尺每站都有胶"
    );
    assert!((m.width_mean.unwrap() - 4.0).abs() < 1e-6);
    let gray = FrameImage::new(FRAME[0], FRAME[1], vec![0; n]);
    assert!(measure_frame_with(c, &gray, 7, |_, _| Ok(output())).is_err());
}

#[test]
fn color_corridor_station_under_highlight_is_unknown_but_a_bare_hole_stays_a_gap() {
    // 防的回归：示教走廊段里被高光盖住的站当成缺胶，连成断口误报断胶、拉低有胶比例；
    // 或反过来把没有高光的真断口也当无信息放过
    let c = color_corridor();
    let n = (FRAME[0] * FRAME[1]) as usize;
    let color = FrameImage::rgb(FRAME[0], FRAME[1], vec![0; n * 3]);
    let at = |i| match i {
        10..=14 => ColorStation::Highlight,
        30..=32 => ColorStation::Bare,
        _ => ColorStation::Bead,
    };
    let m = measure_frame_with(&c, &color, 7, |_, _| Ok(color_output(at)))
        .unwrap()
        .metrics;
    assert!(
        (m.gap_max_mm - 3.0 * m.spacing_mm).abs() < 1e-9,
        "断口应只有真空的 3 站：{}",
        m.gap_max_mm
    );
    assert!(
        (m.present_ratio - 33.0 / 36.0).abs() < 1e-9,
        "有胶比例的分母应去掉 5 个高光站：{}",
        m.present_ratio
    );
}

#[test]
fn wide_color_beads_keep_exact_widths_while_clipping_and_occlusion_do_not_erase_real_gaps() {
    let c = color_corridor();
    let image = FrameImage::rgb(
        FRAME[0],
        FRAME[1],
        vec![0; (FRAME[0] * FRAME[1] * 3) as usize],
    );
    let mut output = color_output(|i| match i {
        33 => ColorStation::Highlight,
        34..=39 => ColorStation::Bare,
        _ => ColorStation::Bead,
    });
    let stations = output.outputs["runs"]["value"]["data"]["stations"]
        .as_array_mut()
        .unwrap();
    for station in &mut stations[..30] {
        let r = &mut station["runs"][0];
        r["lo"] = json!(-18.0);
        r["hi"] = json!(22.0);
        r["width"] = json!(40.0);
    }
    let clipped = &mut stations[32]["runs"][0];
    clipped["hi"] = json!(32.0);
    clipped["width"] = json!(38.0);
    clipped["mid"] = json!(13.0);
    clipped["hiComplete"] = json!(false);
    clipped["hiCause"] = json!("caliper");
    let m = measure_frame_with(&c, &image, 7, |_, _| Ok(output)).unwrap();
    assert_eq!(
        (m.metrics.width_max, m.metrics.width_min),
        (Some(10.0), Some(4.0))
    );
    assert_eq!(
        (
            m.metrics.widths[30],
            m.metrics.widths[32],
            m.metrics.gap_max_mm
        ),
        (Some(4.0), None, 6.0)
    );
    let evidence = m.metrics.measurement_evidence.unwrap();
    assert_eq!(
        (
            evidence.exact_stations,
            evidence.clipped_stations,
            evidence.unknown_stations,
            evidence.absent_stations
        ),
        (33, 1, 1, 6)
    );
    assert_eq!(evidence.stations[32].width_lower_bound_mm, Some(9.5));
    assert_eq!(evidence.stations[32].lo_edge, Some([168.0, 74.0]));
    assert!(evidence.stations[32].hi_edge.is_none() && m.edges[32].is_none());
}

#[test]
fn similarly_near_parallel_color_beads_are_unknown_instead_of_arbitrarily_chosen() {
    let output = lateral(&[Some((0.0, 16.0)); 5]);
    let mut raw = output;
    for station in raw["stations"].as_array_mut().unwrap() {
        station["runs"] = json!([dark_run(-9.0, 16.0), dark_run(9.0, 16.0)]);
    }
    let runs = lateral_runs(&raw, "test").unwrap();
    for tolerance in [None, Some(20.0)] {
        let calls = judge_color_stations(&runs, 8.0, 24.0, 0.3, tolerance);
        assert!(calls
            .iter()
            .all(|c| c.state == StationEvidenceState::Unknown && c.bead.is_none()));
    }
}

#[test]
fn color_corridor_exit_marks_only_proven_end_stations_unknown_and_preserves_width_and_gaps() {
    let c = color_corridor();
    let image = FrameImage::rgb(
        FRAME[0],
        FRAME[1],
        vec![0; (FRAME[0] * FRAME[1] * 3) as usize],
    );
    let mut checks = burst_doc().segments[0].checks.clone();
    checks.width.enabled = true;
    checks.width.nominal_mm = 4.0;
    checks.width.tol_upper = 1.0;
    checks.width.tol_lower = 1.0;
    checks.width.max_excursion_mm = 0.0;
    checks.width.wide_warn_only = false;
    checks.gap.enabled = true;
    checks.gap.max_gap_mm = 6.0;
    checks.gap.min_present = 0.0;
    checks.overflow.enabled = false;
    for (absent, exits, clipped, expected_gap, expected_unknown) in [
        (vec![30..=40], vec![(29, 18.0)], false, 0.0, 11),
        (vec![30..=40], vec![], false, 11.0, 0),
        (vec![10..=16, 30..=40], vec![(29, 18.0)], false, 7.0, 11),
        (vec![30..=40], vec![(29, 18.0)], true, 11.0, 0),
    ] {
        let mut values = beads(&absent, &exits);
        values[20] = Some((2.0, 40.0));
        let mut output = corridor_output(&values);
        if clipped {
            let r = &mut output.outputs["runs"]["value"]["data"]["stations"][29]["runs"][0];
            r["hi"] = json!(32.0);
            r["width"] = json!(22.0);
            r["mid"] = json!(21.0);
            r["hiComplete"] = json!(false);
            r["hiCause"] = json!("caliper");
        }
        let m = measure_frame_with(&c, &image, 7, |_, _| Ok(output))
            .unwrap()
            .metrics;
        assert_eq!(
            (m.state, m.width_max, m.gap_max_mm),
            (FrameState::Measured, Some(10.0), expected_gap)
        );
        let evidence = m.measurement_evidence.as_ref().unwrap();
        assert_eq!(evidence.unknown_stations, expected_unknown);
        assert!(evidence
            .stations
            .iter()
            .filter(|s| s.state == StationEvidenceState::Unknown)
            .all(|s| s.reason.as_deref() == Some("out_of_corridor")));
        assert_eq!(
            crate::burst_judge::frame_verdict(&m, &checks),
            if expected_gap > 6.0 {
                crate::judge::Verdict::NgGap
            } else {
                crate::judge::Verdict::NgWidth
            }
        );
    }
}

#[test]
fn part_edge_distance_and_overlay_follow_the_majority_side_from_bead_center() {
    // 防的回归：胶边距线画到另一侧、或终点不在测得的胶边距上（叠图与 edgeMm 对不上，现场按图调极性、对比度会被误导）
    let mut doc = burst_doc();
    doc.segments[0].offset_ref = Some(crate::plan::OffsetRef {
        measure: OffsetMeasure::ToEdge,
        camera: "cam1".into(),
        x_ref: None,
        spread: None,
        edge: Some(crate::plan::EdgeParams {
            polarity: EdgePolarity::BrightToDark,
            search_mm: 8.0,
            contrast_min: 40.0,
            nominal_mm: None,
        }),
    });
    let c = corridor_of(doc, 0);
    // 胶两边 (−6, 10)、胶中心 +2：右侧 t = 24 与左侧 t = 20 离胶中心都是 22 px（5.5 mm）
    for (right, left, end_y) in [(Some(24.0), None, 104.0), (None, Some(20.0), 60.0)] {
        let side = |t: Option<f64>| json!({"length": 100, "edges": t.map(|t| vec![json!({"t": t})]).unwrap_or_default()});
        let mut output = corridor_output(&beads(&[], &[]));
        output.outputs["edges"] = record(json!({"start": 0.0, "searchLength": 100.0,
            "stations": (0..41).map(|i| {
                let (r, l) = if i == 40 { (left, right) } else { (right, left) };
                json!({"right": side(r), "left": side(l)})
            }).collect::<Vec<_>>()}));
        let m = measure(&c, FRAME, output).unwrap();
        assert!((m.metrics.edge_mm.unwrap() - 5.5).abs() < 1e-6);
        assert_eq!(m.part_edges.len(), 40);
        for (i, line) in m.part_edges.iter().enumerate() {
            let x = 40.0 + 4.0 * i as f32;
            assert_eq!(*line, [[x, 82.0], [x, end_y]], "第 {} 站", i + 1);
        }
    }
}

#[test]
fn malformed_engine_output_fails_only_that_frame() {
    // 防的回归（规格 §0 一帧失败只影响本帧）：输出不合契约（缺记录、站数、弧长、中线、暗段字段）或图像尺寸不对时
    // panic、拿错位的数据算指标，或把走廊测量图弄坏、后面正常的帧也测不了
    let c = corridor_of(burst_doc(), 0);
    let good = || corridor_output(&beads(&[], &[]));
    let truncate = |key: &'static str| {
        move |o: &mut Value| {
            o["outputs"][key]["value"]["data"]["stations"]
                .as_array_mut()
                .unwrap()
                .pop();
        }
    };
    let cases: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
        (
            "运行失败",
            Box::new(|o| o["summary"]["status"] = json!("failed")),
        ),
        (
            "缺合并前的卡尺",
            Box::new(|o| {
                o["outputs"].as_object_mut().unwrap().remove("split");
            }),
        ),
        ("卡尺少一站", Box::new(truncate("runs"))),
        ("合并前的卡尺少一站", Box::new(truncate("split"))),
        (
            "弧长错位",
            Box::new(|o| o["outputs"]["runs"]["value"]["data"]["stations"][3]["s"] = json!(0.0)),
        ),
        (
            "中线错位",
            Box::new(|o| {
                o["outputs"]["runs"]["value"]["data"]["stations"][3]["center"] = json!([0.0, 0.0])
            }),
        ),
        (
            "暗段缺峰值",
            Box::new(|o| {
                o["outputs"]["runs"]["value"]["data"]["stations"][3]["runs"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("peak");
            }),
        ),
    ];
    for (name, break_it) in cases {
        let output = good();
        let mut raw = json!({"summary": output.summary, "outputs": output.outputs});
        break_it(&mut raw);
        let bad = RunResult {
            summary: raw["summary"].clone(),
            outputs: raw["outputs"].clone(),
        };
        assert!(measure(&c, FRAME, bad).is_err(), "接受了：{name}");
    }
    assert!(
        measure(&c, [FRAME[0], FRAME[1] + 1], good()).is_err(),
        "图像尺寸与示教帧不同"
    );
    assert_eq!(
        measure(&c, FRAME, good()).unwrap().metrics.state,
        FrameState::Measured
    );
}

#[test]
fn bead_leaving_through_a_corridor_end_is_out_of_corridor_but_breaks_and_centred_stops_stay_gaps() {
    // 防的回归（默认做法 4）：弯道帧胶条从走廊侧边走出时整帧按断胶判 NG；或反过来把真断胶、停胶、急转处居中的缺胶
    // 当成“走出走廊”放过（只记检测不完整）。中心偏移容差 20 px，判据是紧挨端部缺胶的胶中心偏离 ≥ 10 px
    for (name, absent, exits, out) in [
        ("终点一侧侧向走出", vec![30..=40], vec![(29, 12.0)], true),
        ("起点一侧侧向走出", vec![0..=8], vec![(9, -11.0)], true),
        (
            "两端都侧向走出",
            vec![0..=3, 36..=40],
            vec![(4, 10.0), (35, 14.0)],
            true,
        ),
        (
            "端部缺胶但胶中心居中（停胶、急转）",
            vec![30..=40],
            vec![],
            false,
        ),
        (
            "一端走出、另一端缺胶处居中",
            vec![0..=3, 36..=40],
            vec![(35, 14.0)],
            false,
        ),
        (
            "中段断胶，端部走出",
            vec![15..=17, 35..=40],
            vec![(34, 12.0)],
            false,
        ),
        ("整帧没有胶", vec![0..=40], vec![], false),
    ] {
        let m = measure(
            &corridor_of(burst_doc(), 2),
            FRAME,
            corridor_output(&beads(&absent, &exits)),
        )
        .unwrap()
        .metrics;
        let want = if out {
            FrameState::OutOfCorridor
        } else {
            FrameState::Measured
        };
        assert_eq!(m.state, want, "{name}");
        assert_eq!(m.error.is_some(), out, "{name}");
        assert!(m.gap_max_mm > 0.0, "{name}：缺胶照常计入");
    }
}

#[test]
fn camera_override_of_a_judge_threshold_reaches_station_presence() {
    // 防的回归（LF-5）：x.gluesight.judge 里的判定门限按相机覆盖后没进到判定（专家参数卡改了不生效）。
    // 胶宽 20 px，第 20 站只有 9 px：有胶宽度比默认 0.5（10 px）判无胶，本相机改成 0.4（8 px）判有胶
    let narrow: Vec<_> = (0..41)
        .map(|i| Some((2.0, if i == 20 { 9.0 } else { 20.0 })))
        .collect();
    let mut doc = burst_doc();
    let default = measure(
        &corridor_of(doc.clone(), 0),
        FRAME,
        corridor_output(&narrow),
    )
    .unwrap()
    .metrics;
    doc.inspect[0].expert = [("presentRatio".to_string(), json!(0.4))]
        .into_iter()
        .collect();
    let tuned = measure(&corridor_of(doc, 0), FRAME, corridor_output(&narrow))
        .unwrap()
        .metrics;
    assert_eq!(
        (default.widths[20].is_some(), tuned.widths[20].is_some()),
        (false, true)
    );
}

/// 第 1 段开逐帧定向：检测窗口距出胶点 1–11 mm（4–44 px），出胶点由走廊起点推出。
fn heading_graph() -> HeadingGraph {
    let mut doc = burst_doc();
    doc.segments[0].heading = Some(Default::default());
    for inspect in &mut doc.inspect {
        inspect.window = Some(HeadingWindow {
            start_mm: 1.0,
            length_mm: 10.0,
        });
    }
    let recipe = doc.build().unwrap();
    plan_heading(&recipe.inspect[0], 1, &recipe, FRAME).unwrap()
}

#[test]
fn sector_turned_on_one_segment_leaves_other_segments_of_the_camera_automatic() {
    // 防的回归（D-SB-47）：扇区按相机存，在一段的示教帧上拖扇区，同一相机其他段的测量图跟着转
    let mut doc = burst_doc();
    let first = doc.segments[0].clone();
    doc.segments = (1..=2)
        .map(|index| crate::plan::PathSegment {
            index,
            heading: Some(Default::default()),
            ..first.clone()
        })
        .collect();
    for inspect in &mut doc.inspect {
        inspect.window = Some(HeadingWindow {
            start_mm: 1.0,
            length_mm: 10.0,
        });
        let corridor = inspect.segments[0].clone();
        inspect.segments = vec![
            corridor.clone(),
            crate::recipe::SegmentCorridor {
                segment: 2,
                sector_deg: Some(90.0),
                color_model: None,
                ..corridor
            },
        ];
    }
    let recipe = doc.build().unwrap();
    let sector = |segment| {
        plan_heading(&recipe.inspect[0], segment, &recipe, FRAME)
            .unwrap()
            .params["sector"]
            .clone()
    };
    assert_eq!(
        (sector(1), sector(2)),
        (json!([-90.0, 90.0]), json!([0.0, 180.0]))
    );
}

/// 假的定向图运行：只有第 0 个峰，粗找到 `found` 个胶点（10 步），卡尺量到 16 px 宽的胶。
fn fake_heading_run(
    found: u64,
) -> impl FnOnce(GraphRun<'_>, &FrameImage) -> Result<RunResult, String> {
    move |g, _| {
        let mut outputs = serde_json::Map::new();
        for target in g.targets {
            let (kind, k) = target.split_at(3);
            match kind {
                "n_t" => {
                    let mut scores = vec![0.0; 21];
                    scores[10] = 40.0;
                    outputs.insert("scan".to_string(), record(json!({"sector": [-100.0, -80.0], "scores": scores, "peaks": [{"index": 10, "angle": -90.0, "score": 40.0}]})));
                    let track = if k == "0" {
                        json!({"valid": true, "peakIndex": 0, "heading": -80.0, "steps": 10, "found": found, "sharpness": 0.9})
                    } else {
                        json!({"valid": false, "peakIndex": k.parse::<u64>().unwrap(), "heading": null, "steps": null, "found": null, "sharpness": null})
                    };
                    outputs.insert(format!("track{k}"), record(track));
                    outputs.insert(
                        format!("line{k}"),
                        record(json!({"points": [[40.0, 76.0], [40.0, 36.0]]})),
                    );
                }
                "n_r" => {
                    outputs.insert(
                        format!("runs{k}"),
                        record(lateral(&[Some((0.0, 16.0)); 11])),
                    );
                }
                _ => {}
            }
        }
        Ok(RunResult {
            summary: json!({"status": "ok"}),
            outputs: Value::Object(outputs),
        })
    }
}

#[test]
fn heading_result_uses_fitted_direction_when_found_and_marks_window_absent_otherwise() {
    // 防的回归（D-SB-47）：选中的方向没找到胶条时仍按它的卡尺量出“有胶”（阴影、纹理被当成胶），或不标、不记原因
    let g = heading_graph();
    let image = FrameImage::new(FRAME[0], FRAME[1], vec![0; (FRAME[0] * FRAME[1]) as usize]);
    for (found, missed, width_mm, present) in [(8, false, Some(4.0), 1.0), (2, true, None, 0.0)] {
        let m = measure_heading_with(&g, &image, 7, fake_heading_run(found))
            .unwrap()
            .metrics;
        assert_eq!(
            (
                m.heading_fallback,
                m.width_mean,
                m.present_ratio,
                m.heading_deg
            ),
            (
                missed,
                width_mm,
                present,
                Some(if missed { -80.0 } else { -90.0 })
            ),
            "找到 {found} 个胶点"
        );
        assert_eq!(
            m.fallback_reason.is_some_and(|r| r.contains("minCoverage")),
            missed,
            "找到 {found} 个胶点"
        );
    }
}

#[test]
fn color_heading_preserves_wide_beads_and_distinguishes_untrusted_paths_from_background() {
    let mut doc = burst_doc();
    doc.segments[0].heading = Some(Default::default());
    doc.inspect[0].segments[0].color_model = Some(r#"{"version":1}"#.into());
    doc.inspect[0].window = Some(HeadingWindow {
        start_mm: 1.0,
        length_mm: 10.0,
    });
    let recipe = doc.build().unwrap();
    let g = plan_heading(&recipe.inspect[0], 1, &recipe, FRAME).unwrap();
    let image = FrameImage::rgb(
        FRAME[0],
        FRAME[1],
        vec![0; (FRAME[0] * FRAME[1] * 3) as usize],
    );
    for (found, unknown, state) in [
        (8, 0, StationEvidenceState::Exact),
        (0, 5, StationEvidenceState::Unknown),
        (0, 0, StationEvidenceState::Absent),
    ] {
        let m = measure_heading_with(&g, &image, 7, |run, frame| {
            assert_eq!(run.params["widthPolicy"], "extent");
            let mut result = fake_heading_run(found)(run, frame)?;
            result.outputs["track0"]["value"]["data"]["unknownSteps"] = json!(unknown);
            result.outputs["runs0"] = record(lateral(&[Some((0.0, 40.0)); 11]));
            Ok(result)
        })
        .unwrap()
        .metrics;
        assert!(m
            .measurement_evidence
            .as_ref()
            .unwrap()
            .stations
            .iter()
            .all(|s| s.state == state));
        assert_eq!(m.width_max, (found > 0).then_some(10.0));
        assert_eq!(
            m.gap_max_mm,
            if state == StationEvidenceState::Absent {
                11.0
            } else {
                0.0
            }
        );
    }
}

#[test]
fn build_available_keeps_usable_pairs_while_build_still_rejects_broken_ones() {
    // 防的回归（D-SB-57）：改段后有段没示教或建不成时，历史复测整件都不能做；发布、预热用的 build 也不能因此放宽
    let mut doc = burst_doc();
    doc.inspect[1].segments.clear();
    let recipe = doc.clone().build().unwrap();
    let pairs = |u: &[UnavailableCorridor]| {
        u.iter()
            .map(|u| (u.camera_idx, u.untaught))
            .collect::<Vec<_>>()
    };
    let (set, unavailable) = CorridorSet::build_available(&recipe, &[FRAME; 3]);
    assert!(set.get(0, 1).is_some() && set.get(1, 1).is_none() && set.get(2, 1).is_some());
    assert_eq!(pairs(&unavailable), vec![(1, true)]);
    assert!(
        CorridorSet::build(&recipe, &[FRAME; 3]).is_ok(),
        "没示教的组合照旧跳过"
    );
    doc.inspect[2].mm_per_px = None;
    let recipe = doc.build().unwrap();
    assert!(
        CorridorSet::build(&recipe, &[FRAME; 3]).is_err(),
        "建不成的组合照旧报错"
    );
    let (set, unavailable) = CorridorSet::build_available(&recipe, &[FRAME; 3]);
    assert!(set.get(0, 1).is_some() && set.get(2, 1).is_none());
    assert_eq!(pairs(&unavailable), vec![(1, true), (2, false)]);
}

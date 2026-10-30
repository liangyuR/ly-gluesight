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
fn heading_without_bead_found_marks_whole_window_absent_with_reason() {
    // 防的回归（D-SB-47）：选中的方向没找到胶条时仍按它的卡尺量出“有胶”（阴影、纹理被当成胶），或不标、不记原因
    let g = heading_graph();
    let image = FrameImage::new(FRAME[0], FRAME[1], vec![0; (FRAME[0] * FRAME[1]) as usize]);
    for (found, missed, width_mm, present) in [(8, false, Some(4.0), 1.0), (2, true, None, 0.0)] {
        let m = measure_heading_with(&g, &image, 7, fake_heading_run(found))
            .unwrap()
            .metrics;
        assert_eq!(
            (m.heading_fallback, m.width_mean, m.present_ratio),
            (missed, width_mm, present),
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
fn heading_reads_the_fitted_path_not_the_start_direction() {
    // 防的回归（D-SB-47）：方向记成候选的起跑方向，同一条胶因起跑方向不同读数跳动，分段边界细化被带偏
    let g = heading_graph();
    let image = FrameImage::new(FRAME[0], FRAME[1], vec![0; (FRAME[0] * FRAME[1]) as usize]);
    let m = measure_heading_with(&g, &image, 7, fake_heading_run(8))
        .unwrap()
        .metrics;
    assert_eq!(m.heading_deg, Some(-90.0));
}

use std::cell::RefCell;

use super::*;
use crate::plan::{EdgeParams, EdgePolarity, HeadingSettings, OffsetMeasure, OffsetRef};
use crate::recipe::{DetectParams, HeadingWindow, Polarity};

const FRAME: [u32; 2] = [400, 300];

/// 单段直线：三台相机的走廊都从出胶点 (40, 80) 朝右（0°）到 (200, 80)；0.25 mm/px，窗口 [2, 22] mm → [8, 88] px。
fn heading_recipe(edge: bool) -> Recipe {
    let mut doc = crate::recipe::samples_burst();
    doc.spacing = 1.0;
    doc.detect = DetectParams {
        search_mm: 8.0,
        polarity: Polarity::Dark,
        width_range: [2.0, 6.0],
    };
    for inspect in &mut doc.inspect {
        inspect.segments = vec![crate::recipe::SegmentCorridor {
            segment: 1,
            corridor: vec![[40.0, 80.0], [200.0, 80.0]],
        }];
        inspect.mm_per_px = Some(0.25);
        inspect.window = Some(HeadingWindow {
            start_mm: 2.0,
            length_mm: 20.0,
        });
    }
    doc.segments[0].heading = Some(HeadingSettings { tolerance_deg: 4.0 });
    if edge {
        doc.segments[0].offset_ref = Some(OffsetRef {
            measure: OffsetMeasure::ToEdge,
            camera: doc.inspect[0].camera.clone(),
            x_ref: None,
            spread: None,
            edge: Some(EdgeParams {
                polarity: EdgePolarity::BrightToDark,
                search_mm: 10.0,
                contrast_min: 40.0,
                nominal_mm: None,
            }),
        });
    }
    doc.build().unwrap()
}

/// 定向测量图的输出：`found` 为 bead_path 的 info.ok；`present[i]` 为第 i 站有胶（胶宽 16 px、居中）。
fn output(
    found: bool,
    heading: f64,
    present: &[bool],
    edge: Option<(&[&str], &[Option<f64>])>,
) -> RunResult {
    let n = present.len();
    let field = |v: f64| {
        present
            .iter()
            .map(|&p| if p { json!(v) } else { Value::Null })
            .collect::<Vec<_>>()
    };
    let mut outputs = json!({
        "pathInfo": {"value": {"data": {"ok": found, "heading": heading}}},
        "pathLine": {"value": {"data": {"points": [[48.0, 80.0], [128.0, 80.0]]}}},
        "stations": {"value": {"data": {"unit": "px", "count": n,
            "center": (0..n).map(|i| [48.0 + 4.0 * i as f64, 80.0]).collect::<Vec<_>>(), "normal": vec![[0.0, 1.0]; n],
            "present": present, "lo": field(-8.0), "hi": field(8.0), "widthPx": field(16.0)}}},
    });
    if let Some((status, distance)) = edge {
        outputs["edge"] = json!({"value": {"data": {"status": status, "distancePx": distance}}});
    }
    RunResult {
        summary: json!({"status": "ok"}),
        outputs,
    }
}

#[test]
fn search_without_bead_reruns_along_the_taught_heading_and_an_empty_window_still_reads_as_a_gap() {
    // 防的回归（D-SB-47）：扇形搜索没找到胶时不回退、把整帧记成检测不完整（整窗断胶就放过了），
    // 回退图不是按本段示教方向找；或回退后仍无胶时没有按缺胶算最长断口
    let recipe = heading_recipe(false);
    let g = plan_heading(&recipe.inspect[0], 1, &recipe, FRAME).unwrap();
    assert_eq!(g.graph["nodes"][1]["params"]["zone"], json!([8.0, 88.0]));
    assert!(g.graph["nodes"][1]["params"]["sector"].is_array());
    assert_eq!(g.fallback["nodes"][1]["params"]["headingSource"], "param");
    assert_eq!(g.fallback["nodes"][1]["params"]["heading"], json!(0.0));
    let image = FrameImage::new(FRAME[0], FRAME[1], vec![0; (FRAME[0] * FRAME[1]) as usize]);

    let runs = RefCell::new(Vec::new());
    let found = measure_heading_with(&g, &image, 3, |graph, _| {
        runs.borrow_mut().push(graph.contains("\"param\""));
        Ok(output(true, 2.0, &[true; 21], None))
    })
    .unwrap();
    assert_eq!(*runs.borrow(), vec![false], "找到胶就只跑搜索图");
    assert_eq!(
        (
            found.metrics.heading_deg,
            found.metrics.heading_fallback,
            found.metrics.x_mm
        ),
        (Some(2.0), false, None)
    );
    assert_eq!(found.line.len(), 2);

    runs.borrow_mut().clear();
    let empty = measure_heading_with(&g, &image, 3, |graph, _| {
        let fallback = graph.contains("\"param\"");
        runs.borrow_mut().push(fallback);
        Ok(output(false, 0.0, &[false; 21], None))
    })
    .unwrap();
    assert_eq!(*runs.borrow(), vec![false, true]);
    assert!(empty.metrics.heading_fallback);
    assert_eq!(
        empty.metrics.state,
        crate::burst_judge::FrameState::Measured
    );
    assert_eq!(empty.metrics.present_ratio, 0.0);
    assert!(
        (empty.metrics.gap_max_mm - 21.0).abs() < 1e-9,
        "21 站全无胶 = 21 mm 断口"
    );
}

#[test]
fn edge_distance_is_the_mean_of_found_stations_and_missing_when_under_half_of_the_searched_stations(
) {
    // 防的回归（D-SB-48）：胶边距漏乘当量、把没找到边的站当 0 平均进去，或大半测站没找到边仍给出胶边距（据此判了胶偏）
    let recipe = heading_recipe(true);
    let g = plan_heading(&recipe.inspect[0], 1, &recipe, FRAME).unwrap();
    assert_eq!(g.graph["nodes"][3]["op"], "glue.edge_distance");
    assert_eq!(g.graph["nodes"][3]["params"]["searchLength"], json!(40.0));
    let image = FrameImage::new(FRAME[0], FRAME[1], vec![0; (FRAME[0] * FRAME[1]) as usize]);
    let present = [true, true, true, false];
    let measure = |status: &'static [&'static str], distance: &'static [Option<f64>]| {
        measure_heading_with(&g, &image, 1, |_, _| {
            Ok(output(true, 0.0, &present, Some((status, distance))))
        })
        .unwrap()
        .metrics
        .edge_mm
    };
    let found = measure(
        &["ok", "ok", "none", "nobead"],
        &[Some(10.0), Some(14.0), None, None],
    );
    assert!(
        (found.unwrap() - 3.0).abs() < 1e-9,
        "(10 + 14) / 2 px × 0.25"
    );
    assert_eq!(
        measure(
            &["ok", "none", "none", "nobead"],
            &[Some(10.0), None, None, None]
        ),
        None
    );
    let other = plan_heading(&recipe.inspect[1], 1, &recipe, FRAME).unwrap();
    assert_eq!(
        other.graph["nodes"].as_array().unwrap().len(),
        3,
        "只有胶偏参考相机接找边"
    );
}

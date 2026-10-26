use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};

use super::{Engine, FrameImage, RunResult};
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::recipe::{CameraInspect, DetectParams, Polarity, Recipe};

/// 走廊测量图的像素参数，按 lyFlow 契约约束（glue.taught_path + glue.bead_width）核对。
struct GraphParams {
    /// 站距（px）
    step: f64,
    /// 搜索半宽（px）
    half: f64,
    lo: f64,
    hi: f64,
    /// 中心偏移容差（px）= 搜索半宽 − 胶宽上限的一半
    tolerance: f64,
    /// 中线总长（px）
    length: f64,
}

/// 按当量、站距与检测参数换算像素参数并核对 lyFlow 的取值范围；出错时返回原因（调用方加上主语）。
fn graph_params(path: &[[f32; 2]], scale: f64, step: f64, detect: &DetectParams) -> Result<GraphParams, &'static str> {
    if !scale.is_finite() || scale <= 0.0 || scale > 10.0 { return Err("像素当量必须在 0–10 mm/px 之间"); }
    if path.iter().flatten().any(|x| !x.is_finite()) { return Err("中线坐标必须为有限数"); }
    let length: f64 = path.windows(2).map(|p| {
        (p[1][0] as f64 - p[0][0] as f64).hypot(p[1][1] as f64 - p[0][1] as f64)
    }).sum();
    if !length.is_finite() || length < 8.0 { return Err("lyFlow 示教中线至少需要 8 px"); }
    if !step.is_finite() || step < 1.0 { return Err("lyFlow 站距至少需要 1 px，请检查站距和像素当量"); }
    let half = detect.search_mm as f64 / scale;
    let lo = detect.width_range[0] as f64 / scale;
    let hi = detect.width_range[1] as f64 / scale;
    if !half.is_finite() || half < 2.0 { return Err("lyFlow 搜索半宽至少需要 2 px"); }
    if !lo.is_finite() || !hi.is_finite() || lo <= 0.0 || hi <= lo || hi >= 2.0 * half {
        return Err("胶宽范围需满足 0 < 下限 < 上限 < 两倍搜索半宽");
    }
    if !(4.0..=400.0).contains(&hi) { return Err("lyFlow 胶宽搜索上限需在 4–400 px 之间"); }
    let tolerance = half - 0.5 * hi;
    if !(0.0..=400.0).contains(&tolerance) {
        return Err("lyFlow 中心偏移容差（搜索半宽减去胶宽上限的一半）需在 0–400 px 之间");
    }
    Ok(GraphParams {
        step,
        half,
        lo,
        hi,
        tolerance,
        length,
    })
}

/// lyFlow 沿中线按站距重采样的站数（含两端）。
fn station_count(length: f64, step: f64) -> usize {
    ((length * 1000.0).round() / 1000.0 / step + 1e-6 / step).floor() as usize + 1
}

/// 沿中线量胶的测量图：io.load_image → glue.taught_path（示教中线）→ glue.bead_width（逐站胶边与胶宽）。
fn taught_graph(path: &[[f32; 2]], detect: &DetectParams, p: &GraphParams) -> Result<Value, String> {
    Ok(json!({
        "schemaVersion": 1, "id": "01GLUESIGHTTAUGHT00000000",
        "nodes": [
            {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
            {"id": "n_path", "op": "glue.taught_path", "params": {
                "points": serde_json::to_string(path).map_err(|e| e.to_string())?,
                "zone": [0.0, 0.0], "tolerance": p.tolerance, "widthMax": p.hi.ceil(),
                "polarity": if detect.polarity == Polarity::Light { "bright" } else { "dark" }, "sharpMin": 0.4
            }},
            {"id": "n_width", "op": "glue.bead_width", "params": {
                "form": "straight", "widthRange": [p.lo, p.hi], "stationStep": p.step, "searchHalf": p.half,
                "window": 30.0, "mergeGap": 8.0, "contrastMin": 16.0,
                "presentRatio": 0.5, "centerRatio": 0.3, "contrastRatio": 0.3
            }}
        ],
        "edges": [
            {"id": "e_image", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_width", "port": "image"}},
            {"id": "e_path", "from": {"node": "n_path", "port": "path"}, "to": {"node": "n_width", "port": "path"}}
        ],
        "outputs": {
            "stations": {"node": "n_width", "port": "bead.stations"},
            "beadInfo": {"node": "n_width", "port": "bead.info"}
        }
    }))
}

// ───────────── 段配方：每台相机一条检测走廊，每帧独立测（D-SB-12、D-SB-17） ─────────────

/// 胶条从走廊端点一侧走出的判据：紧挨端部缺胶段的有胶站，胶中心偏离走廊超过中心偏移容差的这一比例。
/// 默认做法 4 的实现，判据没有经过真实弯道图像验证。
const EXIT_OFFSET_RATIO: f64 = 0.5;

/// 段配方一台相机的走廊测量图：沿走廊按站距取测站，所有帧共用（D-SB-17）。测站不进配方全局站表，
/// 每帧只输出帧内汇总的 `FrameMetrics`（规格 §4 每帧独立指标）。
#[derive(Clone, Debug)]
pub struct CorridorGraph {
    pub camera: String,
    /// 计划相机下标（`Recipe.inspect` 的顺序），即 `FrameMetrics.camera`
    pub camera_idx: usize,
    /// 示教帧尺寸；每帧必须与它相同
    pub size: [u32; 2],
    pub graph: Value,
    graph_text: String,
    /// 各测站在图里的位置（px），沿走廊弧长 i × step
    stations: Vec<[f64; 2]>,
    scale: f64,
    step: f64,
    half: f64,
    tolerance: f64,
    spacing_mm: f64,
}

/// 一台相机一帧的测量：帧内指标与逐站胶边（px，无胶站为 None，叠图用）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameMeasurement {
    pub metrics: FrameMetrics,
    pub edges: Vec<Option<[[f32; 2]; 2]>>,
    pub ms: u32,
}

/// 走廊各点都在 size 的图像内（像素中心坐标 0..=宽−1、0..=高−1）。
pub fn corridor_inside(corridor: &[[f32; 2]], size: [u32; 2]) -> bool {
    size[0] > 0 && size[1] > 0 && corridor.iter().all(|p| {
        p[0] >= 0.0 && p[1] >= 0.0 && p[0] as f64 <= size[0] as f64 - 1.0 && p[1] as f64 <= size[1] as f64 - 1.0
    })
}

/// 折线上弧长 t（px）处的点。
fn polyline_at(path: &[[f32; 2]], t: f64) -> [f64; 2] {
    let mut left = t.max(0.0);
    for w in path.windows(2) {
        let (a, b) = ([w[0][0] as f64, w[0][1] as f64], [w[1][0] as f64, w[1][1] as f64]);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        if left <= l && l > 0.0 {
            let f = left / l;
            return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
        }
        left -= l;
    }
    path.last().map_or([0.0, 0.0], |p| [p[0] as f64, p[1] as f64])
}

/// 段配方一台检测相机的走廊测量图：示教帧尺寸为 size。走廊必须完整落在图内（不然 lyFlow 量不到走廊两端）；
/// 当量、站距与检测参数须在 lyFlow 的取值范围内；至少三站。
pub fn plan_corridor(inspect: &CameraInspect, recipe: &Recipe, size: [u32; 2]) -> Result<CorridorGraph, String> {
    let camera_idx = recipe.inspect.iter().position(|i| i.camera == inspect.camera)
        .ok_or_else(|| format!("相机 {} 不是配方 {} 的检测相机", inspect.camera, recipe.id))?;
    let fail = |why: &str| format!("相机 {}：{why}", inspect.camera);
    let Some(scale) = inspect.mm_per_px.filter(|_| inspect.corridor.len() >= 2) else {
        return Err(fail("尚未示教检测走廊和像素当量"));
    };
    let detect = inspect.detect.as_ref().unwrap_or(&recipe.detect);
    let params = graph_params(&inspect.corridor, scale, recipe.spacing as f64 / scale, detect).map_err(fail)?;
    if !corridor_inside(&inspect.corridor, size) {
        return Err(fail(&format!("检测走廊超出示教帧 {}×{}，请在这台相机的帧上重新示教走廊", size[0], size[1])));
    }
    let n = station_count(params.length, params.step);
    if !(3..=200_000).contains(&n) {
        return Err(fail("检测走廊不足三站或超过 20 万站，请调整走廊长度或站距"));
    }
    let stations = (0..n).map(|i| polyline_at(&inspect.corridor, i as f64 * params.step)).collect();
    let graph = taught_graph(&inspect.corridor, detect, &params)?;
    Ok(CorridorGraph {
        camera: inspect.camera.clone(), camera_idx, size, graph_text: graph.to_string(), graph, stations,
        scale, step: params.step, half: params.half, tolerance: params.tolerance, spacing_mm: recipe.spacing as f64,
    })
}

/// 用 lyFlow 量一帧（段配方）：见 [`measure_frame_with`]。
pub fn measure_frame(engine: &Engine, corridor: &CorridorGraph, image: &FrameImage, seq: u64, run_id: &str) -> Result<FrameMeasurement, String> {
    let start = Instant::now();
    let mut m = measure_frame_with(corridor, image, seq, |graph, image| engine.run(graph, run_id, "", image, &json!({})))?;
    m.ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    Ok(m)
}

/// 量一帧：`run` 用走廊测量图（JSON 文本）跑这张图。图像尺寸与示教帧不同、算法失败或输出不合契约时返回 Err，
/// 只影响这一帧（调用方记为 Error 帧，规格 §0）；胶条从走廊一侧走出时这一帧为 OutOfCorridor（默认做法 4），都不 panic。
pub fn measure_frame_with(
    corridor: &CorridorGraph, image: &FrameImage, seq: u64,
    run: impl FnOnce(&str, &FrameImage) -> Result<RunResult, String>,
) -> Result<FrameMeasurement, String> {
    if [image.width, image.height] != corridor.size {
        return Err(format!("相机 {} 这一帧图像 {}×{} 与示教帧 {}×{} 尺寸不一致", corridor.camera, image.width, image.height, corridor.size[0], corridor.size[1]));
    }
    if image.pixels.len() != image.width as usize * image.height as usize {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    let result = run(&corridor.graph_text, image)?;
    parse_corridor(corridor, &result, seq)
}

/// 逐站输出 → 帧内指标。核对单位、示教胶路、站数、弧长、中线与法向、胶边与胶宽、汇总，不读候选胶边。
fn parse_corridor(c: &CorridorGraph, result: &RunResult, seq: u64) -> Result<FrameMeasurement, String> {
    if result.status() != "ok" { return Err(result.failure()); }
    let stations = result.record("stations").ok_or("算法缺少逐站结果")?;
    let info = result.record("beadInfo").ok_or("算法缺少逐站汇总")?;
    if stations["unit"] != "px" || info["unit"] != "px" || info["lineSource"] != "taught" || info["pathOk"] != true {
        return Err("算法输出的单位或示教胶路契约不一致".into());
    }
    let n = c.stations.len();
    if stations["count"].as_u64() != Some(n as u64) || info["stations"].as_u64() != Some(n as u64) {
        return Err("算法逐站结果数量与走廊测站数不一致".into());
    }
    for key in ["s", "center", "normal", "present", "lo", "hi", "widthPx"] {
        if stations[key].as_array().map(Vec::len) != Some(n) { return Err(format!("算法逐站结果 {key} 长度与走廊测站数不一致")); }
    }
    let number = |key: &str, i: usize| stations[key][i].as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("算法第 {} 站的 {key} 不是有限数", i + 1));
    let xy = |key: &str, i: usize| -> Result<[f64; 2], String> {
        let a = stations[key][i].as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("算法第 {} 站的 {key} 不是二维坐标", i + 1))?;
        Ok([a[0].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?, a[1].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?])
    };
    // 有胶站的胶宽（px）与胶中心相对走廊的法向偏移（px，沿算法法向为正）
    let mut beads: Vec<Option<(f64, f64)>> = Vec::with_capacity(n);
    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let s = number("s", i)?;
        let expected_s = i as f64 * c.step;
        if (s - expected_s).abs() > 0.0015 + expected_s.abs() * f32::EPSILON as f64 * 4.0 {
            return Err(format!("算法第 {} 站弧长与走廊不一致", i + 1));
        }
        let center = xy("center", i)?;
        let normal = xy("normal", i)?;
        let station = c.stations[i];
        if (normal[0].hypot(normal[1]) - 1.0).abs() > 0.002 || (center[0] - station[0]).hypot(center[1] - station[1]) > 2.01 {
            return Err(format!("算法第 {} 站的中线或法向与走廊不一致", i + 1));
        }
        if stations["present"][i].as_bool().ok_or("算法有胶状态不是布尔值")? {
            let lo = number("lo", i)?;
            let hi = number("hi", i)?;
            let width = number("widthPx", i)?;
            if hi <= lo || width <= 0.0 || (width - (hi - lo)).abs() > 0.003 || lo < -c.half - 0.001 || hi > c.half + 0.001 {
                return Err(format!("算法第 {} 站胶边或胶宽超出搜索范围", i + 1));
            }
            beads.push(Some((width, (lo + hi) * 0.5)));
            edges.push(Some([
                [(center[0] + normal[0] * lo) as f32, (center[1] + normal[1] * lo) as f32],
                [(center[0] + normal[0] * hi) as f32, (center[1] + normal[1] * hi) as f32],
            ]));
        } else {
            if ["lo", "hi", "widthPx"].iter().any(|key| !stations[*key][i].is_null()) {
                return Err("算法无胶站仍返回胶宽，输出契约不一致".into());
            }
            beads.push(None);
            edges.push(None);
        }
    }
    let present = beads.iter().flatten().count();
    let coverage = present as f64 / n as f64;
    if info["present"].as_u64() != Some(present as u64) || info["coverage"].as_f64().is_none_or(|x| !x.is_finite() || (x - coverage).abs() > 0.00015) {
        return Err("算法逐站汇总与有胶状态不一致".into());
    }
    let mut metrics = frame_metrics(c, seq, &beads);
    if let Some(why) = corridor_exit(&beads, c.tolerance, c.scale) {
        metrics.state = FrameState::OutOfCorridor;
        metrics.error = Some(why);
    }
    Ok(FrameMeasurement { metrics, edges, ms: 0 })
}

/// 帧内汇总（规格 §4）：逐站胶宽（mm）、胶宽极值与均值、最长连续缺胶（缺胶站数 × 站距）、
/// 有胶站占比、胶中心相对走廊的法向偏移均值 x（AbsoluteX，直线段胶偏用）。一站胶都没有时胶宽与 x 为空，整条走廊算缺胶。
fn frame_metrics(c: &CorridorGraph, seq: u64, beads: &[Option<(f64, f64)>]) -> FrameMetrics {
    let widths: Vec<Option<f64>> = beads.iter().map(|b| b.map(|(w, _)| w * c.scale)).collect();
    let present: Vec<f64> = widths.iter().flatten().copied().collect();
    let offsets: Vec<f64> = beads.iter().flatten().map(|&(_, x)| x * c.scale).collect();
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let longest_gap = beads.split(Option::is_some).map(<[_]>::len).max().unwrap_or(0);
    FrameMetrics {
        camera: c.camera_idx,
        seq,
        state: FrameState::Measured,
        error: None,
        width_min: present.iter().copied().reduce(f64::min),
        width_max: present.iter().copied().reduce(f64::max),
        width_mean: mean(&present),
        widths: widths.iter().map(|w| w.map(|w| w as f32)).collect(),
        spacing_mm: c.spacing_mm,
        gap_max_mm: longest_gap as f64 * c.spacing_mm,
        present_ratio: present.len() as f64 / beads.len().max(1) as f64,
        x_mm: mean(&offsets),
    }
}

/// 胶条从走廊端点一侧走出（默认做法 4：弯道帧常见，记检测不完整、不判 NG）：缺胶只在连到走廊端点的一段或两段上，
/// 且紧挨每段缺胶的有胶站胶中心都已偏离走廊超过中心偏移容差的 `EXIT_OFFSET_RATIO`。整帧没有胶、有胶段中间有缺胶、
/// 或紧挨端部缺胶的胶中心仍在走廊中部时不算走出（照常按缺胶判，免得把真断胶、停胶当成检测不完整放过）。返回原因。
fn corridor_exit(beads: &[Option<(f64, f64)>], tolerance_px: f64, scale: f64) -> Option<String> {
    let first = beads.iter().position(Option::is_some)?;
    let last = beads.iter().rposition(Option::is_some)?;
    if beads[first..=last].iter().any(Option::is_none) {
        return None;
    }
    let exits = |i: usize| beads[i].is_some_and(|(_, x)| x.abs() >= EXIT_OFFSET_RATIO * tolerance_px);
    let ends: Vec<(&str, usize)> = [(first > 0).then_some(("起点", first)), (last + 1 < beads.len()).then_some(("终点", last))]
        .into_iter().flatten().collect();
    if ends.is_empty() || !ends.iter().all(|&(_, i)| exits(i)) {
        return None;
    }
    let detail: Vec<String> = ends.iter().map(|&(end, i)| {
        format!("{end}一侧第 {} 站胶中心偏离走廊 {:.2} mm", i + 1, beads[i].map_or(0.0, |(_, x)| x * scale))
    }).collect();
    Some(format!("胶条从走廊走出：{}，之外无胶", detail.join("；")))
}

#[cfg(test)]
#[path = "taught/tests.rs"]
mod tests;

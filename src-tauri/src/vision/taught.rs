use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};

use super::heading::{measure_heading_with, plan_heading, HeadingGraph};
use super::{Engine, FrameImage, RunResult};
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::plan::{EdgePolarity, OffsetMeasure};
use crate::recipe::{CameraInspect, DetectParams, Polarity, Recipe};

/// 走廊测量图的像素参数，按 lyFlow 契约约束（glue.taught_path + glue.bead_width）核对。
pub(super) struct GraphParams {
    /// 站距（px）
    step: f64,
    /// 搜索半宽（px）
    pub(super) half: f64,
    lo: f64,
    pub(super) hi: f64,
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
    bead_params(scale, step, detect, length)
}

/// 量胶宽（`glue.bead_width`）的像素参数：示教走廊与逐帧定向共用；`length` 是胶路长度（px）。
pub(super) fn bead_params(
    scale: f64,
    step: f64,
    detect: &DetectParams,
    length: f64,
) -> Result<GraphParams, &'static str> {
    if !scale.is_finite() || scale <= 0.0 || scale > 10.0 {
        return Err("像素当量必须在 0–10 mm/px 之间");
    }
    if !step.is_finite() || step < 1.0 {
        return Err("lyFlow 站距至少需要 1 px，请检查站距和像素当量");
    }
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

/// 走廊至少的站数：lyFlow 1.1 的 glue.bead_width 少于 3 站时不报错，但每站都测成无胶（有胶的帧被判断胶）。
const MIN_STATIONS: usize = 3;

/// 站数不足的说明（与示教页 corridorStationError 同口径）：实际站数、能用的最大站距或需要的走廊长度（mm）。
fn too_few_stations(n: usize, length_mm: f64, spacing_mm: f32) -> String {
    let gaps = (MIN_STATIONS - 1) as f64;
    let spacing_mm = (f64::from(spacing_mm) * 1000.0).round() / 1000.0;
    let max = (length_mm / gaps * 10.0).floor() / 10.0;
    let need = (gaps * spacing_mm * 10.0 - 1e-9).ceil() / 10.0;
    let shrink = if max >= 0.1 {
        format!("把站距改到不大于 {max} mm，或")
    } else {
        String::new()
    };
    format!("按站距 {spacing_mm} mm，检测走廊（{length_mm:.1} mm）只有 {n} 站，至少要 {MIN_STATIONS} 站：{shrink}把走廊画到不短于 {need} mm")
}

/// 沿中线量胶的测量图：io.load_image → glue.taught_path（示教中线）→ glue.bead_width（逐站胶边与胶宽）。
fn taught_graph(
    path: &[[f32; 2]],
    detect: &DetectParams,
    p: &GraphParams,
    edge: Option<&EdgeGraph>,
) -> Result<Value, String> {
    let path = json!({"id": "n_path", "op": "glue.taught_path", "params": {
        "points": serde_json::to_string(path).map_err(|e| e.to_string())?,
        "zone": [0.0, 0.0], "tolerance": p.tolerance, "widthMax": p.hi.ceil(),
        "polarity": polarity(detect), "sharpMin": 0.4
    }});
    Ok(bead_graph(
        "01GLUESIGHTTAUGHT00000000",
        path,
        false,
        p,
        edge,
        json!({}),
    ))
}

pub(super) fn polarity(detect: &DetectParams) -> &'static str {
    if detect.polarity == Polarity::Light {
        "bright"
    } else {
        "dark"
    }
}

/// 找零件边节点（`glue.edge_distance`，D-SB-48）的像素参数。
#[derive(Clone, Debug)]
pub(super) struct EdgeGraph {
    polarity: &'static str,
    search_px: f64,
    contrast_min: f64,
}

/// 第 `segment` 段胶偏按 ToEdge 测、参考相机是 `camera` 时的找边参数；其余返回 None。搜索长度换算后要比 `farRun`（12 px）长 10 px。
pub(super) fn edge_graph(
    recipe: &Recipe,
    segment: u32,
    camera: &str,
    scale: f64,
) -> Result<Option<EdgeGraph>, String> {
    let Some(offset) = recipe
        .path_segments
        .iter()
        .find(|s| s.index == segment)
        .and_then(|s| s.offset_ref.as_ref())
    else {
        return Ok(None);
    };
    let (OffsetMeasure::ToEdge, Some(edge)) = (offset.measure, offset.edge.as_ref()) else {
        return Ok(None);
    };
    if offset.camera != camera {
        return Ok(None);
    }
    let search_px = edge.search_mm / scale;
    if search_px < EDGE_FAR_RUN_PX + 10.0 {
        return Err(format!(
            "第 {segment} 段相机 {camera}：找边搜索长度 {} mm 换算后不足 {} px",
            edge.search_mm,
            EDGE_FAR_RUN_PX + 10.0
        ));
    }
    Ok(Some(EdgeGraph {
        polarity: match edge.polarity {
            EdgePolarity::BrightToDark => "bright_to_dark",
            EdgePolarity::DarkToBright => "dark_to_bright",
        },
        search_px,
        contrast_min: edge.contrast_min,
    }))
}

/// `glue.edge_distance` 的远侧持续长度（px，lyFlow 默认）。
const EDGE_FAR_RUN_PX: f64 = 12.0;

/// 量胶测量图：读图 → 定胶路节点（`path`，id 为 n_path）→ `glue.bead_width`，有找边参数时再接 `glue.edge_distance`。
/// `path_reads_image`：定胶路节点要读图（`glue.bead_path`）。
pub(super) fn bead_graph(
    id: &str,
    path: Value,
    path_reads_image: bool,
    p: &GraphParams,
    edge: Option<&EdgeGraph>,
    mut outputs: Value,
) -> Value {
    let mut nodes = vec![
        json!({"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}}),
        path,
        json!({"id": "n_width", "op": "glue.bead_width", "params": {
            "form": "straight", "widthRange": [p.lo, p.hi], "stationStep": p.step, "searchHalf": p.half,
            "window": 30.0, "mergeGap": 8.0, "contrastMin": 16.0,
            "presentRatio": 0.5, "centerRatio": 0.3, "contrastRatio": 0.3
        }}),
    ];
    let mut edges = vec![
        json!({"id": "e_image", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_width", "port": "image"}}),
        json!({"id": "e_path", "from": {"node": "n_path", "port": "path"}, "to": {"node": "n_width", "port": "path"}}),
    ];
    if path_reads_image {
        edges.push(json!({"id": "e_pimage", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_path", "port": "image"}}));
    }
    outputs["stations"] = json!({"node": "n_width", "port": "bead.stations"});
    outputs["beadInfo"] = json!({"node": "n_width", "port": "bead.info"});
    if let Some(e) = edge {
        nodes.push(
            json!({"id": "n_edge", "op": "glue.edge_distance", "params": {
                "searchLength": e.search_px, "reference": "bead_center", "edgePolarity": e.polarity,
                "contrastMin": e.contrast_min, "farRun": EDGE_FAR_RUN_PX
            }}),
        );
        edges.push(json!({"id": "e_eimage", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_edge", "port": "image"}}));
        edges.push(json!({"id": "e_bead", "from": {"node": "n_width", "port": "bead"}, "to": {"node": "n_edge", "port": "bead"}}));
        outputs["edge"] = json!({"node": "n_edge", "port": "edge"});
    }
    json!({"schemaVersion": 1, "id": id, "nodes": nodes, "edges": edges, "outputs": outputs})
}

/// 找边结果 → 胶中心到零件边距离的均值（mm）。有胶测站里找到边的不到一半时为 None（这一帧胶偏记未测）。
pub(super) fn edge_mm(result: &RunResult, scale: f64) -> Result<Option<f64>, String> {
    let edge = result.record("edge").ok_or("算法缺少找边结果")?;
    let status = edge["status"].as_array().ok_or("找边结果缺少逐站状态")?;
    let distance = edge["distancePx"]
        .as_array()
        .ok_or("找边结果缺少逐站距离")?;
    if status.len() != distance.len() {
        return Err("找边结果逐站状态与距离数量不一致".into());
    }
    let searched = status
        .iter()
        .filter(|s| s.as_str() != Some("nobead"))
        .count();
    let found: Vec<f64> = status
        .iter()
        .zip(distance)
        .filter(|(s, _)| s.as_str() == Some("ok"))
        .filter_map(|(_, d)| d.as_f64().filter(|v| v.is_finite()))
        .collect();
    if searched == 0 || found.len() * 2 < searched {
        return Ok(None);
    }
    Ok(Some(found.iter().sum::<f64>() / found.len() as f64 * scale))
}

// ───────────── 段配方：每台相机一条检测走廊，每帧独立测（D-SB-12、D-SB-17） ─────────────

/// 胶条从走廊端点一侧走出的判据：紧挨端部缺胶段的有胶站，胶中心偏离走廊超过中心偏移容差的这一比例。
/// 默认做法 4 的实现，判据没有经过真实弯道图像验证。
const EXIT_OFFSET_RATIO: f64 = 0.5;

/// 段配方一段、一台检测相机的走廊测量图（D-SB-45）：沿走廊按站距取测站。测站不进配方全局站表，
/// 每帧只输出帧内汇总的 `FrameMetrics`（规格 §4 每帧独立指标）。
#[derive(Clone, Debug)]
pub struct CorridorGraph {
    pub camera: String,
    /// 计划相机下标（`Recipe.inspect` 的顺序），即 `FrameMetrics.camera`
    pub camera_idx: usize,
    /// 走廊所属的段（`PathSegment.index`），即 `FrameMetrics.corridor`
    pub segment: u32,
    /// 示教帧尺寸；每帧必须与它相同
    pub size: [u32; 2],
    pub graph: Value,
    graph_text: String,
    /// 各测站在图里的位置（px），沿走廊弧长 i × step
    stations: Vec<[f64; 2]>,
    /// 图里接了找零件边（D-SB-48）
    edge: bool,
    scale: f64,
    step: f64,
    half: f64,
    tolerance: f64,
    spacing_mm: f64,
}

/// 一台相机一帧的测量：帧内指标与逐站胶边（px，无胶站为 None，叠图用）；逐帧定向时 `line` 是拟合出的胶路（px）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameMeasurement {
    pub metrics: FrameMetrics,
    pub edges: Vec<Option<[[f32; 2]; 2]>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line: Vec<[f32; 2]>,
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

/// 段配方第 `segment` 段、一台检测相机的走廊测量图：示教帧尺寸为 size。走廊必须完整落在图内（不然 lyFlow 量不到走廊两端）；
/// 当量与检测参数按相机，当量、站距与检测参数须在 lyFlow 的取值范围内；至少三站。
pub fn plan_corridor(
    inspect: &CameraInspect,
    segment: u32,
    recipe: &Recipe,
    size: [u32; 2],
) -> Result<CorridorGraph, String> {
    let camera_idx = recipe
        .inspect
        .iter()
        .position(|i| i.camera == inspect.camera)
        .ok_or_else(|| format!("相机 {} 不是配方 {} 的检测相机", inspect.camera, recipe.id))?;
    let fail = |why: &str| format!("第 {segment} 段相机 {}：{why}", inspect.camera);
    let corridor = inspect
        .segment_corridor(segment)
        .map(|s| s.corridor.as_slice())
        .unwrap_or_default();
    if corridor.len() < 2 {
        return Err(fail("尚未示教检测走廊"));
    }
    let Some(scale) = inspect.mm_per_px else {
        return Err(fail("缺少像素当量"));
    };
    let detect = inspect.detect.as_ref().unwrap_or(&recipe.detect);
    let params =
        graph_params(corridor, scale, recipe.spacing as f64 / scale, detect).map_err(fail)?;
    if !corridor_inside(corridor, size) {
        return Err(fail(&format!(
            "检测走廊超出示教帧 {}×{}，请在这一段的示教帧上重新示教走廊",
            size[0], size[1]
        )));
    }
    let n = station_count(params.length, params.step);
    if n < MIN_STATIONS {
        return Err(fail(&too_few_stations(
            n,
            params.length * scale,
            recipe.spacing,
        )));
    }
    if n > 200_000 {
        return Err(fail("检测走廊超过 20 万站，请调整走廊长度或站距"));
    }
    let stations = (0..n)
        .map(|i| polyline_at(corridor, i as f64 * params.step))
        .collect();
    let edge = edge_graph(recipe, segment, &inspect.camera, scale)?;
    let graph = taught_graph(corridor, detect, &params, edge.as_ref())?;
    Ok(CorridorGraph {
        camera: inspect.camera.clone(),
        camera_idx,
        segment,
        size,
        graph_text: graph.to_string(),
        graph,
        stations,
        edge: edge.is_some(),
        scale,
        step: params.step,
        half: params.half,
        tolerance: params.tolerance,
        spacing_mm: recipe.spacing as f64,
    })
}

/// 段配方全部“段 × 检测相机”的走廊测量图（D-SB-45），按（计划相机下标, 段号）取。`sizes` 为各计划相机的示教帧尺寸
/// （计划相机顺序，一台相机的各段示教帧同尺寸）。没示教的组合不建图，取用时返回 None。
#[derive(Clone, Debug, Default)]
pub struct CorridorSet {
    graphs: BTreeMap<(usize, u32), CorridorGraph>,
    /// 开启逐帧定向的段（D-SB-47）：同一（相机, 段）优先用它
    headings: BTreeMap<(usize, u32), HeadingGraph>,
}

/// 一（相机, 段）量帧用的测量图：示教走廊，或逐帧定向（D-SB-47）。
#[derive(Clone, Copy, Debug)]
pub enum FrameGraph<'a> {
    Corridor(&'a CorridorGraph),
    Heading(&'a HeadingGraph),
}

impl FrameGraph<'_> {
    pub fn camera(&self) -> &str {
        match self {
            Self::Corridor(c) => &c.camera,
            Self::Heading(h) => &h.camera,
        }
    }
}

impl CorridorSet {
    pub fn build(recipe: &Recipe, sizes: &[[u32; 2]]) -> Result<Self, String> {
        let cameras = recipe.plan_cameras();
        let mut graphs = BTreeMap::new();
        let mut headings = BTreeMap::new();
        for segment in &recipe.path_segments {
            for camera in crate::plan::segment_cameras(segment, &cameras) {
                let Some((idx, inspect)) = recipe
                    .inspect
                    .iter()
                    .enumerate()
                    .find(|(_, i)| i.camera == camera)
                else {
                    continue;
                };
                if !inspect.segment_taught(segment.index) {
                    continue;
                }
                let size = *sizes
                    .get(idx)
                    .ok_or_else(|| format!("相机 {camera} 没有示教帧尺寸"))?;
                graphs.insert(
                    (idx, segment.index),
                    plan_corridor(inspect, segment.index, recipe, size)?,
                );
                if segment.heading.is_some() {
                    headings.insert(
                        (idx, segment.index),
                        plan_heading(inspect, segment.index, recipe, size)?,
                    );
                }
            }
        }
        Ok(Self { graphs, headings })
    }

    pub fn get(&self, camera: usize, segment: u32) -> Option<FrameGraph<'_>> {
        self.headings
            .get(&(camera, segment))
            .map(FrameGraph::Heading)
            .or_else(|| {
                self.graphs
                    .get(&(camera, segment))
                    .map(FrameGraph::Corridor)
            })
    }

    pub fn iter(&self) -> impl Iterator<Item = &CorridorGraph> {
        self.graphs.values()
    }

    pub fn headings(&self) -> impl Iterator<Item = &HeadingGraph> {
        self.headings.values()
    }
}

/// 用 lyFlow 量一帧（段配方）：示教走廊见 [`measure_frame_with`]，逐帧定向见 [`measure_heading_with`]。
pub fn measure_frame(
    engine: &Engine,
    graph: FrameGraph<'_>,
    image: &FrameImage,
    seq: u64,
    run_id: &str,
) -> Result<FrameMeasurement, String> {
    let start = Instant::now();
    let run = |graph: &str, image: &FrameImage| engine.run(graph, run_id, "", image, &json!({}));
    let mut m = match graph {
        FrameGraph::Corridor(corridor) => measure_frame_with(corridor, image, seq, run)?,
        FrameGraph::Heading(heading) => measure_heading_with(heading, image, seq, run)?,
    };
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
    let mut metrics = bead_metrics(c.camera_idx, c.segment, seq, c.scale, c.spacing_mm, &beads);
    if c.edge {
        metrics.edge_mm = edge_mm(result, c.scale)?;
    }
    if let Some(why) = corridor_exit(&beads, c.tolerance, c.scale) {
        metrics.state = FrameState::OutOfCorridor;
        metrics.error = Some(why);
    }
    Ok(FrameMeasurement {
        metrics,
        edges,
        line: Vec::new(),
        ms: 0,
    })
}

/// 帧内汇总（规格 §4）：逐站胶宽（mm）、胶宽极值与均值、最长连续缺胶（缺胶站数 × 站距）、
/// 有胶站占比、胶中心相对胶路的法向偏移均值 x（AbsoluteX，直线段胶偏用）。一站胶都没有时胶宽与 x 为空，整条胶路算缺胶。
pub(super) fn bead_metrics(
    camera: usize,
    segment: u32,
    seq: u64,
    scale: f64,
    spacing_mm: f64,
    beads: &[Option<(f64, f64)>],
) -> FrameMetrics {
    let widths: Vec<Option<f64>> = beads.iter().map(|b| b.map(|(w, _)| w * scale)).collect();
    let present: Vec<f64> = widths.iter().flatten().copied().collect();
    let offsets: Vec<f64> = beads.iter().flatten().map(|&(_, x)| x * scale).collect();
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let longest_gap = beads.split(Option::is_some).map(<[_]>::len).max().unwrap_or(0);
    FrameMetrics {
        camera,
        seq,
        state: FrameState::Measured,
        error: None,
        width_min: present.iter().copied().reduce(f64::min),
        width_max: present.iter().copied().reduce(f64::max),
        width_mean: mean(&present),
        widths: widths.iter().map(|w| w.map(|w| w as f32)).collect(),
        spacing_mm,
        gap_max_mm: longest_gap as f64 * spacing_mm,
        corridor: Some(segment),
        present_ratio: present.len() as f64 / beads.len().max(1) as f64,
        x_mm: mean(&offsets),
        heading_deg: None,
        heading_fallback: false,
        fallback_reason: None,
        edge_mm: None,
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

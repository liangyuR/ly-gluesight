use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::heading::{measure_heading_with, plan_heading, HeadingGraph};
use super::judge::edge::{judge_edges, EdgeWindow};
use super::judge::taught::{judge_taught, LateralRuns, TaughtParams};
use super::template::{self, number, Template};
use super::{Engine, FrameImage, RunResult};
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::plan::{EdgePolarity, OffsetMeasure};
use crate::recipe::{CameraInspect, DetectParams, Polarity, Recipe};

/// 量胶宽的像素参数，按 lyFlow 1.2 算子的取值范围核对。
pub(super) struct GraphParams {
    /// 站距（px）
    pub(super) step: f64,
    /// 搜索半宽（px）
    pub(super) half: f64,
    pub(super) lo: f64,
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

/// 量胶宽（`glue.lateral_runs`）的像素参数：示教走廊与逐帧定向共用；`length` 是胶路长度（px）。
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

/// 走廊至少的站数：少于 3 站时偏移轨迹一站都立不住，每站都判成无胶（有胶的帧被判断胶）。
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

/// 示教走廊测量图（`template::corridor`）的业务参数：示教中线、响应图、量胶宽，有找边时再加找边参数。
/// 卡尺半长取 max(搜索半宽, 容差 + 胶宽上限 / 2)，与 1.1 示教胶路同一条式子。
fn corridor_params(
    path: &[[f32; 2]],
    detect: &DetectParams,
    p: &GraphParams,
    edge: Option<&EdgeGraph>,
) -> Result<(Value, f64), String> {
    let points = serde_json::to_string(path).map_err(|e| e.to_string())?;
    let parsed: Vec<[f64; 2]> = serde_json::from_str(&points).map_err(|e| e.to_string())?;
    let half = p.half.max(p.tolerance + 0.5 * p.hi);
    let [x0, y0, x1, y1] = taught_bbox(&parsed).ok_or("示教中线无效")?;
    let pad = half + 4.0;
    let roi = [
        (x0 - pad).floor(),
        (y0 - pad).floor(),
        (x1 - x0 + 2.0 * pad).ceil() + 1.0,
        (y1 - y0 + 2.0 * pad).ceil() + 1.0,
    ];
    let mut params = json!({"points": points});
    response_params_into(&mut params, detect, p, roi);
    bead_params_into(&mut params, p, half, edge)?;
    Ok((params, half))
}

/// 1.1 示教胶路（`glue.polyline` 同一套重采样）的点：从起点每 2 px 一点、末端一点，坐标舍入到 3 位小数；返回包围盒。
/// 响应图的块按它算才与 1.1 量胶宽算的那一块相同（块外的采样无效）。
fn taught_bbox(points: &[[f64; 2]]) -> Option<[f64; 4]> {
    let mut q: Vec<[f64; 2]> = Vec::new();
    let mut arc: Vec<f64> = Vec::new();
    for p in points {
        let step = q
            .last()
            .map(|b: &[f64; 2]| (p[0] - b[0]).hypot(p[1] - b[1]));
        if step.is_some_and(|l| l < 1e-9) {
            continue;
        }
        arc.push(arc.last().copied().unwrap_or(0.0) + step.unwrap_or(0.0));
        q.push(*p);
    }
    let total = *arc.last()?;
    if q.len() < 2 || !(total > 0.0) {
        return None;
    }
    let mut targets = Vec::new();
    let mut t = 0.0;
    while t < total - 1e-9 {
        targets.push(t);
        t += 2.0;
    }
    targets.push(total);
    let round3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let mut bbox = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut seg = 0;
    for t in targets {
        while seg + 2 < q.len() && arc[seg + 1] < t {
            seg += 1;
        }
        let span = arc[seg + 1] - arc[seg];
        let f = if span > 1e-12 {
            ((t - arc[seg]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (a, b) = (q[seg], q[seg + 1]);
        let (x, y) = (
            round3(a[0] + (b[0] - a[0]) * f),
            round3(a[1] + (b[1] - a[1]) * f),
        );
        bbox = [
            bbox[0].min(x),
            bbox[1].min(y),
            bbox[2].max(x),
            bbox[3].max(y),
        ];
    }
    Some(bbox)
}

/// 响应图（`glue.response`）的业务参数：极性、结构元直径 = 胶宽上限取整 + 1（同 1.1），只算 roi 那一块。
pub(super) fn response_params_into(
    params: &mut Value,
    detect: &DetectParams,
    p: &GraphParams,
    roi: [f64; 4],
) {
    params["polarity"] = json!(polarity(detect));
    params["diameter"] = json!(p.hi.ceil() as i64 + 1);
    params["roi"] = json!(roi.map(|v| v as i64));
}

/// 量胶宽与找边的业务参数（示教走廊与逐帧定向共用）：站距、卡尺半长 `half`、合并后宽度上限，
/// 有找边时找边剖面要盖住胶边在卡尺端点时 1.1 的整个找边窗口。
pub(super) fn bead_params_into(
    params: &mut Value,
    p: &GraphParams,
    half: f64,
    edge: Option<&EdgeGraph>,
) -> Result<(), String> {
    params["stationStep"] = json!(p.step);
    params["searchHalf"] = json!(half);
    params["widthMax"] = json!(p.hi);
    if let Some(e) = edge {
        let length = e.window(EDGE_FAR_RUN_PX).profile_length(half);
        if length > EDGE_PROFILE_MAX_PX {
            return Err(format!("找边剖面需 {length} px，超过 lyFlow 上限 {EDGE_PROFILE_MAX_PX} px，请缩短找边搜索长度"));
        }
        params["edgeLength"] = json!(length);
        params["edgePolarity"] = json!(e.polarity);
        params["edgeContrastMin"] = json!(e.contrast_min);
    }
    Ok(())
}

/// `glue.part_edges` 剖面长度的上限（px）。
const EDGE_PROFILE_MAX_PX: f64 = 2000.0;

pub(super) fn polarity(detect: &DetectParams) -> &'static str {
    if detect.polarity == Polarity::Light {
        "bright"
    } else {
        "dark"
    }
}

/// 找零件边（`glue.part_edges` + `judge::edge`，D-SB-48）的像素参数；`search_px` 是 1.1 `glue.edge_distance` 的搜索长度。
#[derive(Clone, Debug)]
pub(super) struct EdgeGraph {
    polarity: &'static str,
    search_px: f64,
    contrast_min: f64,
}

impl EdgeGraph {
    pub(super) fn window(&self, far_run_px: f64) -> EdgeWindow {
        EdgeWindow {
            search_px: self.search_px,
            far_run_px,
        }
    }
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

/// 找边的远侧持续长度（px，模板 `edgeFarRun` 的默认值）。
const EDGE_FAR_RUN_PX: f64 = 12.0;

/// 找边记录（`glue.part_edges`）按 1.1 口径判 → 胶中心到零件边距离的均值（mm）。`beads` 是逐站选中的胶边 (lo, hi)（px）；
/// 有胶测站里找到边的不到一半时为 None（这一帧胶偏记未测）。
pub(super) fn edge_mm(
    edges: Option<&Value>,
    beads: &[Option<(f64, f64)>],
    window: &EdgeWindow,
    scale: f64,
) -> Result<Option<f64>, String> {
    let edges = edges.ok_or("算法缺少找边结果")?;
    Ok(judge_edges(edges, beads, window)?
        .mean_distance_px()
        .map(|d| d * scale))
}

/// 一站卡尺的位置：站点与单位法向（px）。
pub(super) fn station_pose(station: &Value, i: usize) -> Result<([f64; 2], [f64; 2]), String> {
    let xy = |key: &str| -> Result<[f64; 2], String> {
        let a = station[key]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| format!("算法第 {} 站的 {key} 不是二维坐标", i + 1))?;
        let v = |k: usize| {
            a[k].as_f64()
                .filter(|v| v.is_finite())
                .ok_or("算法坐标不是有限数")
        };
        Ok([v(0)?, v(1)?])
    };
    Ok((xy("center")?, xy("normal")?))
}

/// 胶两边沿法向的偏移 → 图上的两个点（叠图用）。
pub(super) fn edge_points(
    center: [f64; 2],
    normal: [f64; 2],
    (lo, hi): (f64, f64),
) -> [[f32; 2]; 2] {
    let at = |t: f64| {
        [
            (center[0] + normal[0] * t) as f32,
            (center[1] + normal[1] * t) as f32,
        ]
    };
    [at(lo), at(hi)]
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
    /// `template::corridor` 的全部有效参数与判定门限（发布包冻结它们）
    pub params: Value,
    pub judge: Value,
    targets: Vec<String>,
    /// 各测站在图里的位置（px），沿走廊弧长 i × step
    stations: Vec<[f64; 2]>,
    /// 接了找零件边（D-SB-48）时的找边窗口
    edge: Option<EdgeWindow>,
    taught: TaughtParams,
    scale: f64,
    step: f64,
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
        let (a, b) = (
            [w[0][0] as f64, w[0][1] as f64],
            [w[1][0] as f64, w[1][1] as f64],
        );
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        if left <= l && l > 0.0 {
            let f = left / l;
            return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
        }
        left -= l;
    }
    path.last()
        .map_or([0.0, 0.0], |p| [p[0] as f64, p[1] as f64])
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
    let (business, _) =
        corridor_params(corridor, detect, &params, edge.as_ref()).map_err(|e| fail(&e))?;
    let expert = recipe.expert_layers(inspect);
    let effective = template::corridor()
        .params(business, &expert)
        .map_err(|e| fail(&e))?;
    let judge = template::corridor().judge(&expert);
    let gate = |name: &str| number(&judge, name).map_err(|e| fail(&e));
    let taught = TaughtParams {
        tolerance: params.tolerance,
        sharp_min: gate("trackSharpMin")?,
        width_range: [params.lo, params.hi],
        present_ratio: gate("presentRatio")?,
        center_ratio: gate("centerRatio")?,
        contrast_ratio: gate("contrastRatio")?,
    };
    let mut targets = vec!["n_runs".to_string(), "n_split".to_string()];
    let edge = match edge {
        Some(e) => {
            targets.push("n_edge".into());
            Some(e.window(number(&effective, "edgeFarRun").map_err(|e| fail(&e))?))
        }
        None => None,
    };
    Ok(CorridorGraph {
        camera: inspect.camera.clone(),
        camera_idx,
        segment,
        size,
        params: effective,
        judge,
        targets,
        stations,
        edge,
        taught,
        scale,
        step: params.step,
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
    let run = |g: GraphRun<'_>, image: &FrameImage| {
        engine.run_targets(g.template.text, run_id, image, g.params, g.targets)
    };
    let mut m = match graph {
        FrameGraph::Corridor(corridor) => measure_frame_with(corridor, image, seq, run)?,
        FrameGraph::Heading(heading) => measure_heading_with(heading, image, seq, run)?,
    };
    m.ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    Ok(m)
}

/// 一次测量图运行：模板、全部有效参数与运行目标（空为整张图）。
#[derive(Clone, Copy)]
pub struct GraphRun<'a> {
    pub template: &'static Template,
    pub params: &'a Value,
    pub targets: &'a [String],
}

/// 量一帧：`run` 按模板与参数跑这张图。图像尺寸与示教帧不同、算法失败或输出不合契约时返回 Err，
/// 只影响这一帧（调用方记为 Error 帧，规格 §0）；胶条从走廊一侧走出时这一帧为 OutOfCorridor（默认做法 4），都不 panic。
pub fn measure_frame_with(
    corridor: &CorridorGraph, image: &FrameImage, seq: u64,
    run: impl FnOnce(GraphRun<'_>, &FrameImage) -> Result<RunResult, String>,
) -> Result<FrameMeasurement, String> {
    if [image.width, image.height] != corridor.size {
        return Err(format!("相机 {} 这一帧图像 {}×{} 与示教帧 {}×{} 尺寸不一致", corridor.camera, image.width, image.height, corridor.size[0], corridor.size[1]));
    }
    if image.pixels.len() != image.width as usize * image.height as usize {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    let result = run(
        GraphRun {
            template: template::corridor(),
            params: &corridor.params,
            targets: &corridor.targets,
        },
        image,
    )?;
    parse_corridor(corridor, &result, seq)
}

/// 横向卡尺记录 → 判定 → 帧内指标。核对站数、弧长、中线与法向，按 1.1 示教直胶的规则判每站有无胶（`judge::taught`）。
fn parse_corridor(c: &CorridorGraph, result: &RunResult, seq: u64) -> Result<FrameMeasurement, String> {
    if result.status() != "ok" { return Err(result.failure()); }
    let raw = result.record("runs").ok_or("算法缺少横向卡尺结果")?;
    let runs = lateral_runs(raw, "横向卡尺")?;
    let split = lateral_runs(
        result
            .record("split")
            .ok_or("算法缺少合并前的横向卡尺结果")?,
        "合并前的横向卡尺",
    )?;
    let n = c.stations.len();
    if runs.stations.len() != n || split.stations.len() != n {
        return Err("算法逐站结果数量与走廊测站数不一致".into());
    }
    let mut poses = Vec::with_capacity(n);
    for (i, station) in runs.stations.iter().enumerate() {
        let expected_s = i as f64 * c.step;
        if (station.s - expected_s).abs() > 0.0015 + expected_s.abs() * f32::EPSILON as f64 * 4.0 {
            return Err(format!("算法第 {} 站弧长与走廊不一致", i + 1));
        }
        let (center, normal) = station_pose(&raw["stations"][i], i)?;
        let at = c.stations[i];
        if (normal[0].hypot(normal[1]) - 1.0).abs() > 0.002
            || (center[0] - at[0]).hypot(center[1] - at[1]) > 2.01
        {
            return Err(format!("算法第 {} 站的中线或法向与走廊不一致", i + 1));
        }
        poses.push((center, normal));
    }
    let judged = judge_taught(&runs, Some(&split), &c.taught)?;
    // 有胶站的胶宽（px）与胶中心相对走廊的法向偏移（px，沿算法法向为正）
    let beads: Vec<Option<(f64, f64)>> = judged
        .beads
        .iter()
        .map(|b| b.map(|(lo, hi)| (hi - lo, 0.5 * (lo + hi))))
        .collect();
    let edges = judged
        .beads
        .iter()
        .zip(&poses)
        .map(|(b, &(center, normal))| b.map(|b| edge_points(center, normal, b)))
        .collect();
    let mut metrics = bead_metrics(c.camera_idx, c.segment, seq, c.scale, c.spacing_mm, &beads);
    if let Some(window) = &c.edge {
        metrics.edge_mm = edge_mm(result.record("edges"), &judged.beads, window, c.scale)?;
    }
    if let Some(why) = corridor_exit(&beads, c.taught.tolerance, c.scale) {
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

/// `glue.LateralRuns` 记录里判定用到的字段；`what` 是出错时的主语。
pub(super) fn lateral_runs(record: &Value, what: &str) -> Result<LateralRuns, String> {
    LateralRuns::deserialize(record).map_err(|e| format!("算法{what}结果不合契约：{e}"))
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

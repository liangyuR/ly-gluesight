use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::heading::{measure_heading_with, plan_heading, HeadingGraph};
use super::judge::edge::{judge_edges, EdgeSide, EdgeStatus, EdgeWindow};
use super::judge::taught::{
    judge_color_stations, judge_taught, ColorStation, LateralRuns, TaughtParams,
};
use super::template::{self, number, Template};
use super::{Engine, FrameImage, RunResult};
use crate::burst_judge::{
    FrameMetrics, FrameState, MeasurementEvidence, StationEvidence, StationEvidenceState,
};
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

/// 示教走廊测量图（`template::corridor`，有颜色模型时 `template::corridor_color`）的业务参数：示教中线、响应图（或胶色得分图）、
/// 量胶宽，有找边时再加找边参数。卡尺半长取 max(搜索半宽, 容差 + 胶宽上限 / 2)，与 1.1 示教胶路同一条式子。
fn corridor_params(
    path: &[[f32; 2]],
    detect: &DetectParams,
    p: &GraphParams,
    edge: Option<&EdgeGraph>,
    color_model: Option<&str>,
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
    match color_model {
        Some(model) => color_params_into(&mut params, model, roi),
        None => response_params_into(&mut params, detect, p, roi),
    }
    bead_params_into(&mut params, p, half, edge)?;
    Ok((params, half))
}

/// 这台相机这一段测量用的颜色模型（按选的测量方式，见 `SegmentCorridor::measuring_color_model`）。
pub(super) fn measuring_color_model<'a>(
    inspect: &'a CameraInspect,
    segment: u32,
) -> Result<Option<&'a str>, String> {
    match inspect.segment_corridor(segment) {
        Some(s) => s.measuring_color_model(),
        None => Ok(None),
    }
}

/// 胶色得分图（`glue.color_map`）的业务参数：颜色模型原文，只算 roi 那一块。
pub(super) fn color_params_into(params: &mut Value, model: &str, roi: [f64; 4]) {
    params["colorModel"] = json!(model);
    params["roi"] = json!(roi.map(|v| v as i64));
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

/// 一帧找边的结果：胶中心到零件边距离的均值（mm，有胶测站里找到边的不到一半时为 None，这一帧胶偏记未测），
/// 与叠图用的胶边距线（找边判为 ok 的站，胶中心 → 零件边）。
pub(super) struct EdgeMeasure {
    pub mm: Option<f64>,
    pub lines: Vec<[[f32; 2]; 2]>,
}

/// 找边记录（`glue.part_edges`）按 1.1 口径判。`beads` 是逐站选中的胶边 (lo, hi)（px），`poses` 是逐站站点与单位法向。
pub(super) fn edge_measure(
    edges: Option<&Value>,
    beads: &[Option<(f64, f64)>],
    poses: &[([f64; 2], [f64; 2])],
    window: &EdgeWindow,
    scale: f64,
) -> Result<EdgeMeasure, String> {
    let edges = edges.ok_or("算法缺少找边结果")?;
    let judged = judge_edges(edges, beads, window)?;
    // 距离从胶中心沿零件边一侧量：右侧沿 +n，左侧沿 −n
    let sign = match judged.side {
        Some(EdgeSide::Right) => 1.0,
        Some(EdgeSide::Left) => -1.0,
        None => 0.0,
    };
    let lines = judged
        .stations
        .iter()
        .zip(beads)
        .zip(poses)
        .filter(|((station, _), _)| station.status == EdgeStatus::Ok)
        .filter_map(|((station, bead), &(center, normal))| {
            let (lo, hi) = (*bead)?;
            let mid = 0.5 * (lo + hi);
            let at = |t: f64| {
                [
                    (center[0] + normal[0] * t) as f32,
                    (center[1] + normal[1] * t) as f32,
                ]
            };
            Some([at(mid), at(mid + sign * station.distance_px?)])
        })
        .collect();
    Ok(EdgeMeasure {
        mm: judged.mean_distance_px().map(|d| d * scale),
        lines,
    })
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
    /// 示教帧通道数（1 灰度、3 彩色）；未知时不核对（见 `CorridorSet::with_channels`）
    pub channels: Option<u8>,
    /// 测量图模板：灰度 `template::corridor`，这一段有颜色模型时 `template::corridor_color`
    pub template: &'static Template,
    /// 模板的全部有效参数与判定门限（发布包冻结它们）
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
    /// 胶边距线（ToEdge）：找到零件边的站，胶中心 → 零件边；不找边时为空
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_edges: Vec<[[f32; 2]; 2]>,
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
    let color_model = measuring_color_model(inspect, segment).map_err(|e| fail(&e))?;
    let template = if color_model.is_some() {
        template::corridor_color()
    } else {
        template::corridor()
    };
    let (business, _) = corridor_params(corridor, detect, &params, edge.as_ref(), color_model)
        .map_err(|e| fail(&e))?;
    let expert = recipe.expert_layers(inspect);
    let effective = template.params(business, &expert).map_err(|e| fail(&e))?;
    let judge = template.judge(&expert);
    let gate = |name: &str| number(&judge, name).map_err(|e| fail(&e));
    let taught = TaughtParams {
        tolerance: if color_model.is_some() {
            params.half
        } else {
            params.tolerance
        },
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
        channels: None,
        template,
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

/// 建不成测量图的一（相机, 段）：没示教走廊，或走廊、当量、定向参数有问题。
#[derive(Clone, Debug, PartialEq)]
pub struct UnavailableCorridor {
    pub camera_idx: usize,
    pub camera: String,
    pub segment: u32,
    pub reason: String,
    /// 只是还没示教走廊（`build` 跳过这类，不报错）
    pub untaught: bool,
}

impl CorridorSet {
    /// 全部建成：没示教的组合跳过，其余任一组合建不成即报错（发布、预热、验证用）。
    pub fn build(recipe: &Recipe, sizes: &[[u32; 2]]) -> Result<Self, String> {
        let (set, unavailable) = Self::build_available(recipe, sizes);
        match unavailable.into_iter().find(|u| !u.untaught) {
            Some(u) => Err(u.reason),
            None => Ok(set),
        }
    }

    /// 能建的都建，建不成的（含没示教的）连同原因列出，按段、计划相机顺序（历史件按段部分复测用）。
    pub fn build_available(
        recipe: &Recipe,
        sizes: &[[u32; 2]],
    ) -> (Self, Vec<UnavailableCorridor>) {
        let cameras = recipe.plan_cameras();
        let mut graphs = BTreeMap::new();
        let mut headings = BTreeMap::new();
        let mut unavailable = Vec::new();
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
                let skip = |reason: String, untaught: bool| UnavailableCorridor {
                    camera_idx: idx,
                    camera: camera.clone(),
                    segment: segment.index,
                    reason,
                    untaught,
                };
                if !inspect.segment_taught(segment.index) {
                    unavailable.push(skip(
                        format!("第 {} 段相机 {camera}：尚未示教检测走廊", segment.index),
                        true,
                    ));
                    continue;
                }
                let built = sizes
                    .get(idx)
                    .copied()
                    .ok_or_else(|| format!("相机 {camera} 没有示教帧尺寸"))
                    .and_then(|size| {
                        let corridor = plan_corridor(inspect, segment.index, recipe, size)?;
                        let heading = match segment.heading {
                            Some(_) => Some(plan_heading(inspect, segment.index, recipe, size)?),
                            None => None,
                        };
                        Ok((corridor, heading))
                    });
                match built {
                    Ok((corridor, heading)) => {
                        graphs.insert((idx, segment.index), corridor);
                        if let Some(heading) = heading {
                            headings.insert((idx, segment.index), heading);
                        }
                    }
                    Err(reason) => unavailable.push(skip(reason, false)),
                }
            }
        }
        (Self { graphs, headings }, unavailable)
    }

    /// 记下各计划相机示教帧的通道数（计划相机顺序），量帧时彩色 / 灰度与示教帧不一致即报错。
    pub fn with_channels(mut self, channels: &[u8]) -> Self {
        for g in self.graphs.values_mut() {
            g.channels = channels.get(g.camera_idx).copied();
        }
        for h in self.headings.values_mut() {
            h.channels = channels.get(h.camera_idx).copied();
        }
        self
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

/// 一帧与示教帧的尺寸、像素缓冲核对；示教帧通道数已知时，彩色 / 灰度不一致也报错（lyFlow 能把 RGB 转灰度量，
/// 但示教与生产的成像口径不同，结果不可比）。
pub(super) fn check_frame(
    camera: &str,
    size: [u32; 2],
    channels: Option<u8>,
    image: &FrameImage,
) -> Result<(), String> {
    if [image.width, image.height] != size {
        return Err(format!(
            "相机 {camera} 这一帧图像 {}×{} 与示教帧 {}×{} 尺寸不一致",
            image.width, image.height, size[0], size[1]
        ));
    }
    if !image.consistent() {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    if let Some(taught) = channels.filter(|&c| c != image.channels) {
        let label = |c: u8| if c == 3 { "彩色" } else { "灰度" };
        return Err(format!(
            "相机 {camera} 示教帧为{}，当前帧为{}：相机图像模式与示教时不一致，请恢复图像模式或重新示教",
            label(taught),
            label(image.channels)
        ));
    }
    Ok(())
}

/// 胶色测量图只量彩色帧：灰度帧没有颜色可比（示教帧通道未知时也在这里拦下）。
pub(super) fn check_color(
    camera: &str,
    template: &Template,
    image: &FrameImage,
) -> Result<(), String> {
    let color = std::ptr::eq(template, template::corridor_color())
        || std::ptr::eq(template, template::heading_color());
    if color && !image.is_color() {
        return Err(format!(
            "相机 {camera} 这一段按胶色测量，当前帧却是灰度：请把相机图像模式改回彩色，或把这一段改按灰度测量"
        ));
    }
    Ok(())
}

/// 量一帧：`run` 按模板与参数跑这张图。图像尺寸与示教帧不同、算法失败或输出不合契约时返回 Err，
/// 只影响这一帧（调用方记为 Error 帧，规格 §0）；胶条从走廊一侧走出时这一帧为 OutOfCorridor（默认做法 4），都不 panic。
pub fn measure_frame_with(
    corridor: &CorridorGraph, image: &FrameImage, seq: u64,
    run: impl FnOnce(GraphRun<'_>, &FrameImage) -> Result<RunResult, String>,
) -> Result<FrameMeasurement, String> {
    check_frame(&corridor.camera, corridor.size, corridor.channels, image)?;
    check_color(&corridor.camera, corridor.template, image)?;
    let result = run(
        GraphRun {
            template: corridor.template,
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
    let evidence = (c.params["widthPolicy"] == "extent").then(|| {
        let mut stations = judge_color_stations(
            &runs,
            c.taught.width_range[0],
            c.taught.width_range[1],
            c.taught.center_ratio,
            Some(c.taught.tolerance),
        );
        color_corridor_exits(&mut stations, c.taught.tolerance);
        stations
    });
    let judged = match &evidence {
        Some(stations) => super::judge::taught::TaughtJudgement {
            beads: stations.iter().map(|s| s.bead).collect(),
            unknown: stations
                .iter()
                .map(|s| {
                    matches!(
                        s.state,
                        StationEvidenceState::Clipped | StationEvidenceState::Unknown
                    )
                })
                .collect(),
            expect: Vec::new(),
            no_bead: None,
        },
        None => judge_taught(&runs, Some(&split), &c.taught)?,
    };
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
    let mut metrics = bead_metrics(
        c.camera_idx,
        c.segment,
        seq,
        c.scale,
        c.spacing_mm,
        &beads,
        &judged.unknown,
    );
    let mut part_edges = Vec::new();
    if let Some(stations) = &evidence {
        apply_color_evidence(&mut metrics, stations, &poses, c.scale);
    }
    if let Some(window) = &c.edge {
        let edge = edge_measure(
            result.record("edges"),
            &judged.beads,
            &poses,
            window,
            c.scale,
        )?;
        metrics.edge_mm = edge.mm;
        part_edges = edge.lines;
    }
    if let Some(why) = evidence
        .is_none()
        .then(|| corridor_exit(&beads, c.taught.tolerance, c.scale))
        .flatten()
    {
        metrics.state = FrameState::OutOfCorridor;
        metrics.error = Some(why);
    }
    Ok(FrameMeasurement {
        metrics,
        edges,
        part_edges,
        line: Vec::new(),
        ms: 0,
    })
}

/// `glue.LateralRuns` 记录里判定用到的字段；`what` 是出错时的主语。
pub(super) fn lateral_runs(record: &Value, what: &str) -> Result<LateralRuns, String> {
    LateralRuns::deserialize(record).map_err(|e| format!("算法{what}结果不合契约：{e}"))
}

fn color_corridor_exits(stations: &mut [ColorStation], tolerance: f64) {
    let exits = |station: &ColorStation| {
        station.state == StationEvidenceState::Exact
            && station
                .bead
                .is_some_and(|(lo, hi)| (0.5 * (lo + hi)).abs() >= EXIT_OFFSET_RATIO * tolerance)
    };
    let head = stations
        .iter()
        .take_while(|s| s.state == StationEvidenceState::Absent)
        .count();
    let tail = stations
        .iter()
        .rev()
        .take_while(|s| s.state == StationEvidenceState::Absent)
        .count();
    let head_exits = head > 0 && stations.get(head).is_some_and(exits);
    let tail_exits = tail > 0
        && stations
            .len()
            .checked_sub(tail + 1)
            .and_then(|i| stations.get(i))
            .is_some_and(exits);
    let end = stations.len().saturating_sub(tail);
    for (i, station) in stations.iter_mut().enumerate() {
        if (head_exits && i < head) || (tail_exits && i >= end) {
            station.state = StationEvidenceState::Unknown;
            station.reason = Some("out_of_corridor".into());
        }
    }
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
    unknown: &[bool],
) -> FrameMetrics {
    let widths: Vec<Option<f64>> = beads.iter().map(|b| b.map(|(w, _)| w * scale)).collect();
    let present: Vec<f64> = widths.iter().flatten().copied().collect();
    let offsets: Vec<f64> = beads.iter().flatten().map(|&(_, x)| x * scale).collect();
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    // 无信息的站（高光、暗区盖住了胶）既不算有胶也不算缺胶：不进有胶比例的分母，也不连成断口
    let hidden = |i: usize| unknown.get(i).copied().unwrap_or(false);
    let missing: Vec<bool> = beads
        .iter()
        .enumerate()
        .map(|(i, b)| b.is_none() && !hidden(i))
        .collect();
    let longest_gap = missing.split(|m| !m).map(<[_]>::len).max().unwrap_or(0);
    let informative = beads.len() - (0..beads.len()).filter(|&i| hidden(i)).count();
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
        present_ratio: present.len() as f64 / informative.max(1) as f64,
        x_mm: mean(&offsets),
        heading_deg: None,
        heading_fallback: false,
        fallback_reason: None,
        edge_mm: None,
        measurement_evidence: None,
    }
}

pub(super) fn apply_color_evidence(
    metrics: &mut FrameMetrics,
    stations: &[ColorStation],
    poses: &[([f64; 2], [f64; 2])],
    scale: f64,
) {
    let mut evidence = MeasurementEvidence {
        version: 1,
        ..Default::default()
    };
    for (s, &(center, normal)) in stations.iter().zip(poses) {
        match s.state {
            StationEvidenceState::Exact => evidence.exact_stations += 1,
            StationEvidenceState::Clipped => evidence.clipped_stations += 1,
            StationEvidenceState::Absent => evidence.absent_stations += 1,
            StationEvidenceState::Unknown => evidence.unknown_stations += 1,
        }
        let point = |t: f64| [center[0] + normal[0] * t, center[1] + normal[1] * t];
        evidence.stations.push(StationEvidence {
            state: s.state,
            width_lower_bound_mm: s.lower_bound.map(|w| w * scale),
            reason: s.reason.clone(),
            lo_edge: s.lo.map(point),
            hi_edge: s.hi.map(point),
            lo_clipped: s.lo_clipped,
            hi_clipped: s.hi_clipped,
            normal: (normal[0].hypot(normal[1]) > 0.0).then_some(normal),
            position: (normal[0].hypot(normal[1]) > 0.0).then_some(center),
            ..Default::default()
        });
    }
    evidence.max_width_lower_bound_mm = evidence
        .stations
        .iter()
        .filter_map(|s| s.width_lower_bound_mm)
        .reduce(f64::max);
    let present = evidence.exact_stations + evidence.clipped_stations;
    let informative = present + evidence.absent_stations;
    evidence.present_ratio = (informative > 0).then(|| f64::from(present) / f64::from(informative));
    metrics.present_ratio = evidence.present_ratio.unwrap_or(0.0);
    metrics.measurement_evidence = Some(evidence);
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

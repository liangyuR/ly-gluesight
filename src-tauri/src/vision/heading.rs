//! 逐帧定向测量（D-SB-47）：开启定向的（段, 检测相机）在出胶点周围的扇区里定向、拟合胶路，只量距出胶点 [s₀, s₀ + L] 的一段。
//! 测量图是 `template::heading`：一张响应图，扇形搜索与回退（扇区 = 本段示教方向 ± 方向容差）各一支，每支 3 个候选方向。
//! 两支一次跑完，再按 `judge::fit` 先判搜索支，没找到胶才用回退支；回退后仍没有胶就照常按缺胶判，不记检测不完整。
//! 不分阶段跑（先只跑搜索支的粗跟踪、按需再跑）：每次运行都要重新注入整帧、统计三张响应图，实测比一次跑完慢。

use serde_json::{json, Value};

use super::judge::edge::EdgeWindow;
use super::judge::fit::{choose_path, judge_stations, PathChoice, PathGate, StationGate};
use super::taught::{
    bead_metrics, bead_params, bead_params_into, edge_graph, edge_mm, edge_points, response_params_into,
    FrameMeasurement, GraphRun,
};
use super::template::{self, number};
use super::{FrameImage, RunResult};
use crate::heading::{nozzle, sector, segment_heading};
use crate::recipe::{CameraInspect, Recipe};

/// 扇形扫描在检测区前段最多看这么远（px，同 1.1）。
const FAN_LENGTH_PX: f64 = 150.0;

/// lyFlow 的二维向量参数按 f32 存：出胶点、检测区在算子里是这个精度，GlueSight 自己按它们推算时也取这个精度。
fn vec_param(v: [f64; 2]) -> [f64; 2] {
    v.map(|x| x as f32 as f64)
}

/// 一（相机, 段）的逐帧定向测量图。
#[derive(Clone, Debug)]
pub struct HeadingGraph {
    pub camera: String,
    pub camera_idx: usize,
    pub segment: u32,
    pub size: [u32; 2],
    /// `template::heading` 的全部有效参数与判定门限（发布包 graph.json 冻结它们）
    pub params: Value,
    pub judge: Value,
    path_gate: PathGate,
    station_gate: StationGate,
    /// 接了找零件边（D-SB-48）时的找边窗口
    edge: Option<EdgeWindow>,
    /// 两支的粗跟踪与卡尺，有找边时加找边节点
    targets: Vec<String>,
    /// 出胶点与检测区（px，算子里的精度）
    nozzle: [f64; 2],
    zone: [f64; 2],
    scale: f64,
    step: f64,
    spacing_mm: f64,
}

/// 模板里的两支：节点与图级输出名带的后缀。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Branch {
    Search,
    Fallback,
}

impl Branch {
    fn tag(self) -> &'static str {
        match self {
            Self::Search => "",
            Self::Fallback => "B",
        }
    }
}

/// 运行目标：两支各 3 个候选的粗跟踪与卡尺，`edge` 时加找边。
fn run_targets(edge: bool) -> Vec<String> {
    let kinds: &[&str] = if edge { &["t", "r", "e"] } else { &["t", "r"] };
    [Branch::Search, Branch::Fallback]
        .iter()
        .flat_map(|b| kinds.iter().flat_map(move |kind| (0..3).map(move |k| format!("n_{kind}{}{k}", b.tag()))))
        .collect()
}

/// 第 `segment` 段、一台检测相机的定向测量图：出胶点见 `heading::nozzle`（标定值或走廊起点推出），扇区见 `heading::sector`，
/// 检测窗口按相机当量换成像素；出胶点须在示教帧内。响应图只算出胶点 ± 检测区外沿 + max(胶宽上限 + 8, 卡尺半长 + 5) 的一块，
/// 盖住粗跟踪与卡尺够得着的地方。
pub fn plan_heading(
    inspect: &CameraInspect,
    segment: u32,
    recipe: &Recipe,
    size: [u32; 2],
) -> Result<HeadingGraph, String> {
    let camera_idx = recipe
        .inspect
        .iter()
        .position(|i| i.camera == inspect.camera)
        .ok_or_else(|| format!("相机 {} 不是配方 {} 的检测相机", inspect.camera, recipe.id))?;
    let fail = |why: &str| format!("第 {segment} 段相机 {}：{why}", inspect.camera);
    let segments = &recipe.path_segments;
    let cameras = recipe.plan_cameras();
    let k = segments
        .iter()
        .position(|s| s.index == segment)
        .ok_or_else(|| fail("段不存在"))?;
    let settings = segments[k]
        .heading
        .as_ref()
        .ok_or_else(|| fail("本段没有开启逐帧定向"))?;
    let scale = inspect.mm_per_px.ok_or_else(|| fail("缺少像素当量"))?;
    let window = inspect
        .window
        .as_ref()
        .ok_or_else(|| fail("缺少检测窗口"))?;
    let anchor = nozzle(segments, &cameras, inspect)?
        .ok_or_else(|| fail("尚未标定出胶点或示教走廊，推不出出胶点"))?
        .point;
    if !(anchor[0] >= 0.0
        && anchor[1] >= 0.0
        && anchor[0] <= f64::from(size[0]) - 1.0
        && anchor[1] <= f64::from(size[1]) - 1.0)
    {
        return Err(fail(&format!(
            "出胶点 ({:.0}, {:.0}) 不在示教帧 {}×{} 内",
            anchor[0], anchor[1], size[0], size[1]
        )));
    }
    let span = sector(segments, &cameras, inspect, k)
        .ok_or_else(|| fail("尚未示教走廊，推不出方向范围"))?;
    let own =
        segment_heading(inspect, segment).ok_or_else(|| fail("尚未示教走廊，推不出示教方向"))?;
    let tolerance = settings.tolerance_deg.max(1.0);
    let zone = [
        window.start_mm / scale,
        (window.start_mm + window.length_mm) / scale,
    ];
    let detect = inspect.detect.as_ref().unwrap_or(&recipe.detect);
    let params = bead_params(
        scale,
        f64::from(recipe.spacing) / scale,
        detect,
        zone[1] - zone[0],
    )
    .map_err(fail)?;
    let edge = edge_graph(recipe, segment, &inspect.camera, scale)?;
    let path_width_max = params.hi.ceil();
    let ([z0, z1], [ax, ay]) = (vec_param(zone), vec_param(anchor));
    let reach = z1 + (path_width_max + 8.0).max(params.half + 5.0);
    let side = (2.0 * reach).ceil() + 1.0;
    let roi = [(ax - reach).floor(), (ay - reach).floor(), side, side];
    let mut business = json!({
        "nozzle": anchor, "zone": zone, "radial": [z0, z0 + FAN_LENGTH_PX.min(z1 - z0)],
        "sector": span, "fallbackSector": [own - tolerance, own + tolerance], "pathWidthMax": path_width_max,
    });
    response_params_into(&mut business, detect, &params, roi);
    bead_params_into(&mut business, &params, params.half, edge.as_ref()).map_err(|e| fail(&e))?;
    let expert = recipe.expert_layers(inspect);
    let effective = template::heading().params(business, &expert).map_err(|e| fail(&e))?;
    let judge = template::heading().judge(&expert);
    let gate = |values: &Value, name: &str| number(values, name).map_err(|e| fail(&e));
    Ok(HeadingGraph {
        camera: inspect.camera.clone(),
        camera_idx,
        segment,
        size,
        path_gate: PathGate {
            min_coverage: gate(&judge, "minCoverage")?,
            sharp_min: gate(&effective, "pathSharpMin")?,
        },
        station_gate: StationGate {
            width_range: [params.lo, params.hi],
            present_ratio: gate(&judge, "presentRatio")?,
            center_ratio: gate(&judge, "centerRatio")?,
            contrast_ratio: gate(&judge, "contrastRatio")?,
        },
        targets: run_targets(edge.is_some()),
        edge: match edge {
            Some(e) => Some(e.window(gate(&effective, "edgeFarRun")?)),
            None => None,
        },
        params: effective,
        judge,
        nozzle: [ax, ay],
        zone: [z0, z1],
        scale,
        step: params.step,
        spacing_mm: f64::from(recipe.spacing),
    })
}

/// 量一帧：搜索支选出的方向找到了胶就量它；没找到用回退支，并记下搜索没找到胶的原因。
/// 图像尺寸与示教帧不同、算法失败或输出不合契约时返回 Err。
pub fn measure_heading_with(
    g: &HeadingGraph,
    image: &FrameImage,
    seq: u64,
    run: impl FnOnce(GraphRun<'_>, &FrameImage) -> Result<RunResult, String>,
) -> Result<FrameMeasurement, String> {
    if [image.width, image.height] != g.size {
        return Err(format!(
            "相机 {} 这一帧图像 {}×{} 与示教帧 {}×{} 尺寸不一致",
            g.camera, image.width, image.height, g.size[0], g.size[1]
        ));
    }
    if image.pixels.len() != image.width as usize * image.height as usize {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    let result = run(GraphRun { template: template::heading(), params: &g.params, targets: &g.targets }, image)?;
    if result.status() != "ok" {
        return Err(result.failure());
    }
    let searched = choose(g, &result, Branch::Search)?;
    if searched.ok {
        return measure_chosen(g, &result, Branch::Search, &searched, seq);
    }
    let fallback = choose(g, &result, Branch::Fallback)?;
    let mut measured = if fallback.ok {
        measure_chosen(g, &result, Branch::Fallback, &fallback, seq)?
    } else {
        no_bead(g, &fallback, seq)
    };
    measured.metrics.heading_fallback = true;
    measured.metrics.fallback_reason = Some(searched.reason).filter(|reason| !reason.is_empty());
    Ok(measured)
}

/// 一支的扇形扫描与 3 个粗跟踪 → 选方向。
fn choose(g: &HeadingGraph, result: &RunResult, branch: Branch) -> Result<PathChoice, String> {
    let tag = branch.tag();
    let scan = result.record(&format!("scan{tag}")).ok_or("算法缺少扇形扫描结果")?;
    let tracks = (0..3)
        .map(|k| result.record(&format!("track{tag}{k}")).cloned().ok_or("算法缺少粗跟踪结果"))
        .collect::<Result<Vec<_>, _>>()?;
    choose_path(scan, &tracks, g.path_gate)
}

/// 选中的候选找到了胶：按它的卡尺逐站判有无胶（有找边时再判零件边）。AbsoluteX 对拟合出的胶路没有意义，x 记为空。
fn measure_chosen(
    g: &HeadingGraph,
    result: &RunResult,
    branch: Branch,
    choice: &PathChoice,
    seq: u64,
) -> Result<FrameMeasurement, String> {
    let (tag, k) = (branch.tag(), choice.chosen.ok_or("选中的方向没有候选")?);
    let runs = result.record(&format!("runs{tag}{k}")).ok_or("算法缺少横向卡尺结果")?;
    let calls = judge_stations(runs, g.station_gate, true)?;
    if calls.is_empty() {
        return Err("定向测量没有测站".into());
    }
    let lohi: Vec<Option<(f64, f64)>> = calls.iter().map(|c| c.bead).collect();
    let beads: Vec<Option<(f64, f64)>> = lohi.iter().map(|b| b.map(|(lo, hi)| (hi - lo, 0.5 * (lo + hi)))).collect();
    let mut metrics = bead_metrics(g.camera_idx, g.segment, seq, g.scale, g.spacing_mm, &beads);
    metrics.x_mm = None;
    if let Some(window) = &g.edge {
        metrics.edge_mm = edge_mm(result.record(&format!("edges{tag}{k}")), &lohi, window, g.scale)?;
    }
    let line: Vec<[f32; 2]> = result
        .record(&format!("line{tag}{k}"))
        .and_then(|l| l["points"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32]))
        .collect();
    metrics.heading_deg = chord_heading(&line).or(choice.heading);
    Ok(FrameMeasurement {
        metrics,
        edges: calls.iter().map(|c| c.bead.map(|b| edge_points(c.center, c.normal, b))).collect(),
        line,
        ms: 0,
    })
}

/// 胶条方向取拟合胶路在检测区里首尾连线的方向（度），不取候选的起跑方向：从扇区边上起跑、接上同一条胶的候选，
/// 胶路一样，方向读数也应一样（界面显示与 D-SB-47 的分段边界细化都用它）。不足两点时为 None。
fn chord_heading(line: &[[f32; 2]]) -> Option<f64> {
    let (a, b) = (line.first()?, line.last()?);
    let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    (dx.hypot(dy) > 0.0).then(|| dy.atan2(dx).to_degrees())
}

/// 回退后仍没找到胶：检测区的站全部无胶（站数与站距同 1.1），胶路画成从出胶点沿选中方向的直线。
fn no_bead(g: &HeadingGraph, choice: &PathChoice, seq: u64) -> FrameMeasurement {
    let mut n = 0;
    let mut s = g.zone[0];
    while s <= g.zone[1] + 1e-6 {
        n += 1;
        s += g.step;
    }
    let mut metrics = bead_metrics(g.camera_idx, g.segment, seq, g.scale, g.spacing_mm, &vec![None; n]);
    metrics.x_mm = None;
    metrics.heading_deg = choice.heading;
    FrameMeasurement {
        metrics,
        edges: vec![None; n],
        line: choice.heading.map(|h| straight_line(g.nozzle, h, g.zone)).unwrap_or_default(),
        ms: 0,
    }
}

/// 从出胶点沿 `heading_deg` 的直线：检测区里每 2 px 一点、末端一点，坐标舍入到 3 位小数（与 1.1 没找到胶时的胶路相同）。
fn straight_line(anchor: [f64; 2], heading_deg: f64, zone: [f64; 2]) -> Vec<[f32; 2]> {
    let a = heading_deg * std::f64::consts::PI / 180.0;
    let d = [a.cos(), a.sin()];
    let mut s = Vec::new();
    let mut t = 0.0;
    while t < zone[1] - zone[0] - 1e-9 {
        s.push(zone[0] + t);
        t += 2.0;
    }
    s.push(zone[1]);
    let round3 = |v: f64| ((v * 1000.0).round() / 1000.0) as f32;
    s.into_iter().map(|v| [round3(anchor[0] + d[0] * v), round3(anchor[1] + d[1] * v)]).collect()
}

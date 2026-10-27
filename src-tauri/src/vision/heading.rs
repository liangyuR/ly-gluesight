//! 逐帧定向测量（D-SB-47）：开启定向的（段, 检测相机）用 `glue.bead_path` 在出胶点周围的扇区里定向、拟合胶路，
//! 只量距出胶点 [s₀, s₀ + L] 的一段。扇区与回退方向按段写进测量图：每（相机, 段）一张搜索图、一张回退图
//! （`headingSource = param`，方向取本段示教方向）。搜索图没找到胶（`info.ok` 为假）时用回退图再量一次，
//! 回退后仍没有胶就照常按缺胶判，不记检测不完整。

use serde_json::{json, Value};

use super::taught::{
    bead_graph, bead_metrics, bead_params, edge_graph, edge_mm, polarity, FrameMeasurement,
};
use super::{FrameImage, RunResult};
use crate::heading::{nozzle, sector, segment_heading};
use crate::recipe::{CameraInspect, Recipe};

/// 一（相机, 段）的逐帧定向测量图。
#[derive(Clone, Debug)]
pub struct HeadingGraph {
    pub camera: String,
    pub camera_idx: usize,
    pub segment: u32,
    pub size: [u32; 2],
    /// 扇形搜索图与回退图（发布包 graph.json 里两张都存）
    pub graph: Value,
    pub fallback: Value,
    graph_text: String,
    fallback_text: String,
    scale: f64,
    half: f64,
    spacing_mm: f64,
    edge: bool,
}

/// 第 `segment` 段、一台检测相机的定向测量图：出胶点见 `heading::nozzle`（标定值或走廊起点推出），扇区见 `heading::sector`，
/// 检测窗口按相机当量换成像素；出胶点须在示教帧内。
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
    let base = json!({"nozzle": anchor, "zone": zone, "widthMax": params.hi.ceil(), "polarity": polarity(detect)});
    let outputs = || json!({"pathInfo": {"node": "n_path", "port": "path.info"}, "pathLine": {"node": "n_path", "port": "path.line"}});
    let mut search = base.clone();
    search["sector"] = json!(span);
    let mut fallback = base;
    fallback["headingSource"] = json!("param");
    fallback["heading"] = json!(own);
    fallback["headingTol"] = json!(settings.tolerance_deg.max(1.0));
    let node = |p: Value| json!({"id": "n_path", "op": "glue.bead_path", "params": p});
    let graph = bead_graph(
        "01GLUESIGHTHEADING0000000",
        node(search),
        true,
        &params,
        edge.as_ref(),
        outputs(),
    );
    let fallback = bead_graph(
        "01GLUESIGHTHEADINGFALLBK0",
        node(fallback),
        true,
        &params,
        edge.as_ref(),
        outputs(),
    );
    Ok(HeadingGraph {
        camera: inspect.camera.clone(),
        camera_idx,
        segment,
        size,
        graph_text: graph.to_string(),
        fallback_text: fallback.to_string(),
        graph,
        fallback,
        scale,
        half: params.half,
        spacing_mm: f64::from(recipe.spacing),
        edge: edge.is_some(),
    })
}

/// 量一帧：先用搜索图，没找到胶（`info.ok` 为假）再用回退图。图像尺寸与示教帧不同、算法失败或输出不合契约时返回 Err。
pub fn measure_heading_with(
    g: &HeadingGraph,
    image: &FrameImage,
    seq: u64,
    run: impl Fn(&str, &FrameImage) -> Result<RunResult, String>,
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
    let (measured, found) = parse_heading(g, &run(&g.graph_text, image)?, seq, false)?;
    if found {
        return Ok(measured);
    }
    Ok(parse_heading(g, &run(&g.fallback_text, image)?, seq, true)?.0)
}

/// 定向测量图的输出 → 帧内指标；第二个值是 `bead_path` 是否找到了胶。测站与胶边取 `bead_width` 的输出，
/// AbsoluteX 对拟合出的胶路没有意义，x 记为空。
fn parse_heading(
    g: &HeadingGraph,
    result: &RunResult,
    seq: u64,
    fallback: bool,
) -> Result<(FrameMeasurement, bool), String> {
    if result.status() != "ok" {
        return Err(result.failure());
    }
    let info = result.record("pathInfo").ok_or("算法缺少定胶路结果")?;
    let found = info["ok"].as_bool().ok_or("定胶路结果没有 ok")?;
    let heading = info["heading"].as_f64().filter(|h| h.is_finite());
    let stations = result.record("stations").ok_or("算法缺少逐站结果")?;
    if stations["unit"] != "px" {
        return Err("算法逐站结果的单位不是像素".into());
    }
    let n = stations["present"].as_array().map_or(0, Vec::len);
    if n == 0 {
        return Err("定向测量没有测站".into());
    }
    for key in ["center", "normal", "lo", "hi", "widthPx"] {
        if stations[key].as_array().map(Vec::len) != Some(n) {
            return Err(format!("算法逐站结果 {key} 长度与测站数不一致"));
        }
    }
    let number = |key: &str, i: usize| {
        stations[key][i]
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("算法第 {} 站的 {key} 不是有限数", i + 1))
    };
    let xy = |key: &str, i: usize| -> Result<[f64; 2], String> {
        let a = stations[key][i]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| format!("算法第 {} 站的 {key} 不是二维坐标", i + 1))?;
        Ok([
            a[0].as_f64().ok_or("算法坐标不是有限数")?,
            a[1].as_f64().ok_or("算法坐标不是有限数")?,
        ])
    };
    let mut beads = Vec::with_capacity(n);
    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        if stations["present"][i]
            .as_bool()
            .ok_or("算法有胶状态不是布尔值")?
        {
            let (center, normal) = (xy("center", i)?, xy("normal", i)?);
            let (lo, hi, width) = (number("lo", i)?, number("hi", i)?, number("widthPx", i)?);
            if hi <= lo || width <= 0.0 || lo < -g.half - 0.001 || hi > g.half + 0.001 {
                return Err(format!("算法第 {} 站胶边或胶宽超出搜索范围", i + 1));
            }
            beads.push(Some((width, (lo + hi) * 0.5)));
            edges.push(Some([
                [
                    (center[0] + normal[0] * lo) as f32,
                    (center[1] + normal[1] * lo) as f32,
                ],
                [
                    (center[0] + normal[0] * hi) as f32,
                    (center[1] + normal[1] * hi) as f32,
                ],
            ]));
        } else {
            beads.push(None);
            edges.push(None);
        }
    }
    let mut metrics = bead_metrics(g.camera_idx, g.segment, seq, g.scale, g.spacing_mm, &beads);
    metrics.x_mm = None;
    metrics.heading_deg = heading;
    metrics.heading_fallback = fallback;
    if g.edge {
        metrics.edge_mm = edge_mm(result, g.scale)?;
    }
    let line = result
        .record("pathLine")
        .and_then(|l| l["points"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32]))
        .collect();
    Ok((
        FrameMeasurement {
            metrics,
            edges,
            line,
            ms: 0,
        },
        found,
    ))
}

#[cfg(test)]
#[path = "heading/tests.rs"]
mod tests;

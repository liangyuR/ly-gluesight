//! 示教 MCP 的工具（docs/integration/agent-teaching.md）：调用示教页命令的同一组函数，只改候选的走廊与颜色模型。

use super::burst;
use super::{Workspace, WorkspaceHost, WorkspaceView};
use crate::agent_mcp::canvas::{self, Canvas, CYAN, GREEN, MAGENTA, YELLOW};
use crate::agent_mcp::Content;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::cycle::CycleHost;
use crate::recipe::SegmentCorridor;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

pub enum ToolError {
    Unknown,
    Failed(String),
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_string())
    }
}

const DEFAULT_MAX_SIDE: u32 = 1280;
const DEFAULT_RADIUS: f32 = 30.0;
/// 提取范围在笔划外接框外再扩的像素
const EXTRACT_MARGIN: f64 = 40.0;
/// 与示教页、测量图同口径：走廊按站距不足 3 站时 lyFlow 会把有胶的帧测成无胶
const MIN_STATIONS: usize = 3;
const WORST: usize = 5;
/// 笔划直接作走廊时的点间距（px），同示教页缺省
const STROKE_SPACING: f32 = 20.0;

pub fn tools() -> Value {
    let recipe = json!({ "type": "string", "description": "候选的配方编号，如 NEW-1" });
    let camera = json!({ "type": "string", "description": "相机编号，如 cam1" });
    let segment =
        json!({ "type": "integer", "minimum": 1, "description": "段号（PathSegment.index）" });
    let seq = json!({ "type": "integer", "minimum": 1, "description": "示教件轮次里的帧组号" });
    json!([
        {
            "name": "teach_status",
            "description": "示教现状。不给 recipe 时列出当前项目的候选；给了时返回下一步、示教件、各段的帧组范围与检测相机、各相机各段的走廊/颜色模型/示教帧、发布前未通过的检查。",
            "inputSchema": { "type": "object", "properties": { "recipe": recipe }, "additionalProperties": false },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "teach_view",
            "description": "看示教件一帧：PNG 叠加原图坐标网格（标注为原图像素）。给 segment 时按这一段已保存的走廊测这一帧并叠加走廊（青）、每站胶两侧边连线（黄）、胶边距线（绿），附测量指标。用 crop 放大局部读坐标。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe, "camera": camera, "seq": seq,
                    "segment": segment,
                    "crop": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 4, "maxItems": 4, "description": "原图像素 [x, y, w, h]" },
                    "maxSide": { "type": "integer", "minimum": 64, "maximum": 2048, "description": "输出长边上限，缺省 1280" },
                    "grid": { "type": "integer", "minimum": 5, "description": "网格间距（原图 px），缺省按视野自动取" },
                },
                "required": ["recipe", "camera", "seq"],
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "teach_corridor",
            "description": "示教一台相机一段的走廊：在第 seq 帧上冻结示教帧，沿 stroke（胶条中心的原图像素点列，从起胶端画向收胶端）提取走廊（或 extract: false 直接用笔划）并保存；走廊变了即清空这一段的颜色模型。返回走廊长度、站数与叠图（品红=笔划，青=提取的走廊）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe, "camera": camera, "segment": segment, "seq": seq,
                    "stroke": { "type": "array", "minItems": 2, "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } },
                    "radiusPx": { "type": "number", "exclusiveMinimum": 0, "description": "笔划半径（px），缺省 30；胶条比它宽时调大" },
                    "extract": { "type": "boolean", "description": "缺省 true：沿笔划找胶两侧边提取中线；false：笔划按 20 px 插点直接作走廊（提取失败、笔划已贴着胶条中心时用）" },
                },
                "required": ["recipe", "camera", "segment", "seq", "stroke"],
                "additionalProperties": false,
            },
        },
        {
            "name": "teach_fit_color",
            "description": "沿已保存的走廊取胶色与两侧背景色，写入这一段的颜色模型（示教帧须是彩色）。返回胶色、背景色与分离度。",
            "inputSchema": {
                "type": "object",
                "properties": { "recipe": recipe, "camera": camera, "segment": segment },
                "required": ["recipe", "camera", "segment"],
                "additionalProperties": false,
            },
        },
        {
            "name": "teach_trial",
            "description": "用已保存的走廊试测示教件里这一段的全部帧（只读）。返回状态计数、胶宽/胶边距/偏移分布（mm）、有断口的帧、没找到胶条的帧、出错的帧与最差的几帧。from/to 限定统计范围，用来排除起胶、收胶的过渡帧。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe, "camera": camera, "segment": segment,
                    "from": { "type": "integer", "minimum": 1 },
                    "to": { "type": "integer", "minimum": 1 },
                },
                "required": ["recipe", "camera", "segment"],
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        },
    ])
}

pub async fn call(
    app: &AppHandle,
    name: &str,
    arguments: Value,
) -> Result<Vec<Content>, ToolError> {
    match name {
        "teach_status" => status(app, parse(arguments)?).await,
        "teach_view" => view_frame(app, parse(arguments)?).await,
        "teach_corridor" => corridor(app, parse(arguments)?).await,
        "teach_fit_color" => fit_color(app, parse(arguments)?).await,
        "teach_trial" => trial(app, parse(arguments)?).await,
        _ => Err(ToolError::Unknown),
    }
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolError> {
    serde_json::from_value(arguments).map_err(|e| ToolError::Failed(format!("参数不对：{e}")))
}

fn text(value: Value) -> Content {
    Content::Text(serde_json::to_string_pretty(&value).unwrap_or_default())
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| e.to_string())?
}

/// 候选的当前视图（还没打开过的候选按生产配方建立，同界面打开候选）
async fn open(app: &AppHandle, recipe: &str) -> Result<WorkspaceView, String> {
    let (app, id) = (app.clone(), recipe.to_string());
    blocking(move || super::get(app, id)).await
}

fn teaching_round(w: &Workspace) -> Result<String, String> {
    w.teaching_piece
        .as_ref()
        .map(|p| p.round_id.clone())
        .ok_or_else(|| "还没有选示教件：先在界面的示教件页选一件".to_string())
}

fn group_count(app: &AppHandle, round: &str) -> Result<u64, String> {
    Ok(app
        .state::<CycleHost>()
        .camera
        .recipe_capture()
        .get_burst(round)?
        .group_count)
}

/// 示教件里各段的帧组范围（含两端）；不能确定的段给原因
fn segment_frames(
    w: &Workspace,
    round: &str,
    groups: u64,
) -> Vec<(u32, Result<(u64, u64), String>)> {
    w.doc
        .segments
        .iter()
        .map(|s| (s.index, burst::segment_range(w, round, groups, s.index)))
        .collect()
}

// ───────────── teach_status ─────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    recipe: Option<String>,
}

async fn status(app: &AppHandle, args: StatusArgs) -> Result<Vec<Content>, ToolError> {
    let Some(id) = args.recipe else {
        let candidates: Vec<Value> = super::workspace_list(app.state())
            .iter()
            .map(|w| {
                json!({
                    "recipe": w.doc.id,
                    "name": w.doc.name,
                    "revision": w.revision,
                    "segmentRecipe": w.doc.is_burst(),
                    "teachingPiece": w.teaching_piece.is_some(),
                })
            })
            .collect();
        return Ok(vec![text(json!({
            "project": crate::project::current_id(app),
            "candidates": candidates,
        }))]);
    };
    let view = open(app, &id).await?;
    let w = &view.workspace;
    if !w.doc.is_burst() {
        return Err("旧拍照点配方已停用，请创建段配方".into());
    }
    let cameras = w.doc.plan_cameras();
    let round = w.teaching_piece.as_ref().map(|p| p.round_id.clone());
    let groups = round.as_deref().map(|r| group_count(app, r));
    let frames: Vec<(u32, Value)> = match (&round, &groups) {
        (Some(round), Some(Ok(groups))) => segment_frames(w, round, *groups)
            .into_iter()
            .map(|(index, range)| {
                (
                    index,
                    range.map_or_else(|e| json!(e), |r| json!([r.0, r.1])),
                )
            })
            .collect(),
        (_, Some(Err(e))) => w.doc.segments.iter().map(|s| (s.index, json!(e))).collect(),
        _ => w
            .doc
            .segments
            .iter()
            .map(|s| (s.index, json!("还没有示教件")))
            .collect(),
    };
    let segments: Vec<Value> = w
        .doc
        .segments
        .iter()
        .map(|s| {
            let value = serde_json::to_value(s).unwrap_or_default();
            json!({
                "index": s.index,
                "kind": value["kind"],
                "cameras": cameras.iter().filter(|c| crate::plan::detects(&w.doc.segments, s.index, c, &cameras)).collect::<Vec<_>>(),
                "frames": frames.iter().find(|(i, _)| *i == s.index).map(|(_, f)| f.clone()),
                "checks": value["checks"],
                "offsetRef": value["offsetRef"],
            })
        })
        .collect();
    let configs = app.state::<CycleHost>().camera.configs();
    let teaching: Vec<Value> = view
        .plan
        .camera_teaching
        .iter()
        .map(|t| {
            let inspect = w.doc.inspect.iter().find(|i| i.camera == t.camera);
            let segments: Vec<Value> = w
                .doc
                .segments
                .iter()
                .filter(|s| crate::plan::detects(&w.doc.segments, s.index, &t.camera, &cameras))
                .map(|s| {
                    let taught = t.segments.iter().find(|x| x.segment == s.index);
                    let corridor = inspect.and_then(|i| i.segment_corridor(s.index));
                    json!({
                        "segment": s.index,
                        "seq": taught.map(|x| x.seq),
                        "saved": taught.is_some_and(|x| x.saved),
                        "corridorPoints": corridor.map_or(0, |c| c.corridor.len()),
                        "colorModel": corridor.is_some_and(|c| c.color_model.is_some()),
                    })
                })
                .collect();
            json!({
                "camera": t.camera,
                "color": configs.iter().find(|c| c.id == t.camera).map(|c| c.image_mode.color()),
                "mmPerPx": t.mm_per_px,
                "calibMmPerPx": t.calib_mm_per_px,
                "ready": t.ready,
                "segments": segments,
            })
        })
        .collect();
    let blockers = {
        let (app, id, revision) = (app.clone(), id.clone(), w.revision);
        match blocking(move || super::burst_release::workspace_publish_checks(app, id, revision))
            .await
        {
            Ok(checks) => json!(checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| json!({ "name": c.name, "detail": c.detail }))
                .collect::<Vec<_>>()),
            Err(e) => json!(e),
        }
    };
    let progress = serde_json::to_value(&view.plan.progress).unwrap_or_default();
    Ok(vec![text(json!({
        "recipe": w.doc.id,
        "revision": w.revision,
        "next": progress["next"],
        "teachingPiece": w.teaching_piece.as_ref().map(|p| json!({
            "roundId": p.round_id,
            "recordId": p.record_id,
            "groupCount": groups.as_ref().and_then(|g| g.as_ref().ok()),
        })),
        "segments": segments,
        "cameras": teaching,
        "blockers": blockers,
    }))])
}

// ───────────── teach_view ─────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewArgs {
    recipe: String,
    camera: String,
    seq: u64,
    segment: Option<u32>,
    crop: Option<[u32; 4]>,
    max_side: Option<u32>,
    grid: Option<u32>,
}

fn metrics_value(metrics: &FrameMetrics) -> Value {
    let mut value = serde_json::to_value(metrics).unwrap_or_default();
    if let Some(map) = value.as_object_mut() {
        map.remove("widths");
        map.remove("camera");
    }
    value
}

async fn view_frame(app: &AppHandle, args: ViewArgs) -> Result<Vec<Content>, ToolError> {
    let w = open(app, &args.recipe).await?.workspace;
    let round = teaching_round(&w)?;
    let measured = match args.segment {
        Some(segment) => Some(
            burst::workspace_teach_preview_frame(
                app.clone(),
                args.recipe.clone(),
                args.camera.clone(),
                segment,
                round.clone(),
                args.seq,
            )
            .await,
        ),
        None => None,
    };
    let (image, located) = {
        let (app, round, camera, seq) = (app.clone(), round.clone(), args.camera.clone(), args.seq);
        blocking(move || {
            let cycle = app.state::<CycleHost>();
            let path = cycle
                .camera
                .recipe_capture()
                .burst_frame_path(&round, &camera, seq)?;
            let image = crate::image_file::load(&path)?;
            let groups = group_count(&app, &round)?;
            let located = segment_frames(&w, &round, groups)
                .into_iter()
                .find(|(_, r)| r.as_ref().is_ok_and(|(a, b)| (*a..=*b).contains(&seq)))
                .map(|(index, _)| index);
            Ok((image, located))
        })
        .await?
    };
    let mut canvas = Canvas::new(&image, args.crop, args.max_side.unwrap_or(DEFAULT_MAX_SIDE))?;
    let grid = args.grid.unwrap_or_else(|| canvas.auto_grid());
    canvas.grid(grid);
    let measurement = match &measured {
        Some(Ok(view)) => {
            let points = |p: &[[f32; 2]]| {
                p.iter()
                    .map(|q| [f64::from(q[0]), f64::from(q[1])])
                    .collect::<Vec<_>>()
            };
            canvas.polyline(&points(&view.corridor), CYAN, 2.0);
            for pair in view.edges.iter().flatten() {
                canvas.polyline(&points(pair), YELLOW, 1.0);
            }
            for line in &view.part_edges {
                canvas.polyline(&points(line), GREEN, 1.5);
            }
            json!({ "segment": view.segment, "metrics": metrics_value(&view.metrics) })
        }
        Some(Err(e)) => json!({ "error": e }),
        None => Value::Null,
    };
    Ok(vec![
        Content::Png(canvas.png()?),
        text(json!({
            "size": canvas.source_size(),
            "crop": canvas.crop(),
            "scale": canvas.scale(),
            "grid": grid,
            "segment": located,
            "measured": measurement,
            "legend": "网格标原图像素坐标；青=走廊，黄=每站胶两侧边连线，绿=胶中心到零件边",
        })),
    ])
}

// ───────────── teach_corridor ─────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorridorArgs {
    recipe: String,
    camera: String,
    segment: u32,
    seq: u64,
    stroke: Vec<[f32; 2]>,
    radius_px: Option<f32>,
    extract: Option<bool>,
}

/// 笔划直接作走廊：各段按不超过 `spacing` px 等分插点（同示教页“笔划直接生成走廊”的点间距）
fn densify(stroke: &[[f32; 2]], spacing: f32) -> Vec<[f32; 2]> {
    let mut out = vec![stroke[0]];
    for pair in stroke.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / spacing).ceil().max(1.0) as usize;
        for j in 1..=n {
            let t = j as f32 / n as f32;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

fn path_length(points: &[[f32; 2]]) -> f64 {
    points
        .windows(2)
        .map(|w| f64::from(w[1][0] - w[0][0]).hypot(f64::from(w[1][1] - w[0][1])))
        .sum()
}

/// 走廊按站距重采样的站数（含两端），与测量图 `station_count` 同口径
fn station_count(length_px: f64, mm_per_px: f64, spacing_mm: f64) -> usize {
    let step = spacing_mm / mm_per_px;
    ((length_px * 1000.0).round() / 1000.0 / step + 1e-6 / step).floor() as usize + 1
}

async fn corridor(app: &AppHandle, args: CorridorArgs) -> Result<Vec<Content>, ToolError> {
    if args.stroke.len() < 2 {
        return Err("笔划至少要 2 个点".into());
    }
    let radius = args.radius_px.unwrap_or(DEFAULT_RADIUS);
    let w = open(app, &args.recipe).await?.workspace;
    let round = teaching_round(&w)?;
    let frozen = burst::workspace_teach_segment_corridor(
        app.clone(),
        args.recipe.clone(),
        w.revision,
        args.camera.clone(),
        args.segment,
        round,
        args.seq,
    )
    .await?;
    let frame = frozen
        .plan
        .camera_teaching
        .iter()
        .find(|t| t.camera == args.camera)
        .and_then(|t| t.segments.iter().find(|s| s.segment == args.segment))
        .map(|s| s.frame.clone())
        .ok_or("示教帧没有冻结上")?;
    let stroke: Vec<[f64; 2]> = args
        .stroke
        .iter()
        .map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();
    let (corridor, message) = if args.extract.unwrap_or(true) {
        let roi = canvas::bounds(&stroke, f64::from(radius) + EXTRACT_MARGIN, frame.size)
            .ok_or("笔划不在示教帧内")?;
        let extracted = burst::workspace_extract_corridor(
            app.clone(),
            args.recipe.clone(),
            frozen.workspace.revision,
            args.camera.clone(),
            frame.id.clone(),
            roi,
            Some(super::centerline::BrushGuide {
                points: args.stroke.clone(),
                radius_px: radius,
            }),
            None,
        )
        .await?;
        if extracted.path.len() < 2 {
            let at = extracted
                .failure
                .as_ref()
                .map(|f| {
                    format!(
                        "（在 [{:.0}, {:.0}]：{}）",
                        f.point[0], f.point[1], f.reason
                    )
                })
                .unwrap_or_default();
            return Err(format!(
                "没有提取出走廊：{}{at}。笔划已贴着胶条中心时，可传 extract: false 直接用笔划作走廊",
                extracted.message
            )
            .into());
        }
        (extracted.path, extracted.message)
    } else {
        // 与测量、保存时同一口径：点要落在像素中心范围 [0, 宽 − 1] × [0, 高 − 1] 内
        if !crate::vision::corridor_inside(&args.stroke, frame.size) {
            return Err(format!(
                "笔划有点不在示教帧内（x 0–{}，y 0–{}）",
                frame.size[0].saturating_sub(1),
                frame.size[1].saturating_sub(1)
            )
            .into());
        }
        (
            densify(&args.stroke, STROKE_SPACING),
            "笔划直接作走廊".to_string(),
        )
    };
    let mut doc = frozen.workspace.doc.clone();
    let inspect = doc
        .inspect
        .iter_mut()
        .find(|i| i.camera == args.camera)
        .ok_or_else(|| format!("相机 {} 不是段配方的计划相机", args.camera))?;
    let length_px = path_length(&corridor);
    let scale = inspect.mm_per_px;
    let stations = scale.map(|s| station_count(length_px, s, f64::from(doc.spacing)));
    if stations.is_some_and(|n| n < MIN_STATIONS) {
        return Err(format!(
            "按站距 {} mm，这条走廊（{:.1} mm）只有 {} 站，至少要 {MIN_STATIONS} 站：把笔划画长，或在界面上改小站距",
            doc.spacing,
            length_px * scale.unwrap_or_default(),
            stations.unwrap_or_default()
        )
        .into());
    }
    let previous = inspect.segment_corridor(args.segment).cloned();
    inspect.segments.retain(|s| s.segment != args.segment);
    inspect.segments.push(SegmentCorridor {
        segment: args.segment,
        corridor: corridor.clone(),
        sector_deg: previous.as_ref().and_then(|p| p.sector_deg),
        color_model: None,
    });
    inspect.segments.sort_by_key(|s| s.segment);
    let saved = super::workspace_save_doc(
        app.clone(),
        args.recipe.clone(),
        frozen.workspace.revision,
        doc,
    )
    .await?;
    let image = {
        let (app, id, image_id) = (app.clone(), args.recipe.clone(), frame.id.clone());
        blocking(move || {
            let host = app.state::<WorkspaceHost>();
            crate::image_file::load(&super::image_file(&host, &id, &image_id)?)
        })
        .await?
    };
    let path: Vec<[f64; 2]> = corridor
        .iter()
        .map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();
    let all: Vec<[f64; 2]> = path.iter().chain(&stroke).copied().collect();
    let mut canvas = Canvas::new(
        &image,
        canvas::bounds(&all, 80.0, frame.size),
        DEFAULT_MAX_SIDE,
    )?;
    let grid = canvas.auto_grid();
    canvas.grid(grid);
    canvas.polyline(&stroke, MAGENTA, 1.0);
    canvas.polyline(&path, CYAN, 2.0);
    Ok(vec![
        Content::Png(canvas.png()?),
        text(json!({
            "revision": saved.workspace.revision,
            "frame": frame.id,
            "points": corridor.len(),
            "lengthPx": (length_px * 10.0).round() / 10.0,
            "lengthMm": scale.map(|s| (length_px * s * 100.0).round() / 100.0),
            "stations": stations,
            "message": message,
            "colorModelCleared": previous.is_some_and(|p| p.color_model.is_some()),
        })),
    ])
}

// ───────────── teach_fit_color ─────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentArgs {
    recipe: String,
    camera: String,
    segment: u32,
}

async fn fit_color(app: &AppHandle, args: SegmentArgs) -> Result<Vec<Content>, ToolError> {
    let w = open(app, &args.recipe).await?.workspace;
    let view = burst::workspace_fit_color_model(
        app.clone(),
        args.recipe.clone(),
        w.revision,
        args.camera.clone(),
        args.segment,
    )
    .await?;
    let model: Value = view
        .workspace
        .doc
        .inspect
        .iter()
        .find(|i| i.camera == args.camera)
        .and_then(|i| i.segment_corridor(args.segment))
        .and_then(|s| s.color_model.as_deref())
        .and_then(|m| serde_json::from_str(m).ok())
        .ok_or("取色后没有颜色模型")?;
    let background: Vec<Value> = model["background"]
        .as_array()
        .map(|b| {
            b.iter()
                .map(|c| json!({ "weight": c["weight"], "mean": c["mean"] }))
                .collect()
        })
        .unwrap_or_default();
    Ok(vec![text(json!({
        "revision": view.workspace.revision,
        "glue": { "mean": model["glue"]["mean"], "n": model["glue"]["n"], "luma": model["glue"]["luma"] },
        "background": background,
        "quality": model["quality"],
        "space": model["space"],
    }))])
}

// ───────────── teach_trial ─────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialArgs {
    recipe: String,
    camera: String,
    segment: u32,
    from: Option<u64>,
    to: Option<u64>,
}

/// 稳态范围内按这一段的检测项（`checks`）超限的帧（与生产判定 `judge_frame` 同口径）：帧内断口长于断胶长度、有胶站占比低于下限、平均胶宽出了公差带。
/// 整件判定还看跨帧的连续长度，这里只给逐帧的线索。
fn against_checks(frames: &[FrameMetrics], window: (u64, u64), checks: &Value) -> Value {
    let measured: Vec<&FrameMetrics> = frames
        .iter()
        .filter(|f| (window.0..=window.1).contains(&f.seq) && f.state == FrameState::Measured)
        .collect();
    let enabled = |c: &Value| c["enabled"].as_bool().unwrap_or(false);
    let mut out = serde_json::Map::new();
    let gap = &checks["break"];
    if enabled(gap) {
        let max_gap = gap["maxGapMm"].as_f64().unwrap_or(f64::INFINITY);
        let min_present = gap["minPresent"].as_f64().unwrap_or(0.0);
        out.insert("maxGapMm".into(), json!(max_gap));
        out.insert("minPresent".into(), json!(min_present));
        out.insert(
            "gapOverLimit".into(),
            json!(measured
                .iter()
                .filter(|f| f.gap_max_mm > max_gap)
                .map(|f| f.seq)
                .collect::<Vec<_>>()),
        );
        out.insert(
            "presentBelow".into(),
            json!(measured
                .iter()
                .filter(|f| f.present_ratio < min_present)
                .map(|f| f.seq)
                .collect::<Vec<_>>()),
        );
    }
    let width = &checks["width"];
    if let (true, Some(nominal)) = (enabled(width), width["nominalMm"].as_f64()) {
        let band = [
            nominal - width["tolLower"].as_f64().unwrap_or(0.0),
            nominal + width["tolUpper"].as_f64().unwrap_or(0.0),
        ];
        out.insert("widthBand".into(), json!(band));
        out.insert(
            "widthOutside".into(),
            json!(measured
                .iter()
                .filter(|f| f.width_mean.is_some_and(|m| m < band[0] || m > band[1]))
                .map(|f| f.seq)
                .collect::<Vec<_>>()),
        );
    }
    Value::Object(out)
}

async fn trial(app: &AppHandle, args: TrialArgs) -> Result<Vec<Content>, ToolError> {
    let w = open(app, &args.recipe).await?.workspace;
    let round = teaching_round(&w)?;
    let result = burst::workspace_teach_preview_segment(
        app.clone(),
        args.recipe,
        args.camera,
        args.segment,
        round,
    )
    .await?;
    let window = (
        args.from.unwrap_or(result.start).max(result.start),
        args.to.unwrap_or(result.end).min(result.end),
    );
    if window.0 > window.1 {
        return Err(format!(
            "统计范围与这一段的帧组 {}–{} 没有交集",
            result.start, result.end
        )
        .into());
    }
    let mut summary = summarize(&result.frames, window);
    summary["range"] = json!([result.start, result.end]);
    let checks = w
        .doc
        .segments
        .iter()
        .find(|s| s.index == args.segment)
        .and_then(|s| serde_json::to_value(s).ok())
        .map(|v| v["checks"].clone())
        .unwrap_or_default();
    summary["limits"] = against_checks(&result.frames, window, &checks);
    Ok(vec![text(summary)])
}

fn spread(mut values: Vec<f64>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    json!({
        "n": values.len(),
        "median": round(at(0.5)),
        "p5": round(at(0.05)),
        "p95": round(at(0.95)),
        "min": round(values[0]),
        "max": round(values[values.len() - 1]),
    })
}

/// 统计帧组范围 `window`（含两端）内的试测结果
fn summarize(frames: &[FrameMetrics], window: (u64, u64)) -> Value {
    let frames: Vec<&FrameMetrics> = frames
        .iter()
        .filter(|f| (window.0..=window.1).contains(&f.seq))
        .collect();
    let mut states = serde_json::Map::new();
    for f in &frames {
        let key = serde_json::to_value(&f.state).unwrap_or_default();
        let key = key.as_str().unwrap_or("unknown").to_string();
        let count = states.get(&key).and_then(Value::as_u64).unwrap_or(0);
        states.insert(key, json!(count + 1));
    }
    let measured: Vec<&&FrameMetrics> = frames
        .iter()
        .filter(|f| f.state == FrameState::Measured)
        .collect();
    let values = |pick: fn(&FrameMetrics) -> Option<f64>| -> Vec<f64> {
        measured.iter().filter_map(|f| pick(f)).collect()
    };
    let mut worst: Vec<&&FrameMetrics> = measured
        .iter()
        .copied()
        .filter(|f| f.gap_max_mm > 0.0 || f.present_ratio < 1.0)
        .collect();
    worst.sort_by(|a, b| {
        b.gap_max_mm
            .total_cmp(&a.gap_max_mm)
            .then(a.present_ratio.total_cmp(&b.present_ratio))
    });
    json!({
        "window": [window.0, window.1],
        "states": states,
        "width": spread(values(|f| f.width_mean)),
        "edgeMm": spread(values(|f| f.edge_mm)),
        "xMm": spread(values(|f| f.x_mm)),
        "gapFrames": measured.iter().filter(|f| f.gap_max_mm > 0.0)
            .map(|f| json!([f.seq, (f.gap_max_mm * 100.0).round() / 100.0])).collect::<Vec<_>>(),
        "fallbackFrames": frames.iter().filter(|f| f.heading_fallback)
            .map(|f| json!([f.seq, f.fallback_reason])).collect::<Vec<_>>(),
        "errors": frames.iter().filter(|f| f.state == FrameState::Error)
            .map(|f| json!([f.seq, f.error])).collect::<Vec<_>>(),
        "worst": worst.iter().take(WORST).map(|f| json!({
            "seq": f.seq,
            "gapMm": (f.gap_max_mm * 100.0).round() / 100.0,
            "present": (f.present_ratio * 1000.0).round() / 1000.0,
            "widthMean": f.width_mean.map(|v| (v * 1000.0).round() / 1000.0),
        })).collect::<Vec<_>>(),
    })
}

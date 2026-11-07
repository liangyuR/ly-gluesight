//! 示教 MCP 的工具（docs/integration/agent-teaching.md）：调用示教页命令的同一组函数，只改候选。

use super::burst;
use super::{Workspace, WorkspaceHost, WorkspaceView};
use crate::agent_mcp::canvas::{self, Canvas, CYAN, GREEN, MAGENTA, YELLOW};
use crate::agent_mcp::Content;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::cycle::CycleHost;
use crate::plan::PathSegment;
use crate::recipe::SegmentCorridor;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

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
/// 胶色吸附后的平滑半窗（胶中心线站距 4 px，5 点即 ±20 px）
const SNAP_SMOOTH: usize = 5;
/// 胶色吸附第二遍的偏移上限（px）
const SNAP_REFINE: f32 = 10.0;

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
            "description": "示教一台相机一段的走廊：在第 seq 帧上冻结示教帧，沿 stroke（沿胶条的原图像素点列，从起胶端画向收胶端；彩色帧粗略即可）得出走廊并保存；彩色帧随即沿新走廊重新取色。返回走廊长度、站数、取色结果与叠图（品红=笔划，青=走廊）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe, "camera": camera, "segment": segment, "seq": seq,
                    "stroke": { "type": "array", "minItems": 2, "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } },
                    "radiusPx": { "type": "number", "exclusiveMinimum": 0, "description": "笔划半径（px），缺省 30：按胶色吸附时是笔划到胶中心的最大偏差，边缘提取时是找边范围" },
                    "extract": { "type": "string", "enum": ["auto", "color", "edges", "none"], "description": "缺省 auto：彩色帧按胶色把笔划吸附到胶中心（笔划偏几十 px 也行，radiusPx 内找胶），灰度帧按胶两侧边提取；none：笔划按 20 px 插点直接作走廊" },
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
        {
            "name": "teach_segment",
            "description": "改一段的检测项（checks）与零件边参考（offsetRef），按补丁深合并：只给要改的字段。offsetRef 给 null 即关掉胶偏参考。检测项开关、公差与图纸标称值（offsetRef.edge.nominalMm）须用人给的值。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe, "segment": segment,
                    "checks": { "type": "object", "description": "如 {\"width\": {\"nominalMm\": 4, \"tolUpper\": 1.5}, \"break\": {\"maxGapMm\": 6}}" },
                    "offsetRef": { "type": ["object", "null"], "description": "如 {\"measure\": \"toEdge\", \"camera\": \"cam1\", \"edge\": {\"polarity\": \"brightToDark\", \"searchMm\": 18, \"contrastMin\": 70, \"nominalMm\": 10}}" },
                },
                "required": ["recipe", "segment"],
                "additionalProperties": false,
            },
        },
        {
            "name": "teach_replay",
            "description": "用候选现在的配置复测历史件并整件判定（从原图重测，只读，不改候选）。不给 records 时列出最近 30 件历史记录。返回每件的原判定、复测判定、原因与各区域结果。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe,
                    "records": { "type": "array", "items": { "type": "integer" }, "description": "历史记录编号" },
                },
                "required": ["recipe"],
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
        "teach_segment" => segment_config(app, arguments).await,
        "teach_replay" => replay(app, parse(arguments)?).await,
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
                "heading": value["heading"],
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
                "nozzle": t.nozzle.as_ref().map(|n| json!({ "point": n.point, "calibrated": n.calibrated })),
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
                .map(|c| json!({ "name": c.name, "detail": c.detail, "advisory": c.advisory }))
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

/// 走廊怎样从笔划得出
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Extract {
    /// 彩色示教帧按胶色吸附，灰度帧按边缘提取
    Auto,
    Color,
    Edges,
    None,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorridorArgs {
    recipe: String,
    camera: String,
    segment: u32,
    seq: u64,
    stroke: Vec<[f32; 2]>,
    radius_px: Option<f32>,
    extract: Option<Extract>,
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

/// 告诉界面候选被改了：没有未保存改动时重新载入
fn changed(app: &AppHandle, recipe: &str) {
    let _ = app.emit("workspace://changed", recipe);
}

/// 叠图 overlay 里第一条 `role` 折线的点
fn overlay_polyline(overlay: &Value, role: &str) -> Option<Vec<[f32; 2]>> {
    overlay["items"]
        .as_array()?
        .iter()
        .find(|item| item["kind"] == "polyline" && item["role"] == role)?["points"]
        .as_array()?
        .iter()
        .map(|p| Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32]))
        .collect()
}

/// 滑动平均（窗口 2·`half`+1 点，两端收窄），压掉逐站居中留下的台阶
fn smooth(points: &[[f32; 2]], half: usize) -> Vec<[f32; 2]> {
    (0..points.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(points.len()));
            let window = &points[a..b];
            let n = window.len() as f32;
            [
                window.iter().map(|p| p[0]).sum::<f32>() / n,
                window.iter().map(|p| p[1]).sum::<f32>() / n,
            ]
        })
        .collect()
}

/// 吸附只校正横向位置：两端沿切线补到笔划起点、终点在切线上的投影，走廊沿胶路的起止仍由笔划决定
/// （颜色模型沿走廊取样，走廊缩短会少采一段胶；出胶点未标定时还由定向段的走廊起点推出）
fn extend_ends(mut path: Vec<[f32; 2]>, ends: ([f32; 2], [f32; 2])) -> Vec<[f32; 2]> {
    let reach = |a: [f32; 2], b: [f32; 2], target: [f32; 2]| -> Option<[f32; 2]> {
        let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
        let length = dx.hypot(dy);
        if length < 1e-3 {
            return None;
        }
        let (ux, uy) = (dx / length, dy / length);
        let ahead = (target[0] - a[0]) * ux + (target[1] - a[1]) * uy;
        (ahead > 1.0).then(|| [a[0] + ux * ahead, a[1] + uy * ahead])
    };
    if path.len() >= 2 {
        if let Some(head) = reach(path[0], path[1], ends.0) {
            path.insert(0, head);
        }
        let n = path.len();
        if let Some(tail) = reach(path[n - 1], path[n - 2], ends.1) {
            path.push(tail);
        }
    }
    path
}

/// 每隔 `every` 个点留一个，末点总留
fn thin(points: &[[f32; 2]], every: usize) -> Vec<[f32; 2]> {
    let mut out: Vec<_> = points.iter().copied().step_by(every.max(1)).collect();
    if let (Some(&last), Some(&kept)) = (points.last(), out.last()) {
        if last != kept {
            out.push(last);
        }
    }
    out
}

/// 吸附流程：笔划插点作示教线，`recenter(线, 偏移上限)` 给出重新居中后的胶中心线（站距 4 px）与平均移动距离；
/// 没找到胶的站保留原偏移会留下台阶，所以平滑后以小偏移上限再居中一遍、再平滑，按 20 px 抽稀，两端补回笔划首尾。
/// 返回走廊与第一遍的平均移动距离（px）。
fn snap_path(
    stroke: &[[f32; 2]],
    shift: f32,
    mut recenter: impl FnMut(&[[f32; 2]], f32) -> Result<(Vec<[f32; 2]>, Option<f64>), String>,
) -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
    let (first, moved) = recenter(&densify(stroke, 8.0), shift)?;
    let (second, _) = recenter(&smooth(&first, SNAP_SMOOTH), shift.min(SNAP_REFINE))?;
    let path = thin(&smooth(&second, SNAP_SMOOTH), 5);
    Ok((
        extend_ends(path, (stroke[0], stroke[stroke.len() - 1])),
        moved,
    ))
}

/// 按胶色把粗笔划吸附到胶中心（[`snap_path`]）：重新居中用 `glue.color_model`，以线为示教线，每站在偏移上限内找胶色段。
async fn snap_color(
    app: &AppHandle,
    recipe: &str,
    camera: &str,
    image_id: &str,
    stroke: &[[f32; 2]],
    shift: f32,
) -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
    let (app, recipe, camera, image_id, stroke) = (
        app.clone(),
        recipe.to_string(),
        camera.to_string(),
        image_id.to_string(),
        stroke.to_vec(),
    );
    blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let (path, detect, scale) = {
            let items = host.items.lock().unwrap();
            let w = items.get(&recipe).ok_or("候选不存在")?;
            let inspect = w
                .doc
                .inspect
                .iter()
                .find(|i| i.camera == camera)
                .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
            (
                super::image_file(&host, &recipe, &image_id)?,
                inspect
                    .detect
                    .clone()
                    .unwrap_or_else(|| w.doc.detect.clone()),
                inspect.mm_per_px.ok_or("先填像素当量再按胶色吸附")?,
            )
        };
        let image = crate::image_file::load(&path)?;
        let engine = app
            .state::<crate::vision::VisionHost>()
            .engine()
            .ok_or("lyFlow 核心库未加载")?;
        let recenter =
            |line: &[[f32; 2]], shift: f32| -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
                let mut params = burst::color_teach_params(line, &detect, scale)?;
                params["shiftMax"] = json!(shift);
                let result = engine.run_targets(
                    crate::vision::template::color_teach().text,
                    &crate::vision::unique_run_id(&format!("agent-snap-{recipe}-{camera}")),
                    &image,
                    &params,
                    &["n_model".to_string()],
                )?;
                if result.status() != "ok" {
                    return Err(format!("按胶色吸附失败：{}", result.failure()));
                }
                let centers = result
                    .record("overlay")
                    .and_then(|o| overlay_polyline(o, "center"))
                    .filter(|c| c.len() >= 2)
                    .ok_or("按胶色没有找到胶中心：笔划可能没压在胶条上")?;
                let moved = result
                    .record("model")
                    .and_then(|m| m["quality"]["centerShift"].as_f64());
                Ok((centers, moved))
            };
        snap_path(&stroke, shift, recenter)
    })
    .await
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
    changed(app, &args.recipe);
    let frame = frozen
        .plan
        .camera_teaching
        .iter()
        .find(|t| t.camera == args.camera)
        .and_then(|t| t.segments.iter().find(|s| s.segment == args.segment))
        .map(|s| s.frame.clone())
        .ok_or("示教帧没有冻结上")?;
    let image = {
        let (app, id, image_id) = (app.clone(), args.recipe.clone(), frame.id.clone());
        blocking(move || {
            let host = app.state::<WorkspaceHost>();
            crate::image_file::load(&super::image_file(&host, &id, &image_id)?)
        })
        .await?
    };
    let color = image.is_color();
    let mode = match args.extract.unwrap_or(Extract::Auto) {
        Extract::Auto if color => Extract::Color,
        Extract::Auto => Extract::Edges,
        Extract::Color if !color => {
            return Err("示教帧是灰度图，不能按胶色吸附：用 extract: \"edges\" 或 \"none\"".into())
        }
        mode => mode,
    };
    // 与测量、保存时同一口径：点要落在像素中心范围 [0, 宽 − 1] × [0, 高 − 1] 内
    if !crate::vision::corridor_inside(&args.stroke, frame.size) {
        return Err(format!(
            "笔划有点不在示教帧内（x 0–{}，y 0–{}）",
            frame.size[0].saturating_sub(1),
            frame.size[1].saturating_sub(1)
        )
        .into());
    }
    let stroke: Vec<[f64; 2]> = args
        .stroke
        .iter()
        .map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();
    let (corridor, message) = match mode {
        Extract::Color => {
            let (path, shift) = snap_color(
                app,
                &args.recipe,
                &args.camera,
                &frame.id,
                &args.stroke,
                radius,
            )
            .await?;
            let note = shift.map_or_else(String::new, |s| format!("，平均移动 {s:.1} px"));
            (path, format!("按胶色吸附到胶中心{note}"))
        }
        Extract::Edges => {
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
                let hint = if color {
                    "彩色帧改用 extract: \"color\""
                } else {
                    "笔划已贴着胶条中心时可用 extract: \"none\""
                };
                return Err(format!("没有提取出走廊：{}{at}。{hint}", extracted.message).into());
            }
            (extracted.path, extracted.message)
        }
        _ => (
            densify(&args.stroke, STROKE_SPACING),
            "笔划直接作走廊".to_string(),
        ),
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
    let mut saved = super::workspace_save_doc(
        app.clone(),
        args.recipe.clone(),
        frozen.workspace.revision,
        doc,
    )
    .await?;
    // 彩色帧走廊一变就沿新走廊重新取色，免得这一段没有颜色模型
    let color_model = if color {
        match burst::workspace_fit_color_model(
            app.clone(),
            args.recipe.clone(),
            saved.workspace.revision,
            args.camera.clone(),
            args.segment,
        )
        .await
        {
            Ok(view) => {
                let summary = color_summary(&view, &args.camera, args.segment);
                saved = view;
                summary
            }
            Err(e) => json!({ "error": e }),
        }
    } else {
        Value::Null
    };
    changed(app, &args.recipe);
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
            "extract": format!("{mode:?}").to_lowercase(),
            "points": corridor.len(),
            "lengthPx": (length_px * 10.0).round() / 10.0,
            "lengthMm": scale.map(|s| (length_px * s * 100.0).round() / 100.0),
            "stations": stations,
            "message": message,
            "colorModel": color_model,
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

/// 这一段颜色模型的摘要：胶色、背景色、分离度等质量指标
fn color_summary(view: &WorkspaceView, camera: &str, segment: u32) -> Value {
    let Some(model) = view
        .workspace
        .doc
        .inspect
        .iter()
        .find(|i| i.camera == camera)
        .and_then(|i| i.segment_corridor(segment))
        .and_then(|s| s.color_model.as_deref())
        .and_then(|m| serde_json::from_str::<Value>(m).ok())
    else {
        return json!({ "error": "取色后没有颜色模型" });
    };
    let background: Vec<Value> = model["background"]
        .as_array()
        .map(|b| {
            b.iter()
                .map(|c| json!({ "weight": c["weight"], "mean": c["mean"] }))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "glue": { "mean": model["glue"]["mean"], "n": model["glue"]["n"], "luma": model["glue"]["luma"] },
        "background": background,
        "quality": model["quality"],
        "space": model["space"],
    })
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
    changed(app, &args.recipe);
    let mut summary = color_summary(&view, &args.camera, args.segment);
    summary["revision"] = json!(view.workspace.revision);
    Ok(vec![text(summary)])
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

// ───────────── teach_segment ─────────────

/// 把补丁 `patch` 深合并进 `target`：对象逐键合并，其余（含 null）直接替换
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

/// 段配置补丁（`teach_segment` 的参数）应用到一段：`checks` 深合并；`offsetRef` 为 null 关掉胶偏参考，否则深合并。
/// 结果按段配置的格式校验。
fn patch_segment(
    segment: &PathSegment,
    patch: &serde_json::Map<String, Value>,
) -> Result<PathSegment, String> {
    let mut value = serde_json::to_value(segment).map_err(|e| e.to_string())?;
    if let Some(checks) = patch.get("checks") {
        merge(&mut value["checks"], checks);
    }
    match patch.get("offsetRef") {
        Some(Value::Null) => value["offsetRef"] = Value::Null,
        Some(offset) => merge(&mut value["offsetRef"], offset),
        None => {}
    }
    serde_json::from_value(value).map_err(|e| format!("段配置不对：{e}"))
}

async fn segment_config(app: &AppHandle, arguments: Value) -> Result<Vec<Content>, ToolError> {
    let map = arguments.as_object().ok_or("参数要是对象")?;
    if let Some(key) = map
        .keys()
        .find(|k| !["recipe", "segment", "checks", "offsetRef"].contains(&k.as_str()))
    {
        return Err(format!("参数不对：不认识 {key}").into());
    }
    let recipe = map
        .get("recipe")
        .and_then(Value::as_str)
        .ok_or("缺少 recipe")?
        .to_string();
    let index = map
        .get("segment")
        .and_then(Value::as_u64)
        .ok_or("缺少 segment")? as u32;
    let w = open(app, &recipe).await?.workspace;
    let mut segments = w.doc.segments.clone();
    let target = segments
        .iter_mut()
        .find(|s| s.index == index)
        .ok_or_else(|| format!("没有第 {index} 段"))?;
    *target = patch_segment(target, map)?;
    let view = {
        let (app, recipe, revision) = (app.clone(), recipe.clone(), w.revision);
        blocking(move || burst::workspace_update_segments(app, recipe, revision, segments)).await?
    };
    changed(app, &recipe);
    let saved = view
        .workspace
        .doc
        .segments
        .iter()
        .find(|s| s.index == index)
        .and_then(|s| serde_json::to_value(s).ok())
        .unwrap_or_default();
    Ok(vec![text(json!({
        "revision": view.workspace.revision,
        "segment": index,
        "checks": saved["checks"],
        "offsetRef": saved["offsetRef"],
    }))])
}

// ───────────── teach_replay ─────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayArgs {
    recipe: String,
    records: Option<Vec<i64>>,
}

async fn replay(app: &AppHandle, args: ReplayArgs) -> Result<Vec<Content>, ToolError> {
    let Some(records) = args.records else {
        let page = crate::history::history_query(
            app.clone(),
            crate::store::HistoryQuery {
                limit: 30,
                project: Some(crate::project::current_id(app)),
                ..Default::default()
            },
        )
        .await?;
        let items: Vec<Value> = page
            .items
            .iter()
            .filter_map(|p| serde_json::to_value(p).ok())
            .map(|p| {
                json!({
                    "id": p["id"], "ts": p["ts"], "sn": p["sn"], "recipeId": p["recipeId"],
                    "verdict": p["verdict"], "reason": p["reason"], "mode": p["mode"],
                })
            })
            .collect();
        return Ok(vec![text(json!({ "total": page.total, "recent": items }))]);
    };
    // 复测逐帧占用测量引擎：与示教试测同一规则，有在途件或正在采集时拒绝
    crate::camera::check_idle(&app.state::<CycleHost>())?;
    let mut results = Vec::new();
    for record in records {
        let w = open(app, &args.recipe).await?.workspace;
        let outcome =
            super::workspace_compare(app.clone(), args.recipe.clone(), w.revision, record, true)
                .await;
        results.push(match outcome {
            Ok(c) => {
                let regions: Vec<Value> = c
                    .burst
                    .as_ref()
                    .and_then(|b| serde_json::to_value(&b.judgement).ok())
                    .and_then(|j| j["regions"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .map(|r| {
                        json!({
                            "kind": r["region"]["kind"],
                            "frames": [r["region"]["firstSeq"], r["region"]["lastSeq"]],
                            "verdict": r["verdict"],
                            "reason": r["reason"],
                            "width": r["width"],
                            "offset": r["offset"],
                        })
                    })
                    .collect();
                json!({
                    "record": record,
                    "originalVerdict": c.original_verdict,
                    "verdict": c.judgement.verdict,
                    "reason": c.judgement.reason,
                    "regions": regions,
                })
            }
            Err(e) => json!({ "record": record, "error": e }),
        });
    }
    Ok(vec![text(json!({ "results": results }))])
}

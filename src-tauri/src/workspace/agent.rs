//! 示教 MCP 的工具（docs/integration/agent-teaching.md）：调用示教页命令的同一组函数，只改候选。

use super::burst;
use super::{Workspace, WorkspaceHost, WorkspaceView};
use crate::agent_mcp::canvas::{self, Canvas, CYAN, GREEN, MAGENTA, YELLOW};
use crate::agent_mcp::Content;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::cycle::CycleHost;
use crate::plan::PathSegment;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

pub(crate) mod diagnostics;
pub(crate) mod snapshots;

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
    let expected_revision = json!({ "type": "integer", "minimum": 0, "description": "读到的候选 revision；不一致时拒绝写入，省略则基于入口读取的快照提交" });
    json!([
        {
            "name": "teach_status",
            "description": "示教现状。不给 recipe 时列当前项目候选和引擎；给了时返回候选身份、定向前提、颜色模型可用性与原因、比例分段覆盖、发布配置检查。配置齐全与测量验收分别报告。",
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
            "description": "示教一台相机一段的走廊：在第 seq 帧沿 stroke（原图像素，从起胶端到收胶端）提取，校验最终走廊并完成彩色取色后，一次提交示教帧、走廊与模型；准备失败不改候选。返回提交阶段、质量和叠图。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": recipe, "camera": camera, "segment": segment, "seq": seq,
                    "expectedRevision": expected_revision,
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
                "properties": { "recipe": recipe, "camera": camera, "segment": segment, "expectedRevision": expected_revision },
                "required": ["recipe", "camera", "segment"],
                "additionalProperties": false,
            },
        },
        {
            "name": "teach_trial",
            "description": "用已保存走廊试测示教件这一段（只读）；from/to 限制实际读取和量测的区间。返回真实快照身份、范围、各状态及全部未测成帧、ToEdge 有效率与缺失帧、分布、阈值和参数冲突提示。区间试测不改变生产覆盖。",
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
                    "expectedRevision": expected_revision,
                    "checks": { "type": "object", "description": "如 {\"width\": {\"nominalMm\": 4, \"tolUpper\": 1.5}, \"break\": {\"maxGapMm\": 6}}" },
                    "offsetRef": { "type": ["object", "null"], "description": "如 {\"measure\": \"toEdge\", \"camera\": \"cam1\", \"edge\": {\"polarity\": \"brightToDark\", \"searchMm\": 18, \"contrastMin\": 70, \"nominalMm\": 10}}" },
                },
                "required": ["recipe", "segment"],
                "additionalProperties": false,
            },
        },
        {
            "name": "teach_replay",
            "description": "用候选快照复测历史件并整件判定（只读，不改候选）。不给 records 时列出最近 30 件。返回每件实际修订、stale、flags、完整区域结论、逐相机逐段应测/实测与未测帧、胶偏未判原因；OK 不代表测量完整。",
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

fn expect_revision(w: &Workspace, expected: Option<u64>) -> Result<(), String> {
    w.expect(expected.unwrap_or(w.revision))
}

fn snapshot(app: &AppHandle, project: &str, w: &Workspace) -> Value {
    snapshot_identity(
        app,
        project,
        &w.doc.id,
        w.instance_id,
        w.revision,
        w.teaching_piece.as_ref().map(|p| p.round_id.as_str()),
    )
}

fn snapshot_identity(
    app: &AppHandle,
    project: &str,
    recipe: &str,
    instance: u64,
    revision: u64,
    round: Option<&str>,
) -> Value {
    let used = snapshots::UsedSnapshot {
        project: project.into(),
        recipe: recipe.into(),
        instance_id: instance,
        revision,
        round_id: round.map(str::to_owned),
    };
    serde_json::to_value(finish_snapshots(app, vec![used]).pop()).unwrap_or_default()
}

fn finish_snapshots(
    app: &AppHandle,
    used: Vec<snapshots::UsedSnapshot>,
) -> Vec<snapshots::SnapshotState> {
    let current_project = crate::project::current_id(app);
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    snapshots::finish_batch(used, &current_project, &items)
}

fn engine_status(app: &AppHandle) -> Value {
    let status = app.state::<crate::vision::VisionHost>().status();
    let mut value = serde_json::to_value(&status).unwrap_or_default();
    value["requiredSeries"] = json!(crate::vision::LYFLOW_SERIES);
    value["requiredOperators"] = json!(crate::vision::REQUIRED_OPERATORS
        .split(',')
        .collect::<Vec<_>>());
    value["source"] = json!(if status.overridden {
        "developmentOverride"
    } else {
        "bundledRuntime"
    });
    value
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
            "engine": engine_status(app),
        }))]);
    };
    let project = crate::project::current_id(app);
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
                "skip": s.skip,
                "skipHead": s.skip_head,
                "skipTail": s.skip_tail,
                "parameterWarnings": diagnostics::parameter_warnings(s),
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
                    let model = diagnostics::color_model_status(corridor.and_then(|c| c.color_model.as_deref()));
                    json!({
                        "segment": s.index,
                        "seq": taught.map(|x| x.seq),
                        "saved": taught.is_some_and(|x| x.saved),
                        "corridorPoints": corridor.map_or(0, |c| c.corridor.len()),
                        "colorModel": corridor.is_some_and(|c| c.color_model.is_some()),
                        "modelPresent": model.model_present,
                        "modelUsable": model.model_usable,
                        "quality": model.quality,
                        "modelIssues": model.issues,
                    })
                })
                .collect();
            json!({
                "camera": t.camera,
                "color": configs.iter().find(|c| c.id == t.camera).map(|c| c.image_mode.color()),
                "mmPerPx": t.mm_per_px,
                "calibMmPerPx": t.calib_mm_per_px,
                "ready": t.ready,
                "window": inspect.and_then(|i| i.window.as_ref()),
                "nozzle": t.nozzle.as_ref().map(|n| json!({ "point": n.point, "calibrated": n.calibrated, "source": if n.calibrated { "calibrated" } else { "corridorStarts" }, "warning": n.warning })),
                "headingIssues": crate::heading::issues(&w.doc.segments, &w.doc.inspect).iter()
                    .filter(|issue| issue.camera == t.camera)
                    .map(|issue| json!({ "segment": issue.segment, "message": issue.message })).collect::<Vec<_>>(),
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
    let coverage = groups.as_ref().and_then(|g| g.as_ref().ok()).map(|&total| {
        let excluded: Vec<u64> = round
            .as_ref()
            .and_then(|r| w.capture_review.rounds.get(r))
            .map(|r| r.excluded_groups.iter().map(|g| g.seq).collect())
            .unwrap_or_default();
        crate::segmentation::proportional_coverage(
            &w.doc.segments,
            &cameras,
            total,
            w.doc.boundary_guard,
            w.doc.boundary_refine,
            &excluded,
            super::pieces::piece_pass_rules(&w),
        )
    });
    Ok(vec![text(json!({
        "project": project,
        "recipe": w.doc.id,
        "revision": w.revision,
        "snapshot": snapshot(app, &project, w),
        "engine": engine_status(app),
        "next": progress["next"],
        "teachingPiece": w.teaching_piece.as_ref().map(|p| json!({
            "roundId": p.round_id,
            "recordId": p.record_id,
            "groupCount": groups.as_ref().and_then(|g| g.as_ref().ok()),
        })),
        "segments": segments,
        "cameras": teaching,
        "blockers": blockers,
        "passes": pass_status(app, &w),
        "coverage": coverage,
        "coverageSource": coverage.as_ref().map(|_| "teachingPieceProportional"),
    }))])
}

/// 示教件的笔结构、每笔首尾跳过设置与每笔起涂建议（D-SB-62）；没有示教件为 null。
fn pass_status(app: &AppHandle, w: &Workspace) -> Value {
    let Some(piece) = w.teaching_piece.as_ref() else {
        return Value::Null;
    };
    let group_count = w
        .capture_review
        .rounds
        .get(&piece.round_id)
        .map_or(0, |r| r.group_count);
    let measured = w
        .calibration
        .rounds
        .iter()
        .any(|r| r.round_id == piece.round_id);
    let dir = app
        .state::<WorkspaceHost>()
        .dir(&w.doc.id)
        .join(burst::CALIBRATION_DIR)
        .join(&piece.round_id);
    let metrics = if measured {
        burst::read_features(&dir, &w.doc.plan_cameras())
    } else {
        Err("示教件还没有测量".into())
    };
    let suggestion = super::burst_skip::pass_skip_suggestion(
        &w.doc,
        Some((
            piece.round_id.as_str(),
            group_count,
            piece.passes.as_slice(),
        )),
        &w.capture_review.rounds,
        metrics,
    );
    let skipped: Vec<Value> = crate::passes::skipped(&piece.passes, w.doc.pass_skip)
        .into_iter()
        .map(|(j, first, last, short)| json!({ "pass": j + 1, "frames": [first, last], "short": short }))
        .collect();
    json!({
        "count": piece.passes.len(),
        "frames": piece.passes,
        "skip": w.doc.pass_skip,
        "inRecipe": w.doc.pass_frames.is_some(),
        "skipped": skipped,
        "suggestion": suggestion,
    })
}

/// 按示教件笔结构与配方每笔首尾跳过，不判定的示教件帧组（D-SB-62）。
fn pass_skipped_seqs(w: &Workspace) -> std::collections::BTreeSet<u64> {
    w.teaching_piece
        .as_ref()
        .map(|piece| {
            crate::passes::skipped(&piece.passes, w.doc.pass_skip)
                .into_iter()
                .flat_map(|(_, first, last, _)| first..=last)
                .collect()
        })
        .unwrap_or_default()
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
    let project = crate::project::current_id(app);
    let w = open(app, &args.recipe).await?.workspace;
    let round = teaching_round(&w)?;
    let measured = match args.segment {
        Some(segment) => Some(
            burst::workspace_teach_preview_frame_at(
                app.clone(),
                args.recipe.clone(),
                w.revision,
                w.instance_id,
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
        let w = w.clone();
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
            "snapshot": snapshot(app, &project, &w),
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
    expected_revision: Option<u64>,
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
    w: &Workspace,
    camera: &str,
    image: std::sync::Arc<crate::frame::FrameImage>,
    stroke: &[[f32; 2]],
    shift: f32,
) -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
    let (app, w, camera, stroke) = (app.clone(), w.clone(), camera.to_string(), stroke.to_vec());
    blocking(move || {
        let recipe = &w.doc.id;
        let inspect = w
            .doc
            .inspect
            .iter()
            .find(|i| i.camera == camera)
            .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
        let detect = inspect.detect.as_ref().unwrap_or(&w.doc.detect);
        let scale = inspect.mm_per_px.ok_or("先填像素当量再按胶色吸附")?;
        let engine = app
            .state::<crate::vision::VisionHost>()
            .engine()
            .ok_or("lyFlow 核心库未加载")?;
        let recenter =
            |line: &[[f32; 2]], shift: f32| -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
                let mut params = burst::color_teach_params(line, detect, scale)?;
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
    if !radius.is_finite() || radius <= 0.0 {
        return Err("笔划半径必须是正有限数".into());
    }
    let project = crate::project::current_id(app);
    let w = open(app, &args.recipe).await?.workspace;
    expect_revision(&w, args.expected_revision)?;
    let round = teaching_round(&w)?;
    let prepared = burst::prepare_segment_teaching(
        app.clone(),
        args.recipe.clone(),
        w.revision,
        Some(w.instance_id),
        args.camera.clone(),
        args.segment,
        round,
        args.seq,
    )
    .await?;
    let frame = prepared.frame.clone();
    let image = prepared.image.clone();
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
                &prepared.workspace,
                &args.camera,
                image.clone(),
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
            let polarity = prepared
                .workspace
                .doc
                .inspect
                .iter()
                .find(|i| i.camera == args.camera)
                .and_then(|i| i.detect.as_ref())
                .unwrap_or(&prepared.workspace.doc.detect)
                .polarity;
            let (image, guide) = (
                image.clone(),
                super::centerline::BrushGuide {
                    points: args.stroke.clone(),
                    radius_px: radius,
                },
            );
            let extracted = blocking(move || {
                super::centerline::extract(&image, roi, polarity, Some(&guide), STROKE_SPACING)
                    .map_err(|error| match error {
                        super::centerline::ExtractionError::Invalid(message) => message,
                        super::centerline::ExtractionError::Failed(f) => format!(
                            "没有提取出走廊：{}（在 [{:.0}, {:.0}]：{}）",
                            f.message, f.point[0], f.point[1], f.reason
                        ),
                    })
            })
            .await?;
            (
                extracted.path,
                extracted
                    .notice
                    .unwrap_or_else(|| "走廊已提取，请核对叠图".into()),
            )
        }
        _ => (
            densify(&args.stroke, STROKE_SPACING),
            "笔划直接作走廊".to_string(),
        ),
    };
    let doc = &prepared.workspace.doc;
    let inspect = doc
        .inspect
        .iter()
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
    let png = canvas.png()?;
    let saved = burst::commit_segment_teaching(app.clone(), prepared, corridor.clone()).await?;
    let color_model = color.then(|| color_summary(&saved, &args.camera, args.segment));
    changed(app, &args.recipe);
    Ok(vec![
        Content::Png(png),
        text(json!({
            "revision": saved.workspace.revision,
            "baseRevision": w.revision,
            "snapshot": snapshot(app, &project, &saved.workspace),
            "commit": { "frameUpdated": true, "corridorSaved": true, "colorFitted": color },
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SegmentArgs {
    recipe: String,
    camera: String,
    segment: u32,
    expected_revision: Option<u64>,
}

/// 这一段颜色模型的摘要：胶色、背景色、分离度等质量指标
fn color_summary(view: &WorkspaceView, camera: &str, segment: u32) -> Value {
    let raw = view
        .workspace
        .doc
        .inspect
        .iter()
        .find(|i| i.camera == camera)
        .and_then(|i| i.segment_corridor(segment))
        .and_then(|s| s.color_model.as_deref());
    let status = diagnostics::color_model_status(raw);
    let model = raw
        .and_then(|m| serde_json::from_str::<Value>(m).ok())
        .unwrap_or_default();
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
        "modelPresent": status.model_present,
        "modelUsable": status.model_usable,
        "modelIssues": status.issues,
    })
}

async fn fit_color(app: &AppHandle, args: SegmentArgs) -> Result<Vec<Content>, ToolError> {
    let project = crate::project::current_id(app);
    let w = open(app, &args.recipe).await?.workspace;
    expect_revision(&w, args.expected_revision)?;
    let view = burst::workspace_fit_color_model_at(
        app.clone(),
        args.recipe.clone(),
        w.revision,
        w.instance_id,
        args.camera.clone(),
        args.segment,
    )
    .await?;
    changed(app, &args.recipe);
    let mut summary = color_summary(&view, &args.camera, args.segment);
    summary["revision"] = json!(view.workspace.revision);
    summary["baseRevision"] = json!(w.revision);
    summary["snapshot"] = snapshot(app, &project, &view.workspace);
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

/// 断口、存在率与最大宽度按检测阈值提示；widthOutside 只比较均值，生产胶宽还看逐站超差累计长度。
/// 每笔首尾跳过的帧（`skipped`，D-SB-62）生产不判，不进这些列表，另列在 `passSkipped`。
/// 这些线索不替代逐帧及整件的生产判定。
fn against_checks(
    frames: &[FrameMetrics],
    window: (u64, u64),
    checks: &Value,
    skipped: &std::collections::BTreeSet<u64>,
) -> Value {
    let measured: Vec<&FrameMetrics> = frames
        .iter()
        .filter(|f| {
            (window.0..=window.1).contains(&f.seq)
                && f.state == FrameState::Measured
                && !skipped.contains(&f.seq)
        })
        .collect();
    let enabled = |c: &Value| c["enabled"].as_bool().unwrap_or(false);
    let mut out = serde_json::Map::new();
    out.insert(
        "passSkipped".into(),
        json!(skipped.range(window.0..=window.1).collect::<Vec<_>>()),
    );
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
        out.insert(
            "widthBelow".into(),
            json!(measured
                .iter()
                .filter(|f| f.width_mean.is_some_and(|m| m < band[0]))
                .map(|f| f.seq)
                .collect::<Vec<_>>()),
        );
        out.insert(
            "wideWarnOnly".into(),
            json!(width["wideWarnOnly"].as_bool().unwrap_or(false)),
        );
    }
    let overflow = &checks["overflow"];
    if let (true, Some(limit)) = (enabled(overflow), overflow["maxWidthMm"].as_f64()) {
        out.insert("overflowMaxWidthMm".into(), json!(limit));
        out.insert(
            "overflowFrames".into(),
            json!(measured
                .iter()
                .filter(|f| f.width_max.is_some_and(|width| width > limit))
                .map(|f| f.seq)
                .collect::<Vec<_>>()),
        );
    }
    Value::Object(out)
}

async fn trial(app: &AppHandle, args: TrialArgs) -> Result<Vec<Content>, ToolError> {
    let project = crate::project::current_id(app);
    let w = open(app, &args.recipe).await?.workspace;
    let round = teaching_round(&w)?;
    let result = burst::workspace_teach_preview_range(
        app.clone(),
        args.recipe.clone(),
        args.camera.clone(),
        args.segment,
        round,
        args.from,
        args.to,
        Some(w.revision),
    )
    .await?;
    let window = (result.from, result.to);
    let mut summary = summarize(&result.frames, window);
    summary["range"] = json!([result.start, result.end]);
    let segment = result
        .doc
        .segments
        .iter()
        .find(|s| s.index == args.segment)
        .ok_or("试测快照没有这一段")?;
    let checks = serde_json::to_value(&segment.checks).unwrap_or_default();
    summary["limits"] = against_checks(&result.frames, window, &checks, &pass_skipped_seqs(&w));
    let scope: Vec<u64> = (window.0..=window.1).collect();
    let expected: Vec<u64> = scope
        .iter()
        .copied()
        .filter(|seq| {
            !result
                .frames
                .iter()
                .any(|f| f.seq == *seq && f.state == FrameState::NotInspected)
        })
        .collect();
    summary["measurement"] = serde_json::to_value(diagnostics::measurement_evidence(
        &result.frames,
        &scope,
        &expected,
        diagnostics::edge_requested(segment, &args.camera),
    ))
    .unwrap_or_default();
    summary["parameterWarnings"] = json!(diagnostics::parameter_warnings(segment));
    summary["camera"] = json!(args.camera);
    summary["segment"] = json!(args.segment);
    summary["checks"] = checks;
    summary["offsetRef"] = json!(segment.offset_ref);
    summary["requestedWindow"] = json!({ "from": args.from, "to": args.to });
    summary["measurementPlan"] = json!("selectedSegment");
    summary["snapshot"] = snapshot_identity(
        app,
        &project,
        &result.doc.id,
        result.instance_id,
        result.revision,
        Some(&result.round),
    );
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
    if let Some(key) = map.keys().find(|k| {
        ![
            "recipe",
            "segment",
            "checks",
            "offsetRef",
            "expectedRevision",
        ]
        .contains(&k.as_str())
    }) {
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
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("segment 必须是有效段号")?;
    let expected_revision = map
        .get("expectedRevision")
        .map(|value| value.as_u64().ok_or("expectedRevision 必须是非负整数"))
        .transpose()?;
    let project = crate::project::current_id(app);
    let w = open(app, &recipe).await?.workspace;
    expect_revision(&w, expected_revision)?;
    let mut segments = w.doc.segments.clone();
    let target = segments
        .iter_mut()
        .find(|s| s.index == index)
        .ok_or_else(|| format!("没有第 {index} 段"))?;
    *target = patch_segment(target, map)?;
    let view = {
        let (app, recipe, revision, instance) =
            (app.clone(), recipe.clone(), w.revision, w.instance_id);
        blocking(move || {
            burst::workspace_update_segments_at(app, recipe, revision, instance, segments)
        })
        .await?
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
        "baseRevision": w.revision,
        "snapshot": snapshot(app, &project, &view.workspace),
        "impact": super::capture_review::save_impact(&w, &view.workspace),
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
        let project = crate::project::current_id(app);
        let page = crate::history::history_query(
            app.clone(),
            crate::store::HistoryQuery {
                limit: 30,
                project: Some(project.clone()),
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
        return Ok(vec![text(
            json!({ "project": project, "recipe": args.recipe, "total": page.total, "recent": items }),
        )]);
    };
    // 复测逐帧占用测量引擎：与示教试测同一规则，有在途件或正在采集时拒绝
    crate::camera::check_idle(&app.state::<CycleHost>())?;
    let mut results = Vec::new();
    let mut used = Vec::new();
    for record in records {
        let project = crate::project::current_id(app);
        let w = open(app, &args.recipe).await?.workspace;
        let outcome = super::workspace_compare_at(
            app.clone(),
            args.recipe.clone(),
            w.revision,
            w.instance_id,
            record,
            true,
        )
        .await;
        used.push(snapshots::UsedSnapshot::from_workspace(project, &w));
        results.push(match outcome {
            Ok(c) => {
                let regions: Vec<Value> = c
                    .burst
                    .as_ref()
                    .map(|b| b.judgement.regions.iter().map(|result| {
                        let r = serde_json::to_value(result).unwrap_or_default();
                        json!({
                            "kind": r["region"]["kind"],
                            "frames": [r["region"]["firstSeq"], r["region"]["lastSeq"]],
                            "verdict": r["verdict"],
                            "reason": r["reason"],
                            "width": r["width"],
                            "offset": r["offset"],
                            "offsetUnjudged": r["offsetUnjudged"],
                            "incompleteGroups": r["incompleteGroups"],
                            "frameCount": r["frameCount"],
                            "expected": r["expected"],
                            "tolerance": r["tolerance"],
                            "countStatus": r["countStatus"],
                            "cameraFrames": r["cameraFrames"],
                            "offsetEvidence": diagnostics::offset_evidence(b, result),
                        })
                    }).collect())
                    .unwrap_or_default();
                json!({
                    "record": record,
                    "comparisonId": c.id,
                    "cycleId": c.cycle_id,
                    "originalVerdict": c.original_verdict,
                    "verdict": c.judgement.verdict,
                    "reason": c.judgement.reason,
                    "regions": regions,
                    "flags": c.burst.as_ref().map(|b| &b.judgement.flags),
                    "alarms": c.burst.as_ref().map(|b| &b.judgement.alarms),
                    "uninspected": c.burst.as_ref().map(|b| &b.judgement.uninspected),
                    "unchecked": c.burst.as_ref().map(|b| &b.judgement.unchecked),
                    "unmeasuredSegments": c.burst.as_ref().map(|b| &b.unmeasured),
                    "measurement": c.burst.as_ref().map(|b| diagnostics::replay_measurements(b, c.candidate_recipe.boundary_guard, c.candidate_recipe.boundary_refine, c.candidate_recipe.pass_rules(&b.judgement.passes))),
                    "parameterWarnings": c.candidate_recipe.path_segments.iter().map(|s| json!({
                        "segment": s.index, "warnings": diagnostics::parameter_warnings(s),
                    })).collect::<Vec<_>>(),
                })
            }
            Err(e) => json!({ "record": record, "error": e }),
        });
    }
    for (result, identity) in results.iter_mut().zip(finish_snapshots(app, used)) {
        let field = if result.get("error").is_some() {
            "attemptedSnapshot"
        } else {
            "snapshot"
        };
        result[field] = serde_json::to_value(identity).unwrap_or_default();
    }
    Ok(vec![text(json!({ "results": results }))])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(seq: u64, gap_max_mm: f64) -> FrameMetrics {
        FrameMetrics {
            camera: 0,
            seq,
            state: FrameState::Measured,
            error: None,
            width_min: Some(4.0),
            width_max: Some(4.0),
            width_mean: Some(4.0),
            widths: Vec::new(),
            spacing_mm: 1.0,
            gap_max_mm,
            present_ratio: 1.0,
            x_mm: None,
            corridor: Some(1),
            heading_deg: None,
            heading_fallback: false,
            fallback_reason: None,
            edge_mm: None,
        }
    }

    /// 防的回归（D-SB-62）：每笔开头跳过的帧生产不判，试测线索不能把它们列进断胶，否则示教代理按“断胶列表须为空”误判不合格。
    #[test]
    fn trial_limits_leave_pass_skipped_frames_out_of_gap_findings() {
        let frames = [measured(1, 20.0), measured(2, 20.0), measured(3, 0.0)];
        let checks = json!({ "break": { "enabled": true, "maxGapMm": 6.0, "minPresent": 0.8 } });
        let skipped = std::collections::BTreeSet::from([1]);
        let limits = against_checks(&frames, (1, 3), &checks, &skipped);
        assert_eq!(
            (
                limits["gapOverLimit"].clone(),
                limits["passSkipped"].clone()
            ),
            (json!([2]), json!([1]))
        );
    }
}

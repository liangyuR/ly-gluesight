//! 流程调试页（LF-6）：在 lyFlow 编辑器里逐节点查看一（相机, 段）的测量图，拓扑锁定、只调参数。
//! 编辑器的 Transport 对到这里的 `flow_*` 命令；运行、编计划与清缓存都注入同一份调试帧（`n_load.image`），缓存键才对得上。
//! 同一时刻只有一个运行：新运行先取消并等旧的退出。留着在算的与上一次的，编辑器按 runId 取节点输出。
//! 每次打开按发起顺序编号：晚到的旧打开不顶替新的，校验、编计划、运行与清缓存带着打开编号，编号对不上就拒绝。

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lyflow_client::{Core, RunHandle, RunImageInput, RunSpec};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::cycle::CycleHost;
use crate::vision::{self, template, VisionHost};

/// 执行事件推给前端的事件名（Transport 的 `onExecutionEvent`）。
pub const EVENT_NAME: &str = "flow-execution-event";

/// 图像一次最多给 16 MB，多出的行由编辑器接着要。
const IMAGE_BYTES_LIMIT: usize = 16 * 1024 * 1024;
/// LYIM 载荷（lyFlow docs/http-transport.md「图像」）：12 个 u32 的帧头加紧排像素，补齐到 4 字节。
const IMAGE_MAGIC: u32 = 0x4D49_594C;
const IMAGE_HEADER_BYTES: usize = 48;

/// 打开哪一张图：示教走廊，或逐帧定向。
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Corridor,
    Heading,
}

/// 打开的调试图：打开编号、模板、这一帧原图与打开时的有效参数。
struct Session {
    id: u64,
    template: &'static template::Template,
    image: RunImageInput,
    params: Value,
}

#[derive(Default)]
struct Runs {
    active: Option<RunHandle>,
    finished: Option<RunHandle>,
}

#[derive(Default)]
pub struct FlowDebugHost {
    session: Mutex<Option<Arc<Session>>>,
    /// 最近一次发起的打开编号
    opened: AtomicU64,
    runs: Mutex<Runs>,
    /// 起运行串行：取消并等旧的退出、再起新的，这一段不能交错
    starting: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FlowDebugHost {
    /// 发起一次打开，取它的编号；之后的打开编号更大。
    fn begin_open(&self) -> u64 {
        self.opened.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 编号为 `id` 的打开所建的调试图；已被后来的打开取代或还没打开时拒绝。
    fn session(&self, id: u64) -> Result<Arc<Session>, String> {
        let session = lock(&self.session)
            .clone()
            .ok_or("流程调试页还没有打开测量图")?;
        if session.id != id {
            return Err("流程调试页已切换到别的测量图，这次请求作废".into());
        }
        Ok(session)
    }

    /// 打开完成：只有最近一次发起的打开能换图，晚到的旧打开拒绝。换图时停掉并放掉之前的运行，它们的结果属于别的帧。
    fn reset(&self, session: Session) -> Result<(), String> {
        let _serial = lock(&self.starting);
        if session.id != self.opened.load(Ordering::SeqCst) {
            return Err("已切换到别的测量图，这次打开作废".into());
        }
        let old = std::mem::take(&mut *lock(&self.runs));
        if let Some(run) = &old.active {
            run.cancel();
        }
        drop(old);
        *lock(&self.session) = Some(Arc::new(session));
        Ok(())
    }

    fn start(&self, launch: impl FnOnce() -> Result<RunHandle, String>) -> Result<String, String> {
        let _serial = lock(&self.starting);
        let previous = lock(&self.runs).active.take();
        if let Some(run) = &previous {
            run.cancel();
            run.join();
        }
        let started = launch();
        let mut runs = lock(&self.runs);
        let released = match previous {
            Some(run) => runs.finished.replace(run),
            None => None,
        };
        let run_id = started.map(|run| {
            let id = run.run_id().to_string();
            runs.active = Some(run);
            id
        });
        drop(runs);
        drop(released);
        run_id
    }

    fn cancel(&self, run_id: &str) {
        if let Some(run) = lock(&self.runs)
            .active
            .as_ref()
            .filter(|r| r.run_id() == run_id)
        {
            run.cancel();
        }
    }
}

struct EmitCtx {
    app: AppHandle,
    run_id: String,
}

/// core 工作线程上的回调：事件 JSON 原样转发。panic 不能展开穿过 C++ 栈帧，丢一条事件只影响界面。
unsafe extern "C" fn forward(event_json: *const c_char, user: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        if event_json.is_null() || user.is_null() {
            return;
        }
        let ctx = &*(user as *const EmitCtx);
        let Ok(text) = CStr::from_ptr(event_json).to_str() else {
            return;
        };
        match serde_json::from_str::<Value>(text) {
            Ok(value) => {
                if let Err(e) = ctx.app.emit(EVENT_NAME, value) {
                    eprintln!("推送流程调试事件失败（{}）：{e}", ctx.run_id);
                }
            }
            Err(e) => eprintln!("流程调试事件不是合法 JSON：{e}"),
        }
    });
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn core(app: &AppHandle) -> Result<Arc<Core>, String> {
    Ok(app
        .state::<VisionHost>()
        .engine()
        .ok_or("lyFlow 核心库未加载")?
        .core())
}

fn parse(raw: Result<String, lyflow_client::CoreError>, what: &str) -> Result<Value, String> {
    serde_json::from_str(&raw.map_err(|e| e.to_string())?)
        .map_err(|e| format!("lyFlow 返回的{what}不是合法 JSON：{e}"))
}

/// 模板换上这一次的有效参数作为图参数默认值，编辑器打开就是生产会跑的那一组。
fn with_defaults(doc: &Value, params: &Value, name: String) -> Value {
    let mut doc = doc.clone();
    if let (Some(specs), Some(values)) = (doc["params"].as_object_mut(), params.as_object()) {
        for (key, value) in values {
            if let Some(spec) = specs.get_mut(key).and_then(Value::as_object_mut) {
                spec.insert("default".into(), value.clone());
            }
        }
    }
    doc["name"] = json!(name);
    doc
}

/// 节点（id 与算子）与连线必须与模板一致：流程调试只调参数，不改拓扑。节点位置、参数取值不比较。
fn same_topology(template: &Value, doc: &Value) -> Result<(), String> {
    let nodes = |d: &Value| {
        let mut v: Vec<(String, String)> = d["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|n| {
                (
                    n["id"].as_str().unwrap_or_default().to_string(),
                    n["op"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let edges = |d: &Value| {
        let mut v: Vec<String> = d["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                format!(
                    "{}.{}>{}.{}",
                    e["from"]["node"], e["from"]["port"], e["to"]["node"], e["to"]["port"]
                )
            })
            .collect();
        v.sort();
        v
    };
    if nodes(template) != nodes(doc)
        || edges(template) != edges(doc)
        || doc
            .get("subgraphs")
            .is_some_and(|s| s.as_object().is_some_and(|s| !s.is_empty()))
    {
        return Err("流程调试只能调参数：图的节点或连线与测量图模板不一致".into());
    }
    Ok(())
}

/// 运行要的图文本与参数：拓扑核对过的图，参数缺省时用打开时的有效参数。
fn prepared(
    session: &Session,
    doc: &Value,
    params: Option<Map<String, Value>>,
) -> Result<(String, String), String> {
    same_topology(&session.template.doc, doc)?;
    let params = params
        .map(Value::Object)
        .unwrap_or_else(|| session.params.clone());
    Ok((doc.to_string(), params.to_string()))
}

#[tauri::command]
pub async fn flow_open(
    app: AppHandle,
    id: String,
    camera: String,
    segment: u32,
    round: String,
    seq: u64,
    variant: Variant,
) -> Result<Value, String> {
    let ticket = app.state::<FlowDebugHost>().begin_open();
    blocking(move || {
        let (graphs, index, image) =
            crate::workspace::flow_debug_frame(&app, &id, &camera, segment, &round, seq)?;
        let (template, params, label) = match variant {
            Variant::Corridor => {
                let graph = graphs
                    .iter()
                    .find(|g| g.camera_idx == index && g.segment == segment)
                    .ok_or_else(|| format!("第 {segment} 段相机 {camera} 还没有示教走廊"))?;
                (template::corridor(), graph.params.clone(), "示教走廊")
            }
            Variant::Heading => {
                let graph = graphs
                    .headings()
                    .find(|g| g.camera_idx == index && g.segment == segment)
                    .ok_or_else(|| format!("第 {segment} 段没有开启逐帧定向"))?;
                (template::heading(), graph.params.clone(), "逐帧定向")
            }
        };
        core(&app)?;
        let doc = with_defaults(
            &template.doc,
            &params,
            format!("{camera} · 段 {segment} · 帧组 {seq} · {label}"),
        );
        app.state::<FlowDebugHost>().reset(Session {
            id: ticket,
            template,
            image: vision::image_input(&image)?,
            params,
        })?;
        Ok(json!({ "doc": doc, "migrations": [], "session": ticket }))
    })
    .await
}

#[tauri::command]
pub async fn flow_manifest(app: AppHandle) -> Result<Value, String> {
    blocking(move || parse(core(&app)?.manifest_json(), "算子清单")).await
}

#[tauri::command]
pub async fn flow_core_info(app: AppHandle) -> Result<Value, String> {
    blocking(move || {
        let core = core(&app)?;
        let manifest = parse(core.manifest_json(), "算子清单")?;
        let count = |key: &str| manifest[key].as_array().map_or(0, Vec::len);
        Ok(json!({ "version": core.version(), "operatorCount": count("operators"), "typeCount": count("types"), "generation": 0, "hotReload": false }))
    })
    .await
}

#[tauri::command]
pub async fn flow_validate(
    app: AppHandle,
    session: u64,
    doc: Value,
    params: Option<Map<String, Value>>,
) -> Result<Value, String> {
    blocking(move || {
        let session = app.state::<FlowDebugHost>().session(session)?;
        let (graph, params) = prepared(&session, &doc, params)?;
        parse(
            core(&app)?.validate_with_params(&graph, "", Some(&params)),
            "诊断",
        )
    })
    .await
}

#[tauri::command]
pub async fn flow_plan(
    app: AppHandle,
    session: u64,
    doc: Value,
    targets: Option<Vec<String>>,
    params: Option<Map<String, Value>>,
) -> Result<Value, String> {
    blocking(move || {
        let session = app.state::<FlowDebugHost>().session(session)?;
        let (graph, params) = prepared(&session, &doc, params)?;
        let images = [session.image.clone()];
        parse(
            core(&app)?.plan_with_inputs(
                &graph,
                "",
                &targets.unwrap_or_default(),
                Some(&params),
                &[],
                &images,
            ),
            "计划",
        )
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn flow_run(
    app: AppHandle,
    session: u64,
    doc: Value,
    targets: Option<Vec<String>>,
    isolate: Option<Vec<String>>,
    force: Option<Vec<String>>,
    mode: Option<String>,
    preview_max_points: Option<u32>,
    params: Option<Map<String, Value>>,
) -> Result<String, String> {
    blocking(move || {
        crate::camera::check_idle(&app.state::<CycleHost>())?;
        let host = app.state::<FlowDebugHost>();
        let session = host.session(session)?;
        let (graph, params) = prepared(&session, &doc, params)?;
        let core = core(&app)?;
        let (targets, isolate, force) = (
            targets.unwrap_or_default(),
            isolate.unwrap_or_default(),
            force.unwrap_or_default(),
        );
        let images = [session.image.clone()];
        host.start(|| {
            let run_id = vision::unique_run_id("flow");
            let mut spec = RunSpec::new(&graph, &run_id, "", &targets).with_params_json(&params);
            spec.isolate = &isolate;
            spec.force = &force;
            spec.image_inputs = &images;
            if mode.as_deref() == Some("preview") {
                spec.mode = 1;
                spec.preview_max_points = preview_max_points.unwrap_or(0);
            }
            let ctx = Box::new(EmitCtx {
                app: app.clone(),
                run_id: run_id.clone(),
            });
            unsafe { RunHandle::start(core, spec, forward, ctx) }.map_err(|e| e.to_string())
        })
    })
    .await
}

#[tauri::command]
pub fn flow_cancel(app: AppHandle, run_id: String) {
    app.state::<FlowDebugHost>().cancel(&run_id);
}

#[tauri::command]
pub async fn flow_output_info(
    app: AppHandle,
    run_id: String,
    node_id: String,
) -> Result<Value, String> {
    blocking(move || parse(core(&app)?.output_info(&run_id, &node_id), "输出信息")).await
}

#[tauri::command]
pub async fn flow_run_outputs(app: AppHandle, run_id: String) -> Result<Value, String> {
    blocking(move || parse(core(&app)?.run_outputs(&run_id), "图输出")).await
}

/// 图像的第 `level` 级、从 `row` 起 `rows` 行（0 = 到底），超过 16 MB 少给几行，帧头里的行数是实际给的。
#[tauri::command]
pub async fn flow_output_image(
    app: AppHandle,
    run_id: String,
    node_id: String,
    port: String,
    level: Option<u32>,
    row: Option<u32>,
    rows: Option<u32>,
) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let core = core(&app)?;
        let (level, row) = (level.unwrap_or(0), row.unwrap_or(0));
        let view = core
            .output_image(&run_id, &node_id, &port, level, row, rows.unwrap_or(0))
            .map_err(|e| e.to_string())?;
        let fit = (IMAGE_BYTES_LIMIT / (view.row_bytes().max(1) as usize)).max(1) as u32;
        let view = if view.row_count() <= fit {
            view
        } else {
            drop(view);
            core.output_image(&run_id, &node_id, &port, level, row, fit)
                .map_err(|e| e.to_string())?
        };
        Ok(tauri::ipc::Response::new(encode_image(&view)))
    })
    .await
}

fn encode_image(view: &lyflow_client::ImageView) -> Vec<u8> {
    let pixels = view.pixels();
    let padded = (pixels.len() + 3) & !3;
    let mut out = Vec::with_capacity(IMAGE_HEADER_BYTES + padded);
    for v in [
        IMAGE_MAGIC,
        view.width(),
        view.height(),
        view.channels(),
        view.depth(),
        view.level(),
        view.full_width(),
        view.full_height(),
        view.row_offset(),
        view.row_count(),
        view.row_bytes(),
        0,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(pixels);
    out.resize(IMAGE_HEADER_BYTES + padded, 0);
    out
}

#[tauri::command]
pub async fn flow_evict_cache(
    app: AppHandle,
    session: u64,
    doc: Value,
    node_ids: Vec<String>,
    downstream: Option<bool>,
    params: Option<Map<String, Value>>,
) -> Result<Value, String> {
    blocking(move || {
        let session = app.state::<FlowDebugHost>().session(session)?;
        let (graph, params) = prepared(&session, &doc, params)?;
        let images = [session.image.clone()];
        let raw = core(&app)?.cache_evict_with_inputs(
            &graph,
            "",
            &node_ids,
            downstream.unwrap_or(false),
            Some(&params),
            &[],
            &images,
        );
        let value = parse(raw, "清缓存结果")?;
        // 图编不过时 core 回的是诊断数组
        if let Some(diags) = value.as_array() {
            let first = diags
                .first()
                .and_then(|d| d["message"].as_str())
                .unwrap_or("图校验不过");
            return Err(format!("图有错，清不了缓存：{first}"));
        }
        Ok(value)
    })
    .await
}

#[tauri::command]
pub async fn flow_clear_cache(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        core(&app)?.cache_clear();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn flow_cache_stats(app: AppHandle) -> Result<Value, String> {
    blocking(move || parse(core(&app)?.cache_stats(), "缓存统计")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 防的回归（LF-6）：流程调试页改了拓扑的图照样被校验、编计划或运行；或调参、挪节点也被拒。
    /// 经 `prepared` 测：flow_validate、flow_plan、flow_run 都先经它拿运行用的图与参数。
    /// 防的回归：先发起的打开晚到时顶替了后打开的调试图，编辑器跑的是上一帧或上一张图；旧编辑器的请求打到新图上。
    #[test]
    fn late_stale_open_does_not_replace_newer_session_and_stale_requests_are_rejected() {
        let host = FlowDebugHost::default();
        let session = |id| Session {
            id,
            template: template::corridor(),
            image: vision::image_input(&crate::frame::FrameImage::new(4, 4, vec![0; 16])).unwrap(),
            params: json!({}),
        };
        let (older, newer) = (host.begin_open(), host.begin_open());
        host.reset(session(newer)).unwrap();
        assert!(host.reset(session(older)).is_err());
        assert!(host.session(newer).is_ok());
        assert!(host.session(older).is_err());
    }

    #[test]
    fn topology_change_is_rejected_but_params_and_layout_are_not() {
        let session = Session {
            id: 1,
            template: template::corridor(),
            image: vision::image_input(&crate::frame::FrameImage::new(4, 4, vec![0; 16])).unwrap(),
            params: json!({"segmentMin": 8.0}),
        };
        let base = session.template.doc.clone();
        let mut tuned = base.clone();
        tuned["nodes"][3]["params"]["segmentMin"] = json!(30.0);
        tuned["nodes"][3]["position"] = json!({"x": 10, "y": 20});
        tuned["params"]["segmentMin"]["default"] = json!(30.0);
        let edited = Map::from_iter([("segmentMin".to_string(), json!(30.0))]);
        let (_, params) = prepared(&session, &tuned, Some(edited)).unwrap();
        assert_eq!(params, json!({"segmentMin": 30.0}).to_string());
        let (_, params) = prepared(&session, &tuned, None).unwrap();
        assert_eq!(params, session.params.to_string());
        let mut added = base.clone();
        added["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "n_extra", "op": "io.load_image", "params": {}}));
        let mut rewired = base.clone();
        rewired["edges"][0]["to"]["port"] = json!("other");
        let mut swapped = base.clone();
        swapped["nodes"][1]["op"] = json!("glue.bead_path");
        for doc in [added, rewired, swapped] {
            assert!(prepared(&session, &doc, None).is_err());
        }
    }
}

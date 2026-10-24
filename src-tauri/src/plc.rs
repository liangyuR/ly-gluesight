use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use ly_plc::{
    describe_address, ConnectionConfig, DataType, EventSink, HistorySample, LogPage, LogQuery, Logbook, PlcConfig,
    PlcEngine, PlcEvent, PlcStatus, PointValue, ProtocolKind,
};
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cycle::{CycleHost, Input};
use crate::inspection;
use crate::handshake::Contract;
use crate::plc_plan::PlcPlan;
use crate::recipe::Recipe;

pub struct PlcHost {
    engine: PlcEngine,
    config_path: PathBuf,
}

impl PlcHost {
    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let config_path = crate::storage::config(app).join("plc.json");
        let log_path = crate::storage::root(app).join("logs").join("plc.db");
        let mut config =
            PlcConfig::load(&config_path).unwrap_or_else(inspection::default_plc_config);
        inspection::add_teaching_points(&mut config);
        let handle = app.clone();
        let last_link = std::sync::Mutex::new(None);
        let sink: EventSink = Arc::new(move |event| match event {
            PlcEvent::Status(s) => {
                let link = (s.state, s.message.clone());
                let mut last = last_link.lock().unwrap();
                if last.as_ref() != Some(&link) {
                    let level = if s.state == ly_plc::LinkState::Error { "err" } else { "info" };
                    crate::applog::write(level, "PLC 连接", &format!("{:?}：{}", s.state, s.message));
                    *last = Some(link);
                }
                drop(last);
                let _ = handle.emit("plc://status", s);
            }
            PlcEvent::Values(v) => {
                let _ = handle.emit("plc://values", v);
            }
            PlcEvent::Logs(l) => {
                for entry in l.iter().filter(|e| e.level == "warn" || e.level == "error") {
                    crate::applog::write(&entry.level, &format!("PLC {}", entry.category), &entry.message);
                }
                let _ = handle.emit("plc://log", l);
            }
            PlcEvent::Edge(e) => {
                let _ = handle.emit("plc://edge", &e);
            }
        });
        let logbook = Arc::new(Logbook::open(&log_path, sink.clone(), config.log_retention_days)?);
        Ok(Self { engine: PlcEngine::new(config, logbook, sink), config_path })
    }

    pub fn engine(&self) -> &PlcEngine {
        &self.engine
    }

    pub fn start_if_configured(app: &AppHandle) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let host = app.state::<PlcHost>();
            if host.engine.config().auto_connect {
                if let Err(error) = validate_config(&host.engine.config()) {
                    crate::cycle::log(&app, "err", "PLC 配置", error);
                    return;
                }
                host.engine.connect().await;
            }
        });
    }
}

#[tauri::command]
pub fn plc_get_config(plc: State<'_, PlcHost>) -> PlcConfig {
    plc.engine.config()
}

#[tauri::command]
pub async fn plc_save_config(app: AppHandle, plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, config: PlcConfig) -> Result<(), String> {
    let _gate = cycle.link_gate.lock().await;
    require_idle(&cycle)?;
    if crate::offline::enabled(&app) { return Err("离线模式由站点模式管理 PLC 配置，退出离线后再修改".into()); }
    validate_config(&config)?;
    config.save(&plc.config_path)?;
    let r = plc.engine.apply_config(config).await;
    let _ = cycle.tx.send(Input::Refresh);
    // 配置保存不一定改变连接状态，配置使用方不能只等 disconnected/connected。
    if r.is_ok() {
        let _ = app.emit("plc://config", ());
    }
    r
}

#[tauri::command]
pub async fn plc_connect(plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>) -> Result<(), String> {
    let _gate = cycle.link_gate.lock().await;
    if plc.engine.status().state == ly_plc::LinkState::Connected { return Ok(()); }
    validate_config(&plc.engine.config())?;
    plc.engine.connect().await;
    Ok(())
}

#[tauri::command]
pub async fn plc_disconnect(plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>) -> Result<(), String> {
    let _gate = cycle.link_gate.lock().await;
    require_idle(&cycle)?;
    plc.engine.disconnect().await;
    Ok(())
}

#[tauri::command]
pub fn plc_get_status(plc: State<'_, PlcHost>) -> PlcStatus {
    plc.engine.status()
}

#[tauri::command]
pub fn plc_get_values(plc: State<'_, PlcHost>) -> HashMap<String, PointValue> {
    plc.engine.values()
}

#[tauri::command]
pub async fn plc_write_point(plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>, id: String, value: Value) -> Result<(), String> {
    let _gate = cycle.link_gate.lock().await;
    require_idle(&cycle)?;
    if inspection::reserved_s7_point(&plc.engine.config(), &id) {
        return Err("S7 握手点由生产状态机管理，禁止手动写入".into());
    }
    Ok(plc.engine.write_point(&id, &value).await?)
}

fn require_idle(cycle: &CycleHost) -> Result<(), String> {
    if cycle.busy() { Err("PLC 仍有在途事务，请完成握手或处理故障并复位后再修改".into()) } else { Ok(()) }
}

pub(crate) fn validate_config(config: &PlcConfig) -> Result<(), String> {
    if config.connection.protocol != ProtocolKind::S7 { return Err("只支持西门子 S7 协议".into()); }
    config.validate()?;
    Contract::validate(config)?;
    Ok(())
}

pub fn recipe_plan(cycle: &CycleHost, recipe: &Recipe) -> Result<PlcPlan, String> {
    let configs = cycle.camera.configs();
    if recipe.is_burst() {
        // 段配方：三台计划相机都在相机组里，且是单视角独立设备（D-SB-11）
        crate::camera::check_burst_cameras(&configs, &recipe.plan_cameras()).map_err(|error| format!("配方 {}：{error}", recipe.id))?;
    } else if let Some(missing) = recipe.cameras().into_iter().find(|id| !configs.iter().any(|c| &c.id == id)) {
        return Err(format!("配方 {} 用到的相机 {missing} 不在设备配置里", recipe.id));
    }
    PlcPlan::from_recipe(recipe)
}

#[tauri::command]
pub fn plc_s7_phase1_template(plc_db: u16, pc_db: u16) -> Result<PlcConfig, String> {
    let config = inspection::s7_phase1_config(plc_db, pc_db)?;
    validate_config(&config)?;
    Ok(config)
}

#[tauri::command]
pub async fn plc_query_logs(plc: State<'_, PlcHost>, query: LogQuery) -> Result<LogPage, String> {
    plc.engine.logbook().query(&query)
}

/// 级别、类别的中文文案由前端 meta 传入，后端不另维护一份；未知编码按原值写出。
#[derive(Deserialize)]
pub struct LogLabels {
    levels: HashMap<String, String>,
    categories: HashMap<String, String>,
}

const LOG_CSV_HEADER: &str = "\u{feff}时间,级别,类别,点位,内容,旧值,新值,事务\r\n";
const LOG_EXPORT_PAGE: u32 = 1000;
const LOG_EXPORT_CHANGED: &str = "日志在导出期间已变化，未生成完整文件，请重试";

fn csv_cell(s: &str) -> Cow<'_, str> {
    if s.contains(['"', ',', '\r', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")).into() } else { s.into() }
}

fn write_log_csv(
    out: &mut impl Write,
    mut query: LogQuery,
    labels: &LogLabels,
    fetch: impl Fn(&LogQuery) -> Result<LogPage, String>,
) -> Result<(), String> {
    let label = |map: &HashMap<String, String>, key: &str| map.get(key).cloned().unwrap_or_else(|| key.to_string());
    out.write_all(LOG_CSV_HEADER.as_bytes()).map_err(|e| e.to_string())?;
    query.limit = Some(LOG_EXPORT_PAGE);
    let mut offset = 0u32;
    let mut total = None;
    loop {
        query.offset = Some(offset);
        let page = fetch(&query)?;
        if *total.get_or_insert(page.total) != page.total {
            return Err(LOG_EXPORT_CHANGED.into());
        }
        for e in &page.items {
            let cells = [
                crate::history::format_ts(e.ts),
                label(&labels.levels, &e.level),
                label(&labels.categories, &e.category),
                e.point_name.clone().unwrap_or_default(),
                e.message.clone(),
                e.old_value.clone().unwrap_or_default(),
                e.new_value.clone().unwrap_or_default(),
                e.context.clone().unwrap_or_default(),
            ];
            let row = cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",");
            out.write_all(format!("{row}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        }
        offset += page.items.len() as u32;
        if i64::from(offset) >= page.total {
            return Ok(());
        }
        if page.items.is_empty() {
            return Err(LOG_EXPORT_CHANGED.into());
        }
    }
}

/// 按日志页筛选条件导出全部日志（带 BOM），结束时间不晚于导出时刻，返回文件路径。
#[tauri::command]
pub async fn plc_export_logs(app: AppHandle, mut query: LogQuery, labels: LogLabels) -> Result<String, String> {
    let now = ly_plc::now_ms();
    query.end = Some(query.end.map_or(now, |end| end.min(now)));
    let dir = crate::storage::root(&app).join("exports");
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("PLC日志_{}.csv", chrono::Local::now().format("%Y%m%d_%H%M%S")));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        let logbook = app.state::<PlcHost>().engine.logbook().clone();
        let written = write_log_csv(&mut f, query, &labels, |q| logbook.query(q)).and_then(|_| f.flush().map_err(|e| e.to_string()));
        drop(f);
        if let Err(e) = written {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        Ok(path.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plc_point_history(
    plc: State<'_, PlcHost>,
    point_id: String,
    start: i64,
    end: i64,
) -> Result<Vec<HistorySample>, String> {
    plc.engine.logbook().point_history(&point_id, start, end)
}

#[tauri::command]
pub fn plc_check_address(connection: ConnectionConfig, address: String, data_type: DataType) -> Result<String, String> {
    describe_address(&connection, &address, data_type)
}

#[cfg(test)]
#[path = "plc/tests.rs"]
mod tests;

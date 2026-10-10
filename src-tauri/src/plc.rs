use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ly_plc::{
    describe_address, ConnectionConfig, DataType, EventSink, HistorySample, LogPage, LogQuery, Logbook, PlcConfig,
    PlcEngine, PlcEvent, PlcStatus, PointValue, ProtocolKind,
};
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
        let config_path = app.path().app_config_dir().map_err(|e| e.to_string())?.join("plc.json");
        let log_path = app.path().app_data_dir().map_err(|e| e.to_string())?.join("logs").join("plc.db");
        let config = PlcConfig::load(&config_path).unwrap_or_else(inspection::default_plc_config);
        let handle = app.clone();
        let sink: EventSink = Arc::new(move |event| match event {
            PlcEvent::Status(s) => {
                let _ = handle.emit("plc://status", s);
            }
            PlcEvent::Values(v) => {
                let _ = handle.emit("plc://values", v);
            }
            PlcEvent::Logs(l) => {
                let _ = handle.emit("plc://log", l);
            }
            PlcEvent::Edge(e) => {
                let _ = handle.emit("plc://edge", &e);
                if let Some(cycle) = handle.try_state::<CycleHost>() {
                    let _ = cycle.tx.send(Input::Edge(e));
                }
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
    let _gate = cycle.plc_gate.lock().await;
    require_idle(&cycle)?;
    validate_config(&config)?;
    config.save(&plc.config_path)?;
    let r = plc.engine.apply_config(config).await;
    inspection::invalidate_tags();
    // 开工检查看 PLC 是不是模拟器
    let _ = cycle.tx.send(Input::Refresh);
    // 配置保存不一定改变连接状态，配置使用方不能只等 disconnected/connected。
    if r.is_ok() {
        let _ = app.emit("plc://config", ());
    }
    r
}

#[tauri::command]
pub async fn plc_connect(plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>) -> Result<(), String> {
    let _gate = cycle.plc_gate.lock().await;
    if plc.engine.status().state == ly_plc::LinkState::Connected { return Ok(()); }
    validate_config(&plc.engine.config())?;
    plc.engine.connect().await;
    Ok(())
}

#[tauri::command]
pub async fn plc_disconnect(plc: State<'_, PlcHost>, cycle: State<'_, CycleHost>) -> Result<(), String> {
    let _gate = cycle.plc_gate.lock().await;
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
    let _gate = cycle.plc_gate.lock().await;
    require_idle(&cycle)?;
    if inspection::reserved_s7_point(&plc.engine.config(), &id) {
        return Err("S7 握手点由生产状态机管理，禁止手动写入".into());
    }
    plc.engine.write_point(&id, &value).await
}

fn require_idle(cycle: &CycleHost) -> Result<(), String> {
    if cycle.busy() { Err("PLC 仍有在途事务，请完成握手或处理故障并复位后再修改".into()) } else { Ok(()) }
}

fn validate_config(config: &PlcConfig) -> Result<(), String> {
    config.validate()?;
    if config.connection.protocol == ProtocolKind::S7 { Contract::validate(config)?; }
    Ok(())
}

pub fn recipe_plan(cycle: &CycleHost, recipe: &Recipe) -> Result<PlcPlan, String> {
    let configs = cycle.camera.configs();
    let slots = std::array::from_fn(|index| configs.get(index).map(|c| c.id.clone()).unwrap_or_default());
    PlcPlan::from_recipe(recipe, slots)
}

#[tauri::command]
pub fn plc_s7_phase1_template(db_number: u16) -> Result<PlcConfig, String> {
    let config = inspection::s7_phase1_config(db_number)?;
    validate_config(&config)?;
    Ok(config)
}

#[tauri::command]
pub fn plc_recipe_plan(cycle: State<'_, CycleHost>, recipe_id: String) -> Result<PlcPlan, String> {
    let recipe = cycle.recipe(&recipe_id).ok_or("生产配方不存在")?;
    recipe_plan(&cycle, &recipe)
}

#[tauri::command]
pub async fn plc_query_logs(plc: State<'_, PlcHost>, query: LogQuery) -> Result<LogPage, String> {
    plc.engine.logbook().query(&query)
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
mod tests {
    use super::*;

    #[test]
    fn retired_plan_tags_fail_validation_instead_of_being_renamed() {
        let mut config = inspection::s7_phase1_config(100).unwrap();
        for point in &mut config.points {
            for tag in &mut point.tags {
                if tag == "planReserved" { *tag = "planHash".into(); }
            }
        }
        let original = config.clone();
        let error = validate_config(&config).unwrap_err();
        assert!(error.contains("planHash") && error.contains("planReserved"), "{error}");
        assert_eq!(config, original);
        let mut config = inspection::s7_phase1_config(100).unwrap();
        config.points.iter_mut().find(|point| point.tags.iter().any(|tag| tag == "acceptedPlanReserved")).unwrap()
            .tags = vec!["acceptedPlanHash".into()];
        let error = validate_config(&config).unwrap_err();
        assert!(error.contains("acceptedPlanHash") && error.contains("acceptedPlanReserved"), "{error}");
    }
}

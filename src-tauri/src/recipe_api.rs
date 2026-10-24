//! 配方命令：列表、读取、预览、模板与删除。生产配方只经工作台发布写入。

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::cycle::{CycleHost, Input, RecipeSummary};
use crate::plan::{self, PlanExport};
use crate::recipe::{self, Recipe, RecipeDoc};
use crate::settings::CycleSettings;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeListing {
    pub recipes: Vec<RecipeSummary>,
    /// 读不了的配方文件
    pub errors: Vec<String>,
    pub unavailable: Vec<recipe::UnavailableRecipe>,
}

#[tauri::command]
pub fn recipe_list(cycle: State<'_, CycleHost>) -> RecipeListing {
    RecipeListing { recipes: cycle.recipes.list().iter().map(|r| RecipeSummary::from(&**r)).collect(), errors: cycle.recipes.errors(), unavailable: cycle.recipes.unavailable() }
}

#[tauri::command]
pub fn recipe_doc(cycle: State<'_, CycleHost>, id: String) -> Result<RecipeDoc, String> {
    cycle.recipes.doc(&id).ok_or_else(|| format!("配方不存在：{id}"))
}

#[tauri::command]
pub fn recipe_delete_unavailable(
    app: AppHandle,
    cycle: State<'_, CycleHost>,
    file_name: String,
) -> Result<(), String> {
    let data = serde_json::json!({"id": file_name});
    let result = recipe_delete_unavailable_inner(cycle, file_name);
    crate::cycle::log_op(&app, &crate::applog::codes::OP_RECIPE_DELETE, data, &result);
    result
}

fn recipe_delete_unavailable_inner(
    cycle: State<'_, CycleHost>,
    file_name: String,
) -> Result<(), String> {
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在布防或处理其他配置操作，请稍后重试配方操作")?;
    if cycle.busy() {
        return Err("检测结束或故障复位后才能删除不可用配方".into());
    }
    cycle.recipes.delete_unavailable(&file_name)?;
    let _ = cycle.tx.send(Input::Refresh);
    Ok(())
}

/// 按编辑中的内容生成配方但不保存，配方页画预览用。
#[tauri::command]
pub fn recipe_preview(mut doc: RecipeDoc) -> Result<Recipe, String> {
    doc.sync_imaging();
    doc.build()
}

/// 新建配方的起点：段配方（D-SB-24），相机取相机组前三台，一个待标定的直线段。
#[tauri::command]
pub fn recipe_template(cycle: State<'_, CycleHost>) -> RecipeDoc {
    let used: Vec<u16> = cycle.recipes.list().iter().map(|r| r.product_code).collect();
    let cameras: Vec<String> = cycle.camera.configs().iter().map(|c| c.id.clone()).collect();
    template(&used, &cameras)
}

/// 产品代码取第一个没被占用的；相机按相机组顺序取前三台（不足 3 台照样建，但过不了段配方校验）。
fn template(used_codes: &[u16], rig_cameras: &[String]) -> RecipeDoc {
    let product_code = (1..=i16::MAX as u16).find(|c| !used_codes.contains(c)).unwrap_or(0);
    let cameras = &rig_cameras[..rig_cameras.len().min(plan::PLAN_CAMERAS)];
    recipe::burst_template(format!("NEW-{product_code}"), "新配方".into(), product_code, cameras)
}

/// 段配方的计划导出 JSON（规格 §1）：段序列、各段 N_exp、容差、ΣN_exp，给 PLC / 机器人工程师核对使能点位；
/// PLC 的 shotCount 与 deviceShots 都填 expectedTotal。未标定时 expectedTotal 为 null。
#[tauri::command]
pub fn recipe_plan_export(cycle: State<'_, CycleHost>, id: String) -> Result<String, String> {
    let recipe = cycle.recipes.get(&id).ok_or_else(|| format!("配方不存在：{id}"))?;
    let export = PlanExport::from_recipe(&recipe)?;
    serde_json::to_string_pretty(&export).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn recipe_delete(
    app: AppHandle,
    cycle: State<'_, CycleHost>,
    id: String,
) -> Result<(), String> {
    let data = serde_json::json!({"id": id});
    let result = recipe_delete_inner(app.clone(), cycle, id);
    crate::cycle::log_op(&app, &crate::applog::codes::OP_RECIPE_DELETE, data, &result);
    result
}

fn recipe_delete_inner(
    app: AppHandle,
    cycle: State<'_, CycleHost>,
    id: String,
) -> Result<(), String> {
    let gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在布防或处理其他配置操作，请稍后重试生产配方操作")?;
    if cycle.busy()
        && app
            .state::<crate::plc::PlcHost>()
            .engine()
            .config()
            .connection
            .protocol
            == ly_plc::ProtocolKind::S7
    {
        return Err("S7 在途事务结束或故障复位后才能删除生产配方".into());
    }
    if cycle.recipe_in_use(&id) {
        return Err("该配方正在检测中，工件结束后再删".into());
    }
    let settings = cycle.settings();
    cycle.recipes.delete(&id)?;
    // 删掉的正是人工选中的配方：清掉选择，下一件报"未选择配方"而不是拿着一个不存在的编号
    if settings.manual_recipe_id.as_deref() == Some(id.as_str()) {
        cycle.save_settings_locked(CycleSettings { manual_recipe_id: None, ..settings }, &gate)?;
    }
    let _ = cycle.tx.send(Input::Refresh);
    Ok(())
}

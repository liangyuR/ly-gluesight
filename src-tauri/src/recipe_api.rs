//! 配方页用的命令：列表、编辑、预览、保存（内容变了版本号 +1）、删除、导入胶路点。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::camera::Acquisition;
use crate::cycle::{CycleHost, Input, RecipeSummary};
use crate::recipe::{self, ImportedPath, Recipe, RecipeDoc};
use crate::settings::CycleSettings;
use crate::vision::{self, VisionHost};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeListing {
    pub recipes: Vec<RecipeSummary>,
    /// 读不了的配方文件
    pub errors: Vec<String>,
}

#[tauri::command]
pub fn recipe_list(cycle: State<'_, CycleHost>) -> RecipeListing {
    RecipeListing { recipes: cycle.recipes.list().iter().map(|r| RecipeSummary::from(&**r)).collect(), errors: cycle.recipes.errors() }
}

#[tauri::command]
pub fn recipe_doc(cycle: State<'_, CycleHost>, id: String) -> Result<RecipeDoc, String> {
    cycle.recipes.doc(&id).ok_or_else(|| format!("配方不存在：{id}"))
}

/// 按编辑中的内容生成配方但不保存，配方页画预览用。
#[tauri::command]
pub fn recipe_preview(doc: RecipeDoc) -> Result<Recipe, String> {
    doc.build()
}

/// 新建配方的起点：一份飞拍默认值。
#[tauri::command]
pub fn recipe_template(cycle: State<'_, CycleHost>) -> RecipeDoc {
    let mut doc = recipe::samples().remove(0);
    let used: Vec<u16> = cycle.recipes.list().iter().map(|r| r.product_code).collect();
    doc.product_code = (1..u16::MAX).find(|c| !used.contains(c)).unwrap_or(0);
    doc.id = format!("NEW-{}", doc.product_code);
    doc.name = "新配方".into();
    doc.version = 1;
    // 默认挑触发采集的相机
    let configs = cycle.camera.configs();
    if let Some(c) = configs.iter().find(|c| c.acquisition == Acquisition::Triggered).or(configs.first()) {
        for shot in &mut doc.shots {
            shot.camera = c.id.clone();
        }
    }
    doc
}

#[tauri::command]
pub fn recipe_save(app: AppHandle, cycle: State<'_, CycleHost>, doc: RecipeDoc, original_id: Option<String>) -> Result<RecipeSummary, String> {
    let gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试生产配方操作")?;
    if cycle.busy() && app.state::<crate::plc::PlcHost>().engine().config().connection.protocol == ly_plc::ProtocolKind::S7 {
        return Err("S7 在途事务结束或故障复位后才能修改生产配方".into());
    }
    if original_id.as_deref().is_some_and(|o| o != doc.id && cycle.recipe_in_use(o)) {
        return Err("该配方正在检测中，工件结束后再改编号".into());
    }
    let saved: Arc<Recipe> = cycle.recipes.save(doc, original_id.as_deref())?;
    if let Some(old) = original_id.filter(|o| *o != saved.id) {
        // 示教资料按配方编号存，跟着改名
        if let (Ok(from), Ok(to)) = (vision::taught_dir(&app, &old), vision::taught_dir(&app, &saved.id)) {
            if from.exists() {
                if !old.eq_ignore_ascii_case(&saved.id) {
                    let _ = std::fs::remove_dir_all(&to);
                }
                let _ = std::fs::rename(&from, &to);
            }
        }
        app.state::<VisionHost>().forget(&old);
        // 改了编号的配方正被人工选中时，跟着改过去
        let mut settings = cycle.settings();
        if settings.manual_recipe_id.as_deref() == Some(old.as_str()) {
            settings.manual_recipe_id = Some(saved.id.clone());
            cycle.save_settings_locked(settings, &gate)?;
        }
    }
    let _ = cycle.tx.send(Input::Refresh);
    Ok(RecipeSummary::from(&*saved))
}

#[tauri::command]
pub fn recipe_delete(app: AppHandle, cycle: State<'_, CycleHost>, id: String) -> Result<(), String> {
    let gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理 PLC 事务，请稍后重试生产配方操作")?;
    if cycle.busy() && app.state::<crate::plc::PlcHost>().engine().config().connection.protocol == ly_plc::ProtocolKind::S7 {
        return Err("S7 在途事务结束或故障复位后才能删除生产配方".into());
    }
    if cycle.recipe_in_use(&id) {
        return Err("该配方正在检测中，工件结束后再删".into());
    }
    let settings = cycle.settings();
    cycle.recipes.delete(&id)?;
    if let Ok(dir) = vision::taught_dir(&app, &id) {
        let _ = std::fs::remove_dir_all(dir);
    }
    app.state::<VisionHost>().forget(&id);
    // 删掉的正是人工选中的配方：清掉选择，下一件报"未选择配方"而不是拿着一个不存在的编号
    if settings.manual_recipe_id.as_deref() == Some(id.as_str()) {
        cycle.save_settings_locked(CycleSettings { manual_recipe_id: None, ..settings }, &gate)?;
    }
    let _ = cycle.tx.send(Input::Refresh);
    Ok(())
}

/// 解析胶路文件的内容（前端读文件后把文本传过来）。
#[tauri::command]
pub fn recipe_parse_path(text: String, file_name: String) -> Result<ImportedPath, String> {
    let ext = std::path::Path::new(&file_name).extension().and_then(|e| e.to_str()).unwrap_or("csv");
    recipe::parse_path(&text, ext)
}

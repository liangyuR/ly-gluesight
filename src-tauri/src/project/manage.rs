use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::ProjectMeta;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProject {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub copy_hardware: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUsage {
    pub recipes: u32,
    pub workspaces: u32,
    pub capture_rounds: u32,
    pub records: u64,
    pub first_record_at: Option<i64>,
    pub last_record_at: Option<i64>,
    pub bytes: u64,
}

#[tauri::command]
pub async fn project_create(app: AppHandle, request: CreateProject) -> Result<ProjectMeta, String> {
    let _ = (app, request);
    Err("新建项目尚未实现".into())
}

#[tauri::command]
pub fn project_rename(app: AppHandle, id: String, name: String, description: String) -> Result<ProjectMeta, String> {
    let _ = (app, id, name, description);
    Err("重命名项目尚未实现".into())
}

#[tauri::command]
pub async fn project_delete(app: AppHandle, id: String) -> Result<(), String> {
    let _ = (app, id);
    Err("删除项目尚未实现".into())
}

#[tauri::command]
pub async fn project_usage(app: AppHandle, id: String) -> Result<ProjectUsage, String> {
    let _ = (app, id);
    Err("项目统计尚未实现".into())
}

pub mod manage;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const DEFAULT_ID: &str = "default";
const SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMeta {
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: i64,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub copied_from: Option<String>,
}

impl ProjectMeta {
    pub fn new(id: &str, name: &str, description: &str, copied_from: Option<String>) -> Self {
        Self {
            schema: SCHEMA,
            id: id.into(),
            name: name.into(),
            description: description.into(),
            created_at: ly_plc::now_ms(),
            created_by: env!("CARGO_PKG_VERSION").into(),
            copied_from,
        }
    }
}

#[derive(Debug)]
pub struct ActiveProject {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    pub id: String,
    pub meta: Option<ProjectMeta>,
    pub error: Option<String>,
    pub current: bool,
}

#[derive(Debug)]
pub enum Resolution {
    Open(ActiveProject),
    NeedsChoice(String),
}

pub fn projects_root(data: &Path) -> PathBuf {
    data.join("projects")
}

pub fn trash_root(data: &Path) -> PathBuf {
    projects_root(data).join(".trash")
}

pub fn dir(data: &Path, id: &str) -> PathBuf {
    projects_root(data).join(id)
}

fn pointer_path(data: &Path) -> PathBuf {
    data.join("config").join("current-project.json")
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn read_meta(dir: &Path) -> Result<ProjectMeta, String> {
    let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let text = crate::fsio::read_text(&dir.join("project.json")).map_err(|e| format!("project.json 读不了：{e}"))?;
    let meta: ProjectMeta = serde_json::from_str(&text).map_err(|e| format!("project.json 无法解析：{e}"))?;
    if meta.schema != SCHEMA { return Err(format!("project.json 格式版本 {} 不认识", meta.schema)); }
    if meta.id != id { return Err(format!("project.json 记的编号 {} 与目录名 {id} 不一致", meta.id)); }
    Ok(meta)
}

pub fn write_meta(dir: &Path, meta: &ProjectMeta) -> Result<(), String> {
    crate::fsio::write_atomic(&dir.join("project.json"), &serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?)
}

pub fn read_pointer(data: &Path) -> Option<String> {
    let text = crate::fsio::read_text(&pointer_path(data)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("current")?.as_str().map(str::to_string)
}

pub fn write_pointer(data: &Path, id: &str) -> Result<(), String> {
    crate::fsio::write_atomic(&pointer_path(data), &serde_json::json!({"schema": SCHEMA, "current": id}).to_string())
}

fn project_dirs(data: &Path) -> Result<Vec<PathBuf>, String> {
    let root = projects_root(data);
    if !root.exists() { return Ok(Vec::new()); }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("读取项目目录失败：{e}"))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    Ok(dirs)
}

pub fn list(data: &Path) -> Result<Vec<ProjectEntry>, String> {
    let current = read_pointer(data);
    Ok(project_dirs(data)?
        .into_iter()
        .map(|dir| {
            let id = dir.file_name().unwrap().to_string_lossy().into_owned();
            let (meta, error) = match read_meta(&dir) {
                Ok(meta) => (Some(meta), None),
                Err(error) => (None, Some(error)),
            };
            ProjectEntry { current: current.as_deref() == Some(id.as_str()), id, meta, error }
        })
        .collect())
}

pub fn create_dir(data: &Path, meta: &ProjectMeta, fill: impl FnOnce(&Path) -> Result<(), String>) -> Result<PathBuf, String> {
    if !valid_id(&meta.id) { return Err("项目编号只能包含字母、数字、- 和 _".into()); }
    let target = dir(data, &meta.id);
    if target.exists() { return Err(format!("项目编号 {} 已被占用", meta.id)); }
    let staging = projects_root(data).join(format!(".new-{}-{}", meta.id, ly_plc::now_ms()));
    std::fs::create_dir_all(staging.join("config")).map_err(|e| format!("创建项目目录失败：{e}"))?;
    let built = fill(&staging).and_then(|_| write_meta(&staging, meta));
    let moved = built.and_then(|_| std::fs::rename(&staging, &target).map_err(|e| format!("创建项目目录失败：{e}")));
    if let Err(error) = moved {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(target)
}

pub fn next_id(data: &Path) -> String {
    let day = chrono::Local::now().format("%Y%m%d");
    (1..).map(|n| format!("p-{day}-{n:02}")).find(|id| !dir(data, id).exists()).unwrap()
}

pub fn resolve(data: &Path) -> Result<Resolution, String> {
    if project_dirs(data)?.is_empty() {
        let root = create_dir(data, &ProjectMeta::new(DEFAULT_ID, "默认项目", "", None), |_| Ok(()))?;
        write_pointer(data, DEFAULT_ID)?;
        return Ok(Resolution::Open(ActiveProject { id: DEFAULT_ID.into(), root }));
    }
    let Some(id) = read_pointer(data) else {
        return Ok(Resolution::NeedsChoice("没有记录当前项目".into()));
    };
    if !valid_id(&id) || !dir(data, &id).is_dir() {
        return Ok(Resolution::NeedsChoice(format!("当前项目 {id} 不存在")));
    }
    match read_meta(&dir(data, &id)) {
        Ok(_) => Ok(Resolution::Open(ActiveProject { root: dir(data, &id), id })),
        Err(error) => Ok(Resolution::NeedsChoice(format!("当前项目 {id} 不可用：{error}"))),
    }
}

pub fn current_id(app: &AppHandle) -> String {
    app.state::<ActiveProject>().id.clone()
}

#[tauri::command]
pub fn project_current(app: AppHandle) -> Result<ProjectMeta, String> {
    let active = app.try_state::<ActiveProject>().ok_or("项目尚未打开")?;
    read_meta(&active.root)
}

#[tauri::command]
pub fn project_list(app: AppHandle) -> Result<Vec<ProjectEntry>, String> {
    list(&crate::storage::root(&app))
}

const LEGACY: [&str; 13] = [
    "config/cameras.json", "config/plc.json", "config/cycle.json", "config/feasibility.json", "calib", "recipes", "workspaces",
    "vision", "recipe-capture", "inspection.db", "records", "records-offline", "audit-spool",
];

pub fn legacy_leftovers(data: &Path) -> Vec<&'static str> {
    LEGACY.into_iter().filter(|name| data.join(name).exists()).collect()
}

pub fn open_target(data: &Path, id: &str) -> Result<ProjectMeta, String> {
    if !valid_id(id) || !dir(data, id).is_dir() { return Err(format!("项目 {id} 不存在")); }
    read_meta(&dir(data, id)).map_err(|error| format!("项目 {id} 不可用：{error}"))
}

#[tauri::command]
pub async fn project_switch(app: AppHandle, id: String) -> Result<(), String> {
    use crate::cycle::{CycleHost, Phase};
    let data = crate::storage::root(&app);
    let from = current_id(&app);
    if id == from { return Err("已经是当前项目".into()); }
    let target = open_target(&data, &id)?;
    let cycle = app.state::<CycleHost>();
    let _link = cycle.link_gate.lock().await;
    let _config = cycle.config_gate.lock().await;
    let audit_pending = !matches!(cycle.audit.readiness(), Ok(crate::audit::AuditReadiness::Ready))
        || !cycle.audit.protected_cycle_ids()?.is_empty();
    crate::site::check_project_switch(&crate::site::ProjectSwitch {
        block: cycle.switch_block(),
        busy: !matches!(cycle.phase(), Phase::Idle | Phase::Fault),
        capturing: cycle.camera.recipe_capture().active(),
        audit_pending,
    })?;
    write_pointer(&data, &id)?;
    crate::applog::write("info", "项目切换", &format!("{from} → {}（{}），重启软件", target.id, target.name));
    app.restart()
}

#[cfg(test)]
mod tests;

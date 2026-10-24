use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

use super::{ActiveProject, ProjectMeta};

static LOCK: Mutex<()> = Mutex::new(());

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

fn check_name(data: &Path, name: &str, except: Option<&str>) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() { return Err("项目名称不能为空".into()); }
    let taken = super::list(data)?.into_iter()
        .filter(|e| except != Some(e.id.as_str()))
        .filter_map(|e| e.meta)
        .any(|m| m.name.trim() == name);
    if taken { return Err(format!("项目名称 {name} 已被使用")); }
    Ok(name.into())
}

fn trash_id(name: &str) -> &str {
    name.split('.').next().unwrap_or_default()
}

fn check_new_id(data: &Path, id: &str) -> Result<(), String> {
    if !super::valid_id(id) { return Err("项目编号只能包含字母、数字、- 和 _".into()); }
    let occupied = |root: PathBuf, key: fn(&str) -> &str| -> bool {
        std::fs::read_dir(root).into_iter().flatten().flatten()
            .any(|e| key(&e.file_name().to_string_lossy()).eq_ignore_ascii_case(id))
    };
    if occupied(super::projects_root(data), |n| n) { return Err(format!("项目编号 {id} 已被占用")); }
    if occupied(super::trash_root(data), trash_id) { return Err(format!("项目编号 {id} 的旧项目还在清理中，请换一个编号或重启后再试")); }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_dir() { return Ok(()); }
    std::fs::create_dir_all(to).map_err(|e| format!("创建 {} 失败：{e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("读取 {} 失败：{e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("复制 {} 失败：{e}", entry.path().display()))?;
        }
    }
    Ok(())
}

pub fn create(data: &Path, current: Option<&ActiveProject>, request: &CreateProject) -> Result<ProjectMeta, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let id = request.id.as_str();
    check_new_id(data, id)?;
    let name = check_name(data, &request.name, None)?;
    let source = match (request.copy_hardware, current) {
        (false, _) => None,
        (true, Some(active)) => Some(active),
        (true, None) => return Err("没有打开的当前项目，无法复制硬件配置".into()),
    };
    let meta = ProjectMeta::new(id, &name, request.description.trim(), source.map(|a| a.id.clone()));
    super::create_dir(data, &meta, |staging| match source {
        Some(active) => ["config", "calib"].iter().try_for_each(|sub| copy_tree(&active.root.join(sub), &staging.join(sub))),
        None => Ok(()),
    })?;
    Ok(meta)
}

pub fn rename(data: &Path, id: &str, name: &str, description: &str) -> Result<ProjectMeta, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !super::valid_id(id) { return Err(format!("项目编号 {id} 不合法")); }
    let dir = super::dir(data, id);
    let mut meta = super::read_meta(&dir)?;
    meta.name = check_name(data, name, Some(id))?;
    meta.description = description.trim().into();
    super::write_meta(&dir, &meta)?;
    Ok(meta)
}

pub fn move_to_trash(data: &Path, current: Option<&str>, id: &str) -> Result<PathBuf, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !super::valid_id(id) { return Err(format!("项目编号 {id} 不合法")); }
    if current.is_some_and(|c| c.eq_ignore_ascii_case(id)) { return Err("不能删除当前项目，请先切换到其他项目".into()); }
    let dir = super::dir(data, id);
    if !dir.is_dir() { return Err(format!("项目 {id} 不存在")); }
    let trash = super::trash_root(data);
    std::fs::create_dir_all(&trash).map_err(|e| format!("创建 .trash 失败：{e}"))?;
    let target = trash.join(format!("{id}.{}", ly_plc::now_ms()));
    std::fs::rename(&dir, &target).map_err(|e| format!("删除项目 {id} 失败（文件可能被占用）：{e}"))?;
    Ok(target)
}

pub fn delete(data: &Path, current: Option<&str>, id: &str) -> Result<(), String> {
    let trashed = move_to_trash(data, current, id)?;
    let id = id.to_string();
    std::thread::spawn(move || {
        if let Err(e) = std::fs::remove_dir_all(&trashed) {
            crate::applog::write("warn", "项目删除未完成", &format!("项目 {id} 已移出列表，残留 {} 删除失败：{e}；下次启动继续清理", trashed.display()));
        }
    });
    Ok(())
}

pub fn purge_trash(data: &Path) -> Result<(), String> {
    let trash = super::trash_root(data);
    let Ok(entries) = std::fs::read_dir(&trash) else { return Ok(()) };
    let errors: Vec<String> = entries.flatten()
        .filter_map(|e| {
            let path = e.path();
            let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            removed.err().map(|err| format!("{}：{err}", path.display()))
        })
        .collect();
    if errors.is_empty() { Ok(()) } else { Err(format!("清理已删除项目的残留失败：{}", errors.join("；"))) }
}

fn count_dirs(root: &Path, keep: impl Fn(&Path) -> bool) -> u32 {
    std::fs::read_dir(root).into_iter().flatten().flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| keep(&e.path()))
        .count() as u32
}

fn count_recipes(root: &Path) -> u32 {
    std::fs::read_dir(root).into_iter().flatten().flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| !p.file_name().unwrap_or_default().to_string_lossy().starts_with('.'))
        .count() as u32
}

fn dir_bytes(root: &Path) -> u64 {
    std::fs::read_dir(root).into_iter().flatten().flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn records(db: &Path) -> Result<(u64, Option<i64>, Option<i64>), String> {
    if !db.is_file() { return Ok((0, None, None)); }
    let fail = |e: rusqlite::Error| format!("读取检测记录库失败：{e}");
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(fail)?;
    conn.busy_timeout(std::time::Duration::from_secs(2)).map_err(fail)?;
    let has_parts: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='parts')", [], |r| r.get(0)).map_err(fail)?;
    if !has_parts { return Ok((0, None, None)); }
    conn.query_row("SELECT COUNT(*), MIN(ts), MAX(ts) FROM parts", [], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))).map_err(fail)
}

pub fn usage(data: &Path, id: &str) -> Result<ProjectUsage, String> {
    if !super::valid_id(id) { return Err(format!("项目编号 {id} 不合法")); }
    let root = super::dir(data, id);
    if !root.is_dir() { return Err(format!("项目 {id} 不存在")); }
    let (records, first_record_at, last_record_at) = records(&root.join("inspection.db"))?;
    Ok(ProjectUsage {
        recipes: count_recipes(&root.join("recipes")),
        workspaces: count_dirs(&root.join("workspaces"), |p| p.file_name().is_some_and(|n| n != "comparisons") && p.join("workspace.json").is_file()),
        capture_rounds: count_dirs(&root.join("recipe-capture"), |p| p.join("round.json").is_file()),
        records,
        first_record_at,
        last_record_at,
        bytes: dir_bytes(&root),
    })
}

fn current(app: &AppHandle) -> Option<tauri::State<'_, ActiveProject>> {
    app.try_state::<ActiveProject>()
}

#[tauri::command]
pub async fn project_create(app: AppHandle, request: CreateProject) -> Result<ProjectMeta, String> {
    let data = crate::storage::root(&app);
    let active = current(&app).map(|a| ActiveProject { id: a.id.clone(), root: a.root.clone() });
    tauri::async_runtime::spawn_blocking(move || create(&data, active.as_ref(), &request)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn project_rename(app: AppHandle, id: String, name: String, description: String) -> Result<ProjectMeta, String> {
    rename(&crate::storage::root(&app), &id, &name, &description)
}

#[tauri::command]
pub async fn project_delete(app: AppHandle, id: String) -> Result<(), String> {
    let active = current(&app).map(|a| a.id.clone());
    delete(&crate::storage::root(&app), active.as_deref(), &id)
}

#[tauri::command]
pub async fn project_usage(app: AppHandle, id: String) -> Result<ProjectUsage, String> {
    let data = crate::storage::root(&app);
    tauri::async_runtime::spawn_blocking(move || usage(&data, &id)).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "manage_tests.rs"]
mod tests;

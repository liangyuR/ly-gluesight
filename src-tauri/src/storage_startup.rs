use std::sync::{Condvar, Mutex};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    request_id: u64,
    title: String,
    message: String,
    kind: String,
}

struct Pending { status: Status, answer: Option<bool> }
pub struct Startup { pending: Mutex<Pending>, ready: Condvar }

impl Default for Startup {
    fn default() -> Self {
        Self { pending: Mutex::new(Pending { status: Status { request_id: 0, title: "正在准备资料目录".into(), message: "检查目录、权限和迁移状态…".into(), kind: "working".into() }, answer: None }), ready: Condvar::new() }
    }
}

pub fn ask(app: &AppHandle, title: &str, message: &str, choose_source: bool) -> Result<bool, String> {
    let host = app.state::<Startup>();
    let mut pending = host.pending.lock().map_err(|e| e.to_string())?;
    pending.status = Status { request_id: pending.status.request_id + 1, title: title.into(), message: message.into(), kind: if choose_source { "chooseSource" } else { "confirm" }.into() };
    pending.answer = None;
    while pending.answer.is_none() { pending = host.ready.wait(pending).map_err(|e| e.to_string())?; }
    let answer = pending.answer.take().unwrap();
    pending.status.kind = "working".into();
    pending.status.title = "正在准备资料".into();
    pending.status.message = "正在复制并检查资料，请勿关闭软件。原目录会保留。".into();
    Ok(answer)
}

pub const CHOOSE_PROJECT: &str = "\u{1}choose-project:";

pub fn report(app: &AppHandle, error: String) {
    match error.strip_prefix(CHOOSE_PROJECT) {
        Some(reason) => choose_project(app, reason),
        None => failed(app, error),
    }
}

fn choose_project(app: &AppHandle, reason: &str) {
    if let Some(host) = app.try_state::<Startup>() {
        if let Ok(mut pending) = host.pending.lock() {
            pending.status.request_id += 1;
            pending.status.kind = "chooseProject".into();
            pending.status.title = "选择要打开的项目".into();
            pending.status.message = format!("{reason}。检测服务、相机与 PLC 尚未启动，选定项目后软件会重启。");
        }
    }
}

fn expect_choosing(host: &Startup) -> Result<(), String> {
    let pending = host.pending.lock().map_err(|e| e.to_string())?;
    if pending.status.kind != "chooseProject" { return Err("当前不在选择项目状态".into()); }
    Ok(())
}

#[tauri::command]
pub fn storage_startup_projects(host: State<'_, Startup>) -> Result<Vec<crate::project::ProjectEntry>, String> {
    expect_choosing(&host)?;
    crate::project::list(&crate::storage::selected_root()?)
}

#[tauri::command]
pub fn storage_startup_open_project(app: AppHandle, host: State<'_, Startup>, id: String) -> Result<(), String> {
    expect_choosing(&host)?;
    let data = crate::storage::selected_root()?;
    let meta = crate::project::open_target(&data, &id)?;
    crate::project::write_pointer(&data, &meta.id)?;
    crate::applog::write("info", "项目选择", &format!("启动时选择项目 {}（{}），重启软件", meta.id, meta.name));
    app.restart()
}

#[tauri::command]
pub fn storage_startup_create_project(app: AppHandle, host: State<'_, Startup>, name: String) -> Result<(), String> {
    expect_choosing(&host)?;
    let data = crate::storage::selected_root()?;
    let meta = crate::project::create_empty(&data, &name)?;
    crate::project::write_pointer(&data, &meta.id)?;
    crate::applog::write("info", "项目选择", &format!("启动时新建项目 {}（{}），重启软件", meta.id, meta.name));
    app.restart()
}

pub fn failed(app: &AppHandle, error: String) {
    if let Some(host) = app.try_state::<Startup>() {
        if let Ok(mut pending) = host.pending.lock() {
            pending.status.request_id += 1;
            pending.status.kind = "error".into();
            pending.status.title = "资料目录未启用".into();
            pending.status.message = error;
        }
    }
}

#[tauri::command]
pub fn storage_startup_status(host: State<'_, Startup>) -> Result<Status, String> {
    Ok(host.pending.lock().map_err(|e| e.to_string())?.status.clone())
}

#[tauri::command]
pub fn storage_startup_answer(app: AppHandle, host: State<'_, Startup>, request_id: u64, accepted: bool) -> Result<(), String> {
    let mut pending = host.pending.lock().map_err(|e| e.to_string())?;
    if pending.status.request_id != request_id { return Err("提示已更新，请重新确认".into()); }
    if pending.status.kind == "error" { app.exit(1); return Ok(()); }
    if !matches!(pending.status.kind.as_str(), "confirm" | "chooseSource") || pending.answer.is_some() { return Err("此提示已处理".into()); }
    pending.answer = Some(accepted);
    host.ready.notify_one();
    Ok(())
}

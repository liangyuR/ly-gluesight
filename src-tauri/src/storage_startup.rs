use std::sync::Mutex;
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

pub struct Startup { pending: Mutex<Status> }

impl Default for Startup {
    fn default() -> Self {
        Self { pending: Mutex::new(Status { request_id: 0, title: "正在准备资料目录".into(), message: "检查目录与权限…".into(), kind: "working".into() }) }
    }
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
            pending.request_id += 1;
            pending.kind = "chooseProject".into();
            pending.title = "选择要打开的项目".into();
            pending.message =
                format!("{reason}。检测服务、相机与 PLC 尚未启动，选定项目后软件会重启。");
        }
    }
}

fn expect_choosing(host: &Startup) -> Result<(), String> {
    let pending = host.pending.lock().map_err(|e| e.to_string())?;
    if pending.kind != "chooseProject" {
        return Err("当前不在选择项目状态".into());
    }
    Ok(())
}

#[tauri::command]
pub fn storage_startup_projects(
    host: State<'_, Startup>,
) -> Result<Vec<crate::project::ProjectEntry>, String> {
    expect_choosing(&host)?;
    crate::project::list(&crate::storage::selected_root()?)
}

#[tauri::command]
pub fn storage_startup_open_project(
    app: AppHandle,
    host: State<'_, Startup>,
    id: String,
) -> Result<(), String> {
    expect_choosing(&host)?;
    let data = crate::storage::selected_root()?;
    let meta = crate::project::open_target(&data, &id)?;
    crate::project::write_pointer(&data, &meta.id)?;
    crate::applog::write(
        "info",
        "项目选择",
        &format!("启动时选择项目 {}（{}），重启软件", meta.id, meta.name),
    );
    crate::instance::restart(&app)
}

#[tauri::command]
pub fn storage_startup_create_project(
    app: AppHandle,
    host: State<'_, Startup>,
    name: String,
) -> Result<(), String> {
    expect_choosing(&host)?;
    let data = crate::storage::selected_root()?;
    let request = crate::project::manage::CreateProject {
        id: crate::project::next_id(&data),
        name,
        description: String::new(),
        copy_hardware: false,
    };
    let meta = crate::project::manage::create(&data, None, &request)?;
    crate::project::write_pointer(&data, &meta.id)?;
    crate::applog::write(
        "info",
        "项目选择",
        &format!("启动时新建项目 {}（{}），重启软件", meta.id, meta.name),
    );
    crate::instance::restart(&app)
}

pub fn failed(app: &AppHandle, error: String) {
    if let Some(host) = app.try_state::<Startup>() {
        if let Ok(mut pending) = host.pending.lock() {
            pending.request_id += 1;
            pending.kind = "error".into();
            pending.title = "资料目录未启用".into();
            pending.message = error;
        }
    }
}

#[tauri::command]
pub fn storage_startup_status(host: State<'_, Startup>) -> Result<Status, String> {
    Ok(host.pending.lock().map_err(|e| e.to_string())?.clone())
}

/// 错误页的“关闭”：退出软件。
#[tauri::command]
pub fn storage_startup_answer(app: AppHandle, host: State<'_, Startup>, request_id: u64) -> Result<(), String> {
    let pending = host.pending.lock().map_err(|e| e.to_string())?;
    if pending.request_id != request_id { return Err("提示已更新，请重新确认".into()); }
    if pending.kind != "error" { return Err("此提示已处理".into()); }
    app.exit(1);
    Ok(())
}

use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    request_id: u64,
    title: String,
    message: String,
    kind: String,
    stage: &'static str,
    step: usize,
    steps: usize,
    version: String,
}

pub struct Startup {
    pending: Mutex<Status>,
}

impl Default for Startup {
    fn default() -> Self {
        Self {
            pending: Mutex::new(Status {
                request_id: 0,
                title: "正在准备资料目录".into(),
                message: "检查目录与权限…".into(),
                kind: "working".into(),
                stage: STAGES[0],
                step: 0,
                steps: STAGES.len(),
                version: String::new(),
            }),
        }
    }
}

pub const CHOOSE_PROJECT: &str = "\u{1}choose-project:";

pub const WINDOW: &str = "storage-startup";

/// 启动画面上的阶段，依次对应启动时耗时的几步。
pub const STAGES: [&str; 6] = [
    "Preparing storage",
    "Opening project",
    "Initializing cameras and inspection",
    "Loading recipe workspace",
    "Initializing PLC",
    "Loading interface",
];

pub fn stage(app: &AppHandle, step: usize) {
    if let Some(host) = app.try_state::<Startup>() {
        if let Ok(mut pending) = host.pending.lock() {
            if pending.kind == "working" {
                pending.step = step;
                pending.stage = STAGES[step];
            }
        }
    }
}

pub fn report(app: &AppHandle, error: String) {
    match error.strip_prefix(CHOOSE_PROJECT) {
        Some(reason) => choose_project(app, reason),
        None => failed(app, error),
    }
    if let Some(window) = app.get_webview_window(WINDOW) {
        // 启动画面是无边框小窗；出错或要选项目时放大并恢复系统标题栏，便于阅读、移动和关闭
        let _ = window.set_decorations(true);
        let _ = window.set_size(tauri::LogicalSize::new(720.0, 480.0));
        let _ = window.center();
    }
}

/// 主窗口首屏渲染完成（或等待超时）：显示主窗口并关闭启动画面；配置为隐藏的窗口（测试实例）保持隐藏。
pub fn ready(app: &AppHandle) {
    for config in &app.config().app.windows {
        let Some(window) = app.get_webview_window(&config.label) else {
            continue;
        };
        if config.visible && !window.is_visible().unwrap_or(true) {
            let _ = window.show();
            if config.maximized {
                let _ = window.maximize();
            }
            let _ = window.set_focus();
        }
    }
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.close();
    }
}

#[tauri::command]
pub fn startup_ready(app: AppHandle) {
    ready(&app);
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
pub fn storage_startup_status(app: AppHandle, host: State<'_, Startup>) -> Result<Status, String> {
    let mut status = host.pending.lock().map_err(|e| e.to_string())?.clone();
    status.version = app.package_info().version.to_string();
    Ok(status)
}

/// 错误页的“关闭”：退出软件。
#[tauri::command]
pub fn storage_startup_answer(
    app: AppHandle,
    host: State<'_, Startup>,
    request_id: u64,
) -> Result<(), String> {
    let pending = host.pending.lock().map_err(|e| e.to_string())?;
    if pending.request_id != request_id {
        return Err("提示已更新，请重新确认".into());
    }
    if pending.kind != "error" {
        return Err("此提示已处理".into());
    }
    app.exit(1);
    Ok(())
}

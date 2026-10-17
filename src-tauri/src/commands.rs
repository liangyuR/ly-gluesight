use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::vision::VisionHost;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    name: String,
    version: String,
    log_dir: Option<String>,
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    let pkg = app.package_info();
    AppInfo {
        name: pkg.name.clone(),
        version: pkg.version.to_string(),
        log_dir: crate::applog::dir(&app).ok().map(|dir| dir.display().to_string()),
    }
}

#[tauri::command]
pub fn app_open_log_dir(app: AppHandle) -> Result<(), String> {
    let dir = crate::applog::dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    std::process::Command::new("explorer").arg(&dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    backend: &'static str,
    ready: bool,
    message: String,
    version: Option<String>,
    path: Option<String>,
    measuring: bool,
}

#[tauri::command]
pub async fn engine_status(app: AppHandle) -> EngineStatus {
    tauri::async_runtime::spawn_blocking(move || {
        if !crate::site::image_measurement(&app) {
            return EngineStatus {
                backend: "模拟测量",
                ready: true,
                message: "离线·合成图：模拟测量".into(),
                version: None,
                path: None,
                measuring: false,
            };
        }
        let s = app.state::<VisionHost>().status();
        EngineStatus { backend: "LyFlow", ready: s.loaded, message: format!("飞拍：lyFlow {}", s.message), version: s.version, path: s.path, measuring: s.loaded }
    })
    .await
    .unwrap_or(EngineStatus { backend: "LyFlow", ready: false, message: "查询失败".into(), version: None, path: None, measuring: false })
}

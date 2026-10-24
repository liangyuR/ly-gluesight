use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::vision::VisionHost;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    name: String,
    version: String,
    git_sha: &'static str,
    build_time: String,
    log_dir: Option<String>,
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    let pkg = app.package_info();
    let identity = crate::applog::build_identity();
    AppInfo {
        name: pkg.name.clone(),
        version: pkg.version.to_string(),
        git_sha: identity.git_sha,
        build_time: identity.build_time,
        log_dir: crate::applog::dir(&app).ok().map(|dir| dir.display().to_string()),
    }
}

/// PLC 连接、相机组与检测设置的当前值：运行日志 app.config 的 data，诊断包 manifest 的 configSummary。
pub fn config_summary(app: &AppHandle) -> serde_json::Value {
    let cycle = app.state::<crate::cycle::CycleHost>();
    let connection = app
        .try_state::<crate::plc::PlcHost>()
        .map(|plc| plc.engine().config().connection);
    serde_json::json!({
        "offline": crate::offline::enabled(app),
        "plc": connection,
        "cameras": cycle.camera.configs(),
        "settings": cycle.settings(),
    })
}

/// 运行日志 app.config：启动完成与相关配置保存后各记一次。
pub fn log_config(app: &AppHandle) {
    let data = config_summary(app);
    let plc_text = match (data["plc"]["host"].as_str(), data["plc"]["port"].as_u64()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        _ => "未初始化".to_string(),
    };
    let cameras = data["cameras"].as_array().map_or(0, Vec::len);
    crate::applog::emit_event(
        &crate::applog::codes::APP_CONFIG,
        &format!("PLC {plc_text}，相机 {cameras} 台"),
        crate::applog::Fields::default().with_data(data),
    );
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

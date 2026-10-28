pub(crate) mod relocate;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, Manager};

pub struct StoragePaths {
    root: PathBuf,
    status: StorageStatus,
    preferences: Mutex<()>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    environment: String,
    root: String,
    log_root: Option<String>,
    runtime_log_dir: Option<String>,
    vision_log_dir: Option<String>,
}

#[cfg(windows)]
fn program_data() -> Result<PathBuf, String> {
    #[repr(C)]
    struct Guid {
        a: u32,
        b: u16,
        c: u16,
        d: [u8; 8],
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            id: *const Guid,
            flags: u32,
            token: isize,
            path: *mut *mut u16,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(ptr: *mut std::ffi::c_void);
    }
    let id = Guid {
        a: 0x62ab5d82,
        b: 0xfdc1,
        c: 0x4dc3,
        d: [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
    };
    let mut ptr = std::ptr::null_mut();
    unsafe {
        if SHGetKnownFolderPath(&id, 0, 0, &mut ptr) < 0 {
            return Err("无法读取 Windows ProgramData 目录".into());
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(
            ptr, len,
        )));
        CoTaskMemFree(ptr.cast());
        Ok(path)
    }
}
#[cfg(not(windows))]
fn program_data() -> Result<PathBuf, String> {
    Err("GlueSight 资料目录仅支持 Windows；测试请显式指定 GLUESIGHT_DATA_DIR".into())
}

pub fn selected_root() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("GLUESIGHT_DATA_DIR") {
        if !cfg!(debug_assertions) {
            return Err("安装版不允许 GLUESIGHT_DATA_DIR 覆盖资料目录".into());
        }
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("GLUESIGHT_DATA_DIR 必须是绝对路径".into());
        }
        return Ok(path);
    }
    Ok(program_data()?.join("ly").join(if cfg!(debug_assertions) {
        "GlueSight-dev"
    } else {
        "GlueSight"
    }))
}
fn user_key() -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("whoami")
            .creation_flags(0x08000000)
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&output.stdout);
        if output.status.success() {
            if let Some(sid) = text.split('"').find(|v| {
                v.starts_with("S-1-")
                    && v.chars()
                        .all(|c| c == 'S' || c == '-' || c.is_ascii_digit())
            }) {
                return Ok(sid.into());
            }
        }
        Err("无法识别当前 Windows 用户 SID".into())
    }
    #[cfg(not(windows))]
    {
        Ok("test-user".into())
    }
}
pub fn root(app: &AppHandle) -> PathBuf {
    app.state::<StoragePaths>().root.clone()
}
pub fn config(app: &AppHandle) -> PathBuf {
    root(app).join("config")
}
pub fn project(app: &AppHandle) -> PathBuf {
    app.state::<crate::project::ActiveProject>().root.clone()
}
pub fn project_config(app: &AppHandle) -> PathBuf {
    project(app).join("config")
}
pub fn state(app: &AppHandle) -> PathBuf {
    root(app).join("state")
}
pub fn webview(root: &Path) -> Result<PathBuf, String> {
    Ok(root.join("webview").join(user_key()?))
}
pub fn bootstrap_root(root: &Path) -> PathBuf {
    root.join("migration").join("bootstrap")
}
pub fn initialized(root: &Path) -> bool {
    root.join("storage-layout.json").is_file()
}

const MAIN_PROFILE_MAX: usize = 247 - r"\EBWebView\Default".len();
const STARTUP_PROFILE_MAX: usize = 247 - r"\EBWebView".len();

fn check_webview_profiles(root: &Path, user: &str, startup_window: bool) -> Result<(), String> {
    let main = ("主窗口", root.join("webview").join(user), MAIN_PROFILE_MAX);
    let startup = ("资料准备窗口", bootstrap_root(root).join("webview").join(user), STARTUP_PROFILE_MAX);
    for (window, profile, max) in std::iter::once(main).chain(startup_window.then_some(startup)) {
        let length = profile.to_string_lossy().encode_utf16().count();
        if length > max {
            return Err(format!(
                "资料目录路径过长：{window}的 WebView2 资料目录 {} 共 {length} 个字符，超过上限 {max}，WebView2 无法创建{window}。请把资料目录 {} 移到更短的路径。",
                profile.display(),
                root.display()
            ));
        }
    }
    Ok(())
}

fn environment() -> &'static str {
    if std::env::var_os("GLUESIGHT_DATA_DIR").is_some() {
        "test"
    } else if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
    }
}
fn reject_link_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if !ancestor.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(format!("资料路径包含链接：{}", ancestor.display()));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(format!("资料路径包含链接：{}", ancestor.display()));
        }
    }
    Ok(())
}
pub fn prepare(app: &AppHandle, target: &Path) -> Result<(), String> {
    if std::env::var_os("GLUESIGHT_DATA_DIR").is_some() {
        let identifier = &app.config().identifier;
        if !identifier.starts_with("com.xyzrobotics.tujiaovision.")
            || !identifier.ends_with("-tests")
            || target.file_name().and_then(|s| s.to_str()) != Some(identifier.as_str())
        {
            return Err(
                "GLUESIGHT_DATA_DIR 只允许隔离测试标识，且末级目录须与测试标识完全一致".into(),
            );
        }
    }
    reject_link_ancestors(&target)?;
    check_webview_profiles(target, &user_key()?, !initialized(target))?;
    if std::env::var_os("GLUESIGHT_DATA_DIR").is_none() && !target.is_dir() {
        return Err(format!(
            "资料目录尚未初始化：{}。开发版请运行 pnpm storage:init，安装版请重新运行安装器。",
            target.display()
        ));
    }
    fs::create_dir_all(&target).map_err(|e| {
        format!(
            "无法访问资料目录 {}：{e}。请执行资料目录初始化。",
            target.display()
        )
    })?;
    app.manage(crate::instance::acquire(target)?);
    Ok(())
}

/// 资料目录初始化：已有 `storage-layout.json` 时核对版本与环境；没有时目录里除实例锁和资料准备窗口的 WebView 配置
/// （`migration/bootstrap`）外须为空，按空白资料初始化。不从任何旧目录复制资料。
pub fn initialize(app: &AppHandle, target: PathBuf) -> Result<(), String> {
    let marker = target.join("storage-layout.json");
    if marker.exists() {
        let value: Value =
            serde_json::from_str(&crate::fsio::read_text(&marker).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if value["version"] != 1 {
            return Err("不支持的资料目录版本".into());
        }
        if value["environment"] != environment() {
            return Err("资料目录环境与当前程序不一致，拒绝混用生产、开发和测试资料".into());
        }
    } else {
        let entries = fs::read_dir(&target)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != ".gluesight-instance.lock" && e.file_name() != "migration")
            .count();
        if entries != 0 {
            return Err(format!(
                "资料目录没有初始化标记却已有其它内容，请保留目录并人工检查：{}",
                target.display()
            ));
        }
        crate::fsio::write_atomic(
            &marker,
            &json!({"version":1,"environment":environment()}).to_string(),
        )?;
    }
    for dir in ["config", "state", "samples", "cache", "logs", "exports"] {
        fs::create_dir_all(target.join(dir)).map_err(|e| e.to_string())?;
    }
    let status = StorageStatus {
        environment: environment().to_string(),
        root: target.to_string_lossy().into_owned(),
        log_root: None,
        runtime_log_dir: None,
        vision_log_dir: None,
    };
    app.manage(StoragePaths {
        root: target,
        status,
        preferences: Mutex::new(()),
    });
    Ok(())
}
#[tauri::command]
pub fn storage_status(app: AppHandle) -> Result<StorageStatus, String> {
    let mut status = app
        .try_state::<StoragePaths>()
        .map(|state| state.status.clone())
        .ok_or("资料尚未准备完成")?;
    if let Some(layout) = app.try_state::<crate::log_storage::Layout>() {
        status.log_root = Some(layout.config.root.display().to_string());
        status.runtime_log_dir = Some(layout.runtime().display().to_string());
        if let Some(project) = app.try_state::<crate::project::ActiveProject>() {
            status.vision_log_dir = Some(layout.vision(&project.id).display().to_string());
        }
    }
    Ok(status)
}
#[tauri::command]
pub fn storage_open_directory(app: AppHandle, target: Option<String>) -> Result<(), String> {
    let directory = match target.as_deref().unwrap_or("data") {
        "data" => root(&app),
        "logs" => app
            .try_state::<crate::log_storage::Layout>()
            .ok_or("日志目录尚未准备完成")?
            .config
            .root
            .clone(),
        "runtime" => crate::log_storage::runtime_dir(&app)?,
        "vision" => crate::log_storage::recording_project(&project(&app))?,
        "lyflow" => {
            let directory = crate::vision::runtime_directory()?;
            if !directory.is_dir() {
                return Err("lyFlow 运行库目录不存在".into());
            }
            directory
        }
        _ => return Err("未知目录类型".into()),
    };
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    std::process::Command::new("explorer.exe")
        .arg(directory)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}
const LAST_WORKSPACE_BY_PROJECT: &str = "lastWorkspaceByProject";
fn project_id(app: &AppHandle) -> String {
    app.state::<crate::project::ActiveProject>().id.clone()
}
fn preference_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config(app).join(format!("preferences-{}.json", user_key()?)))
}
fn read_object(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let value: Value =
        serde_json::from_str(&crate::fsio::read_text(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if !value.is_object() {
        return Err("参数文件必须是对象".into());
    }
    Ok(value)
}
fn feasibility_file(app: &AppHandle) -> PathBuf {
    project_config(app).join("feasibility.json")
}
fn preferences(app: &AppHandle) -> Result<Value, String> {
    let mut value = read_object(&preference_file(app)?)?;
    let object = value.as_object_mut().unwrap();
    let last = object
        .remove(LAST_WORKSPACE_BY_PROJECT)
        .and_then(|mut map| map.get_mut(project_id(app).as_str()).map(Value::take));
    object.remove("lastWorkspace");
    if let Some(last) = last {
        object.insert("lastWorkspace".into(), last);
    }
    let shared = read_object(&feasibility_file(app))?;
    object.extend(shared.as_object().unwrap().clone());
    Ok(value)
}
#[tauri::command]
pub fn storage_preferences_get(app: AppHandle) -> Result<Value, String> {
    let paths = app.state::<StoragePaths>();
    let _guard = paths.preferences.lock().map_err(|e| e.to_string())?;
    preferences(&app)
}
#[tauri::command]
pub fn storage_preferences_update(app: AppHandle, patch: Value) -> Result<Value, String> {
    let paths = app.state::<StoragePaths>();
    let _guard = paths.preferences.lock().map_err(|e| e.to_string())?;
    let patch = patch.as_object().ok_or("参数补丁必须是对象")?;
    let file = preference_file(&app)?;
    let shared_file = feasibility_file(&app);
    let project = project_id(&app);
    let mut user = read_object(&file)?;
    user.as_object_mut().unwrap().remove("lastWorkspace");
    let mut shared = read_object(&shared_file)?;
    for (key, value) in patch {
        let target = match key.as_str() {
            "flyshotSpeedMmS" | "flyshotSpacingMm" => {
                if !value.is_null() && !value.as_f64().is_some_and(|n| n.is_finite() && n > 0.0) {
                    return Err("飞拍参数必须为正数".into());
                }
                &mut shared
            }
            "lastWorkspace" => {
                if !value.is_null() && !value.is_string() {
                    return Err("工作区标识必须是字符串".into());
                }
                let map = user
                    .as_object_mut()
                    .unwrap()
                    .entry(LAST_WORKSPACE_BY_PROJECT)
                    .or_insert_with(|| json!({}));
                if !map.is_object() {
                    *map = json!({});
                }
                if value.is_null() {
                    map.as_object_mut().unwrap().remove(&project);
                } else {
                    map[&project] = value.clone();
                }
                continue;
            }
            _ => return Err(format!("未知偏好字段：{key}")),
        };
        if value.is_null() {
            target.as_object_mut().unwrap().remove(key);
        } else {
            target[key] = value.clone();
        }
    }
    crate::fsio::write_atomic(&shared_file, &shared.to_string())?;
    crate::fsio::write_atomic(&file, &user.to_string())?;
    preferences(&app)
}

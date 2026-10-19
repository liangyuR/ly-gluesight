mod relocate;
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
    migrated_this_launch: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    environment: String,
    root: String,
    migration_status: String,
    migration_message: Option<String>,
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
fn mapped_relative(path: &Path) -> PathBuf {
    let name = path.to_string_lossy();
    match name.as_ref() {
        "cameras.json" | "cycle.json" | "plc.json" | "site-offline.json" => {
            Path::new("config").join(path)
        }
        "plc-handshake.json" | "plc-handshake.audit.jsonl" => Path::new("state").join(path),
        _ => path.to_path_buf(),
    }
}
fn copy_tree(
    source: &Path,
    destination: &Path,
    old_root: &Path,
    new_root: &Path,
    count: &mut u64,
) -> Result<(), String> {
    let profile = source
        .components()
        .any(|c| matches!(c.as_os_str().to_str(), Some("webview" | "EBWebView")));
    let metadata = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!("资料包含链接，迁移已停止：{}", source.display()));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(format!("资料包含链接：{}", source.display()));
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name_text = name.to_string_lossy();
            if !profile
                && (name_text == ".gluesight-instance.lock"
                    || name_text.ends_with("-wal")
                    || name_text.ends_with("-shm"))
            {
                continue;
            }
            copy_tree(
                &entry.path(),
                &destination.join(name),
                old_root,
                new_root,
                count,
            )?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if !profile && source.starts_with(old_root) && source.extension().is_some_and(|e| e == "db")
        {
            let conn = rusqlite::Connection::open_with_flags(
                source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| e.to_string())?;
            conn.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
                .map_err(|e| e.to_string())?;
            relocate::database(destination, old_root, new_root)?;
        } else {
            let copied = fs::copy(source, destination).map_err(|e| e.to_string())?;
            if copied != metadata.len() {
                return Err(format!("复制长度不一致：{}", source.display()));
            }
            if !profile
                && source.starts_with(old_root)
                && source.extension().is_some_and(|e| e == "json")
            {
                relocate::json_file(
                    destination,
                    source.strip_prefix(old_root).map_err(|e| e.to_string())?,
                    old_root,
                    new_root,
                )?;
            }
        }
        *count += 1;
    }
    Ok(())
}
fn directory_bytes(path: &Path) -> Result<u64, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!("资料包含链接：{}", path.display()));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err("不能迁移链接目录".into());
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut bytes = 0u64;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        bytes = bytes
            .checked_add(directory_bytes(&entry.map_err(|e| e.to_string())?.path())?)
            .ok_or("资料大小溢出")?;
    }
    Ok(bytes)
}
fn check_space(root: &Path, bytes: u64) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                path: *const u16,
                available: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        let path: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available = 0;
        if unsafe {
            GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err("无法检查资料目录可用空间".into());
        }
        if available < bytes.saturating_add(64 * 1024 * 1024) {
            return Err(format!(
                "迁移空间不足：需要至少 {} 字节，可用 {available} 字节",
                bytes.saturating_add(64 * 1024 * 1024)
            ));
        }
    }
    Ok(())
}
pub fn promote(root: &Path) -> Result<(), String> {
    let journal = root.join("migration/commit.json");
    let value: Value =
        serde_json::from_str(&crate::fsio::read_text(&journal).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    for name in value["entries"].as_array().ok_or("迁移提交清单无效")? {
        let name = name.as_str().ok_or("迁移条目无效")?;
        if Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err("迁移条目越界".into());
        }
        let staged = root.join("migration/staging").join(name);
        let final_path = root.join(name);
        if staged.exists() {
            if final_path.exists() {
                return Err(format!("迁移目标冲突：{}", final_path.display()));
            }
            fs::rename(staged, final_path).map_err(|e| e.to_string())?;
        } else if !final_path.exists() {
            return Err(format!("迁移条目丢失：{name}"));
        }
    }
    crate::fsio::write_atomic(&root.join("migration/report.json"),&json!({"success":true,"source":value["source"],"files":value["files"],"sourcePreserved":true,"warnings":value["warnings"]}).to_string())?;
    Ok(())
}

pub fn copy_legacy_data(source: &Path, staging: &Path, target: &Path) -> Result<u64, String> {
    let mut count = 0;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let text = name.to_string_lossy();
        if matches!(
            text.as_ref(),
            ".gluesight-instance.lock" | "storage-layout.json" | "migration"
        ) || text.ends_with("-wal")
            || text.ends_with("-shm")
        {
            continue;
        }
        copy_tree(
            &entry.path(),
            &staging.join(mapped_relative(Path::new(&name))),
            source,
            target,
            &mut count,
        )?;
    }
    Ok(count)
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

pub fn initialize(app: &AppHandle, target: PathBuf) -> Result<(), String> {
    let marker = target.join("storage-layout.json");
    let mut message = None;
    if !marker.exists() && target.join("migration/commit.json").is_file() {
        promote(&target)?;
        message =
            Some("已恢复上次中断的迁移；本次启动不自动连接设备，请检查资料后手动连接。".into());
        crate::fsio::write_atomic(
            &marker,
            &json!({"version":1,"environment":environment()}).to_string(),
        )?;
    }
    if !marker.exists() {
        let legacy = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let source = std::env::var_os("GLUESIGHT_MIGRATE_FROM")
            .map(PathBuf::from)
            .or_else(|| {
                (cfg!(debug_assertions)
                    && std::env::var_os("GLUESIGHT_DATA_DIR").is_none()
                    && legacy.exists())
                .then_some(legacy.clone())
            });
        let source = if source.is_none() && !cfg!(debug_assertions) {
            let choose = crate::storage_startup::ask(app, "GlueSight 首次启动", "是否选择现有资料目录复制迁移？空白开始会初始化新资料，开发资料不会自动同步。", true)?;
            if choose {
                let window = app.get_webview_window("storage-startup").ok_or("启动窗口已关闭")?;
                Some(
                    rfd::FileDialog::new().set_parent(&window)
                        .set_title("选择现有 GlueSight 资料目录")
                        .set_directory(&legacy)
                        .pick_folder()
                        .ok_or("未选择迁移来源，启动已取消")?,
                )
            } else {
                None
            }
        } else {
            source
        };
        let entries = fs::read_dir(&target)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != ".gluesight-instance.lock" && e.file_name() != "migration")
            .count();
        if entries != 0 {
            return Err(format!(
                "资料目录未完成初始化或已有冲突内容，请保留目录并检查迁移报告：{}",
                target.display()
            ));
        }
        if source.is_none() && target.join("migration/report.json").exists() {
            return Err(
                "存在未完成迁移，请重新选择原资料目录继续迁移；不能用空白初始化跳过失败资料".into(),
            );
        }
        if let Some(source) = source {
            if source.starts_with(&target) || target.starts_with(&source) || !source.is_dir() {
                return Err("迁移来源必须是另一个有效资料目录".into());
            }
            reject_link_ancestors(&source)?;
            let _source_lease = crate::instance::acquire(&source)?;
            let required = directory_bytes(&source)?;
            check_space(&target, required)?;
            let approved = crate::storage_startup::ask(app, "迁移 GlueSight 资料", &format!("来源：{}\n目标：{}\n资料大小：{required} 字节\n将复制资料并保留旧目录。失败后可重启重试。", source.display(), target.display()), false)?;
            if !approved {
                return Err("资料迁移已取消；旧资料未修改".into());
            }
            let staging = target.join("migration").join("staging");
            if staging.exists() {
                fs::rename(
                    &staging,
                    target
                        .join("migration")
                        .join(format!("incomplete-{}", ly_plc::now_ms())),
                )
                .map_err(|e| e.to_string())?;
            }
            fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
            let mut count = 0;
            let mut warnings = Vec::new();
            let result = (|| {
                count = copy_legacy_data(&source, &staging, &target)?;
                let legacy_webview = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
                if source == legacy
                    && legacy_webview != source
                    && legacy_webview.join("EBWebView").exists()
                {
                    let profile = webview(&staging)?;
                    fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
                    check_space(&target, directory_bytes(&legacy_webview.join("EBWebView"))?)?;
                    copy_tree(
                        &legacy_webview.join("EBWebView"),
                        &profile.join("EBWebView"),
                        &source,
                        &target,
                        &mut count,
                    )?;
                }
                let choice = staging.join("config/site-offline.json");
                if choice.exists() {
                    let mut value: Value = serde_json::from_str(
                        &crate::fsio::read_text(&choice).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                    if let Some(dir) = value.get("sampleSetDir").and_then(Value::as_str) {
                        let previous = PathBuf::from(dir);
                        let source_dir = previous
                            .strip_prefix(&target)
                            .map(|p| staging.join(p))
                            .unwrap_or_else(|_| previous.clone());
                        if !source_dir.starts_with(&staging) {
                            check_space(&target, directory_bytes(&source_dir)?)?;
                        }
                        let imported = crate::managed_samples::import(&staging, &source_dir)?;
                        let final_sample = target.join(
                            imported
                                .strip_prefix(staging.canonicalize().map_err(|e| e.to_string())?)
                                .map_err(|e| e.to_string())?,
                        );
                        relocate::tree_references(&staging, &previous, &final_sample)?;
                        value["sampleSetDir"] = Value::String(
                            target
                                .join(
                                    imported
                                        .strip_prefix(
                                            staging.canonicalize().map_err(|e| e.to_string())?,
                                        )
                                        .map_err(|e| e.to_string())?,
                                )
                                .to_string_lossy()
                                .into_owned(),
                        );
                        fs::write(
                            &choice,
                            serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
                warnings = relocate::validate_references(&staging, &source, &target)?;
                let entries = fs::read_dir(&staging)
                    .map_err(|e| e.to_string())?
                    .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                crate::fsio::write_atomic(
                    &target.join("migration/commit.json"),
                    &json!({"entries":entries,"source":source,"files":count,"warnings":warnings})
                        .to_string(),
                )?;
                promote(&target)?;
                Ok::<(), String>(())
            })();
            let report = json!({"source":source,"target":target,"files":count,"success":result.is_ok(),"error":result.as_ref().err(),"sourcePreserved":true,"warnings":warnings});
            crate::fsio::write_atomic(&target.join("migration/report.json"), &report.to_string())?;
            result?;
            message=Some(format!("已复制 {count} 个文件，旧目录保留：{}；首次迁移启动未自动连接相机及 PLC，请检查资料后手动连接。",source.display()));
        }
        crate::fsio::write_atomic(
            &marker,
            &json!({"version":1,"environment":environment()}).to_string(),
        )?;
    } else {
        let value: Value =
            serde_json::from_str(&crate::fsio::read_text(&marker).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if value["version"] != 1 {
            return Err("不支持的资料目录版本".into());
        }
        if value["environment"] != environment() {
            return Err("资料目录环境与当前程序不一致，拒绝混用生产、开发和测试资料".into());
        }
    }
    for dir in ["config", "state", "samples", "cache", "logs", "exports"] {
        fs::create_dir_all(target.join(dir)).map_err(|e| e.to_string())?;
    }
    let migrated_this_launch = message.is_some();
    if message.is_none() && target.join("migration/report.json").is_file() {
        let report: Value = serde_json::from_str(
            &crate::fsio::read_text(&target.join("migration/report.json"))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        message = Some(if report["success"] == true {
            format!(
                "资料迁移已完成；旧来源保留：{}",
                report["source"].as_str().unwrap_or("查看迁移报告")
            )
        } else {
            "上次迁移失败，原资料保留；请查看 migration/report.json。".into()
        });
    }
    let environment = environment().to_string();
    let status = StorageStatus {
        environment,
        root: target.to_string_lossy().into_owned(),
        migration_status: if target.join("migration/commit.json").exists() {
            "migrated"
        } else if target.join("migration/report.json").exists() {
            "failed"
        } else {
            "ready"
        }
        .into(),
        migration_message: message,
    };
    app.manage(StoragePaths {
        root: target,
        status,
        preferences: Mutex::new(()),
        migrated_this_launch,
    });

    Ok(())
}
pub fn migrated_this_launch(app: &AppHandle) -> bool {
    app.state::<StoragePaths>().migrated_this_launch
}
#[tauri::command]
pub fn storage_status(app: AppHandle) -> Result<StorageStatus, String> {
    app.try_state::<StoragePaths>().map(|state| state.status.clone()).ok_or_else(|| "资料尚未准备完成".into())
}
#[tauri::command]
pub fn storage_open_directory(app: AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    std::process::Command::new("explorer.exe")
        .arg(root(&app))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
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
fn preferences(app: &AppHandle) -> Result<Value, String> {
    let mut value = read_object(&preference_file(app)?)?;
    let shared = read_object(&config(app).join("feasibility.json"))?;
    value
        .as_object_mut()
        .unwrap()
        .extend(shared.as_object().unwrap().clone());
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
    let shared_file = config(&app).join("feasibility.json");
    let mut user = read_object(&file)?;
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
                &mut user
            }
            "legacyImported" => {
                if !value.is_boolean() {
                    return Err("迁移标记必须是布尔值".into());
                }
                &mut user
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

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "gluesight-storage-{}-{}-{}",
                std::process::id(),
                ly_plc::now_ms(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn wal_history_and_teaching_references_are_copied_without_changing_source() {
        let test = Fixture::new();
        let source = test.0.join("source");
        let staging = test.0.join("staging");
        let target = test.0.join("target");
        fs::create_dir_all(source.join("workspaces/01")).unwrap();
        let conn = rusqlite::Connection::open(source.join("inspection.db")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE parts(id INTEGER PRIMARY KEY,recording_directory TEXT,frames TEXT,sample TEXT);").unwrap();
        conn.execute(
            "INSERT INTO parts VALUES(1,?1,'[]',NULL)",
            [source.join("records/c1").to_string_lossy().as_ref()],
        )
        .unwrap();
        let teaching = json!({"frames":[{"image":{"calibTag":{"path":source.join("calib/cam2.json"),"value":{}}}}],"note":source.join("notes")});
        let original = serde_json::to_vec(&teaching).unwrap();
        fs::write(source.join("workspaces/01/workspace.json"), &original).unwrap();
        copy_legacy_data(&source, &staging, &target).unwrap();
        let copied = rusqlite::Connection::open(staging.join("inspection.db")).unwrap();
        let dir: String = copied
            .query_row(
                "SELECT recording_directory FROM parts WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(Path::new(&dir), target.join("records/c1"));
        let doc: Value = serde_json::from_slice(
            &fs::read(staging.join("workspaces/01/workspace.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            doc["frames"][0]["image"]["calibTag"]["path"],
            json!(target.join("calib/cam2.json"))
        );
        assert_eq!(doc["note"], teaching["note"]);
        assert_eq!(
            fs::read(source.join("workspaces/01/workspace.json")).unwrap(),
            original
        );
    }
    #[test]
    fn release_resources_keep_exact_bytes_during_directory_migration() {
        let test = Fixture::new();
        let source = test.0.join("source");
        let staging = test.0.join("staging");
        fs::create_dir_all(source.join("vision/releases/r/b")).unwrap();
        let bytes = b"{  \"path\": \"relative/image.png\" }\n";
        fs::write(source.join("vision/releases/r/b/recipe.json"), bytes).unwrap();
        copy_legacy_data(&source, &staging, &test.0.join("target")).unwrap();
        assert_eq!(
            fs::read(staging.join("vision/releases/r/b/recipe.json")).unwrap(),
            bytes
        );
    }
    #[test]
    fn webview_profile_beyond_measured_limit_rejects_data_directory() {
        let user = "S-1-5-21-1-2-3-1001";
        let units = |path: &Path| path.to_string_lossy().encode_utf16().count();
        let probe = Path::new("x");
        for (profile_length, startup_window, accepted) in [
            (229, false, true),
            (230, false, false),
            (237, true, true),
            (238, true, false),
        ] {
            let base = if startup_window { bootstrap_root(probe) } else { probe.to_path_buf() };
            let suffix = units(&base.join("webview").join(user)) - units(probe);
            let root = PathBuf::from("资".repeat(profile_length - suffix));
            assert_eq!(
                check_webview_profiles(&root, user, startup_window).is_ok(),
                accepted,
                "WebView2 资料目录 {profile_length} 个字符，需建资料准备窗口：{startup_window}"
            );
        }
    }
    #[test]
    fn interrupted_commit_resumes_existing_entries_without_overwriting() {
        let test = Fixture::new();
        fs::create_dir_all(test.0.join("migration/staging/config")).unwrap();
        fs::create_dir_all(test.0.join("workspaces")).unwrap();
        fs::write(test.0.join("workspaces/preserved"), "draft").unwrap();
        fs::write(test.0.join("migration/staging/config/cycle.json"), "{}").unwrap();
        fs::write(
            test.0.join("migration/commit.json"),
            json!({"entries":["workspaces","config"],"source":"old","files":2}).to_string(),
        )
        .unwrap();
        promote(&test.0).unwrap();
        promote(&test.0).unwrap();
        assert_eq!(
            fs::read_to_string(test.0.join("workspaces/preserved")).unwrap(),
            "draft"
        );
        assert!(test.0.join("config/cycle.json").exists());
        fs::create_dir_all(test.0.join("migration/staging/config")).unwrap();
        fs::write(
            test.0.join("migration/staging/config/cycle.json"),
            "overwrite",
        )
        .unwrap();
        assert!(promote(&test.0).is_err());
        assert_eq!(
            fs::read_to_string(test.0.join("config/cycle.json")).unwrap(),
            "{}"
        );
    }
}

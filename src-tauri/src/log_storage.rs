use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    schema: u32,
    pub root: PathBuf,
    #[serde(rename = "ownerData")]
    owner_data: PathBuf,
    environment: String,
    instance: String,
}

pub struct Layout {
    pub config: Config,
    _lease: crate::instance::InstanceLease,
}

impl Config {
    pub fn runtime(&self) -> PathBuf {
        self.root
            .join("runtime")
            .join(&self.environment)
            .join(&self.instance)
    }

    pub fn vision(&self, id: &str) -> PathBuf {
        self.root
            .join("vision")
            .join(&self.environment)
            .join(&self.instance)
            .join(id)
    }
}

impl Layout {
    pub fn runtime(&self) -> PathBuf {
        self.config.runtime()
    }
    pub fn vision(&self, id: &str) -> PathBuf {
        self.config.vision(id)
    }
}

fn config_file(data: &Path) -> PathBuf {
    data.join("config/log-storage.json")
}

pub fn read(data: &Path) -> Result<Option<Config>, String> {
    let path = config_file(data);
    if !path.exists() {
        return Ok(None);
    }
    crate::image_file::checked_source_path(&path)?;
    let config: Config =
        serde_json::from_str(&crate::fsio::read_text(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("日志根配置无效：{e}"))?;
    if config.schema != 1
        || !config.root.is_absolute()
        || !matches!(
            config.environment.as_str(),
            "production" | "development" | "test"
        )
        || config.instance.len() != 16
        || !config.instance.bytes().all(|b| b.is_ascii_hexdigit())
        || config
            .root
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("日志根配置的版本、目录或命名空间无效".into());
    }
    let owner = data
        .canonicalize()
        .map_err(|e| format!("无法核对日志所属资料目录：{e}"))?;
    if config.owner_data != owner {
        return Err("日志根配置属于另一个资料目录，不能复制或搬迁后直接沿用日志命名空间".into());
    }
    Ok(Some(config))
}

pub fn recording_project(project: &Path) -> Result<PathBuf, String> {
    let Some(projects) = project
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "projects"))
    else {
        return Ok(project.to_path_buf());
    };
    let Some(data) = projects.parent() else {
        return Ok(project.to_path_buf());
    };
    let id = project
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("项目编号无效")?;
    if !crate::project::valid_id(id) {
        return Err("项目编号无效".into());
    }
    Ok(read(data)?.map_or_else(|| project.to_path_buf(), |config| config.vision(id)))
}

pub fn runtime_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.try_state::<Layout>()
        .map(|layout| layout.runtime())
        .ok_or_else(|| "日志目录尚未初始化".into())
}

pub fn initialize(app: &AppHandle) -> Result<(), String> {
    if app.try_state::<Layout>().is_some() {
        return Ok(());
    }
    let data = crate::storage::root(app);
    let test = std::env::var_os("GLUESIGHT_DATA_DIR").is_some();
    let default = if test {
        data.join("gs_logs")
    } else {
        app.path()
            .home_dir()
            .map_err(|e| format!("无法解析用户主目录：{e}"))?
            .join("gs_logs")
    };
    let environment = if test {
        "test"
    } else if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
    };
    app.manage(prepare(&data, &default, environment)?);
    Ok(())
}

pub fn prepare(data: &Path, default_root: &Path, environment: &str) -> Result<Layout, String> {
    let config = match read(data)? {
        Some(config) if config.environment == environment => config,
        Some(_) => return Err("日志目录环境与当前程序不一致".into()),
        None => {
            if !default_root.is_absolute()
                || !matches!(environment, "production" | "development" | "test")
            {
                return Err("日志根必须为绝对路径且环境有效".into());
            }
            fs::create_dir_all(default_root)
                .map_err(|e| format!("无法创建日志根 {}：{e}", default_root.display()))?;
            crate::image_file::checked_source_path(default_root)?;
            let identity = format!(
                "{}:{}:{}",
                data.display(),
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos()
            );
            let config = Config {
                schema: 1,
                root: default_root.canonicalize().map_err(|e| e.to_string())?,
                owner_data: data.canonicalize().map_err(|e| e.to_string())?,
                environment: environment.into(),
                instance: format!("{:x}", Sha256::digest(identity.as_bytes()))[..16].into(),
            };
            crate::fsio::write_atomic(
                &config_file(data),
                &serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?,
            )?;
            config
        }
    };
    fs::create_dir_all(config.runtime())
        .map_err(|e| format!("无法打开已绑定日志根 {}：{e}", config.root.display()))?;
    crate::image_file::checked_source_path(&config.runtime())?;
    let lease = crate::instance::acquire(&config.runtime())?;
    migrate(&data.join("logs"), &config.runtime(), false, || Ok(()))?;
    let projects = data.join("projects");
    if projects.is_dir() {
        for entry in fs::read_dir(&projects).map_err(|e| e.to_string())? {
            let project = entry.map_err(|e| e.to_string())?.path();
            let id = project
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !project.is_dir() || !crate::project::valid_id(id) {
                continue;
            }
            crate::image_file::checked_source_path(&project)?;
            let target = config.vision(id);
            migrate(&project, &target, true, || {
                for scope in ["records", "records-offline"] {
                    crate::storage::relocate::tree_references(
                        &project,
                        &project.join(scope),
                        &target.join(scope),
                    )?;
                }
                Ok(())
            })?;
        }
    }
    Ok(Layout {
        config,
        _lease: lease,
    })
}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

fn fingerprint(path: &Path) -> Result<(u64, String), String> {
    crate::image_file::checked_source_path(path)?;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut bytes = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}

fn copy(
    source: &Path,
    stage: &Path,
    relative: &Path,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    crate::image_file::checked_source_path(source)?;
    if source.is_dir() {
        fs::create_dir_all(stage.join(relative)).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let text = name.to_string_lossy();
            if text.ends_with("-wal")
                || text.ends_with("-shm")
                || text == ".gluesight-instance.lock"
            {
                continue;
            }
            copy(&entry.path(), stage, &relative.join(name), entries)?;
        }
    } else if source.is_file() {
        let target = stage.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if source.extension().is_some_and(|e| e == "db") {
            let conn = rusqlite::Connection::open_with_flags(
                source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| e.to_string())?;
            conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
                .map_err(|e| format!("迁移日志数据库失败：{e}"))?;
            let integrity: String = rusqlite::Connection::open(&target)
                .map_err(|e| e.to_string())?
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if integrity != "ok" {
                return Err("日志数据库快照校验失败".into());
            }
        } else {
            let expected = fingerprint(source)?;
            fs::copy(source, &target).map_err(|e| e.to_string())?;
            if fingerprint(&target)? != expected {
                return Err(format!("日志迁移校验失败：{}", source.display()));
            }
        }
        let (bytes, sha256) = fingerprint(&target)?;
        entries.push(Entry {
            path: relative.to_path_buf(),
            bytes,
            sha256,
        });
    }
    Ok(())
}

fn migrate(
    source: &Path,
    target: &Path,
    project: bool,
    remap: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    crate::image_file::checked_source_path(target)?;
    if source.exists() {
        let source_path = source.canonicalize().map_err(|e| e.to_string())?;
        let target_path = target.canonicalize().map_err(|e| e.to_string())?;
        if target_path.starts_with(&source_path) || source_path.starts_with(&target_path) {
            return Err("旧日志与新日志目录不能互相包含".into());
        }
    }
    let done = target.join(".legacy-import.json");
    let stage = target.join(".legacy-staging");
    if done.is_file() {
        return clear_stage(&stage);
    }
    let manifest = stage.join("manifest.json");
    if !manifest.exists() {
        clear_stage(&stage)?;
        let required = if project {
            ["records", "records-offline"]
                .into_iter()
                .filter(|scope| source.join(scope).exists())
                .try_fold(0u64, |total, scope| {
                    total
                        .checked_add(crate::storage::directory_bytes(&source.join(scope))?)
                        .ok_or_else(|| "日志迁移大小溢出".to_string())
                })?
        } else if source.exists() {
            crate::storage::directory_bytes(source)?
        } else {
            0
        };
        crate::storage::check_space(target, required)?;
        fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        if project {
            for scope in ["records", "records-offline"] {
                if source.join(scope).exists() {
                    copy(&source.join(scope), &stage, Path::new(scope), &mut entries)?;
                }
            }
        } else if source.exists() {
            copy(source, &stage, Path::new(""), &mut entries)?;
        }
        crate::fsio::write_atomic(
            &manifest,
            &serde_json::to_string(&entries).map_err(|e| e.to_string())?,
        )?;
    }
    let entries: Vec<Entry> =
        serde_json::from_str(&crate::fsio::read_text(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    for entry in &entries {
        if entry.path.is_absolute()
            || entry
                .path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("日志迁移清单路径无效".into());
        }
        let destination = target.join(&entry.path);
        let staged = stage.join(&entry.path);
        if destination.exists() {
            if fingerprint(&destination)? != (entry.bytes, entry.sha256.clone()) {
                return Err(format!(
                    "日志迁移目标冲突，未覆盖：{}",
                    destination.display()
                ));
            }
        } else {
            if fingerprint(&staged)? != (entry.bytes, entry.sha256.clone()) {
                return Err("日志迁移暂存文件校验失败".into());
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::rename(&staged, &destination).map_err(|e| e.to_string())?;
        }
    }
    remap()?;
    crate::fsio::write_atomic(&done, &serde_json::json!({"schema":1,"source":source,"files":entries.len(),"sourcePreserved":true}).to_string())?;
    clear_stage(&stage)
}

fn clear_stage(stage: &Path) -> Result<(), String> {
    if stage.exists() {
        crate::image_file::checked_source_path(stage)?;
        fs::remove_dir_all(stage)
            .map_err(|e| format!("日志迁移暂存清理失败，下次启动重试：{e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

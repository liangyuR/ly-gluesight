use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
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
    Ok(Layout {
        config,
        _lease: lease,
    })
}

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    schema: u32,
    pub root: PathBuf,
    /// 录制盘，按写入优先顺序；旧配置没有这一项，录制只在 `root` 上
    #[serde(
        rename = "recordingRoots",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    recording_roots: Vec<PathBuf>,
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

    /// 录制盘，按写入优先顺序。
    pub fn recording_disks(&self) -> Vec<PathBuf> {
        if self.recording_roots.is_empty() {
            vec![self.root.clone()]
        } else {
            self.recording_roots.clone()
        }
    }

    fn vision_on(&self, disk: &Path, id: &str) -> PathBuf {
        disk.join("vision")
            .join(&self.environment)
            .join(&self.instance)
            .join(id)
    }

    /// 项目在第一块录制盘上的录制目录。
    pub fn vision(&self, id: &str) -> PathBuf {
        self.vision_on(&self.recording_disks()[0], id)
    }

    /// 项目在每块录制盘上的录制目录，与 `recording_disks` 同序。
    pub fn visions(&self, id: &str) -> Vec<PathBuf> {
        self.recording_disks()
            .iter()
            .map(|disk| self.vision_on(disk, id))
            .collect()
    }
}

impl Layout {
    pub fn runtime(&self) -> PathBuf {
        self.config.runtime()
    }
}

fn config_file(data: &Path) -> PathBuf {
    data.join("config/log-storage.json")
}

fn valid_root(root: &Path) -> bool {
    root.is_absolute() && !root.components().any(|c| matches!(c, Component::ParentDir))
}

/// 按 Windows 规则比较用的路径：不分大小写、去掉 `\\?\` 前缀、统一分隔符，以分隔符结尾。
fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => text
            .strip_prefix(r"\\?\")
            .map_or(text.clone(), str::to_owned),
    };
    let mut key = text.trim_end_matches('\\').to_lowercase();
    key.push('\\');
    key
}

/// `path` 是 `root` 或在它之内；只比较路径文本，不要求路径存在（盘离线时也能判断记录属于哪块盘）。
pub fn within(root: &Path, path: &Path) -> bool {
    path_key(path).starts_with(&path_key(root))
}

/// 录制盘列表：绝对路径，不重复、不互相包含。
fn check_disks(disks: &[PathBuf]) -> Result<(), String> {
    for (i, disk) in disks.iter().enumerate() {
        if !valid_root(disk) {
            return Err(format!("录制盘 {} 必须是绝对路径", disk.display()));
        }
        if let Some(other) = disks[..i]
            .iter()
            .find(|other| within(other, disk) || within(disk, other))
        {
            return Err(format!(
                "录制盘 {} 与 {} 重复或互相包含",
                disk.display(),
                other.display()
            ));
        }
    }
    Ok(())
}

/// 建目录并试写，返回规范化路径。
pub fn writable(disk: &Path) -> Result<PathBuf, String> {
    if !valid_root(disk) {
        return Err("必须是绝对路径".into());
    }
    fs::create_dir_all(disk).map_err(|e| e.to_string())?;
    probe_write(disk)?;
    disk.canonicalize().map_err(|e| e.to_string())
}

/// 已有目录里写入并删除一个探测文件；不建目录。
pub fn probe_write(disk: &Path) -> Result<(), String> {
    if !disk.is_dir() {
        return Err("目录不存在或所在盘离线".into());
    }
    crate::image_file::checked_source_path(disk)?;
    let probe = disk.join(format!(".gluesight-write-probe-{}", std::process::id()));
    fs::write(&probe, b"probe").map_err(|e| e.to_string())?;
    fs::remove_file(&probe).map_err(|e| e.to_string())
}

/// 改录制盘列表：逐块建目录并试写，不可写、重复或互相包含的拒绝保存；保存规范化路径，返回新配置。
pub fn save_recording_disks(data: &Path, disks: &[PathBuf]) -> Result<Config, String> {
    if disks.is_empty() {
        return Err("至少保留一块录制盘".into());
    }
    check_disks(disks)?;
    let mut config = read(data)?.ok_or("日志根尚未初始化")?;
    let checked = disks
        .iter()
        .map(|disk| writable(disk).map_err(|e| format!("录制盘 {} 不可写：{e}", disk.display())))
        .collect::<Result<Vec<_>, _>>()?;
    check_disks(&checked)?;
    config.recording_roots = checked;
    crate::fsio::write_atomic(
        &config_file(data),
        &serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?,
    )?;
    Ok(config)
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
        || !valid_root(&config.root)
        || !matches!(
            config.environment.as_str(),
            "production" | "development" | "test"
        )
        || config.instance.len() != 16
        || !config.instance.bytes().all(|b| b.is_ascii_hexdigit())
        || check_disks(&config.recording_roots).is_err()
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

#[cfg(test)]
pub fn recording_project(project: &Path) -> Result<PathBuf, String> {
    Ok(recording_projects(project)?.swap_remove(0))
}

/// 项目在每块录制盘上的录制目录（写入优先顺序）；资料目录没有日志根配置时只有项目目录本身。
pub fn recording_projects(project: &Path) -> Result<Vec<PathBuf>, String> {
    let Some(projects) = project
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "projects"))
    else {
        return Ok(vec![project.to_path_buf()]);
    };
    let Some(data) = projects.parent() else {
        return Ok(vec![project.to_path_buf()]);
    };
    let id = project
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("项目编号无效")?;
    if !crate::project::valid_id(id) {
        return Err("项目编号无效".into());
    }
    Ok(read(data)?.map_or_else(|| vec![project.to_path_buf()], |config| config.visions(id)))
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
                recording_roots: Vec::new(),
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

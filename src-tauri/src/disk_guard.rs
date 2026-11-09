//! 磁盘空间守护：新件按录制盘顺序写到第一块剩余不少于 5 GB 的盘；所有录制盘都不足或不可用时告警、新件不录原图，
//! 照常接件检测。资料目录所在盘低于 2 GB 时告警。空间恢复后自动恢复录制。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const GIB_BYTES: u64 = 1024 * 1024 * 1024;
/// 每块录制盘固定保留的剩余空间，不可配置
pub const RECORDING_RESERVE_BYTES: u64 = 5 * GIB_BYTES;
const DATA_MIN_FREE_BYTES: u64 = 2 * GIB_BYTES;
const RECHECK: Duration = Duration::from_secs(5);
const GIB: f64 = GIB_BYTES as f64;
/// 磁盘告警的前缀，在线检测页按它替换或撤下告警
pub const ALARM_PREFIX: &str = "磁盘空间不足";

/// 本项目在各录制盘上的录制根（`.../records`），按写入优先顺序；设置页改录制盘后整体替换。
pub type RecordRoots = Arc<RwLock<Vec<PathBuf>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskStatus {
    /// 新件写入的录制根；None 表示所有录制盘都低于余量或不可用，新件不录原图
    pub target: Option<PathBuf>,
    /// 不录原图时收尾补测用的临时录制写到的录制根：在线录制盘里剩余最多的一块
    pub scratch: Option<PathBuf>,
    pub alarm: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Free {
    Bytes(u64),
    /// 查询失败：不拦录制
    Unknown,
    /// 目录不在（盘离线、拔出）且建不起来
    Offline,
}

pub struct DiskGuard {
    data: PathBuf,
    roots: RecordRoots,
    data_min: u64,
    reserve: u64,
    last: Mutex<Option<(Instant, DiskStatus)>>,
}

impl DiskGuard {
    /// 开发构建可用 GLUESIGHT_DISK_MIN_FREE_MB 同时调高两条下限，在不写满磁盘的情况下做故障注入。
    pub fn new(data: PathBuf, roots: RecordRoots) -> Self {
        let injected = std::env::var("GLUESIGHT_DISK_MIN_FREE_MB")
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .and_then(|mb| mb.parse::<u64>().ok())
            .map(|mb| mb.saturating_mul(1024 * 1024));
        Self {
            data,
            roots,
            data_min: injected.unwrap_or(DATA_MIN_FREE_BYTES),
            reserve: injected.unwrap_or(RECORDING_RESERVE_BYTES),
            last: Mutex::new(None),
        }
    }

    pub fn roots(&self) -> &RecordRoots {
        &self.roots
    }

    /// 录制盘列表变了：下次查询不用缓存。
    pub fn invalidate(&self) {
        *self.last.lock().unwrap() = None;
    }

    pub fn status(&self) -> DiskStatus {
        self.status_at(Instant::now(), probe)
    }

    fn status_at(&self, now: Instant, mut free: impl FnMut(&Path) -> Free) -> DiskStatus {
        let mut last = self.last.lock().unwrap();
        if let Some((_, status)) = last
            .as_ref()
            .filter(|(at, _)| now.saturating_duration_since(*at) < RECHECK)
        {
            return status.clone();
        }
        let roots = self.roots.read().unwrap().clone();
        let probes: Vec<(PathBuf, Free)> = roots
            .into_iter()
            .map(|root| {
                let state = free(&root);
                (root, state)
            })
            .collect();
        let target = probes
            .iter()
            .find(|(_, state)| match state {
                Free::Bytes(bytes) => *bytes >= self.reserve,
                Free::Unknown => true,
                Free::Offline => false,
            })
            .map(|(root, _)| root.clone());
        let scratch = target.clone().or_else(|| {
            probes
                .iter()
                .filter_map(|(root, state)| match state {
                    Free::Bytes(bytes) => Some((*bytes, root)),
                    _ => None,
                })
                .max_by_key(|(bytes, _)| *bytes)
                .map(|(_, root)| root.clone())
        });
        let mut alarms = Vec::new();
        if let Free::Bytes(bytes) = free(&self.data) {
            if bytes < self.data_min {
                alarms.push(format!(
                    "资料盘 {} 剩余 {:.1} GB，低于 {:.1} GB 下限，检测记录与追溯可能写不进去，请清理磁盘",
                    self.data.display(),
                    bytes as f64 / GIB,
                    self.data_min as f64 / GIB
                ));
            }
        }
        if target.is_none() {
            let disks: Vec<String> = probes
                .iter()
                .map(|(root, state)| match state {
                    Free::Bytes(bytes) => {
                        format!("{} 剩余 {:.1} GB", root.display(), *bytes as f64 / GIB)
                    }
                    _ => format!("{} 不可用", root.display()),
                })
                .collect();
            alarms.push(format!(
                "所有录制盘都低于 {:.1} GB 余量或不可用（{}），新件暂停录制原图（照常检测）；清理磁盘、增加录制盘或调低原图保留上限后自动恢复",
                self.reserve as f64 / GIB,
                disks.join("；")
            ));
        }
        let status = DiskStatus {
            target,
            scratch,
            alarm: (!alarms.is_empty()).then(|| format!("{ALARM_PREFIX}：{}", alarms.join("；"))),
        };
        *last = Some((now, status.clone()));
        status
    }
}

/// 按顺序第一块剩余不少于余量的录制盘（查不到剩余空间的不拦）；路径不必存在，按最近的已有上级目录查，不建目录。
pub fn first_with_reserve(paths: &[PathBuf]) -> Option<usize> {
    paths.iter().position(|path| {
        path.ancestors()
            .find(|ancestor| ancestor.is_dir())
            .is_some_and(|existing| {
                free_bytes(existing).map_or(true, |bytes| bytes >= RECORDING_RESERVE_BYTES)
            })
    })
}

/// 目录不在时先试着建（盘重新接上后自动可用），建不起来算离线。
pub fn probe(path: &Path) -> Free {
    if !path.is_dir() && std::fs::create_dir_all(path).is_err() {
        return Free::Offline;
    }
    free_bytes(path).map_or(Free::Unknown, Free::Bytes)
}

/// 目录所在盘的剩余空间；目录不在或查询失败为 None。
pub fn free_space(path: &Path) -> Option<u64> {
    path.is_dir().then(|| free_bytes(path).ok()).flatten()
}

#[cfg(windows)]
fn free_bytes(path: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut available = 0u64;
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(available)
}

#[cfg(not(windows))]
fn free_bytes(_path: &Path) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "仅 Windows 查询磁盘剩余空间",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(roots: &[&str]) -> DiskGuard {
        DiskGuard::new(
            PathBuf::from("data"),
            Arc::new(RwLock::new(roots.iter().map(PathBuf::from).collect())),
        )
    }

    #[test]
    fn first_disk_below_reserve_writes_to_next_disk_even_when_data_disk_is_low() {
        let status =
            guard(&["c", "d", "e"]).status_at(Instant::now(), |path| match path.to_str() {
                Some("data") => Free::Bytes(GIB_BYTES),
                Some("c") => Free::Bytes(RECORDING_RESERVE_BYTES - 1),
                _ => Free::Bytes(RECORDING_RESERVE_BYTES),
            });
        assert_eq!(status.target, Some(PathBuf::from("d")));
        assert!(status.alarm.is_some_and(|alarm| alarm.contains("资料盘")));
    }

    #[test]
    fn all_disks_below_reserve_or_offline_stop_recording_and_scratch_uses_most_free_disk() {
        let status =
            guard(&["c", "d", "e"]).status_at(Instant::now(), |path| match path.to_str() {
                Some("data") => Free::Bytes(u64::MAX),
                Some("c") => Free::Bytes(GIB_BYTES),
                Some("d") => Free::Offline,
                _ => Free::Bytes(3 * GIB_BYTES),
            });
        assert_eq!(status.target, None);
        assert_eq!(status.scratch, Some(PathBuf::from("e")));
        assert!(status.alarm.is_some_and(|alarm| alarm.contains("d 不可用")));
    }
}

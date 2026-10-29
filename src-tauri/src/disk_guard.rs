//! 资料盘剩余空间守护：低于下限时暂停接件、空间恢复后自动放行。
//! 写到一半磁盘满会让持久追溯进入只能重启恢复的故障，所以在接下一件之前就停下。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const RECHECK: Duration = Duration::from_secs(5);
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub struct DiskGuard {
    root: PathBuf,
    recording_root: Option<PathBuf>,
    min_free: u64,
    last: Mutex<Option<(Instant, Result<(), String>)>>,
}

impl DiskGuard {
    /// 开发构建可用 GLUESIGHT_DISK_MIN_FREE_MB 调高下限，在不写满磁盘的情况下做故障注入。
    pub fn new(root: PathBuf) -> Self {
        let min_free = std::env::var("GLUESIGHT_DISK_MIN_FREE_MB")
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .and_then(|mb| mb.parse::<u64>().ok())
            .map_or(MIN_FREE_BYTES, |mb| mb.saturating_mul(1024 * 1024));
        Self {
            root,
            recording_root: None,
            min_free,
            last: Mutex::new(None),
        }
    }

    pub fn with_recording_root(mut self, root: PathBuf) -> Self {
        self.recording_root = Some(root);
        self
    }

    /// 剩余空间低于下限时返回暂停原因；查询失败不拦生产。
    pub fn check(&self) -> Result<(), String> {
        self.check_volumes(Instant::now(), free_bytes)
    }

    fn check_volumes(
        &self,
        now: Instant,
        mut free: impl FnMut(&Path) -> std::io::Result<u64>,
    ) -> Result<(), String> {
        let mut last = self.last.lock().unwrap();
        if let Some((_, result)) = last.as_ref().filter(|(at, _)| now.saturating_duration_since(*at) < RECHECK) {
            return result.clone();
        }
        let result = std::iter::once(&self.root).chain(self.recording_root.iter()).find_map(|root| {
            match free(root) {
                Ok(bytes) if bytes < self.min_free => Some(Err(format!("资料盘 {} 剩余空间 {:.1} GB，低于 {:.1} GB 下限，已暂停接件；清理磁盘或调低原图保留上限后自动恢复",
                    root.display(), bytes as f64 / GIB, self.min_free as f64 / GIB))),
                _ => None,
            }
        }).unwrap_or(Ok(()));
        *last = Some((now, result.clone()));
        result
    }
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
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "仅 Windows 查询资料盘剩余空间"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_recording_disk_low_space_blocks_even_when_project_disk_is_available() {
        let guard =
            DiskGuard::new(PathBuf::from("project")).with_recording_root(PathBuf::from("vision"));
        let result = guard.check_volumes(Instant::now(), |root| {
            Ok(if root == Path::new("vision") {
                0
            } else {
                u64::MAX
            })
        });
        assert!(result.is_err_and(|error| error.contains("vision")));
    }
}

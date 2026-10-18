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
    min_free: u64,
    last: Mutex<Option<(Instant, Result<(), String>)>>,
}

impl DiskGuard {
    /// 开发构建可用 GLUESIGHT_DISK_MIN_FREE_MB 调高下限，在不写满磁盘的情况下做故障注入。
    pub fn new(root: PathBuf) -> Self {
        let min_free = std::env::var("GLUESIGHT_DISK_MIN_FREE_MB").ok().filter(|_| cfg!(debug_assertions))
            .and_then(|mb| mb.parse::<u64>().ok()).map_or(MIN_FREE_BYTES, |mb| mb.saturating_mul(1024 * 1024));
        Self { root, min_free, last: Mutex::new(None) }
    }

    /// 剩余空间低于下限时返回暂停原因；查询失败不拦生产。
    pub fn check(&self) -> Result<(), String> {
        self.check_with(Instant::now(), || free_bytes(&self.root))
    }

    fn check_with(&self, now: Instant, free: impl FnOnce() -> std::io::Result<u64>) -> Result<(), String> {
        let mut last = self.last.lock().unwrap();
        if let Some((_, result)) = last.as_ref().filter(|(at, _)| now.saturating_duration_since(*at) < RECHECK) {
            return result.clone();
        }
        let result = match free() {
            Ok(bytes) if bytes < self.min_free => Err(format!("资料盘剩余空间 {:.1} GB，低于 {:.1} GB 下限，已暂停接件；清理磁盘或调低原图保留上限后自动恢复",
                bytes as f64 / GIB, self.min_free as f64 / GIB)),
            _ => Ok(()),
        };
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
    fn low_free_space_pauses_and_recovers_after_recheck_without_restart() {
        let guard = DiskGuard { root: PathBuf::from("."), min_free: MIN_FREE_BYTES, last: Mutex::new(None) };
        let start = Instant::now();
        let low = guard.check_with(start, || Ok(MIN_FREE_BYTES - 1));
        assert!(low.as_ref().is_err_and(|e| e.contains("剩余空间")), "{low:?}");
        assert!(guard.check_with(start + RECHECK, || Ok(MIN_FREE_BYTES)).is_ok(), "空间恢复后到期复查应自动放行");
        assert!(guard.check_with(start + RECHECK * 2, || Err(std::io::Error::other("拒绝访问"))).is_ok(), "查询失败不能停产");
    }
}

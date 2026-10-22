//! 走真实 S7 的测试工作目录：放在 target/ 下，测试通过后删除，失败时保留供排查。
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) struct TestDir(PathBuf);

impl TestDir {
    pub(crate) fn new(group: &str, name: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(group)
            .join(format!("{name}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl std::ops::Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if std::thread::panicking() { return; }
        // 引擎的轮询任务中止后要等运行时回收才放开 plc.db，Windows 上文件未关闭时删不掉：后台稍等重试
        let path = std::mem::take(&mut self.0);
        std::thread::spawn(move || {
            for _ in 0..100 {
                if std::fs::remove_dir_all(&path).is_ok() || !path.exists() { return; }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
}

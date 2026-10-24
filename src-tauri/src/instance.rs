use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

const RESTARTING: &str = "GLUESIGHT_RESTARTING";
const RESTART_WAIT: Duration = Duration::from_secs(20);

pub struct InstanceLease { _file: File }

/// 切换项目时由旧进程调用：重启出来的新进程要等旧进程退出、释放数据目录锁。
pub fn restart(app: &tauri::AppHandle) -> ! {
    std::env::set_var(RESTARTING, "1");
    app.restart()
}

pub fn acquire(data: &Path) -> Result<InstanceLease, String> {
    let wait = std::env::var_os(RESTARTING)
        .is_some()
        .then_some(RESTART_WAIT);
    std::env::remove_var(RESTARTING);
    acquire_within(data, wait.unwrap_or_default())
}

fn acquire_within(data: &Path, wait: Duration) -> Result<InstanceLease, String> {
    fs::create_dir_all(data).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(data.join(".gluesight-instance.lock")).map_err(|error| format!("无法打开数据目录锁：{error}"))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(InstanceLease { _file: file }),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(error) => {
                return Err(format!(
                    "无法取得数据目录独占锁，请关闭使用同一数据目录的其它实例：{}：{error}",
                    data.display()
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_directory_is_exclusive_until_the_instance_exits() {
        let root = std::env::temp_dir().join(format!("gluesight-instance-{}-{}", std::process::id(), ly_plc::now_ms()));
        fs::create_dir_all(&root).unwrap();
        let marker = root.join(".gluesight-instance.lock");
        fs::write(&marker, b"preserved").unwrap();
        let first = acquire(&root).unwrap();
        assert!(acquire(&root).err().unwrap().contains("独占锁"));
        let independent = root.join("other-profile");
        let other = acquire(&independent).unwrap();
        drop(first);
        let restarted = acquire(&root).unwrap();
        drop(restarted);
        assert_eq!(fs::read(&marker).unwrap(), b"preserved");
        drop(other);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restarted_instance_waits_for_the_previous_one_to_release_the_directory() {
        let root = std::env::temp_dir().join(format!(
            "gluesight-restart-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        let previous = acquire(&root).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(previous);
        });
        assert!(acquire_within(&root, RESTART_WAIT).is_ok());
        release.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

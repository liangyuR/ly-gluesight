use std::fs::{self, File, OpenOptions};
use std::path::Path;

pub struct InstanceLease { _file: File }

pub fn acquire(data: &Path) -> Result<InstanceLease, String> {
    fs::create_dir_all(data).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(data.join(".gluesight-instance.lock")).map_err(|error| format!("无法打开数据目录锁：{error}"))?;
    file.try_lock().map_err(|error| format!("无法取得数据目录独占锁，请关闭使用同一数据目录的其它实例：{}：{error}", data.display()))?;
    Ok(InstanceLease { _file: file })
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
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        assert!(root.file_name().unwrap().to_str().unwrap().starts_with("gluesight-instance-"));
        fs::remove_dir_all(root).unwrap();
    }
}

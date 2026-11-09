use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gs-log-storage-{}-{}-{}",
            std::process::id(),
            ly_plc::now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn data(&self, name: &str) -> PathBuf {
        let data = self.0.join(name);
        fs::create_dir_all(data.join("config")).unwrap();
        data
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn independent_data_roots_are_isolated_and_cloned_configuration_is_rejected_after_owner_exits() {
    let fixture = Fixture::new();
    let data_a = fixture.data("a");
    let data_b = fixture.data("b");
    let data_c = fixture.data("c");
    let root = fixture.0.join("shared");
    let a = prepare(&data_a, &root, "test").unwrap();
    let b = prepare(&data_b, &root, "test").unwrap();
    assert_ne!(a.runtime(), b.runtime());
    assert_ne!(
        a.config.vision("same-project"),
        b.config.vision("same-project")
    );
    let original = a.config.vision("same-project").join("records/owned.pgm");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"original").unwrap();
    drop(a);
    fs::copy(config_file(&data_a), config_file(&data_c)).unwrap();
    assert!(prepare(&data_c, &root, "test").is_err());
    assert!(recording_project(&data_c.join("projects/same-project")).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"original");
    let restarted = prepare(&data_a, &root, "test").unwrap();
    assert_eq!(
        fs::read(
            restarted
                .config
                .vision("same-project")
                .join("records/owned.pgm")
        )
        .unwrap(),
        b"original"
    );
}

/// 读图按每件记下的录制目录找它所在的录制盘：防的回归是只认第一块盘（第二块盘上的件读不了），或接受不在任何录制盘上的路径。
#[test]
fn record_on_second_disk_reads_from_it_and_path_outside_every_disk_is_rejected() {
    let fixture = Fixture::new();
    let data = fixture.data("a");
    let _layout = prepare(&data, &fixture.0.join("logs"), "test").unwrap();
    let disks = [fixture.0.join("disk-c"), fixture.0.join("disk-d")];
    let config = save_recording_disks(&data, &disks).unwrap();
    let second = config.visions("line-a")[1].join("records");
    let part = second.join("20261109/20261109_120000_000_OK_cycle_c1");
    fs::create_dir_all(&part).unwrap();
    let root =
        crate::project::records::record_root_of(&data, "line-a", Some(&part.to_string_lossy()))
            .unwrap();
    assert_eq!(root, second);
    let outside = fixture
        .0
        .join("elsewhere/records/20261109/20261109_120000_000_OK_cycle_c1");
    fs::create_dir_all(&outside).unwrap();
    let error =
        crate::project::records::record_root_of(&data, "line-a", Some(&outside.to_string_lossy()))
            .unwrap_err();
    assert!(error.contains("原图所在盘不可用"), "{error}");
}

/// 录制盘不能重复或互相包含：防的回归是同一块盘换个写法（大小写、末尾分隔符、子目录）被加两次，清理时同一件被算两遍。
#[test]
fn saving_the_same_disk_written_differently_or_nested_is_rejected() {
    let fixture = Fixture::new();
    let data = fixture.data("a");
    let _layout = prepare(&data, &fixture.0.join("logs"), "test").unwrap();
    let disk = fixture.0.join("Disk-D");
    let upper = PathBuf::from(format!("{}\\", disk.display().to_string().to_uppercase()));
    for twin in [upper, disk.join("nested")] {
        assert!(save_recording_disks(&data, &[disk.clone(), twin]).is_err());
    }
    assert_eq!(read(&data).unwrap().unwrap().recording_disks().len(), 1);
}

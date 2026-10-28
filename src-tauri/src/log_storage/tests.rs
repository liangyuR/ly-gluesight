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
    assert_ne!(a.vision("same-project"), b.vision("same-project"));
    let original = a.vision("same-project").join("records/owned.pgm");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"original").unwrap();
    drop(a);
    fs::copy(config_file(&data_a), config_file(&data_c)).unwrap();
    assert!(prepare(&data_c, &root, "test").is_err());
    assert!(recording_project(&data_c.join("projects/same-project")).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"original");
    let restarted = prepare(&data_a, &root, "test").unwrap();
    assert_eq!(
        fs::read(restarted.vision("same-project").join("records/owned.pgm")).unwrap(),
        b"original"
    );
}

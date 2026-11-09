use super::*;
use crate::project::{create_dir, dir};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gluesight-manage-{name}-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn add(data: &Path, id: &str, name: &str) {
    create_dir(data, &ProjectMeta::new(id, name, "", None), |staging| {
        std::fs::write(staging.join("config").join("plc.json"), id).map_err(|e| e.to_string())
    })
    .unwrap();
}


/// 防的回归：删除项目连同它的历史原图一起删（检测历史本机共用，删除项目只删项目资产）；旧版本留在 `.trash` 里、
/// 带外部原图清理标记的项目，标记与目录相符时照旧清掉原图，不相符时不动别的项目的原图。
#[test]
fn deleting_project_keeps_its_recordings_while_old_trash_markers_still_clean() {
    let data = TempDir::new("external-delete");
    add(&data.0, "line-a", "A 线");
    add(&data.0, "line-b", "B 线");
    add(&data.0, "line-c", "C 线");
    std::fs::create_dir_all(data.0.join("config")).unwrap();
    std::fs::write(data.0.join("config/log-storage.json"), serde_json::json!({
        "schema": 1, "root": data.0.join("gs_logs"), "environment": "test", "instance": "3333333333333333",
        "ownerData": data.0.canonicalize().unwrap(),
    }).to_string()).unwrap();
    let recording = |id: &str| crate::log_storage::recording_project(&dir(&data.0, id)).unwrap();
    let (a, b, c) = (
        recording("line-a"),
        recording("line-b"),
        recording("line-c"),
    );
    for root in [&a, &b, &c] {
        std::fs::create_dir_all(root.join("records")).unwrap();
        std::fs::write(root.join("records/image.pgm"), b"image").unwrap();
    }
    let before = usage(&data.0, "line-b", (0, None, None)).unwrap();
    assert_eq!(before.vision_bytes, 5);
    assert_eq!(before.bytes, before.project_bytes + before.vision_bytes);
    let trashed = move_to_trash(&data.0, Some("line-a"), "line-b").unwrap();
    purge_trash(&data.0).unwrap();
    assert!(!trashed.exists());
    assert!(b.join("records/image.pgm").is_file());
    assert!(check_new_id(&data.0, "line-b").is_err());

    let old = move_to_trash(&data.0, Some("line-a"), "line-c").unwrap();
    std::fs::write(old.join(EXTERNAL_CLEANUP), r#"{"projectId":"line-a"}"#).unwrap();
    assert!(purge_trash(&data.0).is_err());
    assert!(a.join("records/image.pgm").is_file());
    std::fs::write(old.join(EXTERNAL_CLEANUP), r#"{"projectId":"line-c"}"#).unwrap();
    purge_trash(&data.0).unwrap();
    assert!(!old.exists());
    assert!(!c.exists());
    assert!(a.join("records/image.pgm").is_file());
}

use super::*;
use crate::project::{create_dir, dir, list, read_meta, trash_root};

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

fn ids(data: &Path) -> Vec<String> {
    list(data).unwrap().into_iter().map(|e| e.id).collect()
}

#[test]
fn delete_current_is_refused_and_deleted_project_leaves_list_at_once_then_purge_clears_trash() {
    let data = TempDir::new("delete");
    add(&data.0, "line-a", "A 线");
    add(&data.0, "line-b", "B 线");

    assert!(move_to_trash(&data.0, Some("line-a"), "line-a").is_err());
    assert!(dir(&data.0, "line-a").join("project.json").is_file());

    let trashed = move_to_trash(&data.0, Some("line-a"), "line-b").unwrap();
    assert_eq!(ids(&data.0), ["line-a"]);
    assert!(trashed.starts_with(trash_root(&data.0)));

    purge_trash(&data.0).unwrap();
    assert!(!trashed.exists());
    assert_eq!(ids(&data.0), ["line-a"]);
}

#[test]
fn create_with_conflicting_or_invalid_id_or_duplicate_name_is_refused_and_existing_project_untouched(
) {
    let cases = [
        ("same-id", "line-a", "新项目"),
        ("trash-id", "old", "新项目"),
        ("invalid-id", "line.a", "新项目"),
        ("same-name", "line-b", " A 线 "),
    ];
    for (case, id, name) in cases {
        let data = TempDir::new(case);
        add(&data.0, "line-a", "A 线");
        std::fs::create_dir_all(trash_root(&data.0).join("old.1700000000000")).unwrap();
        let before = read_meta(&dir(&data.0, "line-a")).unwrap();
        let active = ActiveProject {
            id: "line-a".into(),
            root: dir(&data.0, "line-a"),
        };

        let request = CreateProject {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            copy_hardware: true,
        };
        assert!(create(&data.0, Some(&active), &request).is_err(), "{case}");

        assert_eq!(ids(&data.0), ["line-a"], "{case}");
        assert_eq!(
            read_meta(&dir(&data.0, "line-a")).unwrap(),
            before,
            "{case}"
        );
        assert_eq!(
            std::fs::read_to_string(dir(&data.0, "line-a").join("config").join("plc.json"))
                .unwrap(),
            "line-a",
            "{case}"
        );
    }
}

#[test]
fn deleting_project_cleans_its_external_recordings_on_purge_and_counts_their_usage() {
    let data = TempDir::new("external-delete");
    add(&data.0, "line-a", "A 线");
    add(&data.0, "line-b", "B 线");
    std::fs::create_dir_all(data.0.join("config")).unwrap();
    std::fs::write(data.0.join("config/log-storage.json"), serde_json::json!({
        "schema": 1, "root": data.0.join("gs_logs"), "environment": "test", "instance": "3333333333333333",
        "ownerData": data.0.canonicalize().unwrap(),
    }).to_string()).unwrap();
    let a = crate::log_storage::recording_project(&dir(&data.0, "line-a")).unwrap();
    let b = crate::log_storage::recording_project(&dir(&data.0, "line-b")).unwrap();
    for root in [&a, &b] {
        std::fs::create_dir_all(root.join("records")).unwrap();
        std::fs::write(root.join("records/image.pgm"), b"image").unwrap();
    }
    let before = usage(&data.0, "line-b").unwrap();
    assert_eq!(before.vision_bytes, 5);
    assert_eq!(before.bytes, before.project_bytes + before.vision_bytes);
    let trashed = move_to_trash(&data.0, Some("line-a"), "line-b").unwrap();
    assert!(!dir(&data.0, "line-b").exists());
    assert!(b.exists());
    assert!(check_new_id(&data.0, "line-b").is_err());
    std::fs::write(trashed.join(EXTERNAL_CLEANUP), r#"{"projectId":"line-a"}"#).unwrap();
    assert!(purge_trash(&data.0).is_err());
    assert!(a.join("records/image.pgm").is_file());
    assert!(b.join("records/image.pgm").is_file());
    std::fs::write(trashed.join(EXTERNAL_CLEANUP), r#"{"projectId":"line-b"}"#).unwrap();
    purge_trash(&data.0).unwrap();
    assert!(!trashed.exists());
    assert!(!b.exists());
    assert_eq!(
        std::fs::read(a.join("records/image.pgm")).unwrap(),
        b"image"
    );
    assert!(check_new_id(&data.0, "line-b").is_ok());
}

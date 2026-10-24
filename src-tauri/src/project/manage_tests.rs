use super::*;
use crate::project::{create_dir, dir, list, read_meta, trash_root};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("gluesight-manage-{name}-{}-{}", std::process::id(), ly_plc::now_ms()));
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
fn create_with_conflicting_or_invalid_id_or_duplicate_name_is_refused_and_existing_project_untouched() {
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
        let active = ActiveProject { id: "line-a".into(), root: dir(&data.0, "line-a") };

        let request = CreateProject { id: id.into(), name: name.into(), description: String::new(), copy_hardware: true };
        assert!(create(&data.0, Some(&active), &request).is_err(), "{case}");

        assert_eq!(ids(&data.0), ["line-a"], "{case}");
        assert_eq!(read_meta(&dir(&data.0, "line-a")).unwrap(), before, "{case}");
        assert_eq!(std::fs::read_to_string(dir(&data.0, "line-a").join("config").join("plc.json")).unwrap(), "line-a", "{case}");
    }
}

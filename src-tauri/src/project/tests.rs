use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gluesight-project-{name}-{}-{}",
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

fn add(data: &Path, id: &str) {
    create_dir(data, &ProjectMeta::new(id, id, "", None), |_| Ok(())).unwrap();
}

#[test]
fn invalid_current_pointer_then_operator_must_choose_instead_of_another_project_opening() {
    let cases: [(&str, fn(&Path)); 4] = [
        ("missing-pointer", |_| {}),
        ("missing-dir", |data| write_pointer(data, "gone").unwrap()),
        ("broken-meta", |data| {
            add(data, "broken");
            std::fs::write(dir(data, "broken").join("project.json"), "{").unwrap();
            write_pointer(data, "broken").unwrap();
        }),
        ("mismatched-meta", |data| {
            add(data, "moved");
            std::fs::rename(dir(data, "moved"), dir(data, "renamed")).unwrap();
            write_pointer(data, "renamed").unwrap();
        }),
    ];
    for (name, setup) in cases {
        let data = TempDir::new(name);
        add(&data.0, "line-a");
        setup(&data.0);
        assert!(
            matches!(resolve(&data.0).unwrap(), Resolution::NeedsChoice(_)),
            "{name}"
        );
    }
}

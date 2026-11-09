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

#[test]
fn work_in_progress_or_unwritten_records_then_project_switch_is_refused() {
    let idle = || ProjectSwitch {
        block: None,
        busy: false,
        audit_pending: false,
    };
    let cases = [
        (
            "在途件或未结事务",
            ProjectSwitch {
                block: Some("检测仍有在途工件，完成握手或复位故障后再切换站点模式".into()),
                ..idle()
            },
        ),
        (
            "节拍未回到空闲",
            ProjectSwitch {
                busy: true,
                ..idle()
            },
        ),
        (
            "录制或追溯未写完",
            ProjectSwitch {
                audit_pending: true,
                ..idle()
            },
        ),
    ];
    for (name, pre) in cases {
        assert!(check_project_switch(&pre).is_err(), "{name}");
    }
    assert!(check_project_switch(&idle()).is_ok());
}

/// 示教件可选本机任意项目的历史件：防的回归是只看当前项目的轮次，别的项目的候选引用的录制被滚动清理或按天清理删掉。
/// 只有历史件导入的轮次算引用，删掉轮次（换示教件、删候选）后不再算。
#[test]
fn referenced_cycles_cover_every_project_and_only_history_rounds() {
    let data = TempDir::new("referenced");
    let round = |project: &str, id: &str, origin: serde_json::Value| {
        let dir = dir(&data.0, project).join("recipe-capture").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("round.json"), serde_json::json!({"origin": origin}).to_string()).unwrap();
        dir
    };
    add(&data.0, "line-a");
    add(&data.0, "line-b");
    round("line-a", "capture-1-0", serde_json::json!({"kind": "history", "cycleId": "c-a", "recordId": 1}));
    let other = round("line-b", "capture-2-0", serde_json::json!({"kind": "history", "cycleId": "c-b", "recordId": 2}));
    round("line-b", "capture-3-0", serde_json::json!({"kind": "prepared", "label": "手动"}));
    let cycles = records::referenced_cycles(&data.0).unwrap();
    assert_eq!(cycles.into_iter().collect::<Vec<_>>(), ["c-a", "c-b"]);
    std::fs::remove_dir_all(other).unwrap();
    assert_eq!(records::referenced_cycles(&data.0).unwrap().into_iter().collect::<Vec<_>>(), ["c-a"]);
}

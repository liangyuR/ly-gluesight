use super::*;
use crate::offline::test_dir::TestDir;
use std::fs;

#[test]
fn diagnostic_window_includes_tracing_and_existing_logs_with_original_context() {
    let dir = TestDir::new("applog-tests", "upgrade");
    let now = Local::now();
    let ts = now.timestamp_millis();
    let from = ts - 60_000;
    let to = ts + 60_000;
    let before = Local.timestamp_millis_opt(from - 1).unwrap();
    let boundary = Local.timestamp_millis_opt(from).unwrap();
    let day = boundary.date_naive();
    let json = dir.join(format!("gluesight-{day}.jsonl"));
    fs::write(
        &json,
        format!(
            "{{\"ts\":{},\"msg\":\"outside\"}}\n{{\"ts\":{from},\"msg\":\"json event\"}}\n",
            from - 1
        ),
    )
    .unwrap();
    let legacy = dir.join(format!("gluesight-{day}.log"));
    fs::write(
        &legacy,
        format!(
            "{} [info] outside\n{} [warn] old text\n",
            before.format("%Y-%m-%d %H:%M:%S%.3f"),
            boundary.format("%Y-%m-%d %H:%M:%S%.3f")
        ),
    )
    .unwrap();
    let writer = Arc::new(FileWriter::open(&dir).unwrap());
    tracing::subscriber::with_default(subscriber(writer), || {
        emit(
            Level::Warn,
            "PLC 回写",
            "写入失败\n设备超时",
            Fields {
                sn: Some("SN-EXAMPLE".into()),
                ..Default::default()
            },
        );
    });
    let files = files_in_range(&dir, from, to).unwrap();
    let mut out = Vec::new();
    let count: u64 = files
        .iter()
        .map(|path| export_file(path, from, to, &mut out).unwrap())
        .sum();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(count, 3);
    assert!(text.contains("PLC 回写：写入失败\n设备超时") && text.contains("SN-EXAMPLE"));
    assert!(
        text.contains(&format!("{{\"ts\":{from},\"msg\":\"json event\"}}"))
            && text.contains("[warn] old text")
    );
    assert!(!text.contains("outside"));
}

#[test]
fn restart_with_full_segment_keeps_it_and_only_prunes_managed_logs() {
    let dir = TestDir::new("applog-tests", "rotate");
    let day = Local::now().date_naive();
    let full = dir.join(format!("runtime-{day}.log"));
    let expired = dir.join("gluesight-2000-01-01.jsonl");
    let unrelated = dir.join("plc.db");
    fs::File::create(&full)
        .unwrap()
        .set_len(16 * 1024 * 1024)
        .unwrap();
    fs::write(&expired, "expired").unwrap();
    fs::write(&unrelated, "untouched").unwrap();
    let writer = Arc::new(FileWriter::open(&dir).unwrap());
    tracing::subscriber::with_default(subscriber(writer), || tracing::info!("重启后追加"));
    assert_eq!(fs::metadata(full).unwrap().len(), 16 * 1024 * 1024);
    assert!(fs::read_to_string(dir.join(format!("runtime-{day}.1.log")))
        .unwrap()
        .contains("重启后追加"));
    assert!(!expired.exists());
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "untouched");
}

#[test]
fn tail_cursor_follows_written_lines_across_file_rollover_without_replays() {
    let dir = TestDir::new("applog-tests", "follow");
    let full = dir.join(format!("runtime-{}.log", Local::now().date_naive()));
    fs::File::create(full)
        .unwrap()
        .set_len(16 * 1024 * 1024 - 1)
        .unwrap();
    let writer = Arc::new(FileWriter::open(&dir).unwrap());
    tracing::subscriber::with_default(subscriber(writer.clone()), || {
        tracing::info!("滚动之前");
        let initial = writer.tail(None);
        assert!(
            !initial.skipped && initial.lines.len() == 1 && initial.lines[0].contains("滚动之前")
        );
        tracing::warn!("滚动之后\n错误原因");
        let recent = writer.tail(Some(initial.cursor));
        assert!(!recent.skipped);
        assert!(
            recent.lines.len() == 2
                && recent.lines[0].contains("滚动之后")
                && recent.lines[1] == "错误原因"
        );
        let idle = writer.tail(Some(recent.cursor));
        assert!(idle.lines.is_empty() && !idle.skipped && idle.cursor == recent.cursor);
    });
}

#[test]
fn falling_behind_returns_bounded_utf8_preview_and_keeps_full_file_message() {
    let dir = TestDir::new("applog-tests", "bounded-preview");
    let writer = Arc::new(FileWriter::open(&dir).unwrap());
    let long = "诊".repeat(900);
    tracing::subscriber::with_default(subscriber(writer.clone()), || {
        tracing::info!("{}", "短行\n".repeat(250));
        let rows = writer.tail(Some(0));
        assert!(rows.skipped && rows.lines.len() == 200);
        for i in 0..60 {
            tracing::info!("{i} {long} 详细原因");
        }
    });
    let page = writer.tail(Some(0));
    assert!(page.skipped && !page.lines.is_empty());
    assert!(page
        .lines
        .iter()
        .all(|line| line.len() <= 2048 && line.ends_with('…')));
    assert!(page.lines.iter().map(|line| line.len() + 1).sum::<usize>() <= 64 * 1024);
    let disk =
        fs::read_to_string(dir.join(format!("runtime-{}.log", Local::now().date_naive()))).unwrap();
    assert!(disk.contains(&format!("59 {long} 详细原因")));
}

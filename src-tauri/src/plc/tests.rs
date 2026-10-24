use super::*;
use ly_plc::LogEntry;

#[test]
fn export_spanning_several_pages_writes_every_log_once_in_order() {
    let entries: Vec<LogEntry> = (0..2500)
        .map(|i| LogEntry {
            id: i,
            ts: 1_700_000_000_000 - i,
            level: "info".into(),
            category: "value".into(),
            point_id: None,
            point_name: None,
            message: format!("m{i}"),
            old_value: None,
            new_value: None,
            context: None,
        })
        .collect();
    let labels = LogLabels { levels: HashMap::new(), categories: HashMap::new() };
    let mut out = Vec::new();
    write_log_csv(&mut out, LogQuery::default(), &labels, |q| {
        let offset = q.offset.unwrap_or(0) as usize;
        let limit = q.limit.unwrap_or(200) as usize;
        Ok(LogPage { total: entries.len() as i64, items: entries.iter().skip(offset).take(limit).cloned().collect() })
    })
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let messages: Vec<&str> = text.lines().skip(1).map(|line| line.split(',').nth(4).unwrap()).collect();
    let expected: Vec<String> = (0..2500).map(|i| format!("m{i}")).collect();
    assert_eq!(messages, expected);
}

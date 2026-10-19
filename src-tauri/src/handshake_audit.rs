//! S7 握手审计 state/plc-handshake.audit.jsonl：超过分段大小就滚动到 state/plc-handshake-audit/，
//! 已滚动的分段按保留天数与总量清理；仍有未结 PLC 交付引用的分段不删（启动时 ACK 恢复要读）。

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDateTime, TimeZone};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub segment_bytes: u64,
    pub keep_days: i64,
    pub max_total_bytes: u64,
}

/// 分段 4 MB（约 7000 件）保证当前文件和单个分段都能进诊断包（单文件上限 8 MB）；保留天数与运行日志一致；
/// 256 MB 约是每天 1 万件时 45 天的量。
pub const LIMITS: Limits = Limits { segment_bytes: 4 * 1024 * 1024, keep_days: crate::applog::LIMITS.keep_days, max_total_bytes: 256 * 1024 * 1024 };

const SEGMENT_DIR: &str = "plc-handshake-audit";
const PREFIX: &str = "plc-handshake.audit.";
const SUFFIX: &str = ".jsonl";
const STAMP: &str = "%Y%m%d-%H%M%S-%3f";

fn segment_dir(active: &Path) -> PathBuf {
    active.with_file_name(SEGMENT_DIR)
}

pub fn append(active: &Path, record: &[u8]) -> Result<(), String> {
    append_with(active, record, ly_plc::now_ms(), LIMITS.segment_bytes)
}

fn append_with(active: &Path, record: &[u8], now_ms: i64, segment_bytes: u64) -> Result<(), String> {
    if let Some(parent) = active.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    if fs::metadata(active).is_ok_and(|meta| meta.len() >= segment_bytes) {
        let _ = roll(active, now_ms);
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(active).map_err(|e| format!("无法保存握手审计：{e}"))?;
    file.write_all(record).and_then(|_| file.sync_all()).map_err(|e| format!("握手审计写入失败：{e}"))
}

pub(crate) fn roll(active: &Path, now_ms: i64) -> std::io::Result<()> {
    let dir = segment_dir(active);
    fs::create_dir_all(&dir)?;
    let stamp = Local.timestamp_millis_opt(now_ms).single().map_or_else(|| now_ms.to_string(), |t| t.format(STAMP).to_string());
    let target = dir.join(format!("{PREFIX}{stamp}{SUFFIX}"));
    if target.exists() { return Ok(()); }
    fs::rename(active, target)
}

/// 已滚动的分段，按滚动时间先后：（滚动时刻，路径，大小）。
fn segments(active: &Path) -> Vec<(i64, PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(segment_dir(active)) else { return Vec::new() };
    let mut found: Vec<_> = entries.flatten().filter_map(|entry| {
        let name = entry.file_name();
        let stamp = name.to_str()?.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
        let rolled = NaiveDateTime::parse_from_str(stamp, STAMP).ok()?;
        let ms = Local.from_local_datetime(&rolled).earliest()?.timestamp_millis();
        let meta = entry.metadata().ok().filter(|m| m.is_file())?;
        Some((ms, entry.path(), meta.len()))
    }).collect();
    found.sort();
    found
}

/// 读取顺序：已滚动的分段在前，当前文件最后。
pub fn files(active: &Path) -> Vec<PathBuf> {
    segments(active).into_iter().map(|(_, path, _)| path).chain(std::iter::once(active.to_path_buf())).collect()
}

/// 与导出时间段有交集的已滚动分段（分段覆盖上一次滚动到本次滚动之间的记录），及未选中的分段数。
pub fn overlapping(active: &Path, from_ms: i64, to_ms: i64) -> (Vec<PathBuf>, usize) {
    let all = segments(active);
    let mut previous = i64::MIN;
    let mut chosen = Vec::new();
    for (rolled, path, _) in &all {
        if *rolled >= from_ms && previous <= to_ms { chosen.push(path.clone()); }
        previous = *rolled;
    }
    let skipped = all.len() - chosen.len();
    (chosen, skipped)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub removed: usize,
    pub kept_unresolved: usize,
    pub errors: Vec<String>,
}

fn referenced(path: &Path, unresolved: &BTreeSet<String>) -> std::io::Result<bool> {
    if unresolved.is_empty() { return Ok(false); }
    let text = String::from_utf8_lossy(&fs::read(path)?).into_owned();
    Ok(unresolved.iter().any(|cycle| text.contains(&format!("\"cycleId\":\"{cycle}\""))))
}

/// 从新到旧累计大小：滚动早于保留天数或累计超过总量的分段删除；当前文件不删，未结交付引用的分段保留。
pub fn prune(active: &Path, unresolved: &BTreeSet<String>, limits: Limits, now_ms: i64) -> Pruned {
    let oldest = now_ms.saturating_sub(limits.keep_days.saturating_mul(86_400_000));
    let mut total = fs::metadata(active).map_or(0, |meta| meta.len());
    let mut pruned = Pruned::default();
    for (rolled, path, size) in segments(active).into_iter().rev() {
        total = total.saturating_add(size);
        if rolled >= oldest && total <= limits.max_total_bytes { continue; }
        match referenced(&path, unresolved) {
            Ok(true) => pruned.kept_unresolved += 1,
            Ok(false) => match fs::remove_file(&path) {
                Ok(()) => pruned.removed += 1,
                Err(error) => pruned.errors.push(format!("删除握手审计分段 {} 失败：{error}", path.display())),
            },
            Err(error) => pruned.errors.push(format!("读取握手审计分段 {} 失败，保留：{error}", path.display())),
        }
    }
    pruned
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    const DAY: i64 = 86_400_000;

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("gluesight-handshake-audit-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn active(&self) -> PathBuf { self.0.join("plc-handshake.audit.jsonl") }
    }

    impl Drop for Dir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn line(cycle: &str) -> Vec<u8> {
        format!("{{\"version\":1,\"action\":\"acknowledged\",\"pending\":{{\"cycleId\":\"{cycle}\"}},\"pad\":\"{}\"}}\n", "x".repeat(60)).into_bytes()
    }

    fn write_segment(active: &Path, rolled_at: i64, cycle: &str) {
        let name = format!("{PREFIX}{}{SUFFIX}", Local.timestamp_millis_opt(rolled_at).unwrap().format(STAMP));
        fs::create_dir_all(segment_dir(active)).unwrap();
        fs::write(segment_dir(active).join(name), line(cycle)).unwrap();
    }

    fn cycles_in(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().filter_map(|path| fs::read_to_string(path).ok()).flat_map(|text| {
            text.lines().filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok()?["pending"]["cycleId"].as_str().map(String::from)).collect::<Vec<_>>()
        }).collect()
    }

    fn cycles_left(active: &Path) -> Vec<String> { cycles_in(&files(active)) }

    #[test]
    fn active_file_rolls_into_a_segment_before_the_write_that_would_pass_the_segment_size_and_reads_oldest_first() {
        let dir = Dir::new();
        let active = dir.active();
        let size = line("first").len() as u64;
        append_with(&active, &line("first"), 1_000, size).unwrap();
        assert!(segments(&active).is_empty());
        append_with(&active, &line("second"), 2_000, size).unwrap();
        assert_eq!(segments(&active).len(), 1, "当前文件达到分段大小，下一条写入前滚动");
        assert_eq!(fs::read(&active).unwrap(), line("second"));
        assert_eq!(files(&active).last(), Some(&active), "当前文件最后读");
        assert_eq!(cycles_left(&active), ["first", "second"], "按时间顺序读取");
    }

    #[test]
    fn segments_overlapping_the_export_window_are_chosen_and_the_rest_are_only_counted() {
        let dir = Dir::new();
        let active = dir.active();
        let base = Local.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap().timestamp_millis();
        for (day, cycle) in [(1, "a"), (2, "b"), (3, "c")] { write_segment(&active, base + day * DAY, cycle); }
        let chosen = |from_day: i64, to_day: i64| {
            let (paths, skipped) = overlapping(&active, base + from_day * DAY, base + to_day * DAY);
            (cycles_in(&paths), skipped)
        };
        assert_eq!(chosen(2, 3), (["b", "c"].map(String::from).to_vec(), 1), "滚动早于开始时间的分段不选");
        assert_eq!(chosen(-5, 0), (["a"].map(String::from).to_vec(), 2), "后面的分段从结束时间之后才开始记录，不选");
        assert_eq!(chosen(4, 6), (Vec::new(), 3), "导出时间段在所有分段滚动之后，只剩当前文件");
    }

    #[test]
    fn rolled_segments_expire_by_age_and_cap_except_those_still_needed_by_unresolved_deliveries() {
        let dir = Dir::new();
        let active = dir.active();
        let now = Local.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap().timestamp_millis();
        for (rolled_at, cycle) in [(now - 40 * DAY, "old-unresolved"), (now - 35 * DAY, "old-done"), (now - 3 * DAY, "cap-done"), (now - 2 * DAY, "recent-a"), (now - DAY, "recent-b")] {
            write_segment(&active, rolled_at, cycle);
        }
        fs::write(&active, line("active")).unwrap();
        let newest_three: u64 = ["active", "recent-b", "recent-a"].iter().map(|cycle| line(cycle).len() as u64).sum();
        let unresolved: BTreeSet<String> = ["old-unresolved".to_string()].into();
        let pruned = prune(&active, &unresolved, Limits { segment_bytes: 1, keep_days: 30, max_total_bytes: newest_three }, now);
        assert_eq!(pruned, Pruned { removed: 2, kept_unresolved: 1, errors: Vec::new() });
        assert_eq!(cycles_left(&active), ["old-unresolved", "recent-a", "recent-b", "active"], "过期删、超总量删、未结交付引用的保留");
    }
}

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

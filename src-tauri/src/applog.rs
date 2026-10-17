use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{Local, NaiveDate};
use tauri::{AppHandle, Manager};

const PREFIX: &str = "gluesight-";
const SUFFIX: &str = ".log";
const KEEP_DAYS: i64 = 30;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

struct DailyFile {
    dir: PathBuf,
    date: NaiveDate,
    file: Option<File>,
}

static SINK: OnceLock<Mutex<DailyFile>> = OnceLock::new();

pub fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("logs"))
}

/// 文本日志写到 logs/gluesight-YYYY-MM-DD.log，按本地日期滚动；只清理本前缀的文件，plc.db 等不受影响。
pub fn init(app: &AppHandle) -> Result<(), String> {
    let dir = dir(app)?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败（{}）：{e}", dir.display()))?;
    let today = Local::now().date_naive();
    prune(&dir, today, KEEP_DAYS, MAX_TOTAL_BYTES);
    if SINK.set(Mutex::new(DailyFile { dir, date: today, file: None })).is_err() {
        return Ok(());
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        write("panic", "程序异常", &format!("线程 {}：{info}", thread.name().unwrap_or("?")));
        previous(info);
    }));
    write("info", "启动", &format!("{} {}", app.package_info().name, app.package_info().version));
    Ok(())
}

pub fn write(level: &str, ev: &str, msg: &str) {
    let Some(sink) = SINK.get() else { return };
    let mut sink = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Local::now();
    let date = now.date_naive();
    if sink.date != date {
        prune(&sink.dir, date, KEEP_DAYS, MAX_TOTAL_BYTES);
        sink.date = date;
        sink.file = None;
    }
    if sink.file.is_none() {
        sink.file = OpenOptions::new().create(true).append(true).open(sink.dir.join(file_name(date))).ok();
    }
    if let Some(file) = sink.file.as_mut() {
        let _ = writeln!(file, "{} [{level}] {ev}：{msg}", now.format("%Y-%m-%d %H:%M:%S%.3f"));
    }
}

fn file_name(date: NaiveDate) -> String {
    format!("{PREFIX}{}{SUFFIX}", date.format("%Y-%m-%d"))
}

fn log_date(name: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?, "%Y-%m-%d").ok()
}

/// 删除早于保留天数的日志；总大小超限时从最旧的删起，当天的文件始终保留。
fn prune(dir: &Path, today: NaiveDate, keep_days: i64, max_total_bytes: u64) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut logs: Vec<(NaiveDate, PathBuf, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let date = log_date(entry.file_name().to_str()?)?;
            let size = entry.metadata().ok().filter(|m| m.is_file())?.len();
            Some((date, entry.path(), size))
        })
        .collect();
    logs.sort_by(|a, b| b.0.cmp(&a.0));
    let oldest = today - chrono::Duration::days(keep_days - 1);
    let mut total = 0u64;
    for (date, path, size) in logs {
        total += size;
        if date < today && (date < oldest || total > max_total_bytes) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gluesight-applog-{name}-{}-{}", std::process::id(), ly_plc::now_ms()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn put(dir: &Path, name: &str, bytes: usize) {
        fs::write(dir.join(name), vec![b'x'; bytes]).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn expired_logs_removed_other_files_in_logs_dir_kept() {
        let dir = temp_dir("expire");
        put(&dir, "gluesight-2026-10-01.log", 1);
        put(&dir, "gluesight-2026-10-02.log", 1);
        put(&dir, "gluesight-2026-10-03.log", 1);
        put(&dir, "plc.db", 1);
        put(&dir, "gluesight-notes.log", 1);
        prune(&dir, day("2026-10-03"), 2, u64::MAX);
        assert_eq!(names(&dir), ["gluesight-2026-10-02.log", "gluesight-2026-10-03.log", "gluesight-notes.log", "plc.db"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn over_size_cap_deletes_oldest_first_and_keeps_today() {
        let dir = temp_dir("size");
        put(&dir, "gluesight-2026-10-01.log", 40);
        put(&dir, "gluesight-2026-10-02.log", 40);
        put(&dir, "gluesight-2026-10-03.log", 40);
        prune(&dir, day("2026-10-03"), 30, 90);
        assert_eq!(names(&dir), ["gluesight-2026-10-02.log", "gluesight-2026-10-03.log"]);
        prune(&dir, day("2026-10-03"), 30, 10);
        assert_eq!(names(&dir), ["gluesight-2026-10-03.log"]);
        fs::remove_dir_all(dir).unwrap();
    }
}

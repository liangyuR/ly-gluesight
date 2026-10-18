use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

const PREFIX: &str = "gluesight-";
const SUFFIX: &str = ".jsonl";
const LEGACY_SUFFIX: &str = ".log";

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub keep_days: i64,
    pub max_total_bytes: u64,
    pub segment_bytes: u64,
}

pub const LIMITS: Limits = Limits { keep_days: 30, max_total_bytes: 1024 * 1024 * 1024, segment_bytes: 16 * 1024 * 1024 };

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn from_tone(tone: &str) -> Self {
        match tone {
            "err" | "error" | "panic" => Self::Error,
            "warn" | "ng" => Self::Warn,
            "debug" => Self::Debug,
            _ => Self::Info,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_seq: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Fields {

    pub fn shot(k: usize, shot_id: &str, camera: &str) -> Self {
        Self { k: Some(k), shot_id: Some(shot_id.into()), camera: Some(camera.into()), ..Self::default() }
    }

    pub fn stage(stage: &str) -> Self {
        Self { stage: Some(stage.into()), ..Self::default() }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn with_stage(mut self, stage: &str) -> Self {
        self.stage = Some(stage.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub ts: i64,
    pub level: Level,
    pub ev: String,
    pub msg: String,
    #[serde(flatten)]
    pub fields: Fields,
}

/// 在途工件的关联字段：未显式指明工件的记录补上当前工件；显式指明其他工件的记录不改。
pub fn correlate(active: Option<&Fields>, mut fields: Fields) -> Fields {
    let Some(active) = active else { return fields };
    match fields.cycle_id.as_deref() {
        None => fields.cycle_id = active.cycle_id.clone(),
        Some(id) if Some(id) != active.cycle_id.as_deref() => return fields,
        Some(_) => {}
    }
    if fields.sn.is_none() { fields.sn = active.sn.clone(); }
    if fields.request_seq.is_none() { fields.request_seq = active.request_seq; }
    if fields.recipe_id.is_none() { fields.recipe_id = active.recipe_id.clone(); }
    fields
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FileKey {
    date: NaiveDate,
    seq: u32,
    legacy: bool,
}

fn file_name(date: NaiveDate, seq: u32) -> String {
    if seq == 0 { format!("{PREFIX}{}{SUFFIX}", date.format("%Y-%m-%d")) } else { format!("{PREFIX}{}.{seq}{SUFFIX}", date.format("%Y-%m-%d")) }
}

fn parse_name(name: &str) -> Option<FileKey> {
    let rest = name.strip_prefix(PREFIX)?;
    if let Some(stem) = rest.strip_suffix(LEGACY_SUFFIX) {
        let date = NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()?;
        return Some(FileKey { date, seq: 0, legacy: true });
    }
    let stem = rest.strip_suffix(SUFFIX)?;
    let (day, seq) = match stem.split_once('.') {
        Some((day, seq)) => (day, seq.parse::<u32>().ok().filter(|s| *s > 0 && !seq.starts_with('0'))?),
        None => (stem, 0),
    };
    Some(FileKey { date: NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?, seq, legacy: false })
}

fn log_files(dir: &Path) -> Vec<(FileKey, PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let key = parse_name(entry.file_name().to_str()?)?;
            let size = entry.metadata().ok().filter(|m| m.is_file())?.len();
            Some((key, entry.path(), size))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
}

/// 删除早于保留天数的日志；总大小超限时从最旧的删起；正在写入的文件始终保留。
fn prune(dir: &Path, today: NaiveDate, current: Option<&Path>, limits: Limits) {
    let oldest = today - chrono::Duration::days(limits.keep_days - 1);
    let mut total = 0u64;
    for (key, path, size) in log_files(dir) {
        total += size;
        if current == Some(path.as_path()) { continue; }
        if key.date < oldest || total > limits.max_total_bytes {
            let _ = fs::remove_file(path);
        }
    }
}

fn latest_seq(dir: &Path, date: NaiveDate) -> u32 {
    log_files(dir).into_iter().filter(|(key, ..)| key.date == date && !key.legacy).map(|(key, ..)| key.seq).max().unwrap_or(0)
}

pub struct Sink {
    dir: PathBuf,
    limits: Limits,
    date: NaiveDate,
    seq: u32,
    size: u64,
    file: Option<File>,
}

impl Sink {
    pub fn open(dir: PathBuf, today: NaiveDate, limits: Limits) -> Self {
        let seq = latest_seq(&dir, today);
        let mut sink = Self { dir, limits, date: today, seq, size: 0, file: None };
        prune(&sink.dir, today, None, limits);
        sink.reopen();
        sink
    }

    fn path(&self) -> PathBuf {
        self.dir.join(file_name(self.date, self.seq))
    }

    fn reopen(&mut self) {
        let path = self.path();
        self.file = OpenOptions::new().create(true).append(true).open(&path).ok();
        self.size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    }

    pub fn write(&mut self, record: &Record) {
        let date = Local.timestamp_millis_opt(record.ts).single().map_or(self.date, |t| t.date_naive());
        if date > self.date {
            self.date = date;
            self.seq = latest_seq(&self.dir, date);
            self.reopen();
            prune(&self.dir, date, Some(&self.path()), self.limits);
        } else if self.size >= self.limits.segment_bytes {
            self.seq += 1;
            self.reopen();
            prune(&self.dir, self.date, Some(&self.path()), self.limits);
        } else if self.file.is_none() {
            self.reopen();
        }
        let Ok(mut line) = serde_json::to_string(record) else { return };
        line.push('\n');
        if let Some(file) = self.file.as_mut() {
            if file.write_all(line.as_bytes()).is_ok() {
                self.size += line.len() as u64;
            }
        }
    }
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
static ACTIVE: Mutex<Option<Fields>> = Mutex::new(None);

pub fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::storage::root(app).join("logs"))
}

/// 结构化运行日志写到 logs/gluesight-YYYY-MM-DD[.N].jsonl：按本地日期与单文件大小滚动；只清理本前缀的文件，plc.db 等不受影响。
pub fn init(app: &AppHandle) -> Result<(), String> {
    let dir = dir(app)?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败（{}）：{e}", dir.display()))?;
    if SINK.set(Mutex::new(Sink::open(dir, Local::now().date_naive(), LIMITS))).is_err() {
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

pub fn begin_cycle(fields: Fields) {
    *ACTIVE.lock().unwrap_or_else(|p| p.into_inner()) = Some(fields);
}

pub fn end_cycle() {
    *ACTIVE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

pub fn write(tone: &str, ev: &str, msg: &str) {
    emit(Level::from_tone(tone), ev, msg, Fields::default());
}

pub fn emit(level: Level, ev: &str, msg: &str, fields: Fields) -> Record {
    let fields = correlate(ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).as_ref(), fields);
    let record = Record { ts: ly_plc::now_ms(), level, ev: ev.into(), msg: msg.into(), fields };
    if let Some(sink) = SINK.get() {
        sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).write(&record);
    }
    record
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Query {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub levels: Vec<Level>,
    pub keyword: Option<String>,
    pub cycle_id: Option<String>,
    pub sn: Option<String>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub total: u64,
    pub items: Vec<Record>,
    pub files: usize,
    pub bytes: u64,
}

impl Query {
    fn accepts(&self, record: &Record) -> bool {
        self.from.is_none_or(|from| record.ts >= from)
            && self.to.is_none_or(|to| record.ts <= to)
            && (self.levels.is_empty() || self.levels.contains(&record.level))
            && self.cycle_id.as_deref().filter(|s| !s.is_empty()).is_none_or(|id| record.fields.cycle_id.as_deref() == Some(id))
            && self.sn.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none_or(|sn| record.fields.sn.as_deref().is_some_and(|v| v.contains(sn)))
            && self.keyword.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none_or(|word| {
                let word = word.to_lowercase();
                record.ev.to_lowercase().contains(&word) || record.msg.to_lowercase().contains(&word)
                    || record.fields.cycle_id.as_deref().is_some_and(|v| v.contains(&word))
                    || record.fields.shot_id.as_deref().is_some_and(|v| v.to_lowercase().contains(&word))
            })
    }

    fn quick_reject(&self, line: &str) -> bool {
        self.cycle_id.as_deref().filter(|s| !s.is_empty()).is_some_and(|id| !line.contains(id))
    }
}

fn local_date(ts: i64) -> Option<NaiveDate> {
    Local.timestamp_millis_opt(ts).single().map(|t| t.date_naive())
}

/// 读取一个日志文件里通过筛选的记录，按写入顺序。损坏的行跳过。
fn scan(path: &Path, query: &Query, mut visit: impl FnMut(Record)) {
    let Ok(file) = File::open(path) else { return };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.is_empty() || query.quick_reject(&line) { continue; }
        let Ok(record) = serde_json::from_str::<Record>(&line) else { continue };
        if query.accepts(&record) { visit(record); }
    }
}

/// 按时间倒序分页查询；只读取覆盖查询时间段的文件。
pub fn query(dir: &Path, query: &Query) -> Page {
    let (from_day, to_day) = (query.from.and_then(local_date), query.to.and_then(local_date));
    let files: Vec<_> = log_files(dir).into_iter()
        .filter(|(key, ..)| !key.legacy && from_day.is_none_or(|d| key.date >= d) && to_day.is_none_or(|d| key.date <= d))
        .collect();
    let limit = if query.limit == 0 { 100 } else { query.limit.min(1000) } as usize;
    let skip = query.offset as usize;
    let mut total = 0u64;
    let mut items = Vec::new();
    let bytes = files.iter().map(|(_, _, size)| size).sum();
    for (_, path, _) in &files {
        let mut matched = Vec::new();
        scan(path, query, |record| matched.push(record));
        for record in matched.into_iter().rev() {
            let at = total as usize;
            total += 1;
            if at >= skip && items.len() < limit { items.push(record); }
        }
    }
    Page { total, items, files: files.len(), bytes }
}

/// 某件工件的全部关联记录，按时间顺序；只读该件时间附近的日期文件。
pub fn cycle_records(dir: &Path, cycle_id: &str, around: i64, cap: usize) -> (Vec<Record>, bool) {
    let center = local_date(around);
    let days: Vec<NaiveDate> = center.map(|d| vec![d - chrono::Duration::days(1), d, d + chrono::Duration::days(1)]).unwrap_or_default();
    let query = Query { cycle_id: Some(cycle_id.into()), ..Query::default() };
    let mut files: Vec<_> = log_files(dir).into_iter().filter(|(key, ..)| !key.legacy && days.contains(&key.date)).collect();
    files.reverse();
    let mut records = Vec::new();
    let mut truncated = false;
    for (_, path, _) in files {
        scan(&path, &query, |record| if records.len() < cap { records.push(record) } else { truncated = true });
    }
    records.sort_by_key(|r| r.ts);
    (records, truncated)
}

/// 覆盖时间段的日志文件，按时间先后。
pub fn files_in_range(dir: &Path, from: i64, to: i64) -> Vec<PathBuf> {
    let (from_day, to_day) = (local_date(from), local_date(to));
    let mut files: Vec<_> = log_files(dir).into_iter()
        .filter(|(key, ..)| !key.legacy && from_day.is_none_or(|d| key.date >= d) && to_day.is_none_or(|d| key.date <= d))
        .map(|(_, path, _)| path).collect();
    files.reverse();
    files
}

/// 导出用：一个文件里时间段内的记录逐行写出，返回行数。
pub fn export_file(path: &Path, from: i64, to: i64, out: &mut Vec<u8>) -> u64 {
    let query = Query { from: Some(from), to: Some(to), ..Query::default() };
    let mut count = 0;
    scan(path, &query, |record| {
        if let Ok(line) = serde_json::to_string(&record) {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
            count += 1;
        }
    });
    count
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub dir: String,
    pub files: usize,
    pub bytes: u64,
    pub keep_days: i64,
    pub max_total_bytes: u64,
    pub segment_bytes: u64,
}

pub fn usage(dir: &Path) -> Usage {
    let files = log_files(dir);
    Usage { dir: dir.display().to_string(), files: files.len(), bytes: files.iter().map(|f| f.2).sum(),
        keep_days: LIMITS.keep_days, max_total_bytes: LIMITS.max_total_bytes, segment_bytes: LIMITS.segment_bytes }
}

#[tauri::command]
pub async fn applog_query(app: AppHandle, query: Query) -> Result<Page, String> {
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || self::query(&dir, &query)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn applog_usage(app: AppHandle) -> Result<Usage, String> {
    Ok(usage(&dir(&app)?))
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

    fn at(date: &str, hms: &str) -> i64 {
        let t = chrono::NaiveDateTime::parse_from_str(&format!("{date} {hms}"), "%Y-%m-%d %H:%M:%S").unwrap();
        Local.from_local_datetime(&t).earliest().unwrap().timestamp_millis()
    }

    fn record(ts: i64, level: Level, ev: &str, cycle: Option<&str>) -> Record {
        Record { ts, level, ev: ev.into(), msg: format!("{ev} 内容"), fields: Fields { cycle_id: cycle.map(Into::into), ..Fields::default() } }
    }

    const ROOMY: Limits = Limits { keep_days: 30, max_total_bytes: u64::MAX, segment_bytes: u64::MAX };

    #[test]
    fn expired_logs_removed_other_files_in_logs_dir_kept() {
        let dir = temp_dir("expire");
        put(&dir, "gluesight-2026-10-01.log", 1);
        put(&dir, "gluesight-2026-10-01.jsonl", 1);
        put(&dir, "gluesight-2026-10-02.jsonl", 1);
        put(&dir, "gluesight-2026-10-02.1.jsonl", 1);
        put(&dir, "gluesight-2026-10-03.jsonl", 1);
        put(&dir, "plc.db", 1);
        put(&dir, "gluesight-notes.jsonl", 1);
        prune(&dir, day("2026-10-03"), None, Limits { keep_days: 2, ..ROOMY });
        assert_eq!(names(&dir), ["gluesight-2026-10-02.1.jsonl", "gluesight-2026-10-02.jsonl", "gluesight-2026-10-03.jsonl", "gluesight-notes.jsonl", "plc.db"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn full_segment_rolls_over_and_total_cap_also_deletes_older_segments_of_today() {
        let dir = temp_dir("segment");
        put(&dir, "gluesight-2026-10-02.jsonl", 300);
        let limits = Limits { keep_days: 30, max_total_bytes: 700, segment_bytes: 200 };
        let mut sink = Sink::open(dir.clone(), day("2026-10-03"), limits);
        for i in 0..12 {
            sink.write(&record(at("2026-10-03", "08:00:00") + i, Level::Info, "帧到达", Some("c1")));
        }
        let files = names(&dir);
        assert!(files.iter().any(|f| f == "gluesight-2026-10-03.1.jsonl"), "{files:?}");
        assert!(!files.contains(&"gluesight-2026-10-02.jsonl".to_string()), "{files:?}");
        assert!(!files.contains(&"gluesight-2026-10-03.jsonl".to_string()), "超出总量时当天较早的分段也要清理：{files:?}");
        assert!(files.contains(&file_name(sink.date, sink.seq)), "正在写入的分段不能被清理：{files:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn query_pages_newest_first_across_segments_with_cycle_level_and_keyword_filters() {
        let dir = temp_dir("query");
        let mut sink = Sink::open(dir.clone(), day("2026-10-02"), Limits { segment_bytes: 150, ..ROOMY });
        sink.write(&record(at("2026-10-02", "23:59:58"), Level::Info, "partStart↑", Some("c1")));
        for (i, (level, cycle)) in [(Level::Debug, "c1"), (Level::Error, "c1"), (Level::Info, "c2"), (Level::Warn, "c1")].into_iter().enumerate() {
            sink.write(&record(at("2026-10-03", "08:00:00") + i as i64, level, &format!("事件{i}"), Some(cycle)));
        }
        assert!(log_files(&dir).len() >= 3);
        let page = query(&dir, &Query { cycle_id: Some("c1".into()), limit: 2, ..Query::default() });
        assert_eq!(page.total, 4);
        assert_eq!(page.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["事件3", "事件1"]);
        let next = query(&dir, &Query { cycle_id: Some("c1".into()), limit: 2, offset: 2, ..Query::default() });
        assert_eq!(next.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["事件0", "partStart↑"]);
        let errors = query(&dir, &Query { levels: vec![Level::Error, Level::Warn], ..Query::default() });
        assert_eq!(errors.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["事件3", "事件1"]);
        let today = query(&dir, &Query { from: Some(at("2026-10-03", "00:00:00")), keyword: Some("事件".into()), ..Query::default() });
        assert_eq!(today.total, 4);
        let (cycle, truncated) = cycle_records(&dir, "c1", at("2026-10-03", "08:00:00"), 3);
        assert_eq!(cycle.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["partStart↑", "事件0", "事件1"]);
        assert!(truncated);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cycle_context_fills_missing_fields_but_never_relabels_another_cycle() {
        let active = Fields { cycle_id: Some("c1".into()), sn: Some("SN-A".into()), request_seq: Some(7), recipe_id: Some("R".into()), ..Fields::default() };
        let plain = correlate(Some(&active), Fields::stage("result"));
        assert_eq!((plain.cycle_id.as_deref(), plain.sn.as_deref(), plain.request_seq, plain.stage.as_deref()), (Some("c1"), Some("SN-A"), Some(7), Some("result")));
        let other = Fields { cycle_id: Some("c0".into()), ..Fields::default() };
        assert_eq!(correlate(Some(&active), other.clone()), other);
        assert_eq!(correlate(None, Fields::default()), Fields::default());
    }
}

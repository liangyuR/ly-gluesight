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
    /// 段配方：事件所属帧组序号（与 camera 一起定位一帧）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
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
    /// 缺省为不按级别筛选；空数组表示一个级别都没选
    pub levels: Option<Vec<Level>>,
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
    fn admits(&self, ts: i64, level: Level) -> bool {
        self.from.is_none_or(|from| ts >= from) && self.to.is_none_or(|to| ts <= to) && self.levels.as_ref().is_none_or(|levels| levels.contains(&level))
    }

    fn accepts(&self, record: &Record) -> bool {
        self.admits(record.ts, record.level)
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
            || head(line).is_some_and(|(ts, level)| !self.admits(ts, level))
    }
}

/// 记录行按 Record 字段顺序以 {"ts":…,"level":"…" 开头，按时间和级别排除时不必整行解析。
fn head(line: &str) -> Option<(i64, Level)> {
    let (ts, rest) = line.strip_prefix("{\"ts\":")?.split_once(',')?;
    let level = match rest.strip_prefix("\"level\":\"")?.split('"').next()? {
        "debug" => Level::Debug,
        "info" => Level::Info,
        "warn" => Level::Warn,
        "error" => Level::Error,
        _ => return None,
    };
    Some((ts.parse().ok()?, level))
}

fn local_date(ts: i64) -> Option<NaiveDate> {
    Local.timestamp_millis_opt(ts).single().map(|t| t.date_naive())
}

const ORDER_SLACK_MS: i64 = 60_000;

struct Span {
    path: PathBuf,
    size: u64,
    start: i64,
    end: i64,
}

/// 非旧格式的日志分段，按时间先后。start 取首行时间；end 取最后写入时间（记录的 ts 在写入前取得，不会晚于它），
/// 正在写入的最新分段不看修改时间，视为未结束。
fn spans(dir: &Path, keep: impl Fn(NaiveDate) -> bool) -> Vec<Span> {
    let files: Vec<_> = log_files(dir).into_iter().filter(|(key, ..)| !key.legacy).collect();
    let newest = files.first().map(|(_, path, _)| path.clone());
    let mut spans: Vec<_> = files.into_iter().filter(|(key, ..)| keep(key.date)).map(|(_, path, size)| {
        let end = if newest.as_ref() == Some(&path) { i64::MAX } else { modified_ms(&path).unwrap_or(i64::MAX) };
        Span { start: first_ts(&path).unwrap_or(i64::MIN), end, path, size }
    }).collect();
    spans.reverse();
    spans
}

impl Span {
    fn overlaps(&self, from: Option<i64>, to: Option<i64>) -> bool {
        from.is_none_or(|from| self.end >= from) && to.is_none_or(|to| self.start.saturating_sub(ORDER_SLACK_MS) <= to)
    }
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    i64::try_from(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
}

fn first_ts(path: &Path) -> Option<i64> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?).read_line(&mut line).ok()?;
    head(&line).map(|(ts, _)| ts)
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

/// 按时间倒序分页查询；只读取覆盖查询时间段的分段。
pub fn query(dir: &Path, query: &Query) -> Page {
    let (from_day, to_day) = (query.from.and_then(local_date), query.to.and_then(local_date));
    let mut files = spans(dir, |date| from_day.is_none_or(|d| date >= d) && to_day.is_none_or(|d| date <= d));
    files.retain(|span| span.overlaps(query.from, query.to));
    files.reverse();
    let limit = if query.limit == 0 { 100 } else { query.limit.min(1000) } as usize;
    let skip = query.offset as usize;
    let mut total = 0u64;
    let mut items = Vec::new();
    let bytes = files.iter().map(|span| span.size).sum();
    for span in &files {
        let mut matched = Vec::new();
        scan(&span.path, query, |record| matched.push(record));
        for record in matched.into_iter().rev() {
            let at = total as usize;
            total += 1;
            if at >= skip && items.len() < limit { items.push(record); }
        }
    }
    Page { total, items, files: files.len(), bytes }
}

pub struct CycleLog {
    pub records: Vec<Record>,
    pub truncated: bool,
    /// 本件第一条记录之前，最近一次 S7 事务结束或复位的时间（上一件或示教采集的收尾）
    pub previous_end: Option<i64>,
    /// 本件最后一条记录之后，下一件 partStart 的时间
    pub next_start: Option<i64>,
}

const CYCLE_BEFORE_MS: i64 = 10 * 60_000;
const CYCLE_AFTER_MS: i64 = 60 * 60_000;

#[derive(Default)]
struct CycleScan {
    records: Vec<Record>,
    ends: Vec<i64>,
    starts: Vec<i64>,
    truncated: bool,
}

impl CycleScan {
    fn read(&mut self, path: &Path, cycle_id: &str, cap: usize) {
        let Ok(file) = File::open(path) else { return };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let own = line.contains(cycle_id);
            let start = line.contains("\"stage\":\"partStart\"");
            if !own && !start && !line.contains("\"stage\":\"released\"") && !line.contains("\"stage\":\"reset\"") { continue; }
            let Ok(record) = serde_json::from_str::<Record>(&line) else { continue };
            if record.fields.cycle_id.as_deref() == Some(cycle_id) {
                if self.records.len() < cap { self.records.push(record) } else { self.truncated = true }
            } else if !own && start {
                self.starts.push(record.ts);
            } else if !own {
                self.ends.push(record.ts);
            }
        }
    }

    fn previous_end(&self) -> Option<i64> {
        let first = self.records.iter().map(|r| r.ts).min()?;
        self.ends.iter().copied().filter(|ts| *ts < first).max()
    }

    fn next_start(&self) -> Option<i64> {
        let last = self.records.iter().map(|r| r.ts).max()?;
        self.starts.iter().copied().filter(|ts| *ts > last).min()
    }
}

/// 某件工件的全部关联记录，按时间顺序。只读该件时间前 10 分钟、后 1 小时内有写入的分段；
/// 前后相邻工件的边界不在其中时，再逐个向更早、更晚的分段找，不超过前后一天。
pub fn cycle_records(dir: &Path, cycle_id: &str, around: i64, cap: usize) -> CycleLog {
    let days: Vec<NaiveDate> = local_date(around).map(|d| vec![d - chrono::Duration::days(1), d, d + chrono::Duration::days(1)]).unwrap_or_default();
    let files = spans(dir, |date| days.contains(&date));
    let near: Vec<usize> = (0..files.len()).filter(|&i| files[i].overlaps(Some(around - CYCLE_BEFORE_MS), Some(around + CYCLE_AFTER_MS))).collect();
    let mut scan = CycleScan::default();
    if let (Some(&lo), Some(&hi)) = (near.first(), near.last()) {
        for span in &files[lo..=hi] { scan.read(&span.path, cycle_id, cap); }
        for span in files[..lo].iter().rev() {
            if scan.records.is_empty() || scan.previous_end().is_some() { break; }
            scan.read(&span.path, cycle_id, cap);
        }
        for span in &files[hi + 1..] {
            if scan.records.is_empty() || scan.next_start().is_some() { break; }
            scan.read(&span.path, cycle_id, cap);
        }
    }
    let (previous_end, next_start) = (scan.previous_end(), scan.next_start());
    let CycleScan { mut records, truncated, .. } = scan;
    records.sort_by_key(|r| r.ts);
    CycleLog { records, truncated, previous_end, next_start }
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
    fn query_pages_newest_first_and_cycle_records_report_previous_release() {
        let dir = temp_dir("query");
        let mut sink = Sink::open(dir.clone(), day("2026-10-02"), Limits { segment_bytes: 150, ..ROOMY });
        for ts in [at("2026-10-02", "23:59:50"), at("2026-10-02", "23:59:57")] {
            let mut released = record(ts, Level::Info, "S7 事务结束", Some("c0"));
            released.fields.stage = Some("released".into());
            sink.write(&released);
        }
        let mut start = record(at("2026-10-02", "23:59:58"), Level::Info, "partStart↑", Some("c1"));
        start.fields.stage = Some("partStart".into());
        sink.write(&start);
        for (i, (level, cycle)) in [(Level::Debug, "c1"), (Level::Error, "c1"), (Level::Info, "c2"), (Level::Warn, "c1")].into_iter().enumerate() {
            sink.write(&record(at("2026-10-03", "08:00:00") + i as i64, level, &format!("事件{i}"), Some(cycle)));
        }
        assert!(log_files(&dir).len() >= 3);
        let page = query(&dir, &Query { cycle_id: Some("c1".into()), limit: 2, ..Query::default() });
        assert_eq!(page.total, 4);
        assert_eq!(page.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["事件3", "事件1"]);
        let next = query(&dir, &Query { cycle_id: Some("c1".into()), limit: 2, offset: 2, ..Query::default() });
        assert_eq!(next.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["事件0", "partStart↑"]);
        let errors = query(&dir, &Query { levels: Some(vec![Level::Error, Level::Warn]), ..Query::default() });
        assert_eq!(errors.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["事件3", "事件1"]);
        let today = query(&dir, &Query { from: Some(at("2026-10-03", "00:00:00")), keyword: Some("事件".into()), ..Query::default() });
        assert_eq!(today.total, 4);
        let cycle = cycle_records(&dir, "c1", at("2026-10-03", "08:00:00"), 3);
        assert_eq!(cycle.records.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["partStart↑", "事件0", "事件1"]);
        assert!(cycle.truncated);
        assert_eq!(cycle.previous_end, Some(at("2026-10-02", "23:59:57")), "上一件的事务结束是本件 PLC 信号窗口的下界");
        assert_eq!(cycle_records(&dir, "c0", at("2026-10-02", "23:59:50"), 10).next_start, Some(at("2026-10-02", "23:59:58")), "下一件的 partStart 是上界");
        fs::remove_dir_all(dir).unwrap();
    }

    fn segment(dir: &Path, name: &str, records: &[Record], modified: i64) {
        let text: String = records.iter().map(|r| serde_json::to_string(r).unwrap() + "\n").collect();
        fs::write(dir.join(name), text).unwrap();
        let file = OpenOptions::new().write(true).open(dir.join(name)).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(modified as u64)).unwrap();
    }

    fn staged(ts: i64, cycle: &str, stage: &str) -> Record {
        let mut r = record(ts, Level::Info, stage, Some(cycle));
        r.fields.stage = Some(stage.into());
        r
    }

    #[test]
    fn empty_level_selection_matches_nothing_while_omitted_levels_match_all() {
        let dir = temp_dir("levels");
        let ts = at("2026-10-03", "08:00:00");
        segment(&dir, "gluesight-2026-10-03.jsonl", &[record(ts, Level::Debug, "a", None), record(ts + 1, Level::Error, "b", None)], ts + 1);
        assert_eq!(query(&dir, &Query { levels: Some(Vec::new()), ..Query::default() }).total, 0);
        assert_eq!(query(&dir, &Query::default()).total, 2, "诊断导出不传级别，必须拿到全部级别");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_query_skips_segments_last_written_before_from_but_always_reads_the_active_one() {
        let dir = temp_dir("recent");
        let (early, late) = (at("2026-10-03", "07:00:00"), at("2026-10-03", "08:30:00"));
        segment(&dir, "gluesight-2026-10-03.jsonl", &[record(early, Level::Info, "早", None), record(early + 1, Level::Info, "早", None)], early + 1);
        segment(&dir, "gluesight-2026-10-03.1.jsonl", &[record(late, Level::Info, "近", None)], late);
        segment(&dir, "gluesight-2026-10-03.2.jsonl", &[record(late + 600_000, Level::Info, "在写", None)], early);
        let page = query(&dir, &Query { from: Some(at("2026-10-03", "08:00:00")), ..Query::default() });
        assert_eq!(page.items.iter().map(|r| r.ev.as_str()).collect::<Vec<_>>(), ["在写", "近"]);
        assert_eq!(page.files, 2, "最后写入早于起点的分段不应再读");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cycle_boundaries_outside_the_near_segments_are_found_by_walking_outward() {
        let dir = temp_dir("walk");
        let t = |hms| at("2026-10-03", hms);
        segment(&dir, "gluesight-2026-10-03.jsonl", &[staged(t("06:00:00"), "c0", "released")], t("06:00:00"));
        segment(&dir, "gluesight-2026-10-03.1.jsonl", &[record(t("07:00:00"), Level::Info, "空闲", None)], t("07:00:00"));
        segment(&dir, "gluesight-2026-10-03.2.jsonl", &[staged(t("08:00:00"), "c1", "partStart"), staged(t("08:00:06"), "c1", "result"),
            staged(t("08:00:07"), "c1", "released")], t("08:00:07"));
        segment(&dir, "gluesight-2026-10-03.3.jsonl", &[record(t("10:00:00"), Level::Info, "空闲", None)], t("10:00:00"));
        segment(&dir, "gluesight-2026-10-03.4.jsonl", &[staged(t("11:00:00"), "c2", "partStart")], t("11:00:00"));
        let cycle = cycle_records(&dir, "c1", t("08:00:06"), 100);
        assert_eq!(cycle.records.len(), 3);
        assert_eq!((cycle.previous_end, cycle.next_start), (Some(t("06:00:00")), Some(t("11:00:00"))));
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

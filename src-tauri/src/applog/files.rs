use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone};
use serde::Deserialize;

use super::tail::{TailBuffer, TailPage};

const KEEP_DAYS: i64 = 30;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;
const SEGMENT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Format {
    LegacyText,
    JsonLines,
    Text,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct LogName {
    day: NaiveDate,
    format: Format,
    segment: u32,
}

impl LogName {
    fn parse(name: &str) -> Option<Self> {
        let (format, stem) = if let Some(stem) = name.strip_prefix("runtime-") {
            (Format::Text, stem.strip_suffix(".log")?)
        } else {
            let name = name.strip_prefix("gluesight-")?;
            match name.strip_suffix(".jsonl") {
                Some(stem) => (Format::JsonLines, stem),
                None => (Format::LegacyText, name.strip_suffix(".log")?),
            }
        };
        let (day, segment) = match stem.split_once('.') {
            Some((day, seq)) if format != Format::LegacyText && !seq.starts_with('0') => {
                (day, seq.parse::<u32>().ok()?)
            }
            None => (stem, 0),
            _ => return None,
        };
        let day = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
        Some(Self {
            day,
            format,
            segment,
        })
    }
}

fn log_files(dir: &Path) -> io::Result<Vec<(LogName, PathBuf)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str().and_then(LogName::parse) {
            if entry.file_type()?.is_file() {
                files.push((name, entry.path()));
            }
        }
    }
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

struct Segment {
    dir: PathBuf,
    day: NaiveDate,
    file: File,
    size: u64,
}

impl Segment {
    fn open(dir: &Path, day: NaiveDate) -> io::Result<Self> {
        let files = log_files(dir)?;
        let mut seq = files
            .iter()
            .rev()
            .find(|(n, _)| n.format == Format::Text && n.day == day)
            .map_or(0, |(n, _)| n.segment);
        let path_for = |seq| {
            if seq == 0 {
                dir.join(format!("runtime-{day}.log"))
            } else {
                dir.join(format!("runtime-{day}.{seq}.log"))
            }
        };
        let mut path = path_for(seq);
        if fs::metadata(&path).is_ok_and(|m| m.len() >= SEGMENT_BYTES) {
            seq = seq
                .checked_add(1)
                .ok_or_else(|| io::Error::other("日志分段序号已满"))?;
            path = path_for(seq);
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        let mut total = SEGMENT_BYTES;
        for (name, old) in files.into_iter().rev().filter(|(_, old)| old != &path) {
            let Ok(metadata) = fs::metadata(&old) else {
                continue;
            };
            let bytes = metadata.len();
            if (day - name.day).num_days() >= KEEP_DAYS || total.saturating_add(bytes) > MAX_BYTES {
                if let Err(error) = fs::remove_file(&old) {
                    eprintln!("清理运行日志 {} 失败：{error}", old.display());
                    total = total.saturating_add(bytes);
                }
            } else {
                total += bytes;
            }
        }
        Ok(Self {
            dir: dir.to_owned(),
            day,
            file,
            size,
        })
    }

    fn append(&mut self, buf: &[u8]) -> io::Result<()> {
        let day = Local::now().date_naive();
        if self.day != day || self.size >= SEGMENT_BYTES {
            *self = Self::open(&self.dir, day)?;
        }
        self.file.write_all(buf)?;
        self.size += buf.len() as u64;
        Ok(())
    }
}

pub(super) struct FileWriter {
    segment: Mutex<Segment>,
    preview: Mutex<TailBuffer>,
}

impl FileWriter {
    pub(super) fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Self {
            segment: Mutex::new(Segment::open(dir, Local::now().date_naive())?),
            preview: Mutex::default(),
        })
    }

    pub(super) fn tail(&self, after: Option<u64>) -> TailPage {
        self.preview
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .since(after)
    }

    pub(super) fn write_panic(&self, info: &std::panic::PanicHookInfo<'_>) {
        let now = Local::now().to_rfc3339_opts(SecondsFormat::Millis, false);
        let thread = std::thread::current();
        let line = format!(
            "{now} ERROR 程序异常：线程 {}：{info}\n",
            thread.name().unwrap_or("?")
        );
        if let Ok(mut file) = self.segment.try_lock() {
            if file.file.write_all(line.as_bytes()).is_ok() {
                file.size += line.len() as u64;
                if let Ok(mut preview) = self.preview.try_lock() {
                    preview.append(&line);
                }
                return;
            }
        }
        eprint!("{line}");
    }
}

impl Write for &FileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut file = self.segment.lock().unwrap_or_else(|p| p.into_inner());
        file.append(buf)?;
        let mut preview = self.preview.lock().unwrap_or_else(|p| p.into_inner());
        drop(file);
        if let Ok(text) = std::str::from_utf8(buf) {
            preview.append(text);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.segment
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .file
            .flush()
    }
}

pub fn files_in_range(dir: &Path, from: i64, to: i64) -> io::Result<Vec<PathBuf>> {
    let from = Local
        .timestamp_millis_opt(from)
        .single()
        .map(|t| t.date_naive());
    let to = Local
        .timestamp_millis_opt(to)
        .single()
        .map(|t| t.date_naive());
    Ok(log_files(dir)?
        .into_iter()
        .filter(|(n, _)| from.is_none_or(|d| n.day >= d) && to.is_none_or(|d| n.day <= d))
        .map(|(_, path)| path)
        .collect())
}

impl Format {
    fn timestamp(self, line: &str) -> Option<i64> {
        match self {
            Self::Text => Some(
                DateTime::parse_from_rfc3339(line.split_whitespace().next()?)
                    .ok()?
                    .timestamp_millis(),
            ),
            Self::JsonLines => {
                #[derive(Deserialize)]
                struct Stamp {
                    ts: i64,
                }
                serde_json::from_str::<Stamp>(line).ok().map(|s| s.ts)
            }
            Self::LegacyText => {
                let stamp = NaiveDateTime::parse_from_str(
                    line.split_once(" [")?.0,
                    "%Y-%m-%d %H:%M:%S%.3f",
                )
                .ok()?;
                Some(
                    Local
                        .from_local_datetime(&stamp)
                        .earliest()?
                        .timestamp_millis(),
                )
            }
        }
    }
}

pub fn export_file(path: &Path, from: i64, to: i64, out: &mut Vec<u8>) -> io::Result<u64> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(LogName::parse)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "未知的运行日志文件"))?;
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut reader = BufReader::new(file.take(length));
    let mut line = String::new();
    let mut include = false;
    let mut count = 0;
    while reader.read_line(&mut line)? > 0 {
        if let Some(ts) = name.format.timestamp(&line) {
            include = (from..=to).contains(&ts);
            count += u64::from(include);
        } else if name.format == Format::JsonLines {
            include = false;
        }
        if include {
            out.extend_from_slice(line.as_bytes());
        }
        line.clear();
    }
    Ok(count)
}

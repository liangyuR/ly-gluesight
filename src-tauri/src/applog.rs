use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{Local, SecondsFormat, TimeZone};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;

pub mod events;
mod files;
mod tail;
pub mod throttle;

use files::FileWriter;
pub use files::{export_file, files_in_range};
pub use tail::TailPage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default)]
pub struct Fields {
    pub cycle_id: Option<String>,
    pub sn: Option<String>,
    pub request_seq: Option<u32>,
    pub recipe_id: Option<String>,
    pub camera: Option<String>,
    pub seq: Option<u64>,
    pub stage: Option<String>,
    pub source: Option<String>,
    pub data: Option<Value>,
}

impl Fields {
    pub fn stage(stage: &str) -> Self {
        Self {
            stage: Some(stage.into()),
            ..Self::default()
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn with_stage(mut self, stage: &str) -> Self {
        self.stage = Some(stage.into());
        self
    }

    fn with_active_cycle(mut self) -> Self {
        let active = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(active) = active.as_ref() {
            if self.cycle_id.is_none() || self.cycle_id == active.cycle_id {
                self.cycle_id = self.cycle_id.or_else(|| active.cycle_id.clone());
                self.sn = self.sn.or_else(|| active.sn.clone());
                self.request_seq = self.request_seq.or(active.request_seq);
                self.recipe_id = self.recipe_id.or_else(|| active.recipe_id.clone());
            }
        }
        self
    }
}

static ACTIVE: Mutex<Option<Fields>> = Mutex::new(None);

pub fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::log_storage::runtime_dir(app)
}

pub fn set_project(id: &str) {
    tracing::info!(project = id, "打开项目");
}

struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        write!(
            writer,
            "{}",
            Local::now().to_rfc3339_opts(SecondsFormat::Millis, false)
        )
    }
}

fn subscriber(writer: Arc<FileWriter>) -> impl tracing::Subscriber + Send + Sync {
    let format = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_timer(LocalTime)
        .with_writer(writer)
        .log_internal_errors(true);
    tracing_subscriber::registry().with(format).with(
        Targets::new()
            .with_target("gluesight", LevelFilter::DEBUG)
            .with_default(LevelFilter::WARN),
    )
}

pub fn init(app: &AppHandle) -> Result<(), String> {
    let dir = dir(app)?;
    let writer = Arc::new(
        FileWriter::open(&dir)
            .map_err(|err| format!("打开运行日志失败（{}）：{err}", dir.display()))?,
    );
    tracing::subscriber::set_global_default(subscriber(writer.clone()))
        .map_err(|e| e.to_string())?;
    app.manage(writer.clone());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        writer.write_panic(info);
        previous(info);
    }));
    let identity = build_identity();
    tracing::info!(version = %app.package_info().version, git = identity.git_sha, build = identity.build_time,
        lyflow = crate::vision::LYFLOW_SERIES, data_root = %crate::storage::root(app).display(), "启动");
    Ok(())
}

#[tauri::command]
pub fn app_log_tail(app: AppHandle, after: Option<u64>) -> Result<TailPage, String> {
    let writer = app
        .try_state::<Arc<FileWriter>>()
        .ok_or("运行日志尚未就绪")?;
    Ok(writer.tail(after))
}

pub struct BuildIdentity {
    pub git_sha: &'static str,
    pub build_time: String,
}

pub fn build_identity() -> BuildIdentity {
    let ms = env!("GLUESIGHT_BUILD_MS").parse::<i64>().unwrap_or(0);
    BuildIdentity {
        git_sha: env!("GLUESIGHT_GIT_SHA"),
        build_time: local_time(ms).unwrap_or_default(),
    }
}

pub fn local_time(ts: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, false))
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

pub fn emit_event(event: &events::Event, msg: &str, fields: Fields) {
    emit(event.level, event.ev, msg, fields);
}

pub fn emit(level: Level, ev: &str, msg: &str, fields: Fields) {
    let f = fields.with_active_cycle();
    macro_rules! log {
        ($level:expr) => {
            tracing::event!(
                $level,
                cycle_id = f.cycle_id.as_deref(),
                sn = f.sn.as_deref(),
                part_seq = f.request_seq,
                recipe = f.recipe_id.as_deref(),
                camera = f.camera.as_deref(),
                seq = f.seq,
                stage = f.stage.as_deref(),
                source = f.source.as_deref(),
                data = f.data.as_ref().map(tracing::field::display),
                "{ev}：{msg}"
            )
        };
    }
    match level {
        Level::Debug => log!(tracing::Level::DEBUG),
        Level::Info => log!(tracing::Level::INFO),
        Level::Warn => log!(tracing::Level::WARN),
        Level::Error => log!(tracing::Level::ERROR),
    }
}

#[cfg(test)]
mod tests;

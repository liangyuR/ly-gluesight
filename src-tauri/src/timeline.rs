use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::applog::{self, Level, Record};
use crate::store::Store;

const RECORD_CAP: usize = 3000;
const PLC_CAP: u32 = 400;
const PLC_MARGIN_MS: i64 = 1000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub ts: i64,
    pub source: &'static str,
    pub level: Level,
    pub ev: String,
    pub msg: String,
    pub stage: Option<String>,
    pub k: Option<usize>,
    pub shot_id: Option<String>,
    pub camera: Option<String>,
    /// 段配方：事件所属帧组序号（与 camera 一起定位一帧）；旧拍照点事件为 None
    pub seq: Option<u64>,
    pub data: Option<Value>,
}

impl From<Record> for Entry {
    fn from(r: Record) -> Self {
        Self { ts: r.ts, source: "app", level: r.level, ev: r.ev, msg: r.msg, stage: r.fields.stage, k: r.fields.k,
            shot_id: r.fields.shot_id, camera: r.fields.camera, seq: r.fields.seq, data: r.fields.data }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub key: &'static str,
    pub label: &'static str,
    pub ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub cycle_id: String,
    pub request_seq: Option<u32>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub stages: Vec<Stage>,
    pub entries: Vec<Entry>,
    pub truncated: bool,
    pub notes: Vec<String>,
}

const STAGES: [(&str, &str, &str, &str); 5] = [
    ("intake", "接件登记", "partStart", "admitted"),
    ("acquire", "运动与采集", "admitted", "partEnd"),
    ("drain", "收尾与判定", "partEnd", "result"),
    ("ack", "等待 PLC 确认", "result", "released"),
    ("total", "整件", "partStart", "released"),
];

/// 各阶段耗时取相邻关键事件的首次出现时间；缺任一端点的阶段记为未知，不拿别的事件顶替。
pub fn stages(records: &[Record]) -> Vec<Stage> {
    let mark = |stage: &str| records.iter().find(|r| r.fields.stage.as_deref() == Some(stage)).map(|r| r.ts);
    STAGES.iter().map(|&(key, label, from, to)| Stage { key, label, ms: mark(from).zip(mark(to)).map(|(a, b)| b - a).filter(|ms| *ms >= 0) }).collect()
}

pub fn build(cycle_id: String, records: Vec<Record>, plc: Vec<Entry>, truncated: bool) -> Timeline {
    let stages = stages(&records);
    let request_seq = records.iter().find_map(|r| r.fields.request_seq);
    let mut entries: Vec<Entry> = records.into_iter().map(Entry::from).chain(plc).collect();
    entries.sort_by_key(|e| (e.ts, e.source != "plc"));
    let start = entries.first().map(|e| e.ts);
    let end = entries.last().map(|e| e.ts);
    Timeline { cycle_id, request_seq, start, end, stages, entries, truncated, notes: Vec::new() }
}

fn plc_entries(app: &AppHandle, from: i64, to: i64) -> Result<(Vec<Entry>, bool), String> {
    let host = app.state::<crate::plc::PlcHost>();
    let engine = host.engine();
    let quiet: Vec<String> = engine.config().points.iter()
        .filter(|p| p.tags.iter().any(|t| t == "plcHeartbeat" || t == "pcHeartbeat")).map(|p| p.id.clone()).collect();
    let page = engine.logbook().query(&ly_plc::LogQuery { start: Some(from), end: Some(to), levels: Vec::new(), categories: Vec::new(),
        point_id: None, context: None, keyword: None, limit: Some(PLC_CAP), offset: Some(0) })?;
    let truncated = page.total > i64::from(PLC_CAP);
    let entries = page.items.into_iter().filter(|e| e.point_id.as_ref().is_none_or(|id| !quiet.contains(id))).map(|e| {
        let change = match (&e.old_value, &e.new_value) {
            (Some(old), Some(new)) => format!("：{old} → {new}"),
            (None, Some(new)) => format!("：{new}"),
            _ => String::new(),
        };
        Entry { ts: e.ts, source: "plc", level: Level::from_tone(&e.level), ev: e.point_name.clone().unwrap_or_else(|| e.category.clone()),
            msg: format!("{}{change}", e.message), stage: None, k: None, shot_id: None, camera: None, seq: None,
            data: Some(serde_json::json!({"category": e.category, "pointId": e.point_id})) }
    }).collect();
    Ok((entries, truncated))
}

#[tauri::command]
pub async fn history_timeline(app: AppHandle, id: i64) -> Result<Timeline, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let detail = app.state::<Store>().detail(id)?;
        let cycle_id = detail.summary.cycle_id.clone().ok_or("记录没有 cycleId，无法关联运行日志")?;
        let applog::CycleLog { records, truncated, previous_end, next_start } = applog::cycle_records(&applog::dir(&app)?, &cycle_id, detail.summary.ts, RECORD_CAP);
        let mut notes = Vec::new();
        if records.is_empty() {
            notes.push("没有找到本件的运行日志：日志可能已按保留策略清理，或记录早于结构化日志".to_string());
        }
        if truncated { notes.push(format!("本件运行日志超过 {RECORD_CAP} 条，只显示前 {RECORD_CAP} 条")); }
        let from = (records.first().map_or(detail.summary.ts - 120_000, |r| r.ts) - PLC_MARGIN_MS).max(previous_end.map_or(i64::MIN, |end| end + 1));
        let to = (records.last().map_or(detail.summary.ts, |r| r.ts.max(detail.summary.ts)) + PLC_MARGIN_MS).min(next_start.map_or(i64::MAX, |start| start - 1));
        let plc = match plc_entries(&app, from, to) {
            Ok((entries, plc_truncated)) => {
                if plc_truncated { notes.push(format!("该时段 PLC 日志超过 {PLC_CAP} 条，只显示最近 {PLC_CAP} 条")); }
                entries
            }
            Err(error) => { notes.push(format!("PLC 日志读取失败：{error}")); Vec::new() }
        };
        let mut timeline = build(cycle_id, records, plc, truncated);
        timeline.notes = notes;
        Ok(timeline)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn history_find_cycle(app: AppHandle, cycle_id: String) -> Result<Option<i64>, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(app.state::<Store>().detail_by_cycle(&cycle_id)?.map(|d| d.summary.id)))
        .await.map_err(|e| e.to_string())?
}

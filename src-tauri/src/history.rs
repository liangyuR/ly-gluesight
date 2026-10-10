use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::cycle::CycleHost;
use crate::judge::{self, Verdict};
use crate::recipe::{JudgeParams, Recipe};
use crate::store::{measurement_layout_hash, same_measurement_layout, HistoryPage, HistoryQuery, PartDetail, PartSummary, Store, StoredMeasurement};

pub fn local_midnight_ms() -> i64 {
    let today = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    Local.from_local_datetime(&today).earliest().map_or(0, |t| t.timestamp_millis())
}

pub fn local_day() -> i64 {
    Local::now().date_naive().to_epoch_days() as i64
}

fn format_ts(ts: i64) -> String {
    Local.timestamp_millis_opt(ts).single().map_or_else(|| ts.to_string(), |t| t.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
}

#[tauri::command]
pub async fn history_query(app: AppHandle, query: HistoryQuery) -> Result<HistoryPage, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Store>().query(&query)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn history_detail(app: AppHandle, id: i64) -> Result<PartDetail, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Store>().detail(id)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn history_recipe(store: State<'_, Store>, cycle: State<'_, CycleHost>, hash: Option<String>, recipe_id: Option<String>) -> Result<Option<Recipe>, String> {
    record_recipe(&store, hash.as_deref(), || recipe_id.as_deref().and_then(|id| cycle.recipe(id)))
}

fn record_recipe(store: &Store, hash: Option<&str>, current: impl FnOnce() -> Option<Arc<Recipe>>) -> Result<Option<Recipe>, String> {
    match hash {
        Some(hash) => store.recipe_snapshot(hash),
        None => Ok(current().map(|r| (*r).clone())),
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KindOverride {
    pub tol_upper: Option<f32>,
    pub tol_lower: Option<f32>,
    pub abs_min: Option<f32>,
    pub abs_max: Option<f32>,
    pub max_excursion_len: Option<f32>,
}

impl KindOverride {
    fn apply(&self, p: &mut JudgeParams) {
        p.tol_upper = self.tol_upper.unwrap_or(p.tol_upper);
        p.tol_lower = self.tol_lower.unwrap_or(p.tol_lower);
        p.abs_min = self.abs_min.unwrap_or(p.abs_min);
        p.abs_max = self.abs_max.unwrap_or(p.abs_max);
        p.max_excursion_len = self.max_excursion_len.unwrap_or(p.max_excursion_len);
    }
}

/// 试算参数：覆盖配方中的判定参数，只用于本次重判，不保存。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Overrides {
    /// 允许断胶长度（作用于每个拍照点）
    pub max_gap_len: Option<f32>,
    pub filter_window: Option<usize>,
    /// 位置限值（只作用于判位置的拍照点）
    pub position: KindOverride,
    /// 胶宽限值（只作用于判胶宽的拍照点）
    pub width: KindOverride,
}

impl Overrides {
    fn apply(&self, recipe: &Recipe) -> Recipe {
        let mut r = recipe.clone();
        r.filter_window = self.filter_window.unwrap_or(r.filter_window);
        for seg in &mut r.segments {
            seg.max_gap_len = self.max_gap_len.unwrap_or(seg.max_gap_len);
            if let Some(p) = seg.position.as_mut() {
                self.position.apply(p);
            }
            if let Some(w) = seg.width.as_mut() {
                self.width.apply(w);
            }
        }
        r
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RejudgeRequest {
    pub query: HistoryQuery,
    pub ids: Vec<i64>,
    pub use_current_recipe: bool,
    pub overrides: Overrides,
}

#[derive(Clone, Debug, Serialize)]
pub struct MatrixCell {
    pub from: Verdict,
    pub to: Verdict,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub id: i64,
    pub sn: u32,
    pub ts: i64,
    pub from: Verdict,
    pub to: Verdict,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RejudgeSkip {
    pub id: i64,
    pub sn: u32,
    pub ts: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejudgeResult {
    pub total: u64,
    pub skipped: u64,
    pub limit_hit: bool,
    pub matrix: Vec<MatrixCell>,
    pub changes: Vec<Change>,
    pub skip_reasons: Vec<RejudgeSkip>,
}

const REJUDGE_LIMIT: usize = 5000;

/// 用存储的测量表重新判定，不需要重新拍照。ERR 件与缺少测量数据的记录跳过。
#[tauri::command]
pub async fn history_rejudge(app: AppHandle, request: RejudgeRequest) -> Result<RejudgeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let cycle = app.state::<CycleHost>();
        let rows = store.measurements(&request.query, &request.ids, REJUDGE_LIMIT + 1)?;
        rejudge_rows(rows, &request, |hash| store.recipe_snapshot(hash).map(|r| r.map(Arc::new)), |id| cycle.recipe(id))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn cached_recipe(
    cache: &mut HashMap<String, Option<Arc<Recipe>>>,
    key: &str,
    load: impl FnOnce() -> Result<Option<Arc<Recipe>>, String>,
) -> Result<Option<Arc<Recipe>>, String> {
    if let Some(value) = cache.get(key) {
        return Ok(value.clone());
    }
    let value = load()?;
    cache.insert(key.to_string(), value.clone());
    Ok(value)
}

fn rejudge_rows(
    rows: Vec<StoredMeasurement>,
    request: &RejudgeRequest,
    mut snapshot: impl FnMut(&str) -> Result<Option<Arc<Recipe>>, String>,
    mut current: impl FnMut(&str) -> Option<Arc<Recipe>>,
) -> Result<RejudgeResult, String> {
    let limit_hit = rows.len() > REJUDGE_LIMIT;
    let mut originals = HashMap::new();
    let mut candidates = HashMap::new();
    let mut matrix: HashMap<(Verdict, Verdict), u64> = HashMap::new();
    let mut changes = Vec::new();
    let mut skip_reasons = Vec::new();
    let (mut total, mut skipped) = (0, 0);
    for row in rows.into_iter().take(REJUDGE_LIMIT) {
        let mut skip = |reason: &str| {
            skipped += 1;
            if skip_reasons.len() < 300 {
                skip_reasons.push(RejudgeSkip { id: row.id, sn: row.sn, ts: row.ts, reason: reason.into() });
            }
        };
        if row.verdict == Verdict::ErrInspect {
            skip("原检测为 ERR，不能仅靠判定参数复原测量");
            continue;
        }
        if row.table.is_empty() {
            skip("原记录缺少测量点数据");
            continue;
        }
        let Some(hash) = row.recipe_hash.as_deref() else {
            skip("原记录缺少配方快照身份");
            continue;
        };
        let Some(original) = cached_recipe(&mut originals, hash, || snapshot(hash))? else {
            skip("原配方快照缺失，无法核对测点布局");
            continue;
        };
        if row.table.len() != original.point_count() || row.layout_hash.as_deref() != Some(measurement_layout_hash(&original).as_str()) {
            skip("存储测量点与原配方布局不一致");
            continue;
        }
        let recipe = if request.use_current_recipe {
            let Some(id) = row.recipe_id.as_deref() else {
                skip("原记录缺少配方编号");
                continue;
            };
            let Some(candidate) = cached_recipe(&mut candidates, id, || Ok(current(id)))? else {
                skip("当前候选配方不存在");
                continue;
            };
            if !same_measurement_layout(&original, &candidate) {
                skip("当前候选的拍照点、相机、视角、测点几何或单位已改变，需要使用原图复测");
                continue;
            }
            candidate
        } else {
            original
        };
        let j = judge::judge(&request.overrides.apply(&recipe), &row.table);
        total += 1;
        *matrix.entry((row.verdict, j.verdict)).or_default() += 1;
        if j.verdict != row.verdict && changes.len() < 300 {
            changes.push(Change { id: row.id, sn: row.sn, ts: row.ts, from: row.verdict, to: j.verdict, reason: j.reason });
        }
    }
    let mut matrix: Vec<_> = matrix.into_iter().map(|((from, to), count)| MatrixCell { from, to, count }).collect();
    matrix.sort_by_key(|c| (c.from, c.to));
    Ok(RejudgeResult { total, skipped, limit_hit, matrix, changes, skip_reasons })
}

fn csv_row(p: &PartSummary) -> String {
    let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let verdict = serde_json::to_value(p.verdict).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let delivery = serde_json::to_value(p.delivery.state).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let cells = [
        format_ts(p.ts),
        p.sn.to_string(),
        esc(p.recipe_id.as_deref().unwrap_or("")),
        p.recipe_version.map(|v| v.to_string()).unwrap_or_default(),
        esc(p.recipe_hash.as_deref().unwrap_or("")),
        match p.trigger_mode.as_deref() { Some("fly") => "飞拍", Some("stop") => "停稳拍", _ => "" }.into(),
        verdict,
        p.plc_code.to_string(),
        p.fault_code.to_string(),
        esc(&p.reason),
        p.frames_received.to_string(),
        p.frames_expected.to_string(),
        p.drain_ms.map(|v| v.to_string()).unwrap_or_default(),
        p.retest_of.map(|v| v.to_string()).unwrap_or_default(),
        esc(p.cycle_id.as_deref().unwrap_or("")),
        esc(p.bundle_hash.as_deref().unwrap_or("")),
        delivery,
        format_ts(p.delivery.updated_at),
        esc(p.delivery.message.as_deref().unwrap_or("")),
    ];
    format!("{}\r\n", cells.join(","))
}

/// 按筛选条件导出 CSV（带 BOM，Excel 可直接打开），返回文件路径。
#[tauri::command]
pub async fn history_export(app: AppHandle, query: HistoryQuery) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("exports");
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("检测记录_{}.csv", Local::now().format("%Y%m%d_%H%M%S")));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        write!(f, "\u{feff}时间,SN,配方,版本,配置哈希,模式,结果,PLC 结果码,异常码,原因,收到帧,计划帧,收尾耗时(ms),复检自,cycleId,发布包哈希,PLC 交付状态,PLC 交付更新时间,PLC 交付说明\r\n").map_err(|e| e.to_string())?;
        let store = app.state::<Store>();
        let mut q = HistoryQuery { limit: 500, offset: 0, ..query };
        loop {
            let page = store.query(&q)?;
            for p in &page.items {
                f.write_all(csv_row(p).as_bytes()).map_err(|e| e.to_string())?;
            }
            if page.items.len() < q.limit as usize {
                break;
            }
            q.offset += q.limit;
        }
        f.flush().map_err(|e| e.to_string())?;
        Ok(path.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "history/tests.rs"]
mod tests;

#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

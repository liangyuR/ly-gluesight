use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::cycle::CycleHost;
use crate::judge::{self, Verdict};
use crate::recipe::{JudgeParams, Recipe};
use crate::store::{HistoryPage, HistoryQuery, PartDetail, Store};

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

/// 记录所用的配方快照；快照缺失时退回当前同名配方。
#[tauri::command]
pub fn history_recipe(store: State<'_, Store>, cycle: State<'_, CycleHost>, hash: Option<String>, recipe_id: Option<String>) -> Result<Option<Recipe>, String> {
    if let Some(r) = hash.as_deref().map(|h| store.recipe_snapshot(h)).transpose()?.flatten() {
        return Ok(Some(r));
    }
    Ok(recipe_id.as_deref().and_then(|id| cycle.recipe(id)).map(|r| (*r).clone()))
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
#[serde(rename_all = "camelCase")]
pub struct RejudgeResult {
    pub total: u64,
    pub skipped: u64,
    pub limit_hit: bool,
    pub matrix: Vec<MatrixCell>,
    pub changes: Vec<Change>,
}

const REJUDGE_LIMIT: usize = 5000;

/// 用存储的测量表重新判定，不需要重新拍照。ERR 件与缺少测量数据的记录跳过。
#[tauri::command]
pub async fn history_rejudge(app: AppHandle, request: RejudgeRequest) -> Result<RejudgeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let cycle = app.state::<CycleHost>();
        let rows = store.measurements(&request.query, &request.ids, REJUDGE_LIMIT + 1)?;
        let limit_hit = rows.len() > REJUDGE_LIMIT;
        let mut recipes: HashMap<String, Option<Arc<Recipe>>> = HashMap::new();
        let mut matrix: HashMap<(Verdict, Verdict), u64> = HashMap::new();
        let mut changes = Vec::new();
        let (mut total, mut skipped) = (0, 0);
        for row in rows.into_iter().take(REJUDGE_LIMIT) {
            let key = if request.use_current_recipe { row.recipe_id.clone() } else { row.recipe_hash.clone() }.unwrap_or_default();
            let recipe = recipes
                .entry(key.clone())
                .or_insert_with(|| {
                    let base = if request.use_current_recipe {
                        row.recipe_id.as_deref().and_then(|id| cycle.recipe(id))
                    } else {
                        store.recipe_snapshot(&key).ok().flatten().map(Arc::new)
                    };
                    base.map(|r| Arc::new(request.overrides.apply(&r)))
                })
                .clone();
            let Some(recipe) = recipe.filter(|r| r.point_count() == row.table.len()) else {
                skipped += 1;
                continue;
            };
            if row.verdict == Verdict::ErrInspect {
                skipped += 1;
                continue;
            }
            let j = judge::judge(&recipe, &row.table);
            total += 1;
            *matrix.entry((row.verdict, j.verdict)).or_default() += 1;
            if j.verdict != row.verdict && changes.len() < 300 {
                changes.push(Change { id: row.id, sn: row.sn, ts: row.ts, from: row.verdict, to: j.verdict, reason: j.reason });
            }
        }
        let mut matrix: Vec<_> = matrix.into_iter().map(|((from, to), count)| MatrixCell { from, to, count }).collect();
        matrix.sort_by_key(|c| (c.from, c.to));
        Ok(RejudgeResult { total, skipped, limit_hit, matrix, changes })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 按筛选条件导出 CSV（带 BOM，Excel 可直接打开），返回文件路径。
#[tauri::command]
pub async fn history_export(app: AppHandle, query: HistoryQuery) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("exports");
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("检测记录_{}.csv", Local::now().format("%Y%m%d_%H%M%S")));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        write!(f, "\u{feff}时间,SN,配方,版本,配置哈希,模式,结果,PLC 结果码,异常码,原因,收到帧,计划帧,收尾耗时(ms),复检自\r\n").map_err(|e| e.to_string())?;
        let store = app.state::<Store>();
        let mut q = HistoryQuery { limit: 500, offset: 0, ..query };
        loop {
            let page = store.query(&q)?;
            for p in &page.items {
                let verdict = serde_json::to_value(p.verdict).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
                write!(
                    f,
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\r\n",
                    format_ts(p.ts),
                    p.sn,
                    p.recipe_id.as_deref().unwrap_or(""),
                    p.recipe_version.map(|v| v.to_string()).unwrap_or_default(),
                    p.recipe_hash.as_deref().map(|h| &h[..h.len().min(12)]).unwrap_or(""),
                    match p.trigger_mode.as_deref() {
                        Some("fly") => "飞拍",
                        Some("stop") => "停稳拍",
                        _ => "",
                    },
                    verdict,
                    p.plc_code,
                    p.fault_code,
                    esc(&p.reason),
                    p.frames_received,
                    p.frames_expected,
                    p.drain_ms.map(|v| v.to_string()).unwrap_or_default(),
                    p.retest_of.map(|v| v.to_string()).unwrap_or_default(),
                )
                .map_err(|e| e.to_string())?;
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

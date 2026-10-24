use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::burst_judge::{self, BurstJudgement, Flags, FrameState};
use crate::cycle::CycleHost;
use crate::frame_group::FrameGroup;
use crate::judge::{self, Verdict};
use crate::plan::{PathSegment, SegmentKind, WidthCheck};
use crate::recipe::{JudgeParams, Recipe};
use crate::segmentation::RegionKind;
use crate::store::{same_measurement_layout, FrameRecord, HistoryPage, HistoryQuery, HistoryScope, PartDetail, PartSummary, Store, StoredMeasurement};

pub fn local_midnight_ms() -> i64 {
    let today = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    Local.from_local_datetime(&today).earliest().map_or(0, |t| t.timestamp_millis())
}

pub fn local_day() -> i64 {
    Local::now().date_naive().to_epoch_days() as i64
}

pub(crate) fn format_ts(ts: i64) -> String {
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

#[derive(Serialize)]
pub struct Neighbors {
    pub older: Option<i64>,
    pub newer: Option<i64>,
}

/// 逐件回看：按历史列表当时的筛选找前后一件；没有筛选时只在本件所属的真机 / 离线 / 调试范围内找。
#[tauri::command]
pub async fn history_neighbors(app: AppHandle, id: i64, query: Option<HistoryQuery>) -> Result<Neighbors, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let query = match query {
            Some(query) => query,
            None => HistoryQuery { scope: HistoryScope::of(store.detail(id)?.summary.source), ..HistoryQuery::default() },
        };
        let (older, newer) = store.neighbors(&query, id)?;
        Ok(Neighbors { older, newer })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn history_recipe(store: State<'_, Store>, cycle: State<'_, CycleHost>, revision_id: Option<String>, recipe_id: Option<String>) -> Result<Option<Recipe>, String> {
    record_recipe(&store, revision_id.as_deref(), || recipe_id.as_deref().and_then(|id| cycle.recipe(id)))
}

fn record_recipe(store: &Store, revision_id: Option<&str>, _current: impl FnOnce() -> Option<Arc<Recipe>>) -> Result<Option<Recipe>, String> {
    match revision_id {
        Some(revision_id) => store.recipe_snapshot(revision_id),
        None => Ok(None),
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
    /// 段配方记录：直线段的胶宽允差（上 / 下公差、帧内允许超差长度）；绝对限不适用
    pub straight: KindOverride,
    /// 段配方记录：弯道段的胶宽允差，字段同 straight；未分段区域按所覆盖各段取较宽一档（burst_judge）
    pub corner: KindOverride,
}

impl KindOverride {
    /// 段配方：覆盖一段的胶宽允差。abs_min / abs_max 没有对应的段级限值，不适用。
    fn apply_width(&self, width: &mut WidthCheck) {
        width.tol_upper = self.tol_upper.map_or(width.tol_upper, f64::from);
        width.tol_lower = self.tol_lower.map_or(width.tol_lower, f64::from);
        width.max_excursion_mm = self.max_excursion_len.map_or(width.max_excursion_mm, f64::from);
    }
}

impl Overrides {
    /// 段配方记录：直线段用 straight、弯道段用 corner 覆盖胶宽允差；允许断胶长度作用于每帧（每段）。
    fn apply_segments(&self, segments: &[PathSegment]) -> Vec<PathSegment> {
        segments
            .iter()
            .cloned()
            .map(|mut segment| {
                let kind = match segment.kind {
                    SegmentKind::Straight => &self.straight,
                    SegmentKind::Corner => &self.corner,
                };
                kind.apply_width(&mut segment.checks.width);
                segment.checks.gap.max_gap_mm = self.max_gap_len.map_or(segment.checks.gap.max_gap_mm, f64::from);
                segment
            })
            .collect()
    }

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
    pub sn: String,
    pub ts: i64,
    pub from: Verdict,
    pub to: Verdict,
    pub reason: String,
    /// 段配方记录重判后的整件标记（检测不完整、疑似提前终止等与阈值无关，照常带出）；旧记录为 None
    pub flags: Option<Flags>,
}

/// 重判跳过一条记录的原因种类；`RejudgeSkip::reason` 是给界面看的说明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipKind {
    ErrResult,
    NoTable,
    NoRevision,
    SnapshotMissing,
    LayoutMismatch,
    NoRecipeId,
    NoCandidate,
    LayoutChanged,
}

impl SkipKind {
    fn reason(self) -> &'static str {
        match self {
            Self::ErrResult => "原检测为 ERR，不能仅靠判定参数复原测量",
            Self::NoTable => "原记录缺少测量点数据",
            Self::NoRevision => "原记录缺少配方快照身份",
            Self::SnapshotMissing => "原配方快照缺失，无法核对测点布局",
            Self::LayoutMismatch => "存储测量点与原配方布局不一致",
            Self::NoRecipeId => "原记录缺少配方编号",
            Self::NoCandidate => "当前候选配方不存在",
            Self::LayoutChanged => "当前候选的拍照点、相机、视角、测点几何、特征定位（模板来源）或单位已改变（段配方：段序列 / 类型 / 预期帧数、相机或走廊已改变），需要使用原图复测",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RejudgeSkip {
    pub id: i64,
    pub sn: String,
    pub ts: i64,
    pub kind: SkipKind,
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
        rejudge_rows(rows, &request, |revision_id| store.recipe_snapshot(revision_id).map(|r| r.map(Arc::new)), |id| cycle.recipe(id), |id| store.burst_frames(id))
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

/// 段配方记录的帧组按帧记录还原（与 Grouper 同一口径）：到了的帧（不论测没测成）算 present，缺帧不算。
fn frame_groups(frames: &[FrameRecord], cameras: usize) -> Vec<FrameGroup> {
    let total = frames.iter().map(|frame| frame.seq).max().unwrap_or(0);
    (1..=total)
        .map(|seq| {
            let present: Vec<bool> = (0..cameras)
                .map(|camera| frames.iter().any(|f| f.camera == camera && f.seq == seq && f.status != FrameState::Missing))
                .collect();
            FrameGroup { seq, incomplete: present.contains(&false), present }
        })
        .collect()
}

/// 段配方记录的区域都落在原配方的段内、帧都属于原配方的计划相机；否则存储与原配方布局不一致。
fn burst_layout_matches(recipe: &Recipe, judgement: &BurstJudgement, frames: &[FrameRecord]) -> bool {
    let segments = recipe.path_segments.len() as u32;
    let cameras = recipe.plan_cameras().len();
    let inside = |first: u32, last: u32| 1 <= first && first <= last && last <= segments;
    judgement.regions.iter().all(|result| match result.region.kind {
        RegionKind::Straight { segment } | RegionKind::Corner { segment } => inside(segment, segment),
        RegionKind::Unsegmented { first_segment, last_segment, .. } => inside(first_segment, last_segment),
    }) && frames.iter().all(|frame| frame.camera < cameras)
}

/// 段配方记录重判：用已存的每帧指标、记录里的区域和丢帧告警，按（覆盖了允差的）配方段重跑 burst_judge。
fn rejudge_burst(recipe: &Recipe, overrides: &Overrides, judgement: &BurstJudgement, frames: &[FrameRecord]) -> BurstJudgement {
    let cameras = recipe.plan_cameras();
    let segments = overrides.apply_segments(&recipe.path_segments);
    let regions: Vec<_> = judgement.regions.iter().map(|result| result.region.clone()).collect();
    let metrics: Vec<_> = frames.iter().filter_map(|frame| frame.metrics.clone()).collect();
    burst_judge::judge(&segments, recipe.expected_total(), &regions, &frame_groups(frames, cameras.len()), &metrics, &cameras, &judgement.alarms)
}

fn rejudge_rows(
    rows: Vec<StoredMeasurement>,
    request: &RejudgeRequest,
    mut snapshot: impl FnMut(&str) -> Result<Option<Arc<Recipe>>, String>,
    mut current: impl FnMut(&str) -> Option<Arc<Recipe>>,
    mut burst_frames: impl FnMut(i64) -> Result<Vec<FrameRecord>, String>,
) -> Result<RejudgeResult, String> {
    let limit_hit = rows.len() > REJUDGE_LIMIT;
    let mut originals = HashMap::new();
    let mut candidates = HashMap::new();
    let mut matrix: HashMap<(Verdict, Verdict), u64> = HashMap::new();
    let mut changes = Vec::new();
    let mut skip_reasons = Vec::new();
    let (mut total, mut skipped) = (0, 0);
    for row in rows.into_iter().take(REJUDGE_LIMIT) {
        let mut skip = |kind: SkipKind| {
            skipped += 1;
            if skip_reasons.len() < 300 {
                skip_reasons.push(RejudgeSkip { id: row.id, sn: row.sn.clone(), ts: row.ts, kind, reason: kind.reason().into() });
            }
        };
        if row.verdict == Verdict::ErrInspect {
            skip(SkipKind::ErrResult);
            continue;
        }
        if let Some(burst) = &row.burst {
            // 段配方记录：区域用记录里的，指标用 part_frames 的，允差按 kind 覆盖；计划身份或走廊变了跳过
            let Some(revision_id) = row.recipe_revision.as_deref() else {
                skip(SkipKind::NoRevision);
                continue;
            };
            let Some(original) = cached_recipe(&mut originals, revision_id, || snapshot(revision_id))? else {
                skip(SkipKind::SnapshotMissing);
                continue;
            };
            let frames = burst_frames(row.id)?;
            if frames.is_empty() {
                skip(SkipKind::NoTable);
                continue;
            }
            if !original.is_burst() || row.recipe_id.as_deref() != Some(original.id.as_str()) || revision_id != original.revision_id
                || !burst_layout_matches(&original, burst, &frames) {
                skip(SkipKind::LayoutMismatch);
                continue;
            }
            let recipe = if request.use_current_recipe {
                let Some(candidate) = cached_recipe(&mut candidates, &original.id, || Ok(current(&original.id)))? else {
                    skip(SkipKind::NoCandidate);
                    continue;
                };
                if !same_measurement_layout(&original, &candidate) {
                    skip(SkipKind::LayoutChanged);
                    continue;
                }
                candidate
            } else {
                original
            };
            let j = rejudge_burst(&recipe, &request.overrides, burst, &frames);
            total += 1;
            *matrix.entry((row.verdict, j.verdict)).or_default() += 1;
            if j.verdict != row.verdict && changes.len() < 300 {
                changes.push(Change { id: row.id, sn: row.sn.clone(), ts: row.ts, from: row.verdict, to: j.verdict, reason: j.reason, flags: Some(j.flags) });
            }
            continue;
        }
        if row.table.is_empty() {
            skip(SkipKind::NoTable);
            continue;
        }
        let Some(revision_id) = row.recipe_revision.as_deref() else {
            skip(SkipKind::NoRevision);
            continue;
        };
        let Some(original) = cached_recipe(&mut originals, revision_id, || snapshot(revision_id))? else {
            skip(SkipKind::SnapshotMissing);
            continue;
        };
        if row.table.len() != original.point_count() || row.recipe_id.as_deref() != Some(original.id.as_str()) || revision_id != original.revision_id {
            skip(SkipKind::LayoutMismatch);
            continue;
        }
        let recipe = if request.use_current_recipe {
            let Some(id) = row.recipe_id.as_deref() else {
                skip(SkipKind::NoRecipeId);
                continue;
            };
            let Some(candidate) = cached_recipe(&mut candidates, id, || Ok(current(id)))? else {
                skip(SkipKind::NoCandidate);
                continue;
            };
            if !same_measurement_layout(&original, &candidate) {
                skip(SkipKind::LayoutChanged);
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
            changes.push(Change { id: row.id, sn: row.sn.clone(), ts: row.ts, from: row.verdict, to: j.verdict, reason: j.reason, flags: None });
        }
    }
    let mut matrix: Vec<_> = matrix.into_iter().map(|((from, to), count)| MatrixCell { from, to, count }).collect();
    matrix.sort_by_key(|c| (c.from, c.to));
    Ok(RejudgeResult { total, skipped, limit_hit, matrix, changes, skip_reasons })
}

pub(crate) fn csv_row(p: &PartSummary) -> String {
    let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let verdict = serde_json::to_value(p.verdict).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let delivery = serde_json::to_value(p.delivery.state).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let burst = p.burst.as_ref();
    let cells = [
        format_ts(p.ts),
        p.source.label().into(),
        esc(&p.sn),
        esc(p.recipe_id.as_deref().unwrap_or("")),
        p.recipe_version.map(|v| v.to_string()).unwrap_or_default(),
        esc(p.recipe_revision.as_deref().unwrap_or("")),
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
        esc(p.bundle_id.as_deref().unwrap_or("")),
        delivery,
        format_ts(p.delivery.updated_at),
        esc(p.delivery.message.as_deref().unwrap_or("")),
        // 段配方：ΣN_exp、Σtol、各相机帧数（按计划相机顺序，/ 分隔）、PLC deviceTriggers（只作参考）、整件标记；旧记录留空
        burst.and_then(|b| b.expected_total).map(|n| n.to_string()).unwrap_or_default(),
        burst.map(|b| b.tolerance_total.to_string()).unwrap_or_default(),
        burst.map(|b| b.camera_frames.iter().map(u32::to_string).collect::<Vec<_>>().join("/")).unwrap_or_default(),
        burst.and_then(|b| b.device_triggers).map(|n| n.to_string()).unwrap_or_default(),
        esc(&p.flags.as_ref().map(flag_labels).unwrap_or_default().join("、")),
    ];
    format!("{}\r\n", cells.join(","))
}

fn flag_labels(flags: &Flags) -> Vec<&'static str> {
    [
        (flags.incomplete, "检测不完整"),
        (flags.frame_count_anomaly, "帧数异常"),
        (flags.offset_unjudged, "胶偏未判"),
        (flags.unsegmented, "含未分段区域"),
        (flags.short_part, "疑似提前终止"),
    ]
    .into_iter()
    .filter_map(|(set, label)| set.then_some(label))
    .collect()
}

pub(crate) const CSV_HEADER: &str = "\u{feff}时间,来源,SN,配方,版本,配方修订号,模式,结果,PLC 结果码,异常码,原因,收到帧,计划帧,收尾耗时(ms),复检自,cycleId,发布包 ID,PLC 交付状态,PLC 交付更新时间,PLC 交付说明,预期帧组数(ΣN_exp),帧组容差(Σtol),各相机帧数,deviceTriggers,标记\r\n";

/// 按筛选条件导出 CSV（带 BOM，Excel 可直接打开），返回文件路径。
#[tauri::command]
pub async fn history_export(app: AppHandle, query: HistoryQuery) -> Result<String, String> {
    let dir = crate::storage::root(&app).join("exports");
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("检测记录_{}.csv", Local::now().format("%Y%m%d_%H%M%S")));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        f.write_all(CSV_HEADER.as_bytes()).map_err(|e| e.to_string())?;
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

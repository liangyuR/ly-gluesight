//! 示教件与标定件（docs/decisions/recipe-build-flow.md RB-1～RB-8）：候选从历史件选一件示教件（分段边界与全部走廊示教帧的
//! 唯一来源，换件即清空图像派生内容），再另选标定件；选件即导入为轮次，标定件按示教件边界对齐、可信即确认并加入标定。
//! 轮次只作存储格式，界面面对历史件。旧流程候选（`flow` 缺省）停用，只能删除。

use std::collections::BTreeSet;

use super::burst::{self, CalibrationRound, CALIBRATION_DIR};
use super::capture_review::{self, ConfirmedBy, ReviewStatus};
use super::*;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::frame_align::AlignedCut;
use crate::recipe::TeachingPieceRef;
use crate::recipe_capture::{CaptureRoundV2, CaptureState, CaptureSummary, RecipeCaptureHost};
use crate::segmentation::SegOutcome;

/// 按本决策的步骤建的候选的流程版本。
pub const FLOW: u32 = 2;
pub(super) const LEGACY: &str = "旧流程候选已停用：请在配方库删除后重建";

/// 候选的示教件：导入成的轮次与来源历史件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeachingPiece {
    pub round_id: String,
    pub record_id: i64,
    pub cycle_id: String,
    pub sn: String,
    pub ts: i64,
}

impl TeachingPiece {
    pub(super) fn recipe_ref(&self) -> TeachingPieceRef {
        TeachingPieceRef {
            cycle_id: self.cycle_id.clone(),
            record_id: self.record_id,
            sn: self.sn.clone(),
            ts: self.ts,
        }
    }
}

fn is_teaching(w: &Workspace, round: &str) -> bool {
    w.teaching_piece.as_ref().is_some_and(|t| t.round_id == round)
}

/// 走廊示教帧与示教页试测只能用示教件的帧（RB-1）。
pub(super) fn require_teaching_round(w: &Workspace, round: &str) -> Result<(), String> {
    if is_teaching(w, round) {
        Ok(())
    } else {
        Err("示教帧只能取自示教件".into())
    }
}

/// 示教件边界已确认且与当前段列表一致时各段帧组数；没有示教件或未确认、已失效为 None。
pub(super) fn teaching_counts(w: &Workspace) -> Option<Vec<u32>> {
    let piece = w.teaching_piece.as_ref()?;
    let review = w.capture_review.rounds.get(&piece.round_id)?;
    capture_review::confirmed_segment_counts(
        &w.doc,
        &w.capture_review.rounds,
        &piece.round_id,
        review.group_count,
    )
}

/// 走廊示教完成：每个（段, 检测相机）都有走廊，每台计划相机都有像素当量。
fn corridor_ready(w: &Workspace) -> bool {
    burst::untaught_pairs(w).is_empty() && w.doc.inspect.iter().all(|i| i.mm_per_px.is_some())
}

/// 设定（更换）示教件（RB-2、RB-8）：由示教件图像得出的内容全部清空——全部轮次的边界复核与模板、走廊与示教帧、
/// 标定件与标定派生结果（N_exp、标定统计、partCalib、x_ref）；段列表结构与配方级参数保留。轮次目录与标定特征由调用方删除。
pub(super) fn replace_teaching_piece(w: &mut Workspace, piece: TeachingPiece) -> Result<(), String> {
    burst::require_burst(&w.doc)?;
    if w.teaching_piece.as_ref().is_some_and(|t| t.record_id == piece.record_id || t.cycle_id == piece.cycle_id) {
        return Err("这一件已是示教件".into());
    }
    w.camera_teaching.clear();
    w.capture_review = capture_review::CaptureReview::default();
    w.calibration = burst::Calibration::default();
    burst::clear_calibration(&mut w.doc);
    w.doc.inspect.iter_mut().for_each(|i| i.segments.clear());
    w.teaching_piece = Some(piece);
    w.changed();
    Ok(())
}

/// 弃用一件标定件：移出标定（已应用的标定结果清空）并删除它的复核记录；示教件不能单独移出（RB-3）。
pub(super) fn discard_piece(w: &mut Workspace, round: &str) -> Result<(), String> {
    burst::require_burst(&w.doc)?;
    if is_teaching(w, round) {
        return Err("示教件不能单独移出标定：要换件请重新选示教件".into());
    }
    let before = w.calibration.rounds.len();
    w.calibration.rounds.retain(|r| r.round_id != round);
    if w.calibration.rounds.len() != before {
        burst::clear_calibration(&mut w.doc);
    }
    w.capture_review.rounds.remove(round);
    w.changed();
    Ok(())
}

/// 本候选的示教件与标定件：属于本配方、已完整的轮次，不含验证样本库用到的轮次；示教件在前，其余按创建时间。
pub(super) fn piece_rounds(capture: &RecipeCaptureHost, w: &Workspace) -> Result<Vec<CaptureSummary>, String> {
    let bank: BTreeSet<&str> = w
        .sample_bank
        .iter()
        .flat_map(|b| std::iter::once(b.id.as_str()).chain(b.capture_id.as_deref()))
        .collect();
    let mut rounds: Vec<_> = capture
        .list(&w.doc.id)?
        .into_iter()
        .filter(|r| r.state == CaptureState::Complete && !bank.contains(r.round_id.as_str()))
        .collect();
    rounds.sort_by_key(|r| (!is_teaching(w, &r.round_id), r.created_at));
    Ok(rounds)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PieceRole {
    Teaching,
    Calibration,
    Standard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PieceReview {
    None,
    Draft,
    Confirmed,
    Stale,
}

/// 示教件或一件标定件（TS `PieceView`）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceView {
    pub round_id: String,
    pub role: PieceRole,
    pub record_id: Option<i64>,
    pub cycle_id: Option<String>,
    pub sn: Option<String>,
    pub ts: Option<i64>,
    pub group_count: u64,
    pub review: PieceReview,
    pub confirmed_by: Option<ConfirmedBy>,
    pub in_calibration: bool,
    pub excluded_groups: u32,
}

/// 各件的视图（`rounds` 已按示教件在前排好）；`lookup(记录)` 给出历史件的 SN 与时间，查不到为 None。其余件按时间排。
pub(super) fn piece_views(
    w: &Workspace,
    rounds: &[CaptureSummary],
    lookup: impl Fn(i64) -> Option<(String, i64)>,
) -> Vec<PieceView> {
    let mut views: Vec<PieceView> = rounds
        .iter()
        .map(|r| {
            let teaching = w.teaching_piece.as_ref().filter(|t| t.round_id == r.round_id);
            let (record_id, cycle_id) = match &r.origin {
                Some(crate::recipe_capture::RoundOrigin::History { cycle_id, record_id, .. }) => {
                    (Some(*record_id), Some(cycle_id.clone()))
                }
                _ => (None, None),
            };
            let (sn, ts) = match teaching {
                Some(t) => (Some(t.sn.clone()), Some(t.ts)),
                None => record_id.and_then(&lookup).unzip(),
            };
            let review = w.capture_review.rounds.get(&r.round_id);
            let group_count = review.map_or(u64::from(r.received_count), |v| v.group_count);
            let state = match review {
                None => PieceReview::None,
                Some(v) if v.status == ReviewStatus::Confirmed => {
                    if capture_review::confirmed_segment_counts(&w.doc, &w.capture_review.rounds, &r.round_id, v.group_count).is_some() {
                        PieceReview::Confirmed
                    } else {
                        PieceReview::Stale
                    }
                }
                Some(v) if v.status == ReviewStatus::Stale => PieceReview::Stale,
                Some(_) => PieceReview::Draft,
            };
            PieceView {
                round_id: r.round_id.clone(),
                role: if teaching.is_some() {
                    PieceRole::Teaching
                } else if r.origin.as_ref().is_some_and(|o| o.standard()) {
                    PieceRole::Standard
                } else {
                    PieceRole::Calibration
                },
                record_id,
                cycle_id,
                sn,
                ts,
                group_count,
                review: state,
                confirmed_by: (state == PieceReview::Confirmed).then(|| review.map(|v| v.confirmed_by)).flatten(),
                in_calibration: w.calibration.rounds.iter().any(|c| c.round_id == r.round_id),
                excluded_groups: review.map_or(0, |v| u32::try_from(v.excluded_groups.len()).unwrap_or(u32::MAX)),
            }
        })
        .collect();
    views.sort_by_key(|v| (v.role != PieceRole::Teaching, v.ts.unwrap_or(i64::MAX)));
    views
}

/// 候选视图里的示教件与标定件：SN 与时间先按本配方最近的录制件一次查出，查不到的再逐件查记录。
pub(super) fn plan_pieces(app: &AppHandle, w: &Workspace) -> Vec<PieceView> {
    let Ok(rounds) = piece_rounds(app.state::<CycleHost>().camera.recipe_capture(), w) else {
        return Vec::new();
    };
    let store = app.state::<Store>();
    let recent: BTreeMap<i64, (String, i64)> = if rounds.iter().any(|r| !is_teaching(w, &r.round_id)) {
        store
            .recorded_parts(&w.doc.id, 500)
            .map(|parts| parts.into_iter().map(|(s, _)| (s.id, (s.sn, s.ts))).collect())
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    piece_views(w, &rounds, |id| {
        recent
            .get(&id)
            .cloned()
            .or_else(|| store.detail(id).ok().map(|d| (d.summary.sn, d.summary.ts)))
    })
}

/// 删除旧流程候选的目录内容（RB-5）：保留 `published/` 发布归档，生产配方照常运行；目录空了一并删除。
pub(super) fn remove_legacy_dir(dir: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let mut kept = false;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == "published" {
            kept = true;
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }
        .map_err(|e| format!("删除 {} 失败：{e}", path.display()))?;
    }
    if !kept {
        std::fs::remove_dir(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 删除本候选标定目录下这一轮的特征（轮次编号只含字母、数字与短横线）。
fn remove_features(host: &WorkspaceHost, id: &str, round: &str) {
    if round.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        let _ = std::fs::remove_dir_all(host.dir(id).join(CALIBRATION_DIR).join(round));
    }
}

// ───────────── 示教件 ─────────────

/// 设定（更换）示教件：按历史件的可用条件核对并导入为轮次，再按 RB-2 清空候选里由原示教件图像得出的内容，删除原示教件与
/// 全部标定件的轮次目录及标定特征。同一件不能重复设置。
#[tauri::command]
pub async fn workspace_set_teaching_piece(
    app: AppHandle,
    id: String,
    revision: u64,
    record_id: i64,
) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task = app.clone();
    tauri::async_runtime::spawn_blocking(move || set_teaching_piece(&task, &id, revision, record_id))
        .await
        .map_err(|e| e.to_string())?
}

fn set_teaching_piece(app: &AppHandle, id: &str, revision: u64, record_id: i64) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let w = {
        let items = host.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.clone()
    };
    burst::require_burst(&w.doc)?;
    if w.teaching_piece.as_ref().is_some_and(|t| t.record_id == record_id) {
        return Err("这一件已是示教件".into());
    }
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    let picks = [HistoryPick { record_id, standard: false }];
    let (round_id, part) = history_import::import_records(app, &w, &picks)?
        .pop()
        .ok_or("示教件导入失败")?;
    let replaced = (|| {
        let piece = TeachingPiece {
            round_id: round_id.clone(),
            record_id,
            cycle_id: part.cycle_id.clone().ok_or("记录没有 cycleId")?,
            sn: part.sn.clone(),
            ts: part.ts,
        };
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(id).ok_or("候选配置不存在")?;
        current
            .expect(revision)
            .map_err(|_| "导入期间候选已被修改，本次没有更换示教件，请重试".to_string())?;
        let stale: Vec<String> = piece_rounds(capture, current)?
            .into_iter()
            .map(|r| r.round_id)
            .filter(|r| *r != round_id)
            .collect();
        let mut next = current.clone();
        replace_teaching_piece(&mut next, piece)?;
        host.save(&next)?;
        *current = next;
        Ok::<_, String>((current.clone(), stale))
    })();
    let (next, stale) = match replaced {
        Ok(done) => done,
        Err(error) => {
            let _ = capture.delete_round(&round_id);
            return Err(error);
        }
    };
    let _ = std::fs::remove_dir_all(host.dir(id).join(CALIBRATION_DIR));
    for round in stale {
        if let Err(error) = capture.delete_round(&round) {
            cycle::log(app, "warn", "更换示教件", error);
        }
    }
    view(app, next)
}

// ───────────── 标定件 ─────────────

/// 一件标定件这次同步的结果。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPiece {
    pub round_id: String,
    /// 本次加入了标定
    pub added: bool,
    /// 没加入的原因：待复核的边界、走廊示教未完成、测量或核对失败
    pub issue: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationSyncResult {
    pub view: WorkspaceView,
    pub pieces: Vec<SyncPiece>,
}

/// 同步算出的新候选、各件结果与要落盘的每帧特征（轮次, 计划相机, 特征, 分段结果）。
pub(super) struct SyncOutcome {
    pub next: Workspace,
    pub pieces: Vec<SyncPiece>,
    pub features: Vec<(String, Vec<FrameMetrics>, SegOutcome)>,
}

/// 同步标定件（RB-3、D-SB-51）：对还没加入标定的每一件——示教件要求边界已确认；其余件边界未确认或已失效时用示教件边界
/// 对齐（`align`），全部可信则确认（`ConfirmedBy::Aligned`），否则不动并给出待复核的边界。边界确认后、走廊示教完成时用已示教的
/// 走廊测量（`measure`）并加入标定；示教件只在走廊示教完成后加入，且不按标准件。`rounds` 为各件轮次，示教件在前。
pub(super) fn sync_pieces(
    w: &Workspace,
    rounds: &[CaptureRoundV2],
    current: &BTreeMap<String, Value>,
    mut align: impl FnMut(&CaptureRoundV2) -> Result<Vec<AlignedCut>, String>,
    mut measure: impl FnMut(&Workspace, &CaptureRoundV2) -> Result<Vec<FrameMetrics>, String>,
) -> SyncOutcome {
    let mut next = w.clone();
    let mut pieces = Vec::new();
    let mut features = Vec::new();
    let teaching_ready = teaching_counts(w).is_some();
    let ready = corridor_ready(w);
    for round in rounds {
        let id = &round.round_id;
        if next.calibration.rounds.iter().any(|r| &r.round_id == id) {
            continue;
        }
        let teaching = is_teaching(w, id);
        let mut step = || -> Result<(), String> {
            if !teaching_ready {
                return Err("示教件边界未确认：先在分段步骤确认".into());
            }
            let confirmed = capture_review::confirmed_segment_counts(&next.doc, &next.capture_review.rounds, id, round.group_count).is_some();
            if !teaching && !confirmed {
                let cuts: Vec<u64> = if next.doc.segments.len() == 1 {
                    Vec::new()
                } else {
                    let aligned = align(round)?;
                    if let Some(issue) = capture_review::review_issue(&aligned, u64::from(next.doc.boundary_guard)) {
                        return Err(format!("待复核：{issue}"));
                    }
                    aligned.iter().map(|c| c.cut).collect()
                };
                let mut trial = next.clone();
                capture_review::confirm_aligned(&mut trial, round, cuts)?;
                next = trial;
            }
            if !ready {
                return Err("走廊示教未完成".into());
            }
            burst::check_round(&next.doc, &next.calibration.rounds, round, current)?;
            let metrics = measure(&next, round)?;
            let outcome = burst::segment_round(&next.doc.segments, next.doc.plan_cameras().len(), round.group_count, &metrics);
            let standard = !teaching && round.origin.as_ref().is_some_and(|o| o.standard());
            let entry: CalibrationRound = burst::calibration_round(&next.doc, round, standard, &metrics);
            burst::add_calibration_round(&mut next, entry)?;
            features.push((id.clone(), metrics, outcome));
            Ok(())
        };
        let result = step();
        pieces.push(SyncPiece { round_id: id.clone(), added: result.is_ok(), issue: result.err() });
    }
    SyncOutcome { next, pieces, features }
}

/// 把选中的历史件导入为标定件（`standard` 记为标准件），随后同步标定件。须先有示教件。
#[tauri::command]
pub async fn workspace_calibration_pick(
    app: AppHandle,
    id: String,
    revision: u64,
    picks: Vec<HistoryPick>,
) -> Result<CalibrationSyncResult, String> {
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let w = {
            let host = task.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.clone()
        };
        if w.teaching_piece.is_none() {
            return Err("先选示教件，再选标定件".into());
        }
        crate::camera::check_idle(&task.state::<CycleHost>())?;
        history_import::import_records(&task, &w, &picks)?;
        sync(&task, &id, revision)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 同步标定件（见 [`sync_pieces`]）：人工复核边界后、走廊示教完成后再调用。
#[tauri::command]
pub async fn workspace_calibration_sync(app: AppHandle, id: String, revision: u64) -> Result<CalibrationSyncResult, String> {
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task = app.clone();
    tauri::async_runtime::spawn_blocking(move || sync(&task, &id, revision))
        .await
        .map_err(|e| e.to_string())?
}

/// 读图对齐与测量都在候选锁外；写入前核对修订号，计算期间候选被改过时整批不写入。调用方持配置门。
fn sync(app: &AppHandle, id: &str, revision: u64) -> Result<CalibrationSyncResult, String> {
    let cycle = app.state::<CycleHost>();
    crate::camera::check_idle(&cycle)?;
    let host = app.state::<WorkspaceHost>();
    let w = {
        let items = host.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.clone()
    };
    burst::require_burst(&w.doc)?;
    if w.teaching_piece.is_none() {
        return Err("先选示教件".into());
    }
    let capture = cycle.camera.recipe_capture();
    let mut unreadable = Vec::new();
    let mut rounds = Vec::new();
    for summary in piece_rounds(capture, &w)? {
        if w.calibration.rounds.iter().any(|r| r.round_id == summary.round_id) {
            continue;
        }
        match capture.get_burst(&summary.round_id) {
            Ok(round) => rounds.push(round),
            Err(error) => unreadable.push(SyncPiece { round_id: summary.round_id, added: false, issue: Some(error) }),
        }
    }
    let current: BTreeMap<String, Value> = cycle.camera.configs().iter().map(|c| (c.id.clone(), state_value(c))).collect();
    let mut template = None;
    let align = |round: &CaptureRoundV2| {
        if template.is_none() {
            template = Some(capture_review::AlignTemplate::load(capture, &w));
        }
        match template.as_ref().unwrap() {
            Ok(template) => template.align(capture, &round.round_id),
            Err(error) => Err(error.clone()),
        }
    };
    let engine = app.state::<vision::VisionHost>().engine();
    let mut graphs = None;
    let run = vision::unique_run_id(&format!("calibration-{id}"));
    let measure = |w: &Workspace, round: &CaptureRoundV2| -> Result<Vec<FrameMetrics>, String> {
        let engine = engine.as_ref().ok_or("lyFlow 核心库未加载")?;
        if graphs.is_none() {
            graphs = Some(w.doc.build().and_then(|recipe| vision::CorridorSet::build(&recipe, &burst::teaching_sizes(w)?)));
        }
        let graphs = graphs.as_ref().unwrap().as_ref().map_err(Clone::clone)?;
        let choice = burst::calibration_corridors(w, &round.round_id, round.group_count)?;
        let round_id = &round.round_id;
        Ok(capture_review::measure_calibration_round(w, round, |camera, camera_id, seq| {
            let Some(segment) = choice[camera][(seq - 1) as usize] else {
                return Ok(burst::frame_without_metrics(camera, seq, FrameState::NotInspected, None));
            };
            let graph = graphs
                .get(camera, segment)
                .ok_or_else(|| format!("第 {segment} 段相机 {camera_id} 还没有示教走廊"))?;
            let image = crate::image_file::load(&capture.burst_frame_path(round_id, camera_id, seq)?)?;
            vision::measure_frame(engine, graph, &image, seq, &format!("{run}-{round_id}-c{camera}-{seq}")).map(|m| m.metrics)
        }))
    };
    let outcome = sync_pieces(&w, &rounds, &current, align, measure);
    let mut pieces = outcome.pieces;
    pieces.extend(unreadable);
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(id).ok_or("候选配置不存在")?;
    current
        .expect(revision)
        .map_err(|_| "标定期间候选已被修改，本次没有写入，请重试".to_string())?;
    if outcome.next.revision != current.revision {
        let cameras = outcome.next.doc.plan_cameras();
        for (round, metrics, segmented) in &outcome.features {
            burst::write_features(&host.dir(id).join(CALIBRATION_DIR).join(round), &cameras, metrics, segmented)?;
        }
        host.save(&outcome.next)?;
        *current = outcome.next;
    }
    Ok(CalibrationSyncResult { view: view(app, current.clone())?, pieces })
}

/// 彻底弃用一件标定件（见 [`discard_piece`]）：移出标定、删除复核记录、标定特征与轮次目录。示教件报错。
#[tauri::command]
pub fn workspace_calibration_remove_round(app: AppHandle, id: String, revision: u64, round: String) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在处理采集、标定或设备事务，请稍后操作")?;
    let capture = cycle.camera.recipe_capture();
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    if !piece_rounds(capture, &w)?.iter().any(|r| r.round_id == round) && !w.calibration.rounds.iter().any(|r| r.round_id == round) {
        return Err("这一件不是本候选的标定件".into());
    }
    discard_piece(&mut w, &round)?;
    host.save(&w)?;
    items.insert(id.clone(), w.clone());
    remove_features(&host, &id, &round);
    if let Err(error) = capture.delete_round(&round) {
        cycle::log(&app, "warn", "弃用标定件", error);
    }
    view(&app, w)
}

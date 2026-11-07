//! 示教件（docs/decisions/recipe-build-flow.md RB-1～RB-8）：候选从历史件选一件示教件，它是分段边界、全部走廊示教帧、
//! 各段 N_exp 与胶偏参考 x_ref 的唯一来源，换件即清空图像派生内容；选件即导入为轮次。
//! 轮次只作存储格式，界面面对历史件。旧流程候选（`flow` 缺省）停用，只能删除。

use std::collections::{BTreeMap, BTreeSet};

use super::burst::{self, CalibrationRound, CALIBRATION_DIR};
use super::capture_review::{self, ConfirmedBy, ReviewStatus};
use super::*;
use crate::burst_judge::{FrameMetrics, FrameState};
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
    w.teaching_piece
        .as_ref()
        .is_some_and(|t| t.round_id == round)
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

/// 示教件已按当前计划相机与走廊身份测量（`measured_with_current_corridors`）。
pub(super) fn teaching_measured(w: &Workspace) -> bool {
    w.teaching_piece.as_ref().is_some_and(|t| {
        w.calibration
            .rounds
            .iter()
            .any(|r| r.round_id == t.round_id && measured_with_current_corridors(&w.doc, r))
    })
}

/// 设定（更换）示教件（RB-2、RB-8）：保留走廊与示教帧、段列表结构与配方级参数；只清空跟着原示教件帧组走的内容——
/// 全部轮次的边界复核与模板、示教件测量与派生结果（N_exp、标定统计、partCalib、x_ref），在新示教件上重新确认分段、重新测量即可得出。
/// 保留的示教帧改指向新示教件：轮次换成新件（旧轮次随后删除），帧组号按两件的帧组数等比例换算（`groups` 为各旧轮次的帧组数，
/// 查不到的只夹在新件范围内）；冻结的示教图不变，走廊照旧在它上面编辑。轮次目录与测量特征由调用方删除。
pub(super) fn replace_teaching_piece(
    w: &mut Workspace,
    piece: TeachingPiece,
    piece_groups: u64,
    groups: &BTreeMap<String, u64>,
) -> Result<(), String> {
    burst::require_burst(&w.doc)?;
    if w.teaching_piece
        .as_ref()
        .is_some_and(|t| t.record_id == piece.record_id || t.cycle_id == piece.cycle_id)
    {
        return Err("这一件已是示教件".into());
    }
    for t in &mut w.camera_teaching {
        t.seq = scaled_seq(t.seq, groups.get(&t.round).copied(), piece_groups);
        t.round = piece.round_id.clone();
    }
    w.capture_review = capture_review::CaptureReview::default();
    w.calibration = burst::Calibration::default();
    burst::clear_calibration(&mut w.doc);
    w.teaching_piece = Some(piece);
    w.changed();
    Ok(())
}

/// 原件第 seq 帧组按帧组数等比例落到新件（都从 1 起），夹在新件范围内；原件帧组数不知道时只夹范围。
fn scaled_seq(seq: u64, from: Option<u64>, to: u64) -> u64 {
    let to = to.max(1);
    let scaled = match from.filter(|&n| n > 0) {
        Some(from) => (seq.saturating_sub(1) * to + from / 2) / from + 1,
        None => seq,
    };
    scaled.clamp(1, to)
}

/// 本候选导入的轮次：属于本配方、已完整的轮次，不含验证样本库用到的轮次；示教件在前，其余（旧流程留下的标定件）按创建时间。
pub(super) fn piece_rounds(
    capture: &RecipeCaptureHost,
    w: &Workspace,
) -> Result<Vec<CaptureSummary>, String> {
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
pub enum PieceReview {
    None,
    Draft,
    Confirmed,
    Stale,
}

/// 示教件（TS `PieceView`）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceView {
    pub round_id: String,
    pub record_id: i64,
    pub cycle_id: String,
    pub sn: String,
    pub ts: i64,
    pub group_count: u64,
    pub review: PieceReview,
    pub confirmed_by: Option<ConfirmedBy>,
    /// 已按当前走廊测量
    pub measured: bool,
    pub excluded_groups: u32,
    /// 原图不在本项目（导出或复制项目时没带），可从同一件历史记录补回
    pub images_missing: bool,
}

/// 示教件的视图；还没选示教件，或它的轮次已不在本配方完整的轮次里时为空。
pub(super) fn piece_views(w: &Workspace, rounds: &[CaptureSummary]) -> Vec<PieceView> {
    let Some(piece) = w.teaching_piece.as_ref() else {
        return Vec::new();
    };
    let Some(round) = rounds.iter().find(|r| r.round_id == piece.round_id) else {
        return Vec::new();
    };
    let review = w.capture_review.rounds.get(&round.round_id);
    let group_count = review.map_or(u64::from(round.received_count), |v| v.group_count);
    let state = match review {
        None => PieceReview::None,
        Some(v) if v.status == ReviewStatus::Confirmed => {
            if teaching_counts(w).is_some() {
                PieceReview::Confirmed
            } else {
                PieceReview::Stale
            }
        }
        Some(v) if v.status == ReviewStatus::Stale => PieceReview::Stale,
        Some(_) => PieceReview::Draft,
    };
    vec![PieceView {
        round_id: round.round_id.clone(),
        record_id: piece.record_id,
        cycle_id: piece.cycle_id.clone(),
        sn: piece.sn.clone(),
        ts: piece.ts,
        group_count,
        review: state,
        confirmed_by: (state == PieceReview::Confirmed)
            .then(|| review.map(|v| v.confirmed_by))
            .flatten(),
        measured: teaching_measured(w),
        excluded_groups: review.map_or(0, |v| {
            u32::try_from(v.excluded_groups.len()).unwrap_or(u32::MAX)
        }),
        images_missing: false,
    }]
}

/// 候选视图里的示教件。
pub(super) fn plan_pieces(app: &AppHandle, w: &Workspace) -> Vec<PieceView> {
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    let mut pieces = piece_rounds(capture, w)
        .map(|rounds| piece_views(w, &rounds))
        .unwrap_or_default();
    for piece in &mut pieces {
        piece.images_missing = capture.images_missing(&piece.round_id).unwrap_or(false);
    }
    pieces
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

// ───────────── 示教件 ─────────────

/// 设定（更换）示教件：按历史件的可用条件核对并导入为轮次，再按 RB-2 清空跟着原示教件帧组走的内容（保留走廊与示教帧），删除本候选
/// 原有的全部轮次目录及测量特征。同一件不能重复设置。
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
    tauri::async_runtime::spawn_blocking(move || {
        set_teaching_piece(&task, &id, revision, record_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn set_teaching_piece(
    app: &AppHandle,
    id: &str,
    revision: u64,
    record_id: i64,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let w = {
        let items = host.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.clone()
    };
    burst::require_burst(&w.doc)?;
    if w.teaching_piece
        .as_ref()
        .is_some_and(|t| t.record_id == record_id)
    {
        return Err("这一件已是示教件".into());
    }
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    let picks = [HistoryPick { record_id }];
    let (round_id, part) = history_import::import_records(app, &w, &picks)?
        .pop()
        .ok_or("示教件导入失败")?;
    // 保留的示教帧要按帧组数换算到新件：先记下它们所在旧轮次的帧组数（旧轮次随后删除）
    let groups: BTreeMap<String, u64> = w
        .camera_teaching
        .iter()
        .map(|t| t.round.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|round| {
            capture
                .get_burst(&round)
                .ok()
                .map(|r| (round, r.group_count))
        })
        .collect();
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
        replace_teaching_piece(&mut next, piece, part.group_count, &groups)?;
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

/// 补回示教件原图（导出或复制项目时没带）：从同一件历史记录重新物化，候选内容与修订不变。
#[tauri::command]
pub async fn workspace_refill_teaching_images(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<WorkspaceView, String> {
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
        history_import::refill_teaching_images(&task, &w)?;
        view(&task, w)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ───────────── 示教件测量 ─────────────

/// 测量算出的新候选与要落盘的每帧特征、分段结果。
pub(super) struct MeasureOutcome {
    pub next: Workspace,
    pub metrics: Vec<FrameMetrics>,
    pub segmented: SegOutcome,
}

/// 测量过的示教件是否按当前的计划相机与走廊身份（走廊、当量、检测参数、站距）测量；改过之后旧特征与 x 不再可比，要重新测量。
pub(super) fn measured_with_current_corridors(doc: &RecipeDoc, entry: &CalibrationRound) -> bool {
    let cameras = doc.plan_cameras();
    entry.cameras == cameras
        && entry.corridors
            == cameras
                .iter()
                .map(|camera| burst::corridor_tag(doc, camera))
                .collect::<Vec<_>>()
}

/// 测量示教件（RB-3）：示教件分段已确认、走廊示教完成时，按示教件分段用已示教的走廊测量（`measure`）每一帧，替换候选里原有的
/// 测量（含旧流程留下的标定件轮次），再按这次测量写单段配方的胶偏参考 x_ref（示教件兼作标准件）。
pub(super) fn measure_teaching(
    w: &Workspace,
    round: &CaptureRoundV2,
    current: &BTreeMap<String, Value>,
    measure: impl FnOnce(&Workspace, &CaptureRoundV2) -> Result<Vec<FrameMetrics>, String>,
) -> Result<MeasureOutcome, String> {
    if !w
        .teaching_piece
        .as_ref()
        .is_some_and(|t| t.round_id == round.round_id)
    {
        return Err("只能测量示教件".into());
    }
    if teaching_counts(w).is_none() {
        return Err("示教件分段未确认：先在分段步骤确认".into());
    }
    if !corridor_ready(w) {
        return Err("走廊示教未完成：每段的检测相机都要有走廊，每台相机都要有像素当量".into());
    }
    burst::check_round(&w.doc, round, current)?;
    let metrics = measure(w, round)?;
    let segmented = burst::segment_round(
        &w.doc.segments,
        w.doc.plan_cameras().len(),
        round.group_count,
        &metrics,
    );
    let mut next = w.clone();
    next.calibration.rounds = vec![burst::calibration_round(&next.doc, round, &metrics)];
    burst::apply_reference(&mut next.doc, &next.calibration.rounds);
    next.changed();
    Ok(MeasureOutcome {
        next,
        metrics,
        segmented,
    })
}

/// 测量示教件（见 [`measure_teaching`]）：走廊示教完成后、改过走廊或检测参数后调用。
#[tauri::command]
pub async fn workspace_measure_teaching(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task = app.clone();
    tauri::async_runtime::spawn_blocking(move || measure(&task, &id, revision))
        .await
        .map_err(|e| e.to_string())?
}

/// 测量在候选锁外；写入前核对修订号，测量期间候选被改过时不写入。调用方持配置门。
fn measure(app: &AppHandle, id: &str, revision: u64) -> Result<WorkspaceView, String> {
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
    let piece = w.teaching_piece.as_ref().ok_or("先选示教件")?;
    let capture = cycle.camera.recipe_capture();
    let round = capture.get_burst(&piece.round_id)?;
    let current: BTreeMap<String, Value> = cycle
        .camera
        .configs()
        .iter()
        .map(|c| (c.id.clone(), state_value(c)))
        .collect();
    let engine = app.state::<vision::VisionHost>().engine();
    let run = vision::unique_run_id(&format!("teaching-{id}"));
    let measure = |w: &Workspace, round: &CaptureRoundV2| -> Result<Vec<FrameMetrics>, String> {
        let engine = engine.as_ref().ok_or("lyFlow 核心库未加载")?;
        let recipe = w.doc.build()?;
        let graphs = vision::CorridorSet::build(&recipe, &burst::teaching_sizes(w)?)?;
        let choice = burst::calibration_corridors(w, &round.round_id, round.group_count)?;
        let round_id = &round.round_id;
        Ok(capture_review::measure_calibration_round(
            w,
            round,
            |camera, camera_id, seq| {
                let Some(segment) = choice[camera][(seq - 1) as usize] else {
                    return Ok(burst::frame_without_metrics(
                        camera,
                        seq,
                        FrameState::NotInspected,
                        None,
                    ));
                };
                let graph = graphs
                    .get(camera, segment)
                    .ok_or_else(|| format!("第 {segment} 段相机 {camera_id} 还没有示教走廊"))?;
                let image =
                    crate::image_file::load(&capture.burst_frame_path(round_id, camera_id, seq)?)?;
                vision::measure_frame(
                    engine,
                    graph,
                    &image,
                    seq,
                    &format!("{run}-{round_id}-c{camera}-{seq}"),
                )
                .map(|m| m.metrics)
            },
        ))
    };
    let outcome = measure_teaching(&w, &round, &current, measure)?;
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(id).ok_or("候选配置不存在")?;
    current
        .expect(revision)
        .map_err(|_| "测量期间候选已被修改，本次没有写入，请重试".to_string())?;
    let dir = host.dir(id).join(CALIBRATION_DIR);
    let _ = std::fs::remove_dir_all(&dir);
    burst::write_features(
        &dir.join(&round.round_id),
        &outcome.next.doc.plan_cameras(),
        &outcome.metrics,
        &outcome.segmented,
    )?;
    host.save(&outcome.next)?;
    *current = outcome.next;
    view(app, current.clone())
}

//! 起涂、收尾跳过帧的标定建议（D-SB-50）：第 1 段 `skip_head`、末段 `skip_tail`，由标定轮次（不含标准件）加入时写下的
//! 每帧特征按当前段检测项、检测相机与已确认分段算出，只读；操作员确认（`workspace_confirm_skip`）后才写进配方。
//!
//! 成形帧组 = 本段每台检测相机都测成且独立判定没有 NG 级发现；每轮的起涂帧组数见 `calibration::unformed_lead`。
//! 建议值取参与轮次的最大值，参与轮次少于 `calibration_rounds` 时只给分布。

use std::collections::BTreeSet;

use super::burst::{CalibrationRound, CALIBRATION_DIR};
use super::capture_review::{confirmed_segment_counts, RoundReview};
use super::*;
use crate::burst_judge::{frame_verdict, FrameMetrics, FrameState};
use crate::calibration::{unformed_lead, GroupForm, OnsetError, ONSET_RUN};
use crate::plan::PathSegment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipEnd {
    Head,
    Tail,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipRoundView {
    pub round_id: String,
    /// 本轮的起涂（收尾）帧组数；不参与建议时为 None，原因见 note
    pub frames: Option<u32>,
    pub note: Option<String>,
}

/// 一段一头的跳过帧建议（TS `SkipSuggestView`）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipSuggestView {
    pub segment: u32,
    pub end: SkipEnd,
    /// 配方里当前的 skip_head / skip_tail
    pub current: u32,
    pub suggested: Option<u32>,
    pub min: Option<u32>,
    pub max: Option<u32>,
    pub rounds: Vec<SkipRoundView>,
    /// 不给建议的原因
    pub pending: Option<String>,
}

/// 一轮里一个帧组的状态：本段检测相机有一台未成形即未成形，否则有一台无法判断即无法判断。
fn group_form(
    frames: &BTreeMap<(u64, usize), &FrameMetrics>,
    segment: &PathSegment,
    cameras: &[usize],
    seq: u64,
    excluded: &BTreeSet<u64>,
) -> GroupForm {
    if excluded.contains(&seq) {
        return GroupForm::Unknown;
    }
    let mut form = GroupForm::Formed;
    for &camera in cameras {
        match frames.get(&(seq, camera)) {
            Some(m) if m.state == FrameState::Measured => {
                if !matches!(
                    frame_verdict(m, &segment.checks),
                    Verdict::Ok | Verdict::OkWithExcursion
                ) {
                    return GroupForm::Unformed;
                }
            }
            _ => form = GroupForm::Unknown,
        }
    }
    form
}

/// 一轮在本段一头的起涂（收尾）帧组数。
pub(super) fn round_edge(
    doc: &RecipeDoc,
    round_id: &str,
    group_count: u64,
    reviews: &BTreeMap<String, RoundReview>,
    metrics: &[FrameMetrics],
    end: SkipEnd,
) -> Result<u32, String> {
    let (position, segment) = match end {
        SkipEnd::Head => (0, &doc.segments[0]),
        SkipEnd::Tail => (
            doc.segments.len() - 1,
            &doc.segments[doc.segments.len() - 1],
        ),
    };
    let (first, last) = if doc.segments.len() == 1 {
        (1, group_count)
    } else {
        let counts = confirmed_segment_counts(doc, reviews, round_id, group_count)
            .ok_or("本轮分段未确认或已失效")?;
        let start: u64 = counts[..position]
            .iter()
            .map(|&n| u64::from(n))
            .sum::<u64>()
            + 1;
        (start, start + u64::from(counts[position]) - 1)
    };
    let plan = doc.plan_cameras();
    let cameras: Vec<usize> = crate::plan::segment_cameras(segment, &plan)
        .iter()
        .filter_map(|id| plan.iter().position(|c| c == id))
        .collect();
    let frames: BTreeMap<(u64, usize), &FrameMetrics> =
        metrics.iter().map(|m| ((m.seq, m.camera), m)).collect();
    let excluded: BTreeSet<u64> = reviews
        .get(round_id)
        .map(|r| r.excluded_groups.iter().map(|g| g.seq).collect())
        .unwrap_or_default();
    let seqs: Vec<u64> = match end {
        SkipEnd::Head => (first..=last).collect(),
        SkipEnd::Tail => (first..=last).rev().collect(),
    };
    unformed_lead(
        seqs.iter()
            .map(|&seq| group_form(&frames, segment, &cameras, seq, &excluded)),
    )
    .map_err(|e| match e {
        OnsetError::Unknown(n) => format!(
            "成形之前第 {} 帧组无法判断（缺帧、测量失败、走出走廊或已屏蔽）",
            seqs[n - 1]
        ),
        OnsetError::NotFound => format!("本段内没有连续 {ONSET_RUN} 个成形帧组：检查走廊与检测项"),
    })
}

/// 第 1 段起涂、末段收尾的建议（单段配方两头都在第 1 段）。`features(轮次)` 读该轮加入标定时写下的每帧特征。
pub(super) fn skip_suggestions(
    doc: &RecipeDoc,
    rounds: &[CalibrationRound],
    reviews: &BTreeMap<String, RoundReview>,
    features: impl Fn(&str) -> Result<Vec<FrameMetrics>, String>,
) -> Vec<SkipSuggestView> {
    let (Some(head), Some(tail)) = (doc.segments.first(), doc.segments.last()) else {
        return Vec::new();
    };
    let normal: Vec<&CalibrationRound> = rounds.iter().filter(|r| !r.standard).collect();
    let loaded: Vec<Result<Vec<FrameMetrics>, String>> =
        normal.iter().map(|r| features(&r.round_id)).collect();
    [(head, SkipEnd::Head), (tail, SkipEnd::Tail)]
        .into_iter()
        .map(|(segment, end)| {
            let rounds: Vec<SkipRoundView> = normal
                .iter()
                .zip(&loaded)
                .map(|(round, metrics)| {
                    let result = metrics.as_ref().map_err(Clone::clone).and_then(|metrics| {
                        round_edge(
                            doc,
                            &round.round_id,
                            round.group_count,
                            reviews,
                            metrics,
                            end,
                        )
                    });
                    SkipRoundView {
                        round_id: round.round_id.clone(),
                        frames: result.as_ref().ok().copied(),
                        note: result.err(),
                    }
                })
                .collect();
            let values: Vec<u32> = rounds.iter().filter_map(|r| r.frames).collect();
            let (min, max) = (values.iter().min().copied(), values.iter().max().copied());
            let need = doc.calibration_rounds as usize;
            let pending = (values.len() < need).then(|| {
                format!(
                    "参与的标定轮次 {} 轮，少于 {need} 轮：只显示分布，不给建议",
                    values.len()
                )
            });
            SkipSuggestView {
                segment: segment.index,
                end,
                current: match end {
                    SkipEnd::Head => segment.skip_head,
                    SkipEnd::Tail => segment.skip_tail,
                },
                suggested: if pending.is_none() { max } else { None },
                min,
                max,
                rounds,
                pending,
            }
        })
        .collect()
}

/// 操作员确认第 `segment` 段一头的跳过帧组数（不进计划身份，不换计划版本）。
pub(super) fn confirm_skip(
    w: &mut Workspace,
    segment: u32,
    end: SkipEnd,
    frames: u32,
) -> Result<(), String> {
    super::burst::require_burst(&w.doc)?;
    let mut doc = w.doc.clone();
    let target = doc
        .segments
        .iter_mut()
        .find(|s| s.index == segment)
        .ok_or_else(|| format!("没有第 {segment} 段"))?;
    let slot = match end {
        SkipEnd::Head => &mut target.skip_head,
        SkipEnd::Tail => &mut target.skip_tail,
    };
    if *slot == frames {
        return Ok(());
    }
    *slot = frames;
    doc.build()?;
    w.doc = doc;
    w.changed();
    Ok(())
}

#[tauri::command]
pub async fn workspace_calibration_skip_suggest(
    app: AppHandle,
    id: String,
) -> Result<Vec<SkipSuggestView>, String> {
    let host = app.state::<WorkspaceHost>();
    let w = host
        .items
        .lock()
        .unwrap()
        .get(&id)
        .ok_or("候选配置不存在")?
        .clone();
    super::burst::require_burst(&w.doc)?;
    let dir = host.dir(&id).join(CALIBRATION_DIR);
    let cameras = w.doc.plan_cameras();
    tauri::async_runtime::spawn_blocking(move || {
        skip_suggestions(
            &w.doc,
            &w.calibration.rounds,
            &w.capture_review.rounds,
            |round| super::burst::read_features(&dir.join(round), &cameras),
        )
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn workspace_confirm_skip(
    app: AppHandle,
    id: String,
    revision: u64,
    segment: u32,
    end: SkipEnd,
    frames: u32,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    confirm_skip(&mut w, segment, end, frames)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

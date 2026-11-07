//! 起涂、收尾跳过帧的建议（D-SB-50）：第 1 段 `skip_head`、末段 `skip_tail`，由示教件测量时写下的每帧特征按当前段检测项、
//! 检测相机与已确认分段算出，只读；操作员确认（`workspace_confirm_skip`）后才写进配方。
//!
//! 成形帧组 = 本段每台检测相机都测成且独立判定没有 NG 级发现；起涂帧组数见 `calibration::unformed_lead`。
//! 每笔起涂（D-SB-62）按同一规则在示教件的每一笔上数，确认后写配方的 `pass_skip`。

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
    if segment.skip {
        return Err(format!("第 {} 段不检测，不需要跳过帧数建议", segment.index));
    }
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
    let normal: Vec<&CalibrationRound> = rounds.iter().collect();
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
            let pending = values
                .is_empty()
                .then(|| "示教件还没有可用的测量：测量示教件后给出建议".to_string());
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

/// 示教件一笔的起涂帧组数（D-SB-62）；不参与建议时为 None，原因见 note。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassLeadView {
    /// 第几笔（从 1 起）
    pub pass: u32,
    pub first_seq: u64,
    pub frames_in_pass: u32,
    pub frames: Option<u32>,
    pub note: Option<String>,
}

/// 每笔起涂跳过的建议（TS `PassSkipSuggestView`）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassSkipSuggestView {
    /// 配方里当前每笔开头跳过的帧组数
    pub current: u32,
    pub suggested: Option<u32>,
    pub passes: Vec<PassLeadView>,
    /// 不给建议的原因
    pub pending: Option<String>,
}

/// 每笔起涂的建议（D-SB-62，规则同 D-SB-50）：示教件每一笔从第一帧起，第一处连续成形帧组之前的帧组数，按帧所在段的
/// 检测项与检测相机判成形；建议取各笔最大值。没有连续成形帧组的笔（只点胶、很短的线）不参与，按配方规则整笔不检。
pub(super) fn pass_skip_suggestion(
    doc: &RecipeDoc,
    piece: Option<(&str, u64, &[u32])>,
    reviews: &BTreeMap<String, RoundReview>,
    metrics: Result<Vec<FrameMetrics>, String>,
) -> PassSkipSuggestView {
    let pending = |why: &str| PassSkipSuggestView {
        current: doc.pass_skip.head,
        suggested: None,
        passes: Vec::new(),
        pending: Some(why.into()),
    };
    let Some((round_id, group_count, layout)) = piece.filter(|p| !p.2.is_empty()) else {
        return pending("示教件没有笔结构：先选示教件并保存分段");
    };
    let counts: Vec<u64> = if doc.segments.len() == 1 {
        vec![group_count]
    } else {
        match confirmed_segment_counts(doc, reviews, round_id, group_count) {
            Some(counts) => counts.into_iter().map(u64::from).collect(),
            None => return pending("示教件分段未确认或已失效"),
        }
    };
    let metrics = match metrics {
        Ok(metrics) => metrics,
        Err(_) => return pending("示教件还没有可用的测量：测量示教件后给出建议"),
    };
    let frames: BTreeMap<(u64, usize), &FrameMetrics> =
        metrics.iter().map(|m| ((m.seq, m.camera), m)).collect();
    let excluded: BTreeSet<u64> = reviews
        .get(round_id)
        .map(|r| r.excluded_groups.iter().map(|g| g.seq).collect())
        .unwrap_or_default();
    let plan = doc.plan_cameras();
    let segment_of = |seq: u64| {
        let mut end = 0;
        counts.iter().position(|&n| {
            end += n;
            seq <= end
        })
    };
    let passes: Vec<PassLeadView> = layout
        .iter()
        .enumerate()
        .map(|(j, &n)| {
            let first = crate::passes::first_seq(layout, j);
            let seqs: Vec<u64> = (first..first + u64::from(n)).collect();
            let forms = seqs.iter().map(|&seq| {
                let Some(segment) = segment_of(seq).map(|k| &doc.segments[k]) else {
                    return GroupForm::Unknown;
                };
                let cameras: Vec<usize> = crate::plan::segment_cameras(segment, &plan)
                    .iter()
                    .filter_map(|id| plan.iter().position(|c| c == id))
                    .collect();
                if cameras.is_empty() {
                    return GroupForm::Unknown;
                }
                group_form(&frames, segment, &cameras, seq, &excluded)
            });
            let result = unformed_lead(forms).map_err(|e| match e {
                OnsetError::Unknown(k) => format!(
                    "成形之前第 {} 帧组无法判断（缺帧、测量失败、不检测的段或已屏蔽）",
                    seqs[k - 1]
                ),
                OnsetError::NotFound => {
                    format!("没有连续 {ONSET_RUN} 个成形帧组（点胶或很短的线），整笔按跳过规则处理")
                }
            });
            PassLeadView {
                pass: j as u32 + 1,
                first_seq: first,
                frames_in_pass: n,
                frames: result.as_ref().ok().copied(),
                note: result.err(),
            }
        })
        .collect();
    let suggested = passes.iter().filter_map(|p| p.frames).max();
    PassSkipSuggestView {
        current: doc.pass_skip.head,
        suggested,
        pending: suggested
            .is_none()
            .then(|| "没有一笔找到连续成形的帧组：检查走廊与检测项".to_string()),
        passes,
    }
}

#[tauri::command]
pub async fn workspace_pass_skip_suggest(
    app: AppHandle,
    id: String,
) -> Result<PassSkipSuggestView, String> {
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
        let piece = w.teaching_piece.as_ref();
        let group_count = piece
            .and_then(|p| w.capture_review.rounds.get(&p.round_id))
            .map_or(0, |r| r.group_count);
        let metrics = piece
            .filter(|p| {
                w.calibration
                    .rounds
                    .iter()
                    .any(|r| r.round_id == p.round_id)
            })
            .ok_or_else(|| "示教件还没有测量".to_string())
            .and_then(|p| super::burst::read_features(&dir.join(&p.round_id), &cameras));
        pass_skip_suggestion(
            &w.doc,
            piece.map(|p| (p.round_id.as_str(), group_count, p.passes.as_slice())),
            &w.capture_review.rounds,
            metrics,
        )
    })
    .await
    .map_err(|e| e.to_string())
}

/// 操作员确认每笔开头、结尾跳过的帧组数（不进计划身份，不换计划版本）。
#[tauri::command]
pub fn workspace_confirm_pass_skip(
    app: AppHandle,
    id: String,
    revision: u64,
    head: u32,
    tail: u32,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    super::burst::require_burst(&w.doc)?;
    let skip = crate::passes::PassSkip { head, tail };
    if w.doc.pass_skip != skip {
        let mut doc = w.doc.clone();
        doc.pass_skip = skip;
        doc.build()?;
        w.doc = doc;
        w.changed();
        host.save(&w)?;
        items.insert(id, w.clone());
    }
    view(&app, w)
}

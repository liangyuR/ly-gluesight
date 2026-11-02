//! 分段标定工作台保存时按示教件上的帧范围对应新旧段（D-SB-52）：没动的段原样保留，只有真正变了的段才清空、重新示教。
//!
//! - 对应：新段取类型相同、在示教件上帧组范围重叠最多的旧段（重叠相同取靠前的），没有就是新段。
//! - 段参数：继承对应旧段的参数，检测相机按工作台所选；段长只在帧范围不变时保留，段首 / 段尾跳过帧数只在该端没动时保留。
//! - 走廊与示教帧：某台相机原来的段示教帧仍落在新段的帧范围内、新段类型与原段相同、这台相机仍检测新段时，走廊随它迁到新段号；
//!   合并的段有多条可用时取对应旧段的那条。不在示教件上的旧示教帧只在所在段帧范围与类型都没变时保留。
//! - 示教件边界没有旧结构可比（还没存过边界，或边界与段数对不上）时返回 None，按段的类型序列整体处理（`update_doc`）。

use super::burst::CameraTeaching;
use super::*;
use crate::plan::{segment_cameras, PathSegment};
use crate::recipe::SegmentCorridor;

/// 各段在示教件上的帧组范围（含两端）：cuts 是每段最后一个帧组。
fn spans(cuts: &[u64], group_count: u64) -> Vec<(u64, u64)> {
    let ends: Vec<u64> = cuts.iter().copied().chain([group_count]).collect();
    ends.iter()
        .enumerate()
        .map(|(i, &end)| (if i == 0 { 1 } else { ends[i - 1] + 1 }, end))
        .collect()
}

fn overlap(a: (u64, u64), b: (u64, u64)) -> u64 {
    (a.1.min(b.1) + 1).saturating_sub(a.0.max(b.0))
}

/// 保存后的段列表与段示教帧（走廊已在 `doc.inspect` 里迁到新段号）。
pub(super) struct Remapped {
    pub doc: RecipeDoc,
    pub teaching: Vec<CameraTeaching>,
}

/// 按示教件 `round` 上旧边界与新边界 `cuts` 对应新旧段，得出要保存的段列表、走廊与段示教帧；没有旧边界可比时为 None。
pub(super) fn remap(
    w: &Workspace,
    round: &str,
    incoming: &[PathSegment],
    cuts: &[u64],
) -> Option<Remapped> {
    let review = w.capture_review.rounds.get(round)?;
    let old = &w.doc.segments;
    if !w.doc.is_burst() || review.cuts.len() + 1 != old.len() || cuts.len() + 1 != incoming.len() {
        return None;
    }
    let old_spans = spans(&review.cuts, review.group_count);
    let new_spans = spans(cuts, review.group_count);
    let plan_cameras = w.doc.plan_cameras();
    let matched: Vec<Option<usize>> = incoming
        .iter()
        .zip(&new_spans)
        .map(|(segment, &span)| {
            (0..old.len())
                .filter(|&j| old[j].kind == segment.kind && overlap(old_spans[j], span) > 0)
                .max_by_key(|&j| (overlap(old_spans[j], span), std::cmp::Reverse(j)))
        })
        .collect();
    let segments: Vec<PathSegment> = incoming
        .iter()
        .enumerate()
        .map(|(i, segment)| {
            let span = new_spans[i];
            let mut next = match matched[i] {
                Some(j) => {
                    let (before, inherited) = (old_spans[j], &old[j]);
                    PathSegment {
                        cameras: segment.cameras.clone(),
                        length_mm: inherited.length_mm.filter(|_| before == span),
                        skip_head: if before.0 == span.0 {
                            inherited.skip_head
                        } else {
                            0
                        },
                        skip_tail: if before.1 == span.1 {
                            inherited.skip_tail
                        } else {
                            0
                        },
                        ..inherited.clone()
                    }
                }
                None => segment.clone(),
            };
            next.index = i as u32 + 1;
            next.kind = segment.kind;
            next.skip = segment.skip;
            let effective = segment_cameras(&next, &plan_cameras);
            if let Some((reference, first)) = next
                .offset_ref
                .as_mut()
                .filter(|r| !effective.contains(&r.camera))
                .zip(effective.first())
            {
                reference.camera = first.clone();
                (reference.x_ref, reference.spread) = (None, None);
            }
            next
        })
        .collect();
    // 每条旧示教帧落到哪个新段：示教件上的按帧号，其余只认帧范围与类型都没变的段
    let target = |t: &CameraTeaching, j: usize| -> Option<usize> {
        let i = if t.round == round {
            new_spans
                .iter()
                .position(|&(first, last)| (first..=last).contains(&t.seq))?
        } else {
            (0..segments.len()).find(|&i| matched[i] == Some(j) && new_spans[i] == old_spans[j])?
        };
        (segments[i].kind == old[j].kind
            && segment_cameras(&segments[i], &plan_cameras).contains(&t.camera))
        .then_some(i)
    };
    let mut kept: Vec<(usize, usize, &CameraTeaching)> = Vec::new();
    for t in &w.camera_teaching {
        let Some(j) = t
            .segment
            .and_then(|index| old.iter().position(|s| s.index == index))
        else {
            continue;
        };
        let Some(i) = target(t, j) else { continue };
        match kept
            .iter()
            .position(|&(i2, _, t2)| i2 == i && t2.camera == t.camera)
        {
            Some(at) if matched[i] == Some(j) && kept[at].1 != j => kept[at] = (i, j, t),
            Some(_) => {}
            None => kept.push((i, j, t)),
        }
    }
    let mut doc = w.doc.clone();
    for inspect in &mut doc.inspect {
        let before = std::mem::take(&mut inspect.segments);
        inspect.segments = kept
            .iter()
            .filter(|(_, _, t)| t.camera == inspect.camera)
            .filter_map(|&(i, j, _)| {
                before
                    .iter()
                    .find(|c| c.segment == old[j].index)
                    .map(|c| SegmentCorridor {
                        segment: segments[i].index,
                        corridor: c.corridor.clone(),
                    })
            })
            .collect();
        inspect.segments.sort_by_key(|c| c.segment);
    }
    doc.segments = segments;
    let teaching = kept
        .into_iter()
        .map(|(i, _, t)| CameraTeaching {
            segment: Some(i as u32 + 1),
            ..t.clone()
        })
        .collect();
    Some(Remapped { doc, teaching })
}

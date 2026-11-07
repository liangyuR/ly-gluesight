//! 分段（规格 §3）：段结构由人定，PC 在每件收尾后定位段间边界（D-SB-40）。
//!
//! - `Proportional`：按标定的各段 N_exp 比例与本件帧组数推算边界（生产、标定与历史重判都用它）；
//! - 各段 N_exp 未标定或单段配方时，整条胶路作为一个未分段区域。

use serde::{Deserialize, Serialize};

use crate::burst_judge::FrameMetrics;
use crate::frame_group::FrameGroup;
use crate::passes::{self, PassRules};
use crate::plan::{PathSegment, SegmentKind};

/// 分段输入：三路帧组和计划（段类型序列、N_exp、tol）。
pub struct SegInput<'a> {
    pub groups: &'a [FrameGroup],
    pub segments: &'a [PathSegment],
}

/// 第 `after_segment` 段与下一段之间的边界落在帧组 `frame_seq`：`frame_seq` 是第 `after_segment` 段的最后一帧，
/// 下一段从 `frame_seq + 1` 起，路径起点视为边界 0。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Boundary {
    pub after_segment: u32,
    pub frame_seq: u64,
    /// 0–1
    pub confidence: f32,
    /// 边界正好在两笔之间（D-SB-62）：位置确定，不留过渡区、不按方向细化
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hard: bool,
}

/// 分段结果；示教轮次写进 boundaries.json。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "boundaries", rename_all = "camelCase")]
pub enum SegOutcome {
    /// 全部未分段
    Unsegmented,
    Boundaries(Vec<Boundary>),
}

pub trait Segmenter {
    fn segment(&self, input: &SegInput) -> SegOutcome;
}

/// 比例分段（D-SB-40）：本件帧组数 total 已知时，第 k 个边界 = round(total × 前 k 段 N_exp 之和 / ΣN_exp)，
/// 每段至少留 1 帧。total ∈ ΣN_exp ± Σtol 时各边界置信度 1，否则 0（由 [`resolve_regions`] 整体合并为置信度不足）。
/// 单段配方没有边界可定位，任一段 N_exp 未标定时没有比例可用，都返回 Unsegmented（与没有分段算法时相同）。
pub struct Proportional;

impl Segmenter for Proportional {
    fn segment(&self, input: &SegInput) -> SegOutcome {
        proportional_for_count(
            input.segments,
            input.groups.iter().map(|g| g.seq).max().unwrap_or(0),
        )
    }
}

fn proportional_for_count(segments: &[PathSegment], total: u64) -> SegOutcome {
    let Some(counts) = segments
        .iter()
        .map(|s| s.expected_frames.map(u64::from))
        .collect::<Option<Vec<u64>>>()
    else {
        return SegOutcome::Unsegmented;
    };
    let expected: u64 = counts.iter().sum();
    if counts.len() < 2 || expected == 0 || total == 0 {
        return SegOutcome::Unsegmented;
    }
    let tolerance: u64 = segments.iter().map(|s| u64::from(s.tolerance)).sum();
    let confidence = if total.abs_diff(expected) <= tolerance {
        1.0
    } else {
        0.0
    };
    let (mut cumulative, mut previous) = (0, 0);
    let last = counts.len() - 1;
    let boundaries = counts[..last]
        .iter()
        .enumerate()
        .map(|(k, &n)| {
            cumulative += n;
            let scaled = (2 * total * cumulative + expected) / (2 * expected);
            let frame_seq = scaled
                .max(previous + 1)
                .min(total.saturating_sub((last - k) as u64));
            previous = frame_seq;
            Boundary {
                after_segment: segments[k].index,
                frame_seq,
                confidence,
                hard: false,
            }
        })
        .collect();
    SegOutcome::Boundaries(boundaries)
}

/// 按笔分段（D-SB-62）：本件笔数与示教件一致、各段 N_exp 之和等于示教件笔结构的帧组数时，示教件的段边界逐个按笔换算到本件
/// （`passes::map_cuts`），置信度 1。条件不满足为 None。
fn by_passes(segments: &[PathSegment], passes: PassRules) -> Option<SegOutcome> {
    let taught = passes
        .taught
        .filter(|_| passes.aligned() && segments.len() >= 2)?;
    let counts: Vec<u64> = segments
        .iter()
        .map(|s| s.expected_frames.map(u64::from))
        .collect::<Option<_>>()?;
    if counts.iter().sum::<u64>() != taught.iter().map(|&n| u64::from(n)).sum::<u64>() {
        return None;
    }
    let cuts: Vec<u64> = counts[..counts.len() - 1]
        .iter()
        .scan(0, |sum, &n| {
            *sum += n;
            Some(*sum)
        })
        .collect();
    let mapped = passes::map_cuts(&cuts, taught, passes.part)?;
    Some(SegOutcome::Boundaries(
        mapped
            .into_iter()
            .zip(segments)
            .map(|((frame_seq, hard), segment)| Boundary {
                after_segment: segment.index,
                frame_seq,
                confidence: 1.0,
                hard,
            })
            .collect(),
    ))
}

/// 本件的分段（生产、复测与历史重判共用）：本件笔数与示教件一致时按笔换算边界（D-SB-62），笔数不一致时比例边界一律不可信
/// （整件按未分段判），没有笔结构时按 [`Proportional`]；配方开了 `boundary_refine` 时再用 [`refine_by_heading`] 细化
/// （要每帧特征）。
pub fn segment_part(
    input: &SegInput,
    features: &[FrameMetrics],
    refine: bool,
    passes: PassRules,
) -> SegOutcome {
    let outcome = by_passes(input.segments, passes).unwrap_or_else(|| {
        let mut outcome = Proportional.segment(input);
        if let (true, SegOutcome::Boundaries(boundaries)) = (passes.mismatch(), &mut outcome) {
            boundaries.iter_mut().for_each(|b| b.confidence = 0.0);
        }
        outcome
    });
    if refine {
        refine_by_heading(input, features, outcome, passes.part)
    } else {
        outcome
    }
}

/// 方向细化（D-SB-47）：直线段与弯道之间、直线段开了逐帧定向的边界，在给定边界 ± (两段容差之和 + 1)
/// 帧内对准直线段检测相机的胶条方向开始偏离（或回到）该段方向的帧。直线段方向取每台相机在该段核心帧（离两端边界都超出窗口）
/// 上测得方向的中位数，偏离阈值为该段的方向容差；某帧多数相机偏离即算离开直线段。窗口内没有方向数据时保留原边界，
/// 置信度不变。只用找到了胶条的帧。`features` 是本件各相机的每帧指标。两笔之间的硬边界不动；知道笔结构（`layout`）时
/// 笔中间的边界只在本笔内移动（D-SB-62）。
pub fn refine_by_heading(
    input: &SegInput,
    features: &[FrameMetrics],
    outcome: SegOutcome,
    layout: &[u32],
) -> SegOutcome {
    let SegOutcome::Boundaries(mut boundaries) = outcome else {
        return outcome;
    };
    let total = input.groups.iter().map(|g| g.seq).max().unwrap_or(0);
    let cuts: Vec<u64> = boundaries.iter().map(|b| b.frame_seq).collect();
    let last = cuts.len();
    for k in 0..last {
        let (a, c) = (&input.segments[k], &input.segments[k + 1]);
        let (s, straight_first) = match (a.kind, c.kind) {
            (SegmentKind::Straight, SegmentKind::Corner) => (k, true),
            (SegmentKind::Corner, SegmentKind::Straight) => (k + 1, false),
            _ => continue,
        };
        let straight = &input.segments[s];
        let Some(settings) = &straight.heading else {
            continue;
        };
        let window = u64::from(a.tolerance + c.tolerance) + 1;
        let (begin, end) = (
            if s == 0 { 1 } else { cuts[s - 1] + 1 },
            if s == last { total } else { cuts[s] },
        );
        let core = (if s == 0 { begin } else { begin + window })..=(if s == last {
            end
        } else {
            end.saturating_sub(window)
        });
        if boundaries[k].hard {
            continue;
        }
        let pass = passes::locate(layout, cuts[k])
            .map(|(j, _)| {
                let first = passes::first_seq(layout, j);
                (first, first + u64::from(layout[j]).saturating_sub(2))
            })
            .unwrap_or((1, u64::MAX));
        let lo = cuts[k]
            .saturating_sub(window)
            .max(if k == 0 {
                1
            } else {
                boundaries[k - 1].frame_seq + 1
            })
            .max(pass.0);
        let hi = (cuts[k] + window)
            .min(if k + 1 < last {
                cuts[k + 1] - 1
            } else {
                total.saturating_sub(1)
            })
            .min(pass.1);
        if lo > hi {
            continue;
        }
        let theta = |m: &&FrameMetrics| {
            m.heading_deg.filter(|_| {
                !m.heading_fallback && m.state == crate::burst_judge::FrameState::Measured
            })
        };
        let cameras: std::collections::BTreeSet<usize> = features
            .iter()
            .filter(|m| m.corridor == Some(straight.index) && theta(m).is_some())
            .map(|m| m.camera)
            .collect();
        let reference: Vec<(usize, f64)> = cameras
            .iter()
            .filter_map(|&camera| {
                let mut core_theta: Vec<f64> = features
                    .iter()
                    .filter(|m| m.camera == camera && core.contains(&m.seq))
                    .filter_map(|m| theta(&m))
                    .collect();
                let base = *core_theta.first()?;
                core_theta
                    .iter_mut()
                    .for_each(|t| *t = base + crate::heading::wrap(*t - base));
                core_theta.sort_by(f64::total_cmp);
                Some((camera, core_theta[core_theta.len() / 2]))
            })
            .collect();
        let in_straight = |seq: u64| -> Option<bool> {
            let votes: Vec<bool> = reference
                .iter()
                .filter_map(|&(camera, r)| {
                    let m = features
                        .iter()
                        .find(|m| m.camera == camera && m.seq == seq)?;
                    theta(&m).map(|t| crate::heading::wrap(t - r).abs() <= settings.tolerance_deg)
                })
                .collect();
            (!votes.is_empty()).then(|| votes.iter().filter(|&&v| v).count() * 2 > votes.len())
        };
        let observed: Vec<(u64, bool)> = (lo..=hi + 1)
            .filter_map(|seq| in_straight(seq).map(|v| (seq, v)))
            .collect();
        if observed.is_empty() {
            continue;
        }
        let score = |cut: u64| -> i64 {
            observed
                .iter()
                .map(|&(seq, v)| {
                    if v == ((seq <= cut) == straight_first) {
                        1
                    } else {
                        -1
                    }
                })
                .sum()
        };
        let best = (lo..=hi)
            .max_by_key(|&cut| (score(cut), std::cmp::Reverse(cut.abs_diff(cuts[k]))))
            .unwrap_or(cuts[k]);
        boundaries[k].frame_seq = best;
    }
    SegOutcome::Boundaries(boundaries)
}

/// 未分段的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnsegReason {
    /// 分段算法未就绪：各段 N_exp 未标定或单段配方
    AlgorithmPending,
    /// 边界置信度不足：两侧两段合并，不回退计划边界
    LowConfidence,
    /// 可信边界两侧各 `boundary_guard` 帧的过渡区（D-SB-40）：边界位置有 ±1–2 帧的不确定，只判每帧指标、不判胶偏
    Transition,
}

/// 区域类型。JSON 里用 `type` 区分，避免与 `Region.kind` 嵌套同名。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RegionKind {
    Straight { segment: u32 },
    Corner { segment: u32 },
    /// 覆盖 first_segment..=last_segment；只跑每帧独立检测，允差取覆盖各段中较宽一档，不判胶偏
    Unsegmented { first_segment: u32, last_segment: u32, reason: UnsegReason },
    /// 段首尾跳过的帧（`skip_head` / `skip_tail`）或每笔首尾跳过的帧（D-SB-62，`pass` 为第几笔，从 1 起；`short`：
    /// 整笔太短全部跳过）：不判定，结果里列为未检区域
    Skipped {
        segment: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pass: Option<u32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        short: bool,
    },
}

/// 判胶的单位：一段连续帧组。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Region {
    pub kind: RegionKind,
    pub first_seq: u64,
    pub last_seq: u64,
}

impl Region {
    /// 区域帧数 = 帧组数 = last_seq − first_seq + 1（与标定同一口径）。
    pub fn frame_count(&self) -> u32 {
        u32::try_from((self.last_seq + 1).saturating_sub(self.first_seq)).unwrap_or(u32::MAX)
    }
}

/// 判定需要的走廊（D-SB-45）：按计划相机、seq（1..=total）给出该用第几段（`PathSegment.index`）的走廊测量，None 为不测。
/// 可信的直线 / 弯道区域：本段检测相机用本段走廊。过渡区：帧按比例边界落在哪一段，该段检测相机用该段走廊，只检测最近边界
/// 另一侧那一段的相机用那一段的走廊；多个过渡区合并（中间的段被取空）时同样按帧所在段与最近的边界取，不跨到更远的段。跳过区域不测。其余未分段区域（置信度不足、算法待定）：用到达时的预测段与它的检测相机（`plan::arrival_corridor`）。
pub fn frame_corridors(
    segments: &[PathSegment],
    cameras: &[String],
    regions: &[Region],
    outcome: &SegOutcome,
    total: u64,
) -> Vec<Vec<Option<u32>>> {
    let boundary = |after: u32| match outcome {
        SegOutcome::Boundaries(boundaries) => boundaries
            .iter()
            .find(|b| b.after_segment == after)
            .map(|b| b.frame_seq),
        SegOutcome::Unsegmented => None,
    };
    let mut out = vec![vec![None; total as usize]; cameras.len()];
    for region in regions {
        for seq in region.first_seq.max(1)..=region.last_seq.min(total) {
            for (c, camera) in cameras.iter().enumerate() {
                let detects = |index: u32| crate::plan::detects(segments, index, camera, cameras);
                out[c][(seq - 1) as usize] = match region.kind {
                    RegionKind::Straight { segment } | RegionKind::Corner { segment } => {
                        detects(segment).then_some(segment)
                    }
                    RegionKind::Skipped { .. } => None,
                    RegionKind::Unsegmented {
                        first_segment,
                        last_segment,
                        reason: UnsegReason::Transition,
                    } => {
                        let covered: Vec<u32> = segments
                            .iter()
                            .map(|s| s.index)
                            .filter(|i| (first_segment..=last_segment).contains(i))
                            .collect();
                        let Some(k) = covered
                            .iter()
                            .position(|&i| boundary(i).is_none_or(|b| seq <= b))
                            .or(covered.len().checked_sub(1))
                        else {
                            continue;
                        };
                        let after_start = (k > 0)
                            .then(|| boundary(covered[k - 1]))
                            .flatten()
                            .map(|b| seq - b);
                        let before_end = (k + 1 < covered.len())
                            .then(|| boundary(covered[k]))
                            .flatten()
                            .map(|b| b + 1 - seq);
                        let neighbor = match (after_start, before_end) {
                            (Some(a), Some(e)) if e < a => Some(k + 1),
                            (Some(_), _) => Some(k - 1),
                            (None, Some(_)) => Some(k + 1),
                            (None, None) => None,
                        };
                        std::iter::once(covered[k])
                            .chain(neighbor.map(|n| covered[n]))
                            .find(|&s| detects(s))
                    }
                    RegionKind::Unsegmented { .. } => {
                        crate::plan::arrival_corridor(segments, cameras, c, seq)
                    }
                };
            }
        }
    }
    out
}

fn core_regions(
    segments: &[PathSegment],
    outcome: &SegOutcome,
    total_seq: u64,
    min_conf: f32,
) -> Vec<(Region, usize, usize)> {
    if segments.is_empty() || total_seq == 0 {
        return Vec::new();
    }
    // 按可信边界切成区域，记下每个区域覆盖的段下标范围
    let mut regions: Vec<(Region, usize, usize)> = Vec::new();
    match outcome {
        SegOutcome::Unsegmented => {
            let kind = RegionKind::Unsegmented {
                first_segment: segments[0].index,
                last_segment: segments[segments.len() - 1].index,
                reason: UnsegReason::AlgorithmPending,
            };
            regions.push((
                Region {
                    kind,
                    first_seq: 1,
                    last_seq: total_seq,
                },
                0,
                segments.len() - 1,
            ));
        }
        SegOutcome::Boundaries(boundaries) => {
            // 每个段间隙的可信边界帧号（第 k 段之后）；None = 置信度不足，两侧合并
            let mut previous = 0;
            let cuts: Vec<Option<u64>> = segments
                .windows(2)
                .map(|pair| {
                    let cut = boundaries
                        .iter()
                        .find(|b| b.after_segment == pair[0].index)
                        .filter(|b| {
                            b.confidence >= min_conf
                                && b.frame_seq > previous
                                && b.frame_seq < total_seq
                        })
                        .map(|b| b.frame_seq);
                    previous = cut.unwrap_or(previous);
                    cut
                })
                .collect();
            let (mut start, mut first_seq) = (0, 1);
            for k in 0..segments.len() {
                let Some(last_seq) = cuts.get(k).copied().unwrap_or(Some(total_seq)) else {
                    continue;
                };
                let kind = match (start == k, segments[k].kind) {
                    (true, SegmentKind::Straight) => RegionKind::Straight {
                        segment: segments[k].index,
                    },
                    (true, SegmentKind::Corner) => RegionKind::Corner {
                        segment: segments[k].index,
                    },
                    (false, _) => RegionKind::Unsegmented {
                        first_segment: segments[start].index,
                        last_segment: segments[k].index,
                        reason: UnsegReason::LowConfidence,
                    },
                };
                regions.push((
                    Region {
                        kind,
                        first_seq,
                        last_seq,
                    },
                    start,
                    k,
                ));
                (start, first_seq) = (k + 1, last_seq + 1);
            }
        }
    }

    regions
}

/// 把分段结果解析成区域：Unsegmented 时一个覆盖 1..=total_seq 的区域（AlgorithmPending）；
/// 置信度低于 min_conf 的边界把两侧两段合并为 Unsegmented(LowConfidence)，连续多个低置信度边界一并合并，不回退计划边界。
/// 每个可信边界两侧各 `guard` 帧（边界帧 b 时为 b − guard + 1..=b + guard，截在 1..=total_seq 内）划为过渡区
/// Unsegmented(Transition)，覆盖边界两侧的段；相邻的过渡区（中间的段被取空）合并成一个。guard = 0 时没有过渡区。
/// 各段的跳过帧（`skip_head` / `skip_tail`）最后划为 Skipped 区域，优先于过渡区：能定位到的段（单段区域）按本段首尾跳过；
/// 合并区域里只有贴着整件首尾的那一头能定位，只跳过第 1 段的 skip_head 与末段的 skip_tail。
/// 两笔之间的硬边界（D-SB-62）不留过渡区。知道本件笔结构时，每笔首尾按 `passes.skip` 跳过（不论分段是否可信），
/// 区域记下第几笔，整笔太短的标 `short`；已按段跳过的帧不重复标。
///
/// 算法缺给某个段间边界，或给出的帧号不在上一可信边界之后、不给下一段留帧，都按置信度不足处理。
/// 区域首尾相接、覆盖 1..=total_seq。没有帧组（total_seq = 0）或没有段时返回空。
pub fn resolve_regions(
    segments: &[PathSegment],
    outcome: &SegOutcome,
    total_seq: u64,
    min_conf: f32,
    guard: u32,
    passes: PassRules,
) -> Vec<Region> {
    let regions = core_regions(segments, outcome, total_seq, min_conf);
    if regions.is_empty() {
        return Vec::new();
    }
    let hard: std::collections::BTreeSet<u64> = match outcome {
        SegOutcome::Boundaries(boundaries) => boundaries
            .iter()
            .filter(|b| b.hard)
            .map(|b| b.frame_seq)
            .collect(),
        SegOutcome::Unsegmented => Default::default(),
    };
    // 逐帧标注：所属区域、过渡区覆盖的段下标范围（多个边界的过渡区重叠时取并），跳过的段，或每笔首尾跳过（段下标、笔下标、整笔）
    #[derive(Clone, Copy, PartialEq)]
    enum Label {
        Core(usize),
        Transition(usize, usize),
        Skipped(usize),
        PassSkipped(usize, usize, bool),
    }
    let mut labels: Vec<Label> = Vec::with_capacity(total_seq as usize);
    for (i, (region, _, _)) in regions.iter().enumerate() {
        labels.extend((region.first_seq..=region.last_seq).map(|_| Label::Core(i)));
    }
    if guard > 0 {
        for i in 1..regions.len() {
            let (cut, left, right) = (regions[i - 1].0.last_seq, regions[i - 1].2, regions[i].1);
            if hard.contains(&cut) {
                continue;
            }
            let from = cut.saturating_sub(u64::from(guard) - 1).max(1);
            let to = (cut + u64::from(guard)).min(total_seq);
            for label in &mut labels[from as usize - 1..to as usize] {
                *label = match *label {
                    Label::Transition(a, b) => Label::Transition(a.min(left), b.max(right)),
                    _ => Label::Transition(left, right),
                };
            }
        }
    }
    let last_segment = segments.len() - 1;
    for (region, a, b) in &regions {
        let (first, last) = (region.first_seq, region.last_seq);
        let mut skip = |from: u64, to: u64, k: usize| {
            for seq in from.max(first)..=to.min(last) {
                labels[seq as usize - 1] = Label::Skipped(k);
            }
        };
        let head = if a == b || *a == 0 {
            u64::from(segments[*a].skip_head)
        } else {
            0
        };
        if head > 0 && (a == b || first == 1) {
            skip(first, first + head - 1, *a);
        }
        let tail = if a == b || *b == last_segment {
            u64::from(segments[*b].skip_tail)
        } else {
            0
        };
        if tail > 0 && (a == b || last == total_seq) {
            skip((last + 1).saturating_sub(tail), last, *b);
        }
    }

    // 不检测的段（D-SB-53）：分段可信时整段划为跳过，并向两侧各延伸 guard 帧——边界有 ±guard 帧的不确定，邻段贴着边界的帧
    // 可能其实属于这一段，照常测会误报。分段不可信时定位不到这一段，只是没有相机量它的帧（`segment_cameras` 为空）
    for (region, a, b) in &regions {
        if a != b || !segments[*a].skip {
            continue;
        }
        let from = region.first_seq.saturating_sub(u64::from(guard)).max(1);
        let to = (region.last_seq + u64::from(guard)).min(total_seq);
        for label in &mut labels[from as usize - 1..to as usize] {
            *label = Label::Skipped(*a);
        }
    }

    for (j, from, to, short) in passes::skipped(passes.part, passes.skip) {
        for seq in from.max(1)..=to.min(total_seq) {
            let label = &mut labels[seq as usize - 1];
            let segment = match *label {
                Label::Core(i) => regions[i].1,
                Label::Transition(a, _) => a,
                Label::Skipped(_) | Label::PassSkipped(..) => continue,
            };
            *label = Label::PassSkipped(segment, j, short);
        }
    }

    let mut out: Vec<(Label, Region)> = Vec::new();
    for (k, &label) in labels.iter().enumerate() {
        let seq = k as u64 + 1;
        match (out.last_mut(), label) {
            (Some((Label::Core(a), region)), Label::Core(b))
            | (Some((Label::Skipped(a), region)), Label::Skipped(b))
                if *a == b =>
            {
                region.last_seq = seq
            }
            (Some((Label::PassSkipped(a, j, s), region)), Label::PassSkipped(b, k, t))
                if (*a, *j, *s) == (b, k, t) =>
            {
                region.last_seq = seq
            }
            (Some((Label::Transition(a, b), region)), Label::Transition(c, d)) => {
                (*a, *b) = ((*a).min(c), (*b).max(d));
                region.last_seq = seq;
            }
            _ => {
                let kind = match label {
                    Label::Core(i) => regions[i].0.kind.clone(),
                    Label::Skipped(k) => RegionKind::Skipped {
                        segment: segments[k].index,
                        pass: None,
                        short: false,
                    },
                    Label::PassSkipped(k, j, short) => RegionKind::Skipped {
                        segment: segments[k].index,
                        pass: Some(j as u32 + 1),
                        short,
                    },
                    Label::Transition(..) => RegionKind::Unsegmented {
                        first_segment: 0,
                        last_segment: 0,
                        reason: UnsegReason::Transition,
                    },
                };
                out.push((
                    label,
                    Region {
                        kind,
                        first_seq: seq,
                        last_seq: seq,
                    },
                ));
            }
        }
    }
    out.into_iter()
        .map(|(label, mut region)| {
            if let Label::Transition(a, b) = label {
                region.kind = RegionKind::Unsegmented {
                    first_segment: segments[a].index,
                    last_segment: segments[b].index,
                    reason: UnsegReason::Transition,
                };
            }
            region
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageRange {
    pub first_seq: u64,
    pub last_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraCoverage {
    pub camera: String,
    pub ranges: Vec<CoverageRange>,
    pub expected_frames: Option<u64>,
    pub available_frames: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OffsetCoverageReason {
    Disabled,
    Skipped,
    Corner,
    Unsegmented,
    LowConfidence,
    NoReference,
    MeasureNotImplemented,
    NoInteriorFrames,
    ExcludedGroups,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffsetCoverage {
    pub camera: Option<String>,
    pub ranges: Vec<CoverageRange>,
    pub expected_frames: u64,
    pub available_frames: u64,
    pub unjudged_reasons: Vec<OffsetCoverageReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentCoverage {
    pub segment: u32,
    pub kind: SegmentKind,
    pub skipped: bool,
    pub range: Option<CoverageRange>,
    pub cameras: Vec<CameraCoverage>,
    pub expected_camera_frames: Option<u64>,
    pub available_camera_frames: Option<u64>,
    pub offset: OffsetCoverage,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectionCoverage {
    pub group_count: u64,
    pub boundary_guard: u32,
    pub boundaries_trusted: bool,
    pub boundary_refinement_pending: bool,
    pub regions: Vec<Region>,
    pub excluded_groups: Vec<u64>,
    pub expected_camera_frames: Option<u64>,
    pub available_camera_frames: Option<u64>,
    pub zero_coverage_segments: Vec<u32>,
    pub segments: Vec<SegmentCoverage>,
}

fn coverage_ranges(seqs: impl IntoIterator<Item = u64>) -> Vec<CoverageRange> {
    let mut ranges: Vec<CoverageRange> = Vec::new();
    for seq in seqs {
        if let Some(last) = ranges.last_mut().filter(|last| last.last_seq + 1 == seq) {
            last.last_seq = seq;
        } else {
            ranges.push(CoverageRange {
                first_seq: seq,
                last_seq: seq,
            });
        }
    }
    ranges
}

fn offset_coverage(
    segment: &PathSegment,
    cameras: &[String],
    regions: &[Region],
    uncertainty: Option<UnsegReason>,
    excluded: &std::collections::BTreeSet<u64>,
) -> OffsetCoverage {
    use crate::burst_judge::{offset_window, OffsetUnjudged};
    use OffsetCoverageReason as Reason;
    let mut result = OffsetCoverage {
        camera: segment.offset_ref.as_ref().map(|r| r.camera.clone()),
        ranges: Vec::new(),
        expected_frames: 0,
        available_frames: 0,
        unjudged_reasons: Vec::new(),
    };
    let inactive = if segment.skip {
        Some(Reason::Skipped)
    } else if segment.kind == SegmentKind::Corner {
        Some(Reason::Corner)
    } else if segment.checks.offset.as_ref().is_none_or(|o| !o.enabled) {
        Some(Reason::Disabled)
    } else {
        uncertainty.map(|reason| match reason {
            UnsegReason::LowConfidence => Reason::LowConfidence,
            _ => Reason::Unsegmented,
        })
    };
    if let Some(reason) = inactive {
        result.unjudged_reasons.push(reason);
        return result;
    }
    for region in regions.iter().filter(
        |r| matches!(r.kind, RegionKind::Straight { segment: index } if index == segment.index),
    ) {
        match offset_window(segment, &segment.checks, region, cameras) {
            Ok(window)
                if crate::plan::detects(
                    std::slice::from_ref(segment),
                    segment.index,
                    &cameras[window.camera],
                    cameras,
                ) =>
            {
                result.expected_frames += window.last_seq - window.first_seq + 1;
                result.available_frames += (window.first_seq..=window.last_seq)
                    .filter(|seq| !excluded.contains(seq))
                    .count() as u64;
                result.ranges.push(CoverageRange {
                    first_seq: window.first_seq,
                    last_seq: window.last_seq,
                });
            }
            other => {
                let reason = match other {
                    Err(Some(OffsetUnjudged::NoReference)) => Reason::NoReference,
                    Err(Some(OffsetUnjudged::MeasureNotImplemented)) => {
                        Reason::MeasureNotImplemented
                    }
                    _ => Reason::NoInteriorFrames,
                };
                if !result.unjudged_reasons.contains(&reason) {
                    result.unjudged_reasons.push(reason);
                }
            }
        }
    }
    if result.expected_frames == 0 && result.unjudged_reasons.is_empty() {
        result.unjudged_reasons.push(Reason::NoInteriorFrames);
    }
    if result.expected_frames > 0 && result.available_frames == 0 {
        result.unjudged_reasons.push(Reason::ExcludedGroups);
    }
    result
}

/// 比例分段下的应测范围；不读取图像、不保证测量成功。屏蔽只影响示教件可用帧，不改变生产应测帧。
/// 给了笔结构时按笔分段并扣掉每笔首尾跳过的帧（D-SB-62）。
pub fn proportional_coverage(
    segments: &[PathSegment],
    cameras: &[String],
    total_seq: u64,
    guard: u32,
    refine: bool,
    excluded_groups: &[u64],
    passes: PassRules,
) -> InspectionCoverage {
    let outcome =
        by_passes(segments, passes).unwrap_or_else(|| proportional_for_count(segments, total_seq));
    let core = core_regions(segments, &outcome, total_seq, 0.5);
    let boundaries_trusted = !core.is_empty()
        && core
            .iter()
            .all(|(r, _, _)| !matches!(r.kind, RegionKind::Unsegmented { .. }));
    let regions = resolve_regions(segments, &outcome, total_seq, 0.5, guard, passes);
    let corridors = frame_corridors(segments, cameras, &regions, &outcome, total_seq);
    let excluded: std::collections::BTreeSet<u64> = excluded_groups
        .iter()
        .copied()
        .filter(|seq| (1..=total_seq).contains(seq))
        .collect();
    let covered: Vec<SegmentCoverage> = segments
        .iter()
        .map(|segment| {
            let region = core
                .iter()
                .find(|(_, first, last)| first == last && segments[*first].index == segment.index)
                .map(|(region, _, _)| region);
            let trusted = region.is_some_and(|r| !matches!(r.kind, RegionKind::Unsegmented { .. }));
            let camera_coverage: Vec<CameraCoverage> =
                crate::plan::segment_cameras(segment, cameras)
                    .into_iter()
                    .map(|camera| {
                        let camera_index = cameras.iter().position(|id| *id == camera).unwrap();
                        let seqs: Vec<u64> = corridors[camera_index]
                            .iter()
                            .enumerate()
                            .filter(|(_, corridor)| trusted && **corridor == Some(segment.index))
                            .map(|(k, _)| k as u64 + 1)
                            .collect();
                        CameraCoverage {
                            camera,
                            ranges: coverage_ranges(seqs.iter().copied()),
                            expected_frames: trusted.then_some(seqs.len() as u64),
                            available_frames: trusted.then_some(
                                seqs.iter().filter(|seq| !excluded.contains(seq)).count() as u64,
                            ),
                        }
                    })
                    .collect();
            SegmentCoverage {
                segment: segment.index,
                kind: segment.kind,
                skipped: segment.skip,
                range: region.filter(|_| trusted).map(|r| CoverageRange {
                    first_seq: r.first_seq,
                    last_seq: r.last_seq,
                }),
                expected_camera_frames: trusted.then(|| {
                    camera_coverage
                        .iter()
                        .filter_map(|c| c.expected_frames)
                        .sum()
                }),
                available_camera_frames: trusted.then(|| {
                    camera_coverage
                        .iter()
                        .filter_map(|c| c.available_frames)
                        .sum()
                }),
                cameras: camera_coverage,
                offset: offset_coverage(
                    segment,
                    cameras,
                    &regions,
                    core.iter().find_map(|(r, first, last)| {
                        (segments[*first].index <= segment.index
                            && segment.index <= segments[*last].index)
                            .then(|| match r.kind {
                                RegionKind::Unsegmented { reason, .. } => Some(reason),
                                _ => None,
                            })
                            .flatten()
                    }),
                    &excluded,
                ),
            }
        })
        .collect();
    InspectionCoverage {
        group_count: total_seq,
        boundary_guard: guard,
        boundaries_trusted,
        boundary_refinement_pending: refine,
        regions,
        excluded_groups: excluded.into_iter().collect(),
        expected_camera_frames: boundaries_trusted.then(|| {
            covered
                .iter()
                .filter_map(|s| s.expected_camera_frames)
                .sum()
        }),
        available_camera_frames: boundaries_trusted.then(|| {
            covered
                .iter()
                .filter_map(|s| s.available_camera_frames)
                .sum()
        }),
        zero_coverage_segments: covered
            .iter()
            .filter(|s| !s.skipped && s.expected_camera_frames == Some(0))
            .map(|s| s.segment)
            .collect(),
        segments: covered,
    }
}

pub mod synthetic {
    use crate::plan::SegmentKind;

    /// 合成序列的剖面。
    #[derive(Clone, Debug, PartialEq)]
    pub struct SequenceSpec {
        /// 各段 (类型, 名义帧数)
        pub segments: Vec<(SegmentKind, u32)>,
        /// 起头 / 收尾帧数波动 ±n
        pub head_tail_jitter: i32,
        /// 弯道减速帧数波动 ±n
        pub corner_jitter: i32,
    }

    /// 各段实际帧数 = 名义帧数 + 波动（至少 1 帧）：首段加起头波动、末段加收尾波动（各 ±head_tail_jitter，
    /// 单段时两者都加），弯道段加减速波动（±corner_jitter）。同一 spec 与 seed 结果确定。
    pub fn frame_counts(spec: &SequenceSpec, seed: u64) -> Vec<u64> {
        realize(spec, &mut SplitMix64(seed))
    }

    fn realize(spec: &SequenceSpec, rng: &mut SplitMix64) -> Vec<u64> {
        let last = spec.segments.len().saturating_sub(1);
        spec.segments
            .iter()
            .enumerate()
            .map(|(k, &(kind, nominal))| {
                let mut jitter = 0;
                if k == 0 {
                    jitter += rng.jitter(spec.head_tail_jitter);
                }
                if k == last {
                    jitter += rng.jitter(spec.head_tail_jitter);
                }
                if kind == SegmentKind::Corner {
                    jitter += rng.jitter(spec.corner_jitter);
                }
                (i64::from(nominal) + jitter).max(1) as u64
            })
            .collect()
    }

    /// 种子化伪随机数（SplitMix64）：不依赖外部 crate，同一种子跨平台、跨版本结果一致。
    struct SplitMix64(u64);

    impl SplitMix64 {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// 均匀取 −n..=n
        fn jitter(&mut self, n: i32) -> i64 {
            let n = u64::from(n.unsigned_abs());
            (self.next() % (2 * n + 1)) as i64 - n as i64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{new_segment, KindDefaults};
    use SegmentKind::{Corner as C, Straight as S};

    fn plan(kinds: &[SegmentKind]) -> Vec<PathSegment> {
        let defaults = KindDefaults::default();
        kinds.iter().enumerate().map(|(i, &kind)| new_segment(i as u32 + 1, kind, &defaults)).collect()
    }

    fn region(kind: RegionKind, first_seq: u64, last_seq: u64) -> Region {
        Region { kind, first_seq, last_seq }
    }

    fn calibrated(kinds: &[SegmentKind], n_exp: &[u32]) -> Vec<PathSegment> {
        let mut segments = plan(kinds);
        segments
            .iter_mut()
            .zip(n_exp)
            .for_each(|(segment, &n)| segment.expected_frames = Some(n));
        segments
    }

    fn proportional(segments: &[PathSegment], total: u64) -> SegOutcome {
        let groups: Vec<FrameGroup> = (1..=total)
            .map(|seq| FrameGroup {
                seq,
                present: vec![true; 3],
                incomplete: false,
            })
            .collect();
        Proportional.segment(&SegInput {
            groups: &groups,
            segments,
        })
    }

    fn cuts(outcome: &SegOutcome) -> Vec<(u64, f32)> {
        match outcome {
            SegOutcome::Boundaries(boundaries) => boundaries
                .iter()
                .map(|b| (b.frame_seq, b.confidence))
                .collect(),
            SegOutcome::Unsegmented => panic!("应给出边界"),
        }
    }

    #[test]
    fn coverage_short_detection_between_skips_reports_zero_until_guard_cleared() {
        let mut segments = calibrated(&[S, S, S], &[4, 2, 4]);
        segments[0].skip = true;
        segments[2].skip = true;
        segments[1].cameras = vec!["camA".into()];
        let cameras = vec!["camA".into(), "camB".into(), "camC".into()];
        for (guard, expected) in [(1, 0), (0, 2)] {
            let coverage =
                proportional_coverage(&segments, &cameras, 10, guard, false, &[], PassRules::NONE);
            assert!(coverage.boundaries_trusted);
            assert_eq!(coverage.expected_camera_frames, Some(expected));
            assert_eq!(coverage.segments[1].expected_camera_frames, Some(expected));
            assert_eq!(
                coverage.zero_coverage_segments,
                if expected == 0 { vec![2] } else { vec![] }
            );
            assert_eq!(
                coverage.segments[1].cameras[0].ranges,
                if expected == 0 {
                    vec![]
                } else {
                    vec![CoverageRange {
                        first_seq: 5,
                        last_seq: 6,
                    }]
                }
            );
        }
    }

    #[test]
    fn coverage_untrusted_boundaries_leave_frame_counts_unknown() {
        let mut missing = calibrated(&[S, C], &[4, 4]);
        missing[1].expected_frames = None;
        let cameras = vec!["camA".into(), "camB".into(), "camC".into()];
        for (segments, total) in [
            (missing, 8),
            (calibrated(&[S], &[8]), 8),
            (calibrated(&[S, C], &[4, 4]), 16),
        ] {
            let coverage =
                proportional_coverage(&segments, &cameras, total, 1, false, &[], PassRules::NONE);
            assert!(!coverage.boundaries_trusted);
            assert_eq!(coverage.expected_camera_frames, None);
            assert!(coverage.zero_coverage_segments.is_empty());
            for segment in coverage.segments {
                assert!(segment.range.is_none());
                assert_eq!(segment.expected_camera_frames, None);
                assert!(segment
                    .cameras
                    .iter()
                    .all(|camera| camera.ranges.is_empty()));
                assert_eq!(segment.offset.expected_frames, 0);
            }
        }
    }

    #[test]
    fn coverage_exclusions_and_skipped_ends_preserve_production_counts_and_reduce_offset_availability(
    ) {
        let mut segments = calibrated(&[S, C], &[8, 4]);
        let cameras: Vec<String> = vec!["camA".into(), "camB".into(), "camC".into()];
        segments[0].cameras = vec![cameras[0].clone()];
        segments[0].skip_head = 1;
        segments[0].skip_tail = 1;
        segments[0].offset_ref = Some(crate::plan::OffsetRef {
            measure: crate::plan::OffsetMeasure::AbsoluteX,
            camera: cameras[0].clone(),
            x_ref: Some(1.0),
            spread: None,
            edge: None,
        });
        let clear = proportional_coverage(&segments, &cameras, 12, 1, false, &[], PassRules::NONE);
        let masked = proportional_coverage(
            &segments,
            &cameras,
            12,
            1,
            false,
            &[3, 4, 5, 6],
            PassRules::NONE,
        );
        assert_eq!(masked.expected_camera_frames, clear.expected_camera_frames);
        assert_eq!(
            masked.available_camera_frames,
            clear.expected_camera_frames.map(|n| n - 4)
        );
        let offset = &masked.segments[0].offset;
        assert_eq!(
            offset.ranges,
            vec![CoverageRange {
                first_seq: 3,
                last_seq: 6
            }]
        );
        assert_eq!(offset.expected_frames, 4);
        assert_eq!(offset.available_frames, 0);
        assert_eq!(
            offset.unjudged_reasons,
            vec![OffsetCoverageReason::ExcludedGroups]
        );
    }

    /// D-SB-40：本件帧组数超出 ΣN_exp ± Σtol 时比例边界不可信，整件合并为一个“分段不确定”区域，不回退计划边界。
    #[test]
    fn proportional_boundaries_outside_total_tolerance_merge_into_low_confidence_region() {
        let segments = calibrated(&[S, C, S], &[10, 4, 10]);
        for total in [17, 31] {
            let outcome = proportional(&segments, total);
            assert!(
                cuts(&outcome)
                    .iter()
                    .all(|&(_, confidence)| confidence == 0.0),
                "{total} 帧"
            );
            assert_eq!(
                resolve_regions(&segments, &outcome, total, 0.5, 2, PassRules::NONE),
                vec![region(
                    RegionKind::Unsegmented {
                        first_segment: 1,
                        last_segment: 3,
                        reason: UnsegReason::LowConfidence
                    },
                    1,
                    total
                )],
                "{total} 帧"
            );
        }
    }

    /// 短段使相邻过渡区合并成一个（第 1–4 段边界过渡）时，每帧仍按所在段与最近边界另一侧的段选走廊：
    /// 中间段的检测相机用中间段的走廊，不会都拿首尾两段的走廊去量（合并区原先只看首尾段，中间段的帧全量成断胶）。
    #[test]
    fn merged_transition_measures_each_frame_with_its_own_and_nearest_neighbor_segment() {
        let cameras: Vec<String> = ["cam1", "cam2", "cam3"].map(String::from).to_vec();
        let mut segments = calibrated(&[S, S, S, S], &[4, 2, 2, 4]);
        for (segment, camera) in segments.iter_mut().zip([0, 1, 2, 0]) {
            segment.cameras = vec![cameras[camera].clone()];
        }
        let outcome = proportional(&segments, 12);
        let regions = resolve_regions(&segments, &outcome, 12, 0.5, 2, PassRules::NONE);
        assert!(regions.contains(&region(
            RegionKind::Unsegmented {
                first_segment: 1,
                last_segment: 4,
                reason: UnsegReason::Transition
            },
            3,
            10
        )));
        let choice = frame_corridors(&segments, &cameras, &regions, &outcome, 12);
        let transition: Vec<[Option<u32>; 3]> = (3..=10)
            .map(|seq| [0, 1, 2].map(|c| choice[c][seq - 1]))
            .collect();
        assert_eq!(
            transition,
            vec![
                [Some(1), Some(2), None],
                [Some(1), Some(2), None],
                [Some(1), Some(2), None],
                [None, Some(2), Some(3)],
                [None, Some(2), Some(3)],
                [Some(4), None, Some(3)],
                [Some(4), None, Some(3)],
                [Some(4), None, Some(3)],
            ]
        );
    }

    /// D-SB-53：不检测的段整段划为跳过并向两侧各延伸 guard 帧（邻段贴着边界的帧可能其实属于这一段，照常量会误报），
    /// 这些帧任何相机都不量；邻段其余的帧照常由本段相机量。
    #[test]
    fn skipped_segment_and_its_guard_frames_are_not_measured_by_any_camera() {
        let cameras: Vec<String> = ["cam1", "cam2", "cam3"].map(String::from).to_vec();
        let mut segments = calibrated(&[S, C, S], &[10, 4, 10]);
        for (segment, camera) in segments.iter_mut().zip([0, 1, 2]) {
            segment.cameras = vec![cameras[camera].clone()];
        }
        segments[1].skip = true;
        let outcome = proportional(&segments, 24);
        let regions = resolve_regions(&segments, &outcome, 24, 0.5, 2, PassRules::NONE);
        assert_eq!(
            regions,
            vec![
                region(RegionKind::Straight { segment: 1 }, 1, 8),
                region(
                    RegionKind::Skipped {
                        segment: 2,
                        pass: None,
                        short: false
                    },
                    9,
                    16
                ),
                region(RegionKind::Straight { segment: 3 }, 17, 24),
            ]
        );
        let choice = frame_corridors(&segments, &cameras, &regions, &outcome, 24);
        let column = |seq: usize| [0, 1, 2].map(|c| choice[c][seq - 1]);
        assert!((9..=16).all(|seq| column(seq) == [None; 3]));
        assert_eq!(column(8), [Some(1), None, None]);
        assert_eq!(column(17), [None, None, Some(3)]);
    }

    /// 段首尾跳过的帧划为 Skipped 区域且优先于过渡区：分段可信时按各段首尾定位；未分段时只有整件首尾能定位
    /// （第 1 段的 skip_head、末段的 skip_tail），中间段的跳过不生效。
    #[test]
    fn skipped_frames_carve_regions_at_located_segment_ends() {
        let mut segments = plan(&[S, C, S]);
        segments[0].skip_head = 2;
        segments[1].skip_tail = 1;
        segments[2].skip_tail = 3;
        let skipped = |segment| RegionKind::Skipped {
            segment,
            pass: None,
            short: false,
        };
        let transition = |first_segment, last_segment| RegionKind::Unsegmented {
            first_segment,
            last_segment,
            reason: UnsegReason::Transition,
        };
        let segmented = SegOutcome::Boundaries(vec![
            Boundary {
                after_segment: 1,
                frame_seq: 10,
                confidence: 1.0,
                hard: false,
            },
            Boundary {
                after_segment: 2,
                frame_seq: 16,
                confidence: 1.0,
                hard: false,
            },
        ]);
        let pending = RegionKind::Unsegmented {
            first_segment: 1,
            last_segment: 3,
            reason: UnsegReason::AlgorithmPending,
        };
        for (case, outcome, expected) in [
            (
                "分段可信",
                segmented,
                vec![
                    region(skipped(1), 1, 2),
                    region(RegionKind::Straight { segment: 1 }, 3, 8),
                    region(transition(1, 2), 9, 12),
                    region(RegionKind::Corner { segment: 2 }, 13, 14),
                    region(transition(2, 3), 15, 15),
                    region(skipped(2), 16, 16),
                    region(transition(2, 3), 17, 18),
                    region(RegionKind::Straight { segment: 3 }, 19, 23),
                    region(skipped(3), 24, 26),
                ],
            ),
            (
                "未分段",
                SegOutcome::Unsegmented,
                vec![
                    region(skipped(1), 1, 2),
                    region(pending, 3, 23),
                    region(skipped(3), 24, 26),
                ],
            ),
        ] {
            assert_eq!(
                resolve_regions(&segments, &outcome, 26, 0.5, 2, PassRules::NONE),
                expected,
                "{case}"
            );
        }
    }

    fn by_pass(segments: &[PathSegment], part: &[u32], taught: &[u32], head: u32) -> Vec<Region> {
        let total: u64 = part.iter().map(|&n| u64::from(n)).sum();
        let groups: Vec<FrameGroup> = (1..=total)
            .map(|seq| FrameGroup {
                seq,
                present: vec![true; 3],
                incomplete: false,
            })
            .collect();
        let passes = PassRules {
            part,
            taught: Some(taught),
            skip: crate::passes::PassSkip { head, tail: 0 },
        };
        let outcome = segment_part(
            &SegInput {
                groups: &groups,
                segments,
            },
            &[],
            false,
            passes,
        );
        resolve_regions(segments, &outcome, total, 0.5, 2, passes)
    }

    /// D-SB-62：段边界落在两笔之间时取本件同一笔的末帧，不留过渡区；每笔开头按笔定位跳过，区域记下第几笔。
    #[test]
    fn pass_boundary_follows_the_part_without_transition_and_skips_each_pass_head() {
        let segments = calibrated(&[S, C], &[5, 5]);
        let skipped = |segment, pass| RegionKind::Skipped {
            segment,
            pass: Some(pass),
            short: false,
        };
        assert_eq!(
            by_pass(&segments, &[7, 4], &[5, 5], 1),
            vec![
                region(skipped(1, 1), 1, 1),
                region(RegionKind::Straight { segment: 1 }, 2, 7),
                region(skipped(2, 2), 8, 8),
                region(RegionKind::Corner { segment: 2 }, 9, 11),
            ]
        );
    }

    /// D-SB-62：本件笔数与示教件不同，边界一律不可信，整件按未分段判（起涂照样按本件的笔跳过）。
    #[test]
    fn pass_count_mismatch_leaves_the_part_unsegmented() {
        let segments = calibrated(&[S, C], &[5, 5]);
        let regions = by_pass(&segments, &[10], &[5, 5], 0);
        assert_eq!(
            regions,
            vec![region(
                RegionKind::Unsegmented {
                    first_segment: 1,
                    last_segment: 2,
                    reason: UnsegReason::LowConfidence,
                },
                1,
                10
            )]
        );
    }
}

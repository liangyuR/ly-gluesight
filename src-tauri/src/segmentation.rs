//! 分段（规格 §3）：段结构由人定，PC 在每件收尾后定位段间边界（D-SB-40）。
//!
//! - `Proportional`：按标定的各段 N_exp 比例与本件帧组数推算边界（生产、标定与历史重判都用它）；
//! - `Unsegmented`：整条胶路一个未分段区域（各段 N_exp 未标定或单段配方时比例分段器也退回它）；
//! - 图像运动细化边界（D-SB-43）未实现，合成序列回归入口留给它。

// SegInput 的帧特征、搜索窗口与合成序列回归入口留给图像细化（D-SB-43）使用，现在只有测试读它们。
#![allow(dead_code)]

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::burst_judge::FrameMetrics;
use crate::frame_group::FrameGroup;
use crate::plan::{PathSegment, SegmentKind};

/// 分段输入：三路帧组、每帧特征和计划（段类型序列、N_exp、tol）。
pub struct SegInput<'a> {
    pub groups: &'a [FrameGroup],
    pub features: &'a [FrameMetrics],
    pub segments: &'a [PathSegment],
}

/// 第 `after_segment` 段与下一段之间的边界落在帧组 `frame_seq`：`frame_seq` 是第 `after_segment` 段的最后一帧，
/// 下一段从 `frame_seq + 1` 起（与 [`search_window`] 的“上一边界 + N_exp”同一口径，路径起点视为边界 0）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Boundary {
    pub after_segment: u32,
    pub frame_seq: u64,
    /// 0–1
    pub confidence: f32,
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

/// 总是返回 `SegOutcome::Unsegmented`。
pub struct Unsegmented;

impl Segmenter for Unsegmented {
    fn segment(&self, _input: &SegInput) -> SegOutcome {
        SegOutcome::Unsegmented
    }
}

/// 比例分段（D-SB-40）：本件帧组数 total 已知时，第 k 个边界 = round(total × 前 k 段 N_exp 之和 / ΣN_exp)，
/// 每段至少留 1 帧。total ∈ ΣN_exp ± Σtol 时各边界置信度 1，否则 0（由 [`resolve_regions`] 整体合并为置信度不足）。
/// 单段配方没有边界可定位，任一段 N_exp 未标定时没有比例可用，都返回 Unsegmented（与没有分段算法时相同）。
pub struct Proportional;

impl Segmenter for Proportional {
    fn segment(&self, input: &SegInput) -> SegOutcome {
        let segments = input.segments;
        let Some(counts) = segments
            .iter()
            .map(|s| s.expected_frames.map(u64::from))
            .collect::<Option<Vec<u64>>>()
        else {
            return SegOutcome::Unsegmented;
        };
        let expected: u64 = counts.iter().sum();
        let total = input.groups.iter().map(|g| g.seq).max().unwrap_or(0);
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
                }
            })
            .collect();
        SegOutcome::Boundaries(boundaries)
    }
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
    /// 段首尾跳过的帧（`skip_head` / `skip_tail`）：不判定，结果里列为未检区域
    Skipped {
        segment: u32,
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

/// 把分段结果解析成区域：Unsegmented 时一个覆盖 1..=total_seq 的区域（AlgorithmPending）；
/// 置信度低于 min_conf 的边界把两侧两段合并为 Unsegmented(LowConfidence)，连续多个低置信度边界一并合并，不回退计划边界。
/// 每个可信边界两侧各 `guard` 帧（边界帧 b 时为 b − guard + 1..=b + guard，截在 1..=total_seq 内）划为过渡区
/// Unsegmented(Transition)，覆盖边界两侧的段；相邻的过渡区（中间的段被取空）合并成一个。guard = 0 时没有过渡区。
/// 各段的跳过帧（`skip_head` / `skip_tail`）最后划为 Skipped 区域，优先于过渡区：能定位到的段（单段区域）按本段首尾跳过；
/// 合并区域里只有贴着整件首尾的那一头能定位，只跳过第 1 段的 skip_head 与末段的 skip_tail。
///
/// 算法缺给某个段间边界，或给出的帧号不在上一可信边界之后、不给下一段留帧，都按置信度不足处理。
/// 区域首尾相接、覆盖 1..=total_seq。没有帧组（total_seq = 0）或没有段时返回空。
pub fn resolve_regions(
    segments: &[PathSegment],
    outcome: &SegOutcome,
    total_seq: u64,
    min_conf: f32,
    guard: u32,
) -> Vec<Region> {
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

    // 逐帧标注：所属区域、过渡区覆盖的段下标范围（多个边界的过渡区重叠时取并），或跳过的段
    #[derive(Clone, Copy, PartialEq)]
    enum Label {
        Core(usize),
        Transition(usize, usize),
        Skipped(usize),
    }
    let mut labels: Vec<Label> = Vec::with_capacity(total_seq as usize);
    for (i, (region, _, _)) in regions.iter().enumerate() {
        labels.extend((region.first_seq..=region.last_seq).map(|_| Label::Core(i)));
    }
    if guard > 0 {
        for i in 1..regions.len() {
            let (cut, left, right) = (regions[i - 1].0.last_seq, regions[i - 1].2, regions[i].1);
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
            (Some((Label::Transition(a, b), region)), Label::Transition(c, d)) => {
                (*a, *b) = ((*a).min(c), (*b).max(d));
                region.last_seq = seq;
            }
            _ => {
                let kind = match label {
                    Label::Core(i) => regions[i].0.kind.clone(),
                    Label::Skipped(k) => RegionKind::Skipped {
                        segment: segments[k].index,
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

/// 边界搜索窗口：(prev + n_exp) ± (tol + 1)，下限不低于 prev + 1。
pub fn search_window(prev: u64, n_exp: u32, tol: u32) -> RangeInclusive<u64> {
    let expected = prev + u64::from(n_exp);
    let half = u64::from(tol) + 1;
    expected.saturating_sub(half).max(prev + 1)..=expected + half
}

/// 合成序列回归入口（规格 §3/§6）：按种子生成带真值边界的帧组序列，评估任一 Segmenter 的边界误差。
pub mod synthetic {
    use super::{Boundary, SegInput, SegOutcome, Segmenter};
    use crate::burst_judge::{FrameMetrics, FrameState};
    use crate::frame_group::FrameGroup;
    use crate::plan::{new_segment, KindDefaults, PathSegment, SegmentKind};

    /// 合成序列固定三台相机（与段计划一致）。
    pub const CAMERAS: usize = 3;

    /// 合成序列的剖面。
    #[derive(Clone, Debug, PartialEq)]
    pub struct SequenceSpec {
        /// 各段 (类型, 名义帧数)
        pub segments: Vec<(SegmentKind, u32)>,
        /// 起头 / 收尾帧数波动 ±n
        pub head_tail_jitter: i32,
        /// 弯道减速帧数波动 ±n
        pub corner_jitter: i32,
        /// 缺帧：(相机下标, seq)
        pub missing: Vec<(usize, u64)>,
    }

    #[derive(Clone, Debug)]
    pub struct SyntheticSequence {
        /// 由 spec 生成的计划（N_exp 取名义帧数），evaluate 用它组 SegInput
        pub segments: Vec<PathSegment>,
        pub groups: Vec<FrameGroup>,
        pub features: Vec<FrameMetrics>,
        /// 真值边界（confidence = 1）
        pub truth: Vec<Boundary>,
    }

    /// 一个真值边界的评估结果。
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BoundaryError {
        pub after_segment: u32,
        pub truth_seq: u64,
        /// None = 算法没给出这个边界
        pub found_seq: Option<u64>,
        /// |found − truth|；未找到为 None
        pub error: Option<u64>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RegressionReport {
        pub boundaries: Vec<BoundaryError>,
    }

    /// 各段实际帧数 = 名义帧数 + 波动（至少 1 帧）：首段加起头波动、末段加收尾波动（各 ±head_tail_jitter，
    /// 单段时两者都加），弯道段加减速波动（±corner_jitter）。同一 spec 与 seed 结果确定。
    ///
    /// 离线虚拟产线（`offline::line::PulseProfile`）按同一剖面与种子发脉冲串，所以离线跑出的帧序列与
    /// [`generate`] 的真值边界一致，可以直接拿来做分段回归。
    pub fn frame_counts(spec: &SequenceSpec, seed: u64) -> Vec<u64> {
        realize(spec, &mut SplitMix64(seed))
    }

    /// 真值边界：各段最后一帧（[`Boundary`] 的口径），最后一段之后没有边界。
    pub fn truth(counts: &[u64]) -> Vec<Boundary> {
        let mut end = 0;
        counts
            .iter()
            .enumerate()
            .take(counts.len().saturating_sub(1))
            .map(|(k, count)| {
                end += count;
                Boundary { after_segment: k as u32 + 1, frame_seq: end, confidence: 1.0 }
            })
            .collect()
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

    /// 同一 spec 与 seed 生成确定的序列。
    ///
    /// 各段实际帧数见 [`frame_counts`]，真值边界见 [`truth`]。
    /// 每帧特征：直线段胶中心 x 贴近走廊中线、有胶比例 1；弯道段 x 沿弯道走出走廊（正弦拱形，峰值 1.5 mm）、
    /// 胶宽略宽、有胶比例下降。三台相机各有固定的 x 偏置。`missing` 中的 (相机, seq) 记 Missing，所在组标 incomplete。
    /// features 按 seq、相机顺序排列，每个 (相机, seq) 一条。
    pub fn generate(spec: &SequenceSpec, seed: u64) -> SyntheticSequence {
        let mut rng = SplitMix64(seed);
        let counts = realize(spec, &mut rng);

        let defaults = KindDefaults::default();
        let segments = spec
            .segments
            .iter()
            .enumerate()
            .map(|(k, &(kind, nominal))| PathSegment { expected_frames: Some(nominal), ..new_segment(k as u32 + 1, kind, &defaults) })
            .collect();

        let truth = truth(&counts);

        let mut groups = Vec::new();
        let mut features = Vec::new();
        let mut seq = 0;
        for (&(kind, _), &count) in spec.segments.iter().zip(&counts) {
            for position in 0..count {
                seq += 1;
                let present: Vec<bool> = (0..CAMERAS).map(|camera| !spec.missing.contains(&(camera, seq))).collect();
                // 段内位置 0–1，弯道 x 呈拱形
                let along = (position as f64 + 0.5) / count as f64;
                for (camera, &here) in present.iter().enumerate() {
                    features.push(if here {
                        let (x, width, present_ratio) = match kind {
                            SegmentKind::Straight => (0.0, 4.0, 1.0),
                            SegmentKind::Corner => (1.5 * (std::f64::consts::PI * along).sin(), 4.3, 0.85),
                        };
                        let width = width + rng.noise(0.1);
                        FrameMetrics {
                            camera,
                            seq,
                            state: FrameState::Measured,
                            error: None,
                            width_min: Some(width - 0.2),
                            width_max: Some(width + 0.2),
                            width_mean: Some(width),
                            widths: Vec::new(),
                            spacing_mm: 0.0,
                            gap_max_mm: 0.0,
                            present_ratio: present_ratio + rng.noise(0.02).min(0.0),
                            x_mm: Some(x + 0.2 * camera as f64 + rng.noise(0.05)),
                        }
                    } else {
                        FrameMetrics {
                            camera,
                            seq,
                            state: FrameState::Missing,
                            error: None,
                            width_min: None,
                            width_max: None,
                            width_mean: None,
                            widths: Vec::new(),
                            spacing_mm: 0.0,
                            gap_max_mm: 0.0,
                            present_ratio: 0.0,
                            x_mm: None,
                        }
                    });
                }
                let incomplete = present.contains(&false);
                groups.push(FrameGroup { seq, present, incomplete });
            }
        }
        SyntheticSequence { segments, groups, features, truth }
    }

    /// 对每个真值边界给出算法找到的帧号与 |找到 − 真值|；算法没给出（含整体返回 Unsegmented）的标为未找到。
    /// 不按置信度过滤：置信度的取舍归 [`super::resolve_regions`]。
    pub fn evaluate(segmenter: &dyn Segmenter, sequence: &SyntheticSequence) -> RegressionReport {
        let input = SegInput { groups: &sequence.groups, features: &sequence.features, segments: &sequence.segments };
        let found = match segmenter.segment(&input) {
            SegOutcome::Unsegmented => Vec::new(),
            SegOutcome::Boundaries(boundaries) => boundaries,
        };
        let boundaries = sequence
            .truth
            .iter()
            .map(|truth| {
                let found_seq = found.iter().find(|b| b.after_segment == truth.after_segment).map(|b| b.frame_seq);
                BoundaryError {
                    after_segment: truth.after_segment,
                    truth_seq: truth.frame_seq,
                    found_seq,
                    error: found_seq.map(|seq| seq.abs_diff(truth.frame_seq)),
                }
            })
            .collect();
        RegressionReport { boundaries }
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

        /// 均匀取 [−amplitude, amplitude)
        fn noise(&mut self, amplitude: f64) -> f64 {
            let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
            (unit * 2.0 - 1.0) * amplitude
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

    /// 规格 §3：算法验证前整条胶路按未分段——一个覆盖全部帧组的区域，原因“分段算法未就绪”。
    #[test]
    fn unsegmented_returns_one_pending_region_covering_all_groups() {
        let segments = plan(&[S, C, S]);
        let outcome = Unsegmented.segment(&SegInput { groups: &[], features: &[], segments: &segments });
        assert_eq!(outcome, SegOutcome::Unsegmented);
        assert_eq!(
            resolve_regions(&segments, &outcome, 23, 0.8, 2),
            vec![region(RegionKind::Unsegmented { first_segment: 1, last_segment: 3, reason: UnsegReason::AlgorithmPending }, 1, 23)]
        );
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
            features: &[],
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

    /// D-SB-40：第 k 个边界 = round(本件帧组数 × 前 k 段 N_exp 之和 / ΣN_exp)。N_exp 10 / 4 / 10（ΣN 24 ± 3），
    /// 本件帧数在容差内时边界可信；按原帧号会把 26 帧件的第二个边界放在 14，比例换算放在 15。
    #[test]
    fn proportional_boundaries_scale_calibrated_counts_to_actual_total() {
        let segments = calibrated(&[S, C, S], &[10, 4, 10]);
        for (total, expected) in [
            (24, vec![(10, 1.0), (14, 1.0)]),
            (26, vec![(11, 1.0), (15, 1.0)]),
            (21, vec![(9, 1.0), (12, 1.0)]),
        ] {
            assert_eq!(
                cuts(&proportional(&segments, total)),
                expected,
                "{total} 帧"
            );
        }
    }

    /// D-SB-40：本件帧组数超出 ΣN_exp ± Σtol 时比例边界不可信，整件合并为一个“分段不确定”区域，不回退计划边界。
    #[test]
    fn proportional_boundaries_outside_total_tolerance_merge_into_low_confidence_region() {
        let segments = calibrated(&[S, C, S], &[10, 4, 10]);
        for total in [20, 28] {
            let outcome = proportional(&segments, total);
            assert!(
                cuts(&outcome)
                    .iter()
                    .all(|&(_, confidence)| confidence == 0.0),
                "{total} 帧"
            );
            assert_eq!(
                resolve_regions(&segments, &outcome, total, 0.5, 2),
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

    /// D-SB-40：有段 N_exp 未标定（没有比例）或单段配方（没有边界）时与没有分段算法相同，整条胶路未分段。
    #[test]
    fn proportional_stays_unsegmented_without_calibrated_counts_or_with_single_segment() {
        let mut partly = calibrated(&[S, C, S], &[10, 4, 10]);
        partly[1].expected_frames = None;
        for (case, segments) in [
            ("第 2 段待标定", partly),
            ("单段配方", calibrated(&[S], &[24])),
        ] {
            assert_eq!(
                proportional(&segments, 24),
                SegOutcome::Unsegmented,
                "{case}"
            );
        }
    }

    /// D-SB-40：可信边界两侧各 guard 帧划为过渡区（覆盖两侧的段，只判每帧指标）；中间的段被取空时两个过渡区合并；
    /// guard = 0 时按边界原样切分。
    #[test]
    fn trusted_boundaries_get_guard_frames_as_transition_regions() {
        let straight = |segment| RegionKind::Straight { segment };
        let corner = |segment| RegionKind::Corner { segment };
        let transition = |first_segment, last_segment| RegionKind::Unsegmented {
            first_segment,
            last_segment,
            reason: UnsegReason::Transition,
        };
        let outcome = |a, b| {
            SegOutcome::Boundaries(vec![
                Boundary {
                    after_segment: 1,
                    frame_seq: a,
                    confidence: 1.0,
                },
                Boundary {
                    after_segment: 2,
                    frame_seq: b,
                    confidence: 1.0,
                },
            ])
        };
        let segments = plan(&[S, C, S]);
        for (case, cut, guard, expected) in [
            (
                "弯道 6 帧、余量 2",
                (10, 16),
                2,
                vec![
                    region(straight(1), 1, 8),
                    region(transition(1, 2), 9, 12),
                    region(corner(2), 13, 14),
                    region(transition(2, 3), 15, 18),
                    region(straight(3), 19, 26),
                ],
            ),
            (
                "弯道 4 帧被两侧余量取空",
                (10, 14),
                2,
                vec![
                    region(straight(1), 1, 8),
                    region(transition(1, 3), 9, 16),
                    region(straight(3), 17, 26),
                ],
            ),
            (
                "余量 0",
                (10, 16),
                0,
                vec![
                    region(straight(1), 1, 10),
                    region(corner(2), 11, 16),
                    region(straight(3), 17, 26),
                ],
            ),
        ] {
            assert_eq!(
                resolve_regions(&segments, &outcome(cut.0, cut.1), 26, 0.5, guard),
                expected,
                "{case}"
            );
        }
    }

    /// 段首尾跳过的帧划为 Skipped 区域且优先于过渡区：分段可信时按各段首尾定位；未分段时只有整件首尾能定位
    /// （第 1 段的 skip_head、末段的 skip_tail），中间段的跳过不生效。
    #[test]
    fn skipped_frames_carve_regions_at_located_segment_ends() {
        let mut segments = plan(&[S, C, S]);
        segments[0].skip_head = 2;
        segments[1].skip_tail = 1;
        segments[2].skip_tail = 3;
        let skipped = |segment| RegionKind::Skipped { segment };
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
            },
            Boundary {
                after_segment: 2,
                frame_seq: 16,
                confidence: 1.0,
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
                resolve_regions(&segments, &outcome, 26, 0.5, 2),
                expected,
                "{case}"
            );
        }
    }
}

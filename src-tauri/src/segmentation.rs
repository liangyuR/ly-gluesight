//! 分段（规格 §3）：PC 图像算法在计划预期边界附近划分直线段 / 弯道段。
//!
//! 本轮没有真实弯道图像，只交付接口、`Unsegmented`（整条胶路一个未分段区域）和合成序列回归入口；
//! 算法验证前整条胶路按未分段处理：只跑每帧独立检测，不判胶偏。

// C3/D2 接入调用方后删掉 dead_code。
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

/// 本轮唯一实现：总是返回 `SegOutcome::Unsegmented`。
pub struct Unsegmented;

impl Segmenter for Unsegmented {
    fn segment(&self, _input: &SegInput) -> SegOutcome {
        SegOutcome::Unsegmented
    }
}

/// 未分段的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnsegReason {
    /// 分段算法未就绪（本轮恒为此）
    AlgorithmPending,
    /// 边界置信度不足：两侧两段合并，不回退计划边界
    LowConfidence,
}

/// 区域类型。JSON 里用 `type` 区分，避免与 `Region.kind` 嵌套同名。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RegionKind {
    Straight { segment: u32 },
    Corner { segment: u32 },
    /// 覆盖 first_segment..=last_segment；只跑每帧独立检测，允差取覆盖各段中较宽一档，不判胶偏
    Unsegmented { first_segment: u32, last_segment: u32, reason: UnsegReason },
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
/// 有边界时边界两侧各一帧归弯道；置信度低于 min_conf 的边界把两侧两段合并为 Unsegmented(LowConfidence)，
/// 连续多个低置信度边界一并合并，不回退计划边界。
///
/// 算法缺给某个段间边界，或给出的帧号不在上一可信边界之后、不给下一段留帧，都按置信度不足处理。
/// “两侧各一帧归弯道”只从解析为直线区域的一侧取帧（弯道侧是弯道或未分段区域都行），直线区域被取空时去掉；
/// 区域首尾相接、覆盖 1..=total_seq。没有帧组（total_seq = 0）或没有段时返回空。
pub fn resolve_regions(segments: &[PathSegment], outcome: &SegOutcome, total_seq: u64, min_conf: f32) -> Vec<Region> {
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else { return Vec::new() };
    if total_seq == 0 {
        return Vec::new();
    }
    let boundaries = match outcome {
        SegOutcome::Unsegmented => {
            let kind = RegionKind::Unsegmented { first_segment: first.index, last_segment: last.index, reason: UnsegReason::AlgorithmPending };
            return vec![Region { kind, first_seq: 1, last_seq: total_seq }];
        }
        SegOutcome::Boundaries(boundaries) => boundaries,
    };

    // 每个段间隙的可信边界帧号（第 k 段之后）；None = 置信度不足，两侧合并
    let mut previous = 0;
    let cuts: Vec<Option<u64>> = segments
        .windows(2)
        .map(|pair| {
            let cut = boundaries
                .iter()
                .find(|b| b.after_segment == pair[0].index)
                .filter(|b| b.confidence >= min_conf && b.frame_seq > previous && b.frame_seq < total_seq)
                .map(|b| b.frame_seq);
            previous = cut.unwrap_or(previous);
            cut
        })
        .collect();

    // 按可信边界切成区域，记下每个区域覆盖的段下标范围
    let mut regions: Vec<(Region, usize, usize)> = Vec::new();
    let (mut start, mut first_seq) = (0, 1);
    for k in 0..segments.len() {
        let Some(last_seq) = cuts.get(k).copied().unwrap_or(Some(total_seq)) else { continue };
        let kind = match (start == k, segments[k].kind) {
            (true, SegmentKind::Straight) => RegionKind::Straight { segment: segments[k].index },
            (true, SegmentKind::Corner) => RegionKind::Corner { segment: segments[k].index },
            (false, _) => RegionKind::Unsegmented {
                first_segment: segments[start].index,
                last_segment: segments[k].index,
                reason: UnsegReason::LowConfidence,
            },
        };
        regions.push((Region { kind, first_seq, last_seq }, start, k));
        (start, first_seq) = (k + 1, last_seq + 1);
    }

    // 边界两侧各一帧归弯道：直线区域贴着弯道段的那一帧让给对侧区域
    for i in 1..regions.len() {
        let (left, right) = regions.split_at_mut(i);
        let ((left, _, left_last), (right, right_first, _)) = (&mut left[i - 1], &mut right[0]);
        let straight = |region: &Region| matches!(region.kind, RegionKind::Straight { .. }) && region.frame_count() > 0;
        match (segments[*left_last].kind, segments[*right_first].kind) {
            (SegmentKind::Straight, SegmentKind::Corner) if straight(left) => {
                left.last_seq -= 1;
                right.first_seq -= 1;
            }
            (SegmentKind::Corner, SegmentKind::Straight) if straight(right) => {
                left.last_seq += 1;
                right.first_seq += 1;
            }
            _ => {}
        }
    }
    regions.into_iter().map(|(region, _, _)| region).filter(|region| region.frame_count() > 0).collect()
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

    /// 同一 spec 与 seed 生成确定的序列。
    ///
    /// 各段实际帧数 = 名义帧数 + 波动（至少 1 帧）：首段加起头波动、末段加收尾波动（各 ±head_tail_jitter，
    /// 单段时两者都加），弯道段加减速波动（±corner_jitter）。真值边界是各段最后一帧（[`Boundary`] 的口径）。
    /// 每帧特征：直线段胶中心 x 贴近走廊中线、有胶比例 1；弯道段 x 沿弯道走出走廊（正弦拱形，峰值 1.5 mm）、
    /// 胶宽略宽、有胶比例下降。三台相机各有固定的 x 偏置。`missing` 中的 (相机, seq) 记 Missing，所在组标 incomplete。
    /// features 按 seq、相机顺序排列，每个 (相机, seq) 一条。
    pub fn generate(spec: &SequenceSpec, seed: u64) -> SyntheticSequence {
        let mut rng = SplitMix64(seed);
        let last = spec.segments.len().saturating_sub(1);
        let counts: Vec<u64> = spec
            .segments
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
            .collect();

        let defaults = KindDefaults::default();
        let segments = spec
            .segments
            .iter()
            .enumerate()
            .map(|(k, &(kind, nominal))| PathSegment { expected_frames: Some(nominal), ..new_segment(k as u32 + 1, kind, &defaults) })
            .collect();

        let mut truth = Vec::new();
        let mut end = 0;
        for (k, count) in counts.iter().enumerate() {
            end += count;
            if k < last {
                truth.push(Boundary { after_segment: k as u32 + 1, frame_seq: end, confidence: 1.0 });
            }
        }

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
    use super::synthetic::{evaluate, generate, SequenceSpec};
    use super::*;
    use crate::plan::{new_segment, KindDefaults};
    use RegionKind::{Corner, Straight};
    use SegmentKind::{Corner as C, Straight as S};

    fn plan(kinds: &[SegmentKind]) -> Vec<PathSegment> {
        let defaults = KindDefaults::default();
        kinds.iter().enumerate().map(|(i, &kind)| new_segment(i as u32 + 1, kind, &defaults)).collect()
    }

    fn cut(after_segment: u32, frame_seq: u64, confidence: f32) -> Boundary {
        Boundary { after_segment, frame_seq, confidence }
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
            resolve_regions(&segments, &outcome, 23, 0.8),
            vec![region(RegionKind::Unsegmented { first_segment: 1, last_segment: 3, reason: UnsegReason::AlgorithmPending }, 1, 23)]
        );
    }

    /// 规格 §3：边界两侧各一帧归弯道；直线段只剩一帧时整段让给弯道，区域仍首尾相接。
    #[test]
    fn confident_boundary_gives_one_frame_each_side_to_corner() {
        for (case, kinds, boundaries, total, expected) in [
            (
                "直线-弯道-直线",
                vec![S, C, S],
                vec![cut(1, 10, 0.9), cut(2, 14, 0.9)],
                22,
                vec![region(Straight { segment: 1 }, 1, 9), region(Corner { segment: 2 }, 10, 15), region(Straight { segment: 3 }, 16, 22)],
            ),
            (
                "两弯道夹一帧直线",
                vec![C, S, C],
                vec![cut(1, 4, 0.9), cut(2, 5, 0.9)],
                9,
                vec![region(Corner { segment: 1 }, 1, 5), region(Corner { segment: 3 }, 6, 9)],
            ),
        ] {
            assert_eq!(resolve_regions(&plan(&kinds), &SegOutcome::Boundaries(boundaries), total, 0.8), expected, "{case}");
        }
    }

    /// 规格 §3：置信度不足（或算法缺给边界）时不回退计划边界，把两侧两段合并为未分段；连续多个一并合并。
    /// 合并区域与可信直线段之间仍按“两侧各一帧归弯道”从直线侧取帧。
    #[test]
    fn low_confidence_boundary_merges_both_sides_into_unsegmented_without_plan_fallback() {
        let segments = plan(&[S, C, S, C, S]);
        let merged = RegionKind::Unsegmented { first_segment: 2, last_segment: 4, reason: UnsegReason::LowConfidence };
        let expected = vec![region(Straight { segment: 1 }, 1, 10), region(merged, 11, 28), region(Straight { segment: 5 }, 29, 33)];
        for (case, boundaries) in [
            ("第 2、3 个边界置信度不足", vec![cut(1, 11, 0.9), cut(2, 15, 0.3), cut(3, 23, 0.2), cut(4, 27, 0.95)]),
            ("算法缺给第 2、3 个边界", vec![cut(1, 11, 0.9), cut(4, 27, 0.95)]),
        ] {
            assert_eq!(resolve_regions(&segments, &SegOutcome::Boundaries(boundaries), 33, 0.8), expected, "{case}");
        }
    }

    /// 规格 §3：以上一边界为起点加本段 N_exp 得预期帧号，窗口 ±(tol + 1)；下限不早于上一边界之后一帧。
    #[test]
    fn search_window_is_previous_plus_n_exp_within_tol_plus_one() {
        for (prev, n_exp, tol, window) in [(0, 10, 1, 8..=12), (20, 4, 2, 21..=27), (20, 2, 1, 21..=24), (0, 1, 1, 1..=3)] {
            assert_eq!(search_window(prev, n_exp, tol), window, "prev {prev} + {n_exp} ± ({tol} + 1)");
        }
    }

    /// 测试用分段器：原样返回给定边界。
    struct Fixed(Vec<Boundary>);

    impl Segmenter for Fixed {
        fn segment(&self, _input: &SegInput) -> SegOutcome {
            SegOutcome::Boundaries(self.0.clone())
        }
    }

    fn spec() -> SequenceSpec {
        SequenceSpec { segments: vec![(S, 12), (C, 4), (S, 9), (C, 5), (S, 10)], head_tail_jitter: 1, corner_jitter: 2, missing: vec![(1, 7)] }
    }

    /// 规格 §3/§6 合成序列回归入口：逐个真值边界报告误差，算法没给出的列为未找到。
    #[test]
    fn segmenter_off_by_k_frames_reports_boundary_error_k_and_missing_boundaries_listed() {
        let sequence = generate(&spec(), 7);
        let shifted = sequence.truth.iter().map(|b| Boundary { frame_seq: b.frame_seq + 2, ..b.clone() }).collect();
        let partial = sequence.truth.iter().filter(|b| b.after_segment != 2).cloned().collect();
        let cases: [(&str, &dyn Segmenter, [Option<u64>; 4]); 4] = [
            ("真值", &Fixed(sequence.truth.clone()), [Some(0); 4]),
            ("整体偏后 2 帧", &Fixed(shifted), [Some(2); 4]),
            ("缺第 2 个边界", &Fixed(partial), [Some(0), None, Some(0), Some(0)]),
            ("未分段", &Unsegmented, [None; 4]),
        ];
        for (case, segmenter, errors) in cases {
            let report = evaluate(segmenter, &sequence);
            assert_eq!(report.boundaries.iter().map(|b| b.after_segment).collect::<Vec<_>>(), vec![1, 2, 3, 4], "{case}");
            assert_eq!(report.boundaries.iter().map(|b| b.error).collect::<Vec<_>>(), errors, "{case}");
        }
    }

    /// 规格 §6 合成序列：同一 spec 与种子结果确定；各段帧数在名义值 ± 波动内且确有波动；真值边界与帧组一致；指定缺帧标出。
    #[test]
    fn same_spec_and_seed_give_identical_sequence_within_jitter_with_missing_frames_marked() {
        let spec = spec();
        let sequence = generate(&spec, 7);
        let again = generate(&spec, 7);
        assert_eq!((&sequence.groups, &sequence.features, &sequence.truth), (&again.groups, &again.features, &again.truth), "同一种子");

        let counts = |truth: &[Boundary], total: u64| {
            let ends: Vec<u64> = truth.iter().map(|b| b.frame_seq).chain([total]).collect();
            ends.iter().scan(0, |start, &end| Some(end - std::mem::replace(start, end))).collect::<Vec<u64>>()
        };
        // 首尾 ±1；弯道 ±2；首段不是弯道，末段不是弯道
        let bounds = [(11, 13), (2, 6), (9, 9), (3, 7), (9, 11)];
        let mut seen_first = std::collections::BTreeSet::new();
        for seed in 0..32 {
            let s = generate(&spec, seed);
            let total = s.groups.len() as u64;
            assert_eq!(s.groups.iter().map(|g| g.seq).collect::<Vec<_>>(), (1..=total).collect::<Vec<_>>(), "种子 {seed}");
            let counts = counts(&s.truth, total);
            for (k, (&count, &(lo, hi))) in counts.iter().zip(&bounds).enumerate() {
                assert!((lo..=hi).contains(&count), "种子 {seed} 第 {} 段 {count} 帧不在 {lo}..={hi}", k + 1);
            }
            seen_first.insert(counts[0]);
        }
        assert!(seen_first.len() > 1, "起头波动没有生效");

        let incomplete: Vec<u64> = sequence.groups.iter().filter(|g| g.incomplete).map(|g| g.seq).collect();
        assert_eq!(incomplete, vec![7]);
        assert_eq!(sequence.groups[6].present, vec![true, false, true]);
        assert_eq!(sequence.features.len(), sequence.groups.len() * synthetic::CAMERAS);
    }
}

//! 分段（规格 §3）：PC 图像算法在计划预期边界附近划分直线段 / 弯道段。
//!
//! 本轮没有真实弯道图像，只交付接口、`Unsegmented`（整条胶路一个未分段区域）和合成序列回归入口；
//! 算法验证前整条胶路按未分段处理：只跑每帧独立检测，不判胶偏。

// 骨架（WP00）：函数体由 A3 填写，填完删掉 unused_variables；C3/D2 接入调用方后删掉 dead_code。
#![allow(dead_code, unused_variables)]

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::burst_judge::FrameMetrics;
use crate::frame_group::FrameGroup;
use crate::plan::PathSegment;

/// 分段输入：三路帧组、每帧特征和计划（段类型序列、N_exp、tol）。
pub struct SegInput<'a> {
    pub groups: &'a [FrameGroup],
    pub features: &'a [FrameMetrics],
    pub segments: &'a [PathSegment],
}

/// 第 `after_segment` 段与下一段之间的边界落在帧组 `frame_seq`。
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
    fn segment(&self, input: &SegInput) -> SegOutcome {
        unimplemented!()
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
        unimplemented!()
    }
}

/// 把分段结果解析成区域：Unsegmented 时一个覆盖 1..=total_seq 的区域（AlgorithmPending）；
/// 有边界时边界两侧各一帧归弯道；置信度低于 min_conf 的边界把两侧两段合并为 Unsegmented(LowConfidence)，
/// 连续多个低置信度边界一并合并，不回退计划边界。
pub fn resolve_regions(segments: &[PathSegment], outcome: &SegOutcome, total_seq: u64, min_conf: f32) -> Vec<Region> {
    unimplemented!()
}

/// 边界搜索窗口：(prev + n_exp) ± (tol + 1)，下限不低于 prev + 1。
pub fn search_window(prev: u64, n_exp: u32, tol: u32) -> RangeInclusive<u64> {
    unimplemented!()
}

/// 合成序列回归入口（规格 §3/§6）：按种子生成带真值边界的帧组序列，评估任一 Segmenter 的边界误差。
pub mod synthetic {
    use super::{Boundary, Segmenter};
    use crate::burst_judge::FrameMetrics;
    use crate::frame_group::FrameGroup;
    use crate::plan::{PathSegment, SegmentKind};

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
    pub fn generate(spec: &SequenceSpec, seed: u64) -> SyntheticSequence {
        unimplemented!()
    }

    pub fn evaluate(segmenter: &dyn Segmenter, sequence: &SyntheticSequence) -> RegressionReport {
        unimplemented!()
    }
}

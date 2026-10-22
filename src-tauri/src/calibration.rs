//! 多轮标定（规格 §2）：同一工件程序连续采集 ≥ min_rounds 轮，帧组数取均值四舍五入作 N_exp，
//! 记录 min / max / mean，并按 min / max 给容差建议；标准件轮次给出胶偏参考 x_ref。
//!
//! 同一套统计既用于单段（写 `PathSegment.expected_frames` / `calib`），也用于整件
//! （写 `plan::PartCalib`：本轮多段配方各段待分段算法，凭整件帧组数发布）。

// 骨架（WP00）：函数体由 A5 填写，填完删掉 unused_variables；D2 接入调用方后删掉 dead_code。
#![allow(dead_code, unused_variables)]

use serde::{Deserialize, Serialize};

use crate::plan::SegmentCalib;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibOutcome {
    /// 各轮帧组数均值四舍五入（.5 进位）
    pub n_exp: u32,
    pub calib: SegmentCalib,
    /// 建议容差；操作员确认后才写进配方
    pub suggested_tolerance: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibError {
    TooFewRounds { have: u32, need: u32 },
    Empty,
}

/// counts：各轮帧组数。轮数少于 min_rounds 时返回 TooFewRounds。
pub fn calibrate_counts(counts: &[u32], min_rounds: u32, default_tol: u32) -> Result<CalibOutcome, CalibError> {
    unimplemented!()
}

/// max(default, n − min, max − n)。
pub fn suggest_tolerance(n: u32, min: u32, max: u32, default: u32) -> u32 {
    unimplemented!()
}

/// 标准件各轮 x 均值 → (x_ref = 均值, spread = max − min)；空输入为 None。
pub fn x_ref(values: &[f64]) -> Option<(f64, f64)> {
    unimplemented!()
}

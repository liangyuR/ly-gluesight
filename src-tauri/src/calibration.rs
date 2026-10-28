//! 多轮标定（规格 §2）：同一工件程序连续采集 ≥ min_rounds 轮，帧组数取均值四舍五入作 N_exp，
//! 记录 min / max / mean，并按 min / max 给容差建议；标准件轮次给出胶偏参考 x_ref；按每帧是否成形数出起涂、收尾帧组数（D-SB-50）。
//!
//! 同一套统计既用于单段（写 `PathSegment.expected_frames` / `calib`），也用于整件
//! （写 `plan::PartCalib`）；多段配方按已确认的手动分段逐段调用。调用方是候选工作区的多轮标定（workspace/burst.rs）。

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

/// counts：各轮帧组数。轮数少于 min_rounds 时返回 TooFewRounds；min_rounds 为 0 且没有轮次时返回 Empty。
pub fn calibrate_counts(counts: &[u32], min_rounds: u32, default_tol: u32) -> Result<CalibOutcome, CalibError> {
    let rounds = u32::try_from(counts.len()).unwrap_or(u32::MAX);
    if rounds < min_rounds {
        return Err(CalibError::TooFewRounds { have: rounds, need: min_rounds });
    }
    let (Some(&min), Some(&max)) = (counts.iter().min(), counts.iter().max()) else { return Err(CalibError::Empty) };
    let mean = counts.iter().map(|&n| u64::from(n)).sum::<u64>() as f64 / counts.len() as f64;
    // f64::round 对 .5 远离零取整，帧组数非负即 .5 进位
    let n_exp = mean.round() as u32;
    Ok(CalibOutcome {
        n_exp,
        calib: SegmentCalib { rounds, min, max, mean },
        suggested_tolerance: suggest_tolerance(n_exp, min, max, default_tol),
    })
}

/// max(default, n − min, max − n)。
pub fn suggest_tolerance(n: u32, min: u32, max: u32, default: u32) -> u32 {
    default.max(n.saturating_sub(min)).max(max.saturating_sub(n))
}

/// 标准件各轮 x 均值 → (x_ref = 均值, spread = max − min)；空输入为 None。
pub fn x_ref(values: &[f64]) -> Option<(f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some((mean, max - min))
}

/// 起涂、收尾判断里一个帧组的状态（D-SB-50）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupForm {
    /// 本段每台检测相机都测成，且独立判定没有 NG 级发现
    Formed,
    Unformed,
    /// 有相机缺帧、测量失败、未测、走出走廊，或帧组被屏蔽：说明不了胶是否成形
    Unknown,
}

/// 连续这么多个成形帧组才算胶条已成形（D-SB-50，暂定，待实际涂胶轮次校准）。
pub const ONSET_RUN: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnsetError {
    /// 成形之前第 n 个（从 1 数）帧组无法判断
    Unknown(usize),
    /// 没有连续 ONSET_RUN 个成形帧组
    NotFound,
}

/// 起涂帧组数：第一处连续 ONSET_RUN 个成形帧组之前的帧组数；收尾帧组数把序列倒过来传入。
pub fn unformed_lead(groups: impl IntoIterator<Item = GroupForm>) -> Result<u32, OnsetError> {
    let mut run = 0;
    for (i, form) in groups.into_iter().enumerate() {
        match form {
            GroupForm::Formed => {
                run += 1;
                if run == ONSET_RUN {
                    return Ok(u32::try_from(i + 1 - ONSET_RUN).unwrap_or(u32::MAX));
                }
            }
            GroupForm::Unformed => run = 0,
            GroupForm::Unknown => return Err(OnsetError::Unknown(i + 1)),
        }
    }
    Err(OnsetError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规格 §2：N_exp = 各轮帧组数均值四舍五入（.5 进位），同时记录轮数与 min / max / mean。
    #[test]
    fn n_exp_is_rounded_mean_with_min_max_recorded() {
        for (counts, n_exp, min, max, mean) in
            [(vec![10, 11], 11, 10, 11, 10.5), (vec![10, 10, 11], 10, 10, 11, 31.0 / 3.0), (vec![12; 10], 12, 12, 12, 12.0)]
        {
            let outcome = calibrate_counts(&counts, counts.len() as u32, 1).unwrap();
            assert_eq!(
                (outcome.n_exp, outcome.calib),
                (n_exp, SegmentCalib { rounds: counts.len() as u32, min, max, mean }),
                "{counts:?}"
            );
        }
    }

    /// 规格 §2：同一程序至少采集 min_rounds 轮（可配）才能标定；恰好 min_rounds 轮可以。
    #[test]
    fn fewer_rounds_than_required_are_rejected_with_have_and_need() {
        assert_eq!(calibrate_counts(&[20; 9], 10, 1), Err(CalibError::TooFewRounds { have: 9, need: 10 }));
        assert!(calibrate_counts(&[20; 10], 10, 1).is_ok());
    }

    /// 规格 §2：容差建议按 min / max 覆盖各轮的实际波动，且不低于默认容差。
    #[test]
    fn suggested_tolerance_covers_observed_spread_never_below_default() {
        for (n, min, max, default, suggested) in [(10, 10, 10, 1, 1), (10, 8, 11, 1, 2), (10, 9, 13, 1, 3), (10, 9, 11, 2, 2)] {
            assert_eq!(suggest_tolerance(n, min, max, default), suggested, "N {n}，{min}..={max}，默认 {default}");
        }
    }

    /// D-SB-50：起涂帧组数数到第一处连续成形为止——偶发的单个成形帧组（暗带、槽）不算起点，
    /// 成形之前出现无法判断的帧组时这一轮不给值，而不是把它当成未成形。
    #[test]
    fn unformed_lead_counts_until_a_formed_run_and_aborts_on_unknown_before_it() {
        use GroupForm::{Formed as F, Unformed as U, Unknown as K};
        for (groups, expected) in [
            (vec![U, U, U, F, F, F], Ok(3)),
            (vec![F, F, U], Ok(0)),
            (vec![U, F, U, U, F, F], Ok(4)),
            (vec![U, K, F, F], Err(OnsetError::Unknown(2))),
            (vec![U, F, F, K], Ok(1)),
            (vec![U, F, U, F], Err(OnsetError::NotFound)),
        ] {
            assert_eq!(unformed_lead(groups.clone()), expected, "{groups:?}");
        }
    }
}

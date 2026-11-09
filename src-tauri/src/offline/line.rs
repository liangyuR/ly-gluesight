use std::time::Duration;
use crate::plan::{PathSegment, SegmentKind};
use crate::segmentation::synthetic;

/// 段配方离线脉冲的默认频率（Hz）：规格 §0 按视野选 10–25 Hz。
pub const SIM_HZ: f64 = 20.0;
/// 段长换算帧数用的机器人涂胶速度（mm/s），只在段还没有 N_exp 时估算。
pub const SIM_SPEED_MM_S: f64 = 50.0;
/// 没有段长、也没有 N_exp 的段按类型取的名义帧数。
const DEFAULT_STRAIGHT_FRAMES: u32 = 12;
const DEFAULT_CORNER_FRAMES: u32 = 4;

/// 段配方一件的脉冲剖面（规格 §0/§6）：相机使能为高时 PLC 按 `hz` 定频发脉冲串，次数 = 各段名义帧数 +
/// 种子化的首尾 ±`head_tail_jitter` 与弯道 ±`corner_jitter` 波动。
#[derive(Clone, Debug, PartialEq)]
pub struct PulseProfile {
    pub hz: f64,
    /// 各段 (类型, 名义帧数)
    pub segments: Vec<(SegmentKind, u32)>,
    pub head_tail_jitter: i32,
    pub corner_jitter: i32,
    pub seed: u64,
}

/// 一个脉冲：三台相机同时触发。`segment` / `kind` / `progress` 是真值（合成图按它画直线或弯道），PC 不知道。
#[derive(Clone, Debug, PartialEq)]
pub struct Pulse {
    /// 本件第几个脉冲，从 1 起
    pub seq: u64,
    /// 所在段（0 起）
    pub segment: usize,
    pub kind: SegmentKind,
    /// 段内进度 0–1（取帧中点）
    pub progress: f32,
    /// 这一脉冲在传输中丢帧的相机（计划相机下标）：相机已曝光计数，帧没送到
    pub lost: Vec<usize>,
}

impl PulseProfile {
    fn spec(&self) -> synthetic::SequenceSpec {
        synthetic::SequenceSpec {
            segments: self.segments.clone(),
            head_tail_jitter: self.head_tail_jitter,
            corner_jitter: self.corner_jitter,
        }
    }

    /// 各段实际脉冲数（含波动）。
    pub fn counts(&self) -> Vec<u64> {
        synthetic::frame_counts(&self.spec(), self.seed)
    }

    /// 相邻两个脉冲的间隔（1 / hz）。
    pub fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.hz.clamp(0.1, 1000.0))
    }

    /// 本件的全部脉冲；`lost` 为 (计划相机下标, seq) 的传输丢帧。
    pub fn pulses(&self, lost: &[(usize, u64)]) -> Vec<Pulse> {
        let mut seq = 0;
        let mut pulses = Vec::new();
        for (segment, (&(kind, _), &count)) in self.segments.iter().zip(&self.counts()).enumerate() {
            for position in 0..count {
                seq += 1;
                let progress = ((position as f64 + 0.5) / count as f64) as f32;
                let lost = lost.iter().filter(|&&(_, s)| s == seq).map(|&(camera, _)| camera).collect();
                pulses.push(Pulse { seq, segment, kind, progress, lost });
            }
        }
        pulses
    }
}

/// 段配方的名义剖面（不含波动）：各段 N_exp；有段待标定时按段长 × 频率 / 速度估算（没有段长按类型取默认帧数），
/// 已有整件 ΣN_exp（整件标定）时把待标定的段按估算比例缩放，使合计等于 ΣN_exp，按比例分段的边界与正常件对得上。
pub fn nominal_frames(segments: &[PathSegment], expected_total: Option<u32>, hz: f64) -> Vec<(SegmentKind, u32)> {
    let estimate = |segment: &PathSegment| segment.length_mm.map_or(match segment.kind {
        SegmentKind::Straight => DEFAULT_STRAIGHT_FRAMES,
        SegmentKind::Corner => DEFAULT_CORNER_FRAMES,
    }, |length| (length / SIM_SPEED_MM_S * hz).round().max(1.0) as u32);
    let mut frames: Vec<(SegmentKind, u32)> = segments.iter().map(|s| (s.kind, s.expected_frames.unwrap_or_else(|| estimate(s)))).collect();
    let pending: Vec<usize> = segments.iter().enumerate().filter(|(_, s)| s.expected_frames.is_none()).map(|(i, _)| i).collect();
    let Some(total) = expected_total.filter(|_| !pending.is_empty()) else { return frames };
    let known: u32 = segments.iter().filter_map(|s| s.expected_frames).sum();
    let budget = total.saturating_sub(known).max(pending.len() as u32);
    let estimated: u32 = pending.iter().map(|&i| frames[i].1).sum();
    let mut assigned = 0;
    for (n, &i) in pending.iter().enumerate() {
        let share = if n + 1 == pending.len() {
            budget - assigned
        } else {
            let left = (pending.len() - n - 1) as u32;
            ((u64::from(frames[i].1) * u64::from(budget) / u64::from(estimated.max(1))) as u32).clamp(1, budget - assigned - left)
        };
        frames[i].1 = share;
        assigned += share;
    }
    frames
}

/// 第 seq 帧在名义剖面里的位置：(段下标, 类型, 段内进度)。超出名义总数的帧算在最后一段末尾。
pub fn locate(frames: &[(SegmentKind, u32)], seq: u64) -> Option<(usize, SegmentKind, f32)> {
    let mut start = 0u64;
    for (segment, &(kind, count)) in frames.iter().enumerate() {
        let count = u64::from(count.max(1));
        if seq <= start + count {
            let position = seq.saturating_sub(start + 1);
            return Some((segment, kind, ((position as f64 + 0.5) / count as f64) as f32));
        }
        start += count;
    }
    frames.last().map(|&(kind, _)| (frames.len() - 1, kind, 1.0))
}

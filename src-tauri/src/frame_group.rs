//! 三台相机按 SDK 帧号配组（规格 §3）：同一触发序号的三帧配成一组，缺帧标 incomplete，丢帧只告警。
//!
//! 口径（docs/decisions/segment-burst.md）：seq = frame_counter − 该相机基线（从 1 起）；帧组数 = 各相机最大 seq 的最大值；
//! 单台相机的帧数只用于丢帧告警，不参与容差比较；下一件每台相机的基线 = max(原基线, 本件见到的最高 frame_counter)。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// 一件（或一轮示教采集）的配组器。相机下标与计划相机顺序一致。
#[derive(Clone, Debug)]
pub struct Grouper {
    /// 每台相机的基线：frame_counter ≤ 基线的帧属于上一件
    baselines: Vec<u64>,
    /// 每台相机已接受的 seq
    seqs: Vec<BTreeSet<u64>>,
    /// 每台相机见到的最高 frame_counter（含 Duplicate，不含 Stale 与 OutOfRange）
    max_counters: Vec<u64>,
    /// 可接受的最大 seq：异常帧号（计数跳变）不能生成海量帧组
    max_seq: u64,
}

/// 默认的 seq 上限：ΣN_exp 的上限（PLC Int，D-SB-31）。生产按本件预期另设更紧的上限（[`Grouper::with_limit`]）。
pub const MAX_SEQ: u64 = crate::plan::MAX_EXPECTED_TOTAL as u64;

/// 收到一帧后的归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    Accepted { seq: u64 },
    /// 同一相机同一 seq 已有帧：保留先到的那帧
    Duplicate,
    /// frame_counter 不大于基线：上一件的迟到帧，不计入本件
    Stale,
    /// seq 超过上限（帧号跳变等计数异常）：与 Stale 一样不计入本件、不抬高下一件基线，调用方另记告警
    OutOfRange { seq: u64 },
}

/// 一个帧组：同一 seq 下三台相机是否都到了。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameGroup {
    pub seq: u64,
    /// 按相机顺序
    pub present: Vec<bool>,
    /// 有任一相机缺帧
    pub incomplete: bool,
}

/// 帧数相对 N_exp ± tol 的位置；越界只告警（帧数异常），不改结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CountStatus {
    Normal,
    Low,
    High,
}

/// 丢帧告警的依据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LossReason {
    /// 与 PLC deviceTriggers 相差超过 1
    Triggers { device_triggers: u32 },
    /// 与另一台相机的帧数相差超过 1
    Cameras { other: usize, other_count: u32 },
}

/// 某台相机疑似丢帧：只告警，不升级故障、不拦停。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LossAlarm {
    /// 计划相机下标
    pub camera: usize,
    /// 这台相机的帧数
    pub count: u32,
    pub reason: LossReason,
}

impl Grouper {
    /// baselines 按计划相机顺序，取布防时各相机的 frame_counter 基线；seq 上限取 [`MAX_SEQ`]。
    pub fn new(baselines: Vec<u64>) -> Self {
        Self::with_limit(baselines, MAX_SEQ)
    }

    /// 同 [`new`](Self::new)，seq 超过 `max_seq` 的帧判 OutOfRange（上限不超过 [`MAX_SEQ`]）。
    pub fn with_limit(baselines: Vec<u64>, max_seq: u64) -> Self {
        let cams = baselines.len();
        Self { max_counters: baselines.clone(), baselines, seqs: vec![BTreeSet::new(); cams], max_seq: max_seq.min(MAX_SEQ) }
    }

    /// 布防时基线未知的相机，收到第一帧前补上基线。
    pub fn set_baseline(&mut self, cam: usize, baseline: u64) {
        self.baselines[cam] = baseline;
        self.max_counters[cam] = baseline;
    }

    /// seq = frame_counter − 该相机基线。不大于基线为 Stale；超过上限为 OutOfRange（两者都不改 max_counter）；
    /// 同一相机重复的 seq 为 Duplicate，保留先到的那帧。相机下标越界（不是计划相机）按 Stale 丢弃。
    pub fn offer(&mut self, cam: usize, frame_counter: u64) -> Offer {
        let Some(&baseline) = self.baselines.get(cam) else { return Offer::Stale };
        if frame_counter <= baseline {
            return Offer::Stale;
        }
        let seq = frame_counter - baseline;
        if seq > self.max_seq {
            return Offer::OutOfRange { seq };
        }
        self.max_counters[cam] = self.max_counters[cam].max(frame_counter);
        if self.seqs[cam].insert(seq) { Offer::Accepted { seq } } else { Offer::Duplicate }
    }

    /// 帧组数 = 各相机已接受的最大 seq 的最大值。
    pub fn group_count(&self) -> u64 {
        self.seqs.iter().filter_map(|seqs| seqs.last().copied()).max().unwrap_or(0)
    }

    /// seq 取 1..=各相机最大 seq 的最大值；长度即帧组数。末尾缺帧的相机同样标 incomplete。
    pub fn groups(&self) -> Vec<FrameGroup> {
        (1..=self.group_count())
            .map(|seq| {
                let present: Vec<bool> = self.seqs.iter().map(|seqs| seqs.contains(&seq)).collect();
                let incomplete = present.contains(&false);
                FrameGroup { seq, present, incomplete }
            })
            .collect()
    }

    /// 每台相机已接受的帧数。
    pub fn camera_counts(&self) -> Vec<u32> {
        self.seqs.iter().map(|seqs| u32::try_from(seqs.len()).unwrap_or(u32::MAX)).collect()
    }

    /// 该相机见到的最高 frame_counter，用作下一件基线；没见到帧时返回基线。cam 必须是计划相机下标。
    pub fn max_counter(&self, cam: usize) -> u64 {
        self.max_counters[cam]
    }
}

/// 某台相机帧数与另外每台相机都相差 > 1 时告警。
///
/// “与另外两台相差 > 1”按“与另外每一台都相差 > 1”判：[10, 10, 8] 只有第 3 台告警，
/// 前两台彼此一致，不因第 3 台少帧而被连带告警。
pub fn loss_alarms(counts: &[u32]) -> Vec<LossAlarm> {
    let off = |a: u32, b: u32| a.abs_diff(b) > 1;
    counts
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(camera, count)| {
            let others = || counts.iter().copied().enumerate().filter(move |&(other, _)| other != camera);
            let reason = match () {
                // 报相差最大的那台，并列取下标小的
                _ if others().next().is_some() && others().all(|(_, other_count)| off(count, other_count)) => others()
                    .max_by_key(|&(other, other_count)| (count.abs_diff(other_count), std::cmp::Reverse(other)))
                    .map(|(other, other_count)| LossReason::Cameras { other, other_count }),
                _ => None,
            };
            reason.map(|reason| LossAlarm { camera, count, reason })
        })
        .collect()
}

/// actual ∈ [expected − tol, expected + tol] 为 Normal。
pub fn count_status(actual: u32, expected: u32, tol: u32) -> CountStatus {
    if actual < expected.saturating_sub(tol) {
        CountStatus::Low
    } else if actual > expected.saturating_add(tol) {
        CountStatus::High
    } else {
        CountStatus::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(seq: u64, present: [bool; 3]) -> FrameGroup {
        FrameGroup { seq, present: present.to_vec(), incomplete: present.contains(&false) }
    }

    /// 规格 §3：帧序号取 SDK 帧号（减各自基线），不按到达顺序数。
    #[test]
    fn frames_are_grouped_by_frame_counter_not_arrival_order() {
        let mut grouper = Grouper::new(vec![100, 200, 300]);
        for (cam, counter, seq) in [(0, 103, 3), (1, 201, 1), (0, 101, 1), (2, 302, 2), (1, 203, 3), (2, 301, 1), (0, 102, 2), (2, 303, 3), (1, 202, 2)] {
            assert_eq!(grouper.offer(cam, counter), Offer::Accepted { seq }, "相机 {cam} 帧号 {counter}");
        }
        assert_eq!(grouper.groups(), (1..=3).map(|seq| group(seq, [true; 3])).collect::<Vec<_>>());
    }

    /// 易错边界：重复帧不顶替先到的帧；帧号不大于基线是上一件的迟到帧，不计入本件，也不抬高下一件基线。
    #[test]
    fn duplicate_seq_keeps_first_and_counter_at_or_below_baseline_is_stale() {
        let mut grouper = Grouper::new(vec![10, 20, 30]);
        assert_eq!(grouper.offer(0, 10), Offer::Stale, "等于基线");
        assert_eq!(grouper.offer(0, 9), Offer::Stale, "低于基线");
        assert_eq!(grouper.max_counter(0), 10, "迟到帧不改下一件基线");
        assert_eq!(grouper.offer(0, 12), Offer::Accepted { seq: 2 });
        assert_eq!(grouper.offer(0, 12), Offer::Duplicate);
        assert_eq!(grouper.offer(0, 11), Offer::Accepted { seq: 1 });
        assert_eq!(grouper.camera_counts(), vec![2, 0, 0]);
        let mut next = Grouper::new(vec![grouper.max_counter(0), 20, 30]);
        assert_eq!(next.offer(0, 12), Offer::Stale, "上一件最后一帧迟到，不计入下一件");
    }

    /// 易错边界（A2 遗留 F0-5）：帧号跳变使 seq 超过上限时判 OutOfRange——不建帧组（否则帧组数随帧号跳到上亿）、
    /// 不抬高下一件基线；上限以内照常接受。不另设上限时取 ΣN_exp 的上限。
    #[test]
    fn frame_counter_jump_beyond_seq_limit_is_out_of_range_and_creates_no_groups() {
        let mut grouper = Grouper::with_limit(vec![100, 200, 300], 10);
        assert_eq!(grouper.offer(0, 110), Offer::Accepted { seq: 10 }, "恰好等于上限");
        assert_eq!(grouper.offer(1, 200 + 4_000_000_000), Offer::OutOfRange { seq: 4_000_000_000 }, "帧号跳变");
        assert_eq!(grouper.offer(2, 311), Offer::OutOfRange { seq: 11 }, "超出上限一帧");
        assert_eq!(grouper.groups().len(), 10);
        assert_eq!((grouper.max_counter(1), grouper.max_counter(2)), (200, 300));
        assert_eq!(Grouper::new(vec![0]).offer(0, MAX_SEQ + 1), Offer::OutOfRange { seq: MAX_SEQ + 1 });
    }

    /// 规格 §2：帧数 ∈ [N_exp − tol, N_exp + tol] 为正常，越界一帧即 Low / High。
    #[test]
    fn frame_count_at_tolerance_edge_is_normal_and_one_beyond_is_low_or_high() {
        for (actual, expected, tol, status) in [
            (9, 10, 1, CountStatus::Normal),
            (11, 10, 1, CountStatus::Normal),
            (8, 10, 1, CountStatus::Low),
            (12, 10, 1, CountStatus::High),
            (11, 10, 0, CountStatus::High),
            (0, 1, 2, CountStatus::Normal),
        ] {
            assert_eq!(count_status(actual, expected, tol), status, "{actual} vs {expected} ± {tol}");
        }
    }
}

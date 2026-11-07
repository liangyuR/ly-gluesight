//! 笔（D-SB-62）：机械臂每次起涂打开、收胶关闭拍照信号（Line0 门控），一笔即一次门控。一件的笔结构按笔给出帧组数，
//! seq 在一件内跨笔连续（D-SB-55），第 j 笔是前面各笔帧组数之和之后的那一段。
//!
//! 生产直接取帧编号器的门控槽数；示教轮次、样本按每帧时间戳重算：同一台相机相邻两个 seq 的间隔明显长于帧周期即中间停过门控。

use serde::{Deserialize, Serialize};

/// 每笔首尾跳过帧组数的上限。
pub const MAX_SKIP: u32 = 50;

/// 新建配方每笔开头跳过的帧组数：起涂时胶条还没铺满走廊，现场从第 3 帧起才完整。
pub const DEFAULT_SKIP_HEAD: u32 = 2;

/// 相邻两个 seq 的间隔超过帧周期的这个倍数，算中间停过门控（门控内间隔恒为一个帧周期，停门控至少多出机械臂空走的时间）。
const PAUSE_RATIO: f64 = 1.5;

/// 配方的笔规则：每笔开头、结尾跳过的帧组数（起涂、收胶时胶条还没铺满走廊）。不进计划身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PassSkip {
    #[serde(default)]
    pub head: u32,
    #[serde(default)]
    pub tail: u32,
}

impl PassSkip {
    pub fn is_zero(&self) -> bool {
        self.head == 0 && self.tail == 0
    }
}

/// 一件判定用的笔信息：本件的笔结构（空 = 不知道，按没有笔处理）、示教件的笔结构与跳过规则。
#[derive(Clone, Copy, Debug, Default)]
pub struct PassRules<'a> {
    pub part: &'a [u32],
    pub taught: Option<&'a [u32]>,
    pub skip: PassSkip,
}

impl PassRules<'_> {
    pub const NONE: PassRules<'static> = PassRules {
        part: &[],
        taught: None,
        skip: PassSkip { head: 0, tail: 0 },
    };

    /// 本件笔数与示教件一致，段边界可按笔换算。
    pub fn aligned(&self) -> bool {
        self.taught
            .is_some_and(|taught| !taught.is_empty() && taught.len() == self.part.len())
    }

    /// 本件与示教件笔数不一致（两边都知道笔结构时）。
    pub fn mismatch(&self) -> bool {
        self.taught.is_some_and(|taught| !taught.is_empty())
            && !self.part.is_empty()
            && !self.aligned()
    }
}

/// 第 `pass` 笔（下标）的第一个 seq。
pub fn first_seq(layout: &[u32], pass: usize) -> u64 {
    1 + layout[..pass].iter().map(|&n| u64::from(n)).sum::<u64>()
}

/// seq 落在第几笔（下标）与笔内第几帧（从 1 起）；超出笔结构为 None。
pub fn locate(layout: &[u32], seq: u64) -> Option<(usize, u32)> {
    let mut start = 1u64;
    for (j, &n) in layout.iter().enumerate() {
        let end = start + u64::from(n);
        if seq >= start && seq < end {
            return Some((j, (seq - start + 1) as u32));
        }
        start = end;
    }
    None
}

/// 帧编号器的门控槽数截到本件帧组数：迟到帧被丢、末笔没收满时以帧组数为准；没有笔为空。
pub fn from_slots(slots: &[u64], total: u64) -> Vec<u32> {
    let mut left = total;
    let mut out = Vec::new();
    for &n in slots {
        if left == 0 {
            break;
        }
        let take = n.min(left);
        out.push(take as u32);
        left -= take;
    }
    if left > 0 {
        match out.last_mut() {
            Some(last) => *last += left as u32,
            None => out.push(left as u32),
        }
    }
    out
}

/// 按每帧时间戳（毫秒或任意同一单位）重算笔结构：帧周期取同一台相机相邻 seq 间隔的中位数；相邻两个 seq 之间，
/// 两帧都在的相机里多数看到间隔超过 1.5 个帧周期，就在后一个 seq 开新笔。没有一对相邻帧时整件算一笔。`total` 为帧组数。
pub fn from_times(frames: impl IntoIterator<Item = (usize, u64, f64)>, total: u64) -> Vec<u32> {
    if total == 0 {
        return Vec::new();
    }
    let mut by_camera: std::collections::BTreeMap<usize, Vec<(u64, f64)>> = Default::default();
    for (camera, seq, ts) in frames {
        by_camera.entry(camera).or_default().push((seq, ts));
    }
    let mut steps: Vec<(u64, f64)> = Vec::new();
    for frames in by_camera.values_mut() {
        frames.sort_by(|a, b| a.0.cmp(&b.0));
        frames.dedup_by_key(|f| f.0);
        steps.extend(
            frames
                .windows(2)
                .filter(|pair| pair[1].0 == pair[0].0 + 1)
                .map(|pair| (pair[1].0, pair[1].1 - pair[0].1)),
        );
    }
    let mut deltas: Vec<f64> = steps.iter().map(|&(_, d)| d).collect();
    deltas.sort_by(f64::total_cmp);
    let Some(&period) = deltas.get(deltas.len() / 2).filter(|&&p| p > 0.0) else {
        return vec![total as u32];
    };
    let mut votes: std::collections::BTreeMap<u64, (u32, u32)> = Default::default();
    for (seq, delta) in steps {
        let vote = votes.entry(seq).or_default();
        if delta > PAUSE_RATIO * period {
            vote.0 += 1;
        } else {
            vote.1 += 1;
        }
    }
    let mut layout = Vec::new();
    let mut start = 1u64;
    for (&seq, &(pause, steady)) in &votes {
        if pause > steady && seq > start && seq <= total {
            layout.push((seq - start) as u32);
            start = seq;
        }
    }
    layout.push((total + 1 - start) as u32);
    layout
}

/// 每笔首尾跳过的帧组（seq 范围，闭区间）与是否整笔跳过（笔太短）：(笔下标, first, last, 整笔)。
pub fn skipped(layout: &[u32], skip: PassSkip) -> Vec<(usize, u64, u64, bool)> {
    if skip.is_zero() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (j, &n) in layout.iter().enumerate() {
        let first = first_seq(layout, j);
        let last = first + u64::from(n) - 1;
        if n <= skip.head + skip.tail {
            out.push((j, first, last, true));
            continue;
        }
        if skip.head > 0 {
            out.push((j, first, first + u64::from(skip.head) - 1, false));
        }
        if skip.tail > 0 {
            out.push((j, last + 1 - u64::from(skip.tail), last, false));
        }
    }
    out
}

/// 示教件的段边界换算到本件的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedCut {
    pub seq: u64,
    /// 正好在两笔之间
    pub hard: bool,
    /// 换算后仍落在示教件这条边界所在的那一笔里；本件这一笔太短、放不下其中的段时为 false（边界不可信）
    pub fits: bool,
}

/// 示教件的段边界（各段最后一个 seq，`cuts` 严格递增）按笔换算到本件：边界正好是示教件某笔的最后一帧时取本件同一笔的
/// 最后一帧（硬边界，不留过渡区）；落在笔中间时按这一笔本件与示教件的帧组数比例换算，并留在同一笔内。
/// 保证严格递增且每段至少一帧；为此被推出原来那一笔的边界标 `fits = false`，也不再算硬边界。两边笔数不一致时为 None。
pub fn map_cuts(cuts: &[u64], taught: &[u32], part: &[u32]) -> Option<Vec<MappedCut>> {
    if taught.is_empty() || taught.len() != part.len() {
        return None;
    }
    let total: u64 = part.iter().map(|&n| u64::from(n)).sum();
    let mut previous = 0u64;
    let mut out = Vec::with_capacity(cuts.len());
    for (k, &cut) in cuts.iter().enumerate() {
        let (j, offset) = locate(taught, cut)?;
        let (n, p) = (u64::from(taught[j]), u64::from(part[j]));
        let first = first_seq(part, j);
        let last = first + p - 1;
        let (seq, hard) = if u64::from(offset) == n {
            (last, true)
        } else {
            let scaled = (2 * u64::from(offset) * p + n) / (2 * n);
            (
                first - 1 + scaled.clamp(1, p.saturating_sub(1).max(1)),
                false,
            )
        };
        let remaining = (cuts.len() - k) as u64;
        let seq = seq.max(previous + 1).min(total.saturating_sub(remaining));
        previous = seq;
        let fits = (first..=last).contains(&seq);
        out.push(MappedCut {
            seq,
            hard: hard && fits,
            fits,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 防的回归：停门控的间隔要按帧周期判，相机丢帧（seq 不相邻）不能当成新的一笔；三台相机多数决定。
    #[test]
    fn pauses_split_passes_but_dropped_frames_do_not() {
        let mut frames = Vec::new();
        for camera in 0..3 {
            let mut ts = 0.0;
            for seq in 1..=10u64 {
                ts += if seq == 5 { 900.0 } else { 200.0 };
                if camera == 1 && seq == 8 {
                    continue;
                }
                frames.push((camera, seq, ts));
            }
        }
        assert_eq!(from_times(frames, 10), vec![4, 6]);
    }

    /// 防的回归：段边界落在笔的末帧时必须对齐本件同一笔的末帧；落在笔中间按这一笔的比例换算，不跨到相邻笔。
    #[test]
    fn cuts_follow_pass_ends_and_scale_within_their_pass() {
        let taught = [10, 20, 4];
        let part = [12, 18, 4];
        let cuts = [10, 20, 30];
        let mapped = |cuts: &[u64], taught: &[u32], part: &[u32]| {
            map_cuts(cuts, taught, part).map(|m| {
                m.iter()
                    .map(|c| (c.seq, c.hard, c.fits))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            mapped(&cuts, &taught, &part),
            Some(vec![(12, true, true), (21, false, true), (30, true, true)])
        );
        assert_eq!(mapped(&cuts, &taught, &[12, 22]), None);
    }

    /// 防的回归（PR #257 评审）：本件某笔太短、放不下示教件在这一笔里的几段时，为保持递增被推进下一笔的边界不能仍算可信、
    /// 更不能仍算两笔之间的硬边界。
    #[test]
    fn cuts_pushed_out_of_their_pass_are_not_trusted() {
        let fits: Vec<(u64, bool, bool)> = map_cuts(&[3, 6, 10], &[10, 10], &[1, 20])
            .unwrap()
            .iter()
            .map(|c| (c.seq, c.hard, c.fits))
            .collect();
        assert_eq!(
            fits,
            vec![(1, false, true), (2, false, false), (3, false, false)]
        );
    }

    /// 防的回归：每笔开头跳过的帧要按每一笔定位；帧数不超过首尾跳过之和的笔整笔不检。
    #[test]
    fn skip_marks_each_pass_head_and_whole_short_passes() {
        let skip = PassSkip { head: 2, tail: 0 };
        assert_eq!(
            skipped(&[5, 2, 4], skip),
            vec![(0, 1, 2, false), (1, 6, 7, true), (2, 8, 9, false)]
        );
    }
}

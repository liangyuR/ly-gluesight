//! 按相机时间戳给帧编号（D-SB-55），取代 SDK 帧号与开流基线（D-SB-10）。
//!
//! 一台相机只用自己的时钟：帧身份 = (门控号 g, 门控内序号 i)。门控号与门控起点优先取 Line0 轮询锁存的“门控开始”
//! 相机时钟（`gate::Gate`）：本件开始后第 n 个开始边沿即第 n − 1 个门控，i = round((时间戳 − 起点) / 帧周期)。
//! 轮询读数不新鲜时退回按相邻帧间隔判：间隔约为帧周期的整数倍 k 即同一门控、i 加 k（k ≥ 2 是丢了帧），否则开新门控。
//! 轮询中断时可能漏记边沿：按边沿数得出的门控号比已推断的小时，以帧间隔为准，并记下差额对齐之后的边沿。
//! seq 在一件内跨门控连续：seq = 前面各门控的槽数之和 + i + 1，门控槽数 = 各相机在该门控见到的最大 i + 1；
//! 某台相机进入后一个门控时，前面的门控就冻结，不再加槽（迟到的越界帧丢弃）。
//! 件的边界：时间戳不大于接件时的相机时钟（下限）即上一件的帧。时间戳明显回退说明相机断电或时钟被复位，这台相机本件
//! 之后的帧都不收。

/// 模拟、样本集相机的合成时间戳：第 n 帧 = n × 合成帧周期（ns）。
pub const SYNTH_PERIOD_NS: u64 = 100_000_000;

/// 间隔与帧周期整数倍相差在这个比例内算同一门控。
const PHASE_TOLERANCE: f64 = 0.1;

/// 门控开始边沿锁存的时钟最多比该门控首帧的时间戳晚这么多（轮询 5 ms 加锁存读数；现场实测晚 1.5 ms）。只放宽到这里，
/// 不放宽到半个帧周期：否则前一门控末帧在途时新门控已开，末帧会被算进新门控（ns）。
pub const EDGE_SLACK_NS: u64 = 15_000_000;

/// 一帧编号的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// 本件帧组序号（从 1 起）
    Seq(u64),
    /// 时间戳不大于本件下限：上一件的迟到帧或接件前的帧
    Stale,
    /// 属于已冻结的前一个门控、又超出它的槽数：没法再插进去
    Late,
    /// 时间戳回退（相机断电或时钟被复位）：这台相机本件之后的帧都不收
    ClockBack,
}

#[derive(Clone, Debug)]
struct Track {
    period: f64,
    floor: u64,
    /// 下限就是上一帧的时间戳（合成时钟）：第一帧不靠边沿也能算出序号
    anchored: bool,
    last: Option<Last>,
    broken: bool,
    /// 轮询中断漏记的门控开始边沿数：按边沿数得出的门控号要加上它
    edge_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct Last {
    ts: u64,
    gate: usize,
    origin: u64,
}

/// 一件内各计划相机的帧编号器。相机下标与计划相机顺序一致。
#[derive(Clone, Debug)]
pub struct FrameClock {
    tracks: Vec<Track>,
    /// 各门控的槽数
    slots: Vec<u64>,
    /// [0, frozen) 的门控已冻结
    frozen: usize,
}

/// 接件时一台计划相机的编号起点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockStart {
    /// 帧周期（ns）
    pub period_ns: f64,
    /// 下限：时间戳不大于它的帧属于上一件
    pub floor: u64,
    /// 合成时钟：下限是“上一帧”的时间戳
    pub anchored: bool,
}

impl FrameClock {
    pub fn new(starts: &[ClockStart]) -> Self {
        Self {
            tracks: starts
                .iter()
                .map(|s| Track {
                    period: s.period_ns.max(1.0),
                    floor: s.floor,
                    anchored: s.anchored,
                    last: None,
                    broken: false,
                    edge_offset: 0,
                })
                .collect(),
            slots: Vec::new(),
            frozen: 0,
        }
    }

    /// 计划相机 `cam` 时间戳 `ts`（ns）的一帧。`opens` 是这台相机本件看到的门控开始时刻（锁存的相机时钟，升序）；
    /// 轮询读数不新鲜、不能确定有没有漏看边沿时为 None。
    pub fn stamp(&mut self, cam: usize, ts: u64, opens: Option<&[u64]>) -> Stamp {
        let Some(track) = self.tracks.get_mut(cam) else {
            return Stamp::Stale;
        };
        if track.broken {
            return Stamp::ClockBack;
        }
        let half = (track.period / 2.0) as u64;
        if track.last.is_some_and(|last| ts + half < last.ts) {
            track.broken = true;
            return Stamp::ClockBack;
        }
        if ts <= track.floor {
            return Stamp::Stale;
        }
        let index =
            |origin: u64| ((ts.saturating_sub(origin)) as f64 / track.period).round() as u64;
        let by_edges = opens.and_then(|opens| {
            let seen: Vec<u64> = opens
                .iter()
                .copied()
                .filter(|&o| o > track.floor && o <= ts + EDGE_SLACK_NS)
                .collect();
            let origin = *seen.last()?;
            Some((seen.len() - 1, origin))
        });
        // 按相邻帧间隔推断：间隔是帧周期整数倍为同一门控，否则开新门控
        let infer = |last: Last| {
            let ratio = (ts.saturating_sub(last.ts)) as f64 / track.period;
            let k = ratio.round();
            if k >= 1.0 && (ratio - k).abs() < PHASE_TOLERANCE {
                (last.gate, last.origin)
            } else {
                (last.gate + 1, ts)
            }
        };
        let (gate, origin) = match (by_edges, track.last) {
            (Some((raw, _)), Some(last)) if raw + track.edge_offset < last.gate => {
                // 边沿记录比已推断的门控少：轮询中断时漏记了边沿，以帧间隔为准，之后的边沿按差额对齐
                let found = infer(last);
                track.edge_offset = found.0 - raw;
                found
            }
            (Some((raw, origin)), _) => (raw + track.edge_offset, origin),
            (None, None) if track.anchored => (0, track.floor + track.period as u64),
            (None, None) => (0, ts),
            (None, Some(last)) => infer(last),
        };
        let i = index(origin);
        track.last = Some(Last { ts, gate, origin });
        self.frozen = self.frozen.max(gate);
        if self.slots.len() <= gate {
            self.slots.resize(gate + 1, 0);
        }
        if gate < self.frozen && i + 1 > self.slots[gate] {
            return Stamp::Late;
        }
        self.slots[gate] = self.slots[gate].max(i + 1);
        Stamp::Seq(self.slots[..gate].iter().sum::<u64>() + i + 1)
    }

    /// 这台相机本件收到的最后一帧时间戳与帧周期（ns）。
    pub fn last(&self, cam: usize) -> Option<(u64, f64)> {
        let track = self.tracks.get(cam)?;
        track.last.map(|last| (last.ts, track.period))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 100_000_000;

    fn clock(cams: usize, anchored: bool) -> FrameClock {
        FrameClock::new(&vec![
            ClockStart {
                period_ns: T as f64,
                floor: 1_000 * T,
                anchored,
            };
            cams
        ])
    }

    /// 门控从相机时钟 `open` 开始，第 i 帧的时间戳（曝光比锁存早 1.5 ms）。
    fn at(open: u64, i: u64) -> u64 {
        open - 1_500_000 + i * T
    }

    /// 防的回归（D-SB-55）：按 SDK 帧号配组时，相机检测中重开（帧号从 1 重数）只能整件判 ERR；改按时间戳后，
    /// 门控内丢一帧 seq 要跳过它，后一个门控的 seq 接着前一个门控的槽数往下排，不因段间空走或丢帧错位。
    #[test]
    fn seq_follows_gate_edges_across_gates_and_skips_lost_frames() {
        let mut c = clock(2, false);
        let (g1, g2) = (1_010 * T, 1_050 * T + 37_000_000);
        let opens = [g1, g2];
        let stamp = |c: &mut FrameClock, cam, ts| c.stamp(cam, ts, Some(&opens));
        assert_eq!(stamp(&mut c, 0, at(g1, 0)), Stamp::Seq(1));
        assert_eq!(stamp(&mut c, 1, at(g1, 0)), Stamp::Seq(1));
        assert_eq!(stamp(&mut c, 0, at(g1, 1)), Stamp::Seq(2));
        assert_eq!(
            stamp(&mut c, 0, at(g1, 3)),
            Stamp::Seq(4),
            "相机 0 丢了第 3 帧"
        );
        assert_eq!(stamp(&mut c, 1, at(g1, 3)), Stamp::Seq(4));
        assert_eq!(
            stamp(&mut c, 1, at(g2, 0)),
            Stamp::Seq(5),
            "第二个门控接着 4 个槽往下排"
        );
        assert_eq!(
            stamp(&mut c, 0, at(g2, 1)),
            Stamp::Seq(6),
            "相机 0 丢了门控首帧，按边沿仍对齐"
        );
        assert_eq!(
            stamp(&mut c, 0, at(g1, 9)),
            Stamp::ClockBack,
            "时间戳回退：相机断电或时钟被复位"
        );
        assert_eq!(stamp(&mut c, 0, at(g2, 2)), Stamp::ClockBack, "之后都不收");
        assert_eq!(stamp(&mut c, 1, at(g2, 1)), Stamp::Seq(6), "另一台不受影响");
    }

    /// 防的回归：没有新鲜的门控边沿时，按相邻帧间隔判——间隔是帧周期整数倍算同一门控（k ≥ 2 是丢帧），
    /// 否则开新门控；上一件的帧（不大于下限）不计入；冻结门控之后再来的越界帧不能插进去。
    #[test]
    fn without_edges_gaps_off_the_frame_grid_open_a_new_gate() {
        let mut c = clock(2, false);
        let base = 1_010 * T + 3_000_000;
        assert_eq!(c.stamp(0, 900 * T, None), Stamp::Stale);
        assert_eq!(c.stamp(0, base, None), Stamp::Seq(1));
        assert_eq!(
            c.stamp(0, base + 2 * T, None),
            Stamp::Seq(3),
            "间隔 2 个周期：丢了一帧"
        );
        assert_eq!(
            c.stamp(1, base + 2 * T + 40, None),
            Stamp::Seq(1),
            "相机 1 第一帧按自己算"
        );
        let reopen = base + 2 * T + 230_000_000;
        assert_eq!(
            c.stamp(0, reopen, None),
            Stamp::Seq(4),
            "间隔 2.3 个周期：新门控"
        );
        assert_eq!(
            c.stamp(1, base + 9 * T, None),
            Stamp::Late,
            "前一个门控已冻结：相机 1 的越界帧不收"
        );
    }

    /// 防的回归（PR #215 评审）：前一门控末帧在途时新门控已开、边沿已锁存，末帧被按半个帧周期的余地算进新门控，
    /// 新门控首帧随之成了重复帧被丢；边沿只能比首帧晚轮询与锁存的误差。
    #[test]
    fn an_edge_latched_after_an_old_frame_does_not_pull_it_into_the_new_gate() {
        let mut c = clock(1, false);
        let (g1, g2) = (
            2_000 * T / 100 + 1_000 * T,
            2_130 * T / 100 + 1_000 * T + 1_500_000,
        );
        let opens = [g1, g2];
        assert_eq!(c.stamp(0, at(g1, 0), Some(&opens)), Stamp::Seq(1));
        assert_eq!(
            c.stamp(0, at(g1, 1), Some(&opens)),
            Stamp::Seq(2),
            "末帧曝光早于新边沿 30 ms：仍属旧门控"
        );
        assert_eq!(
            c.stamp(0, at(g2, 0), Some(&opens)),
            Stamp::Seq(3),
            "新门控首帧"
        );
    }

    /// 防的回归（PR #215 评审）：轮询卡住期间门控关了又开、边沿没记上，恢复后旧的边沿表把已按帧间隔推断出的新门控拉回旧门控，
    /// 之后的帧都成了冻结门控的越界帧；之后再记到的边沿要按漏记的差额对齐。
    #[test]
    fn missed_edges_during_a_polling_gap_do_not_pull_frames_back_to_an_old_gate() {
        let mut c = clock(1, false);
        let g1 = 1_020 * T;
        assert_eq!(c.stamp(0, at(g1, 0), Some(&[g1])), Stamp::Seq(1));
        let reopened = g1 + 2_330_000_000;
        assert_eq!(
            c.stamp(0, reopened, None),
            Stamp::Seq(2),
            "轮询卡住：按间隔开新门控"
        );
        assert_eq!(
            c.stamp(0, reopened + T, Some(&[g1])),
            Stamp::Seq(3),
            "边沿表漏了一次开始：以帧间隔为准"
        );
        let g3 = reopened + 10 * T;
        assert_eq!(
            c.stamp(0, at(g3, 0), Some(&[g1, g3])),
            Stamp::Seq(4),
            "下一次边沿按差额对齐成第 3 个门控"
        );
    }

    /// 防的回归：模拟、样本集的合成时钟没有门控边沿，第一帧要按下限（上一帧）算序号，与原来的帧号配组一致。
    #[test]
    fn synthetic_clock_numbers_frames_like_counters() {
        let mut c = clock(1, true);
        assert_eq!(c.stamp(0, 1_002 * T, None), Stamp::Seq(2), "第 1001 帧丢了");
        assert_eq!(c.stamp(0, 1_003 * T, None), Stamp::Seq(3));
    }
}

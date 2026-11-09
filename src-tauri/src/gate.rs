//! Line0 拍照信号（门控）的轮询状态（D-SB-54）。现场相机没有 GenICam 事件，每台海康相机一个线程每 5 ms 读一次
//! `LineStatusAll` 的 Line0 位（`camera::watch_gate`）；收尾时看到门控已关闭、再等一小段（帧从曝光到 PC 收到约 39 ms），
//! 就不用固定等满收尾窗口。读数不新鲜、有相机看到门控开着或空闲电平还不知道时不提前，照旧等收尾窗口。
//! 看到边沿时同时锁存这台相机的时钟：门控开始的时刻给帧编号定门控号与起点（D-SB-55），门控关闭的时刻判定最后一帧是否已到。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 轮询间隔。
pub const POLL: Duration = Duration::from_millis(5);
/// 读数在这么久内算新鲜：读卡住的相机（现场见过一次 0.5 s）不作数。
pub const FRESH: Duration = Duration::from_millis(50);
/// 这么久没收到帧时读到的电平算空闲电平：状态寄存器的极性随接线而异（现场空闲读 1、拍照时读 0），不按触发方式推断。
pub const QUIET: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug)]
struct Seen {
    active: bool,
    changed_at: Instant,
    read_at: Instant,
}

/// 帧到达时，最近一次读数在这么久内才认为门控开始边沿都已看到：帧从曝光到收到约 39 ms，边沿比曝光还早，
/// 这段时间里读过电平就不会漏掉它（D-SB-55）。
pub const EDGE_FRESH: Duration = Duration::from_millis(30);
/// 每台相机留的门控开始边沿数。
const KEEP_OPENS: usize = 4096;

/// 不同相机看到门控开始的时刻相差在这么久内算同一次门控（现场同一边沿各相机相差 1–2 ms，门控开得比一帧还短的不会出帧）。
const SAME_OPEN: Duration = Duration::from_millis(100);

/// 边沿时锁存的相机时钟（ns）；门控开始还记下它在相机组里的门控序号。
#[derive(Debug, Default)]
struct Clocks {
    opens: std::collections::VecDeque<(u64, u64)>,
    close: Option<u64>,
}

/// 相机组看到的门控开始：累计次数、最近一次看到的时刻，以及看到这一次的相机。
#[derive(Debug, Default)]
struct Opens {
    count: u64,
    last: Option<Instant>,
    cams: Vec<u8>,
}

/// 各相机最近一次读到的门控状态（相机组序号为键），以及边沿时锁存的相机时钟。
#[derive(Debug, Default)]
pub struct Gate {
    cams: Mutex<HashMap<u8, Seen>>,
    clocks: Mutex<HashMap<u8, Clocks>>,
    opens: Mutex<Opens>,
}

impl Gate {
    /// 相机 `cam` 在 `at` 看到门控开始（`opening`）或关闭，锁存的相机时钟为 `clock`（ns）。门控开始按相机组共同的序号记：
    /// 与别的相机看到的开始相差在 `SAME_OPEN` 内算同一次，一台相机轮询中断漏了一次，其余相机的序号照样对得上；
    /// 同一台相机又看到一次开始总是新的一次（门控开得很近时也不并掉）。
    pub fn edge(&self, cam: u8, opening: bool, clock: u64, at: Instant) {
        let index = opening.then(|| {
            let mut opens = self.opens.lock().unwrap();
            if opens.cams.contains(&cam)
                || opens
                    .last
                    .is_none_or(|last| at.saturating_duration_since(last) >= SAME_OPEN)
            {
                opens.count += 1;
                opens.cams.clear();
            }
            opens.cams.push(cam);
            opens.last = Some(at);
            opens.count - 1
        });
        let mut clocks = self.clocks.lock().unwrap();
        let entry = clocks.entry(cam).or_default();
        if let Some(index) = index {
            if entry.opens.len() == KEEP_OPENS {
                entry.opens.pop_front();
            }
            entry.opens.push_back((clock, index));
        } else {
            entry.close = Some(clock);
        }
    }

    /// 相机组到目前为止看到的门控开始次数：接件时记下，本件的门控序号从它起。
    pub fn open_count(&self) -> u64 {
        self.opens.lock().unwrap().count
    }

    /// 相机 `cam` 看到的门控开始（相机时钟，相机组门控序号），升序；它最近一次读数不在 `EDGE_FRESH` 内（可能还没看到最新的边沿）时为 None。
    pub fn opens(&self, cam: u8, now: Instant) -> Option<Vec<(u64, u64)>> {
        let fresh = self
            .cams
            .lock()
            .unwrap()
            .get(&cam)
            .is_some_and(|seen| now.saturating_duration_since(seen.read_at) <= EDGE_FRESH);
        fresh.then(|| {
            self.clocks
                .lock()
                .unwrap()
                .get(&cam)
                .map(|c| c.opens.iter().copied().collect())
                .unwrap_or_default()
        })
    }

    /// 相机 `cam` 最近一次看到门控开始时锁存的相机时钟（ns）。
    pub fn last_open(&self, cam: u8) -> Option<u64> {
        self.clocks
            .lock()
            .unwrap()
            .get(&cam)
            .and_then(|c| c.opens.back().map(|&(clock, _)| clock))
    }

    /// 相机 `cam` 最近一次看到门控关闭时锁存的相机时钟（ns）。
    pub fn close_clock(&self, cam: u8) -> Option<u64> {
        self.clocks.lock().unwrap().get(&cam).and_then(|c| c.close)
    }

    /// 相机 `cam` 在 `at` 读到门控开着（`active`）或关着；状态变了记下变化的时刻，第一次读到时也记为此刻。
    pub fn observe(&self, cam: u8, active: bool, at: Instant) {
        let mut cams = self.cams.lock().unwrap();
        match cams.get_mut(&cam) {
            Some(seen) if seen.active == active => seen.read_at = at,
            _ => {
                cams.insert(
                    cam,
                    Seen {
                        active,
                        changed_at: at,
                        read_at: at,
                    },
                );
            }
        }
    }

    /// 相机读不了电平（没打开、读失败、空闲电平还不知道）：去掉它的状态。
    pub fn forget(&self, cam: u8) {
        self.cams.lock().unwrap().remove(&cam);
    }

    /// `cams` 里读数新鲜的相机都看到门控关着时，返回它们里最晚看到关闭的时刻（读卡过的相机看到得晚，取最晚的只会多等）；
    /// 没有新鲜读数，或有相机看到开着时为 None。
    pub fn closed_since(&self, cams: &[u8], now: Instant) -> Option<Instant> {
        let map = self.cams.lock().unwrap();
        let fresh: Vec<&Seen> = cams
            .iter()
            .filter_map(|cam| map.get(cam))
            .filter(|seen| now.saturating_duration_since(seen.read_at) <= FRESH)
            .collect();
        if fresh.is_empty() || fresh.iter().any(|seen| seen.active) {
            return None;
        }
        fresh.iter().map(|seen| seen.changed_at).max()
    }
}

/// 一台相机的空闲电平：`QUIET`（且不短于三个帧周期）内没收到帧、电平也一直没变时读到的电平即空闲电平，其余电平即门控开着。
#[derive(Debug)]
pub struct LineWatch {
    idle: Option<bool>,
    frames: u64,
    last_frame: Instant,
    level: Option<bool>,
    level_since: Instant,
}

impl LineWatch {
    pub fn new(now: Instant) -> Self {
        Self {
            idle: None,
            frames: 0,
            last_frame: now,
            level: None,
            level_since: now,
        }
    }

    /// 读到 Line0 电平 `level` 时这台相机累计收到 `frames` 帧、帧周期为 `period`：返回门控是否开着；空闲电平还不知道时为 None。
    /// 帧率低时门控开着的帧间空档也可能超过 1 s，安静时长放宽到三个帧周期，免得把拍照电平学成空闲电平；
    /// 电平刚变（空闲后门控刚开、第一帧还在路上）不算安静，要这个电平本身保持满安静时长。
    pub fn read(
        &mut self,
        level: bool,
        frames: u64,
        now: Instant,
        period: Duration,
    ) -> Option<bool> {
        if frames != self.frames {
            self.frames = frames;
            self.last_frame = now;
        }
        if self.level.is_some_and(|before| before != level) {
            self.level_since = now;
        }
        self.level = Some(level);
        let quiet = QUIET.max(period * 3);
        if now.saturating_duration_since(self.last_frame) >= quiet
            && now.saturating_duration_since(self.level_since) >= quiet
        {
            self.idle = Some(level);
        }
        self.idle.map(|idle| level != idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 防的回归：门控还开着、有相机读卡住时提前收尾（会丢掉还在路上的帧）；两台看到关闭的时刻不同时取晚的，只会多等、不会少等。
    #[test]
    fn gate_counts_as_closed_only_when_every_fresh_camera_sees_it_closed() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let gate = Gate::default();
        assert_eq!(gate.closed_since(&[0, 1], ms(0)), None, "还没有读数");
        gate.observe(0, true, ms(0));
        gate.observe(1, true, ms(0));
        gate.observe(0, false, ms(10));
        assert_eq!(
            gate.closed_since(&[0, 1], ms(12)),
            None,
            "相机 1 还看到开着"
        );
        gate.observe(1, false, ms(14));
        assert_eq!(
            gate.closed_since(&[0, 1], ms(15)),
            Some(ms(14)),
            "取最晚看到关闭的时刻"
        );
        gate.observe(0, false, ms(80));
        assert_eq!(
            gate.closed_since(&[0, 1], ms(80)),
            Some(ms(10)),
            "相机 1 读数过期（读卡住）不作数，只看相机 0"
        );
        assert_eq!(gate.closed_since(&[0, 1], ms(200)), None, "都过期");
        gate.observe(0, true, ms(201));
        gate.observe(1, false, ms(201));
        assert_eq!(gate.closed_since(&[0, 1], ms(202)), None, "门控又开了");
        assert_eq!(
            gate.closed_since(&[1], ms(202)),
            Some(ms(14)),
            "只问本件用到的相机"
        );
    }

    const P: Duration = Duration::from_millis(100);

    /// 防的回归（PR #215 评审）：空闲够久后门控刚开、第一帧还在路上时，按“没来帧”把拍照电平学成空闲电平，
    /// 漏掉这次开始、把之后的关闭当成开始，门控起点全错。
    #[test]
    fn a_level_that_just_changed_after_idle_is_not_learned_as_idle() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut watch = LineWatch::new(t0);
        assert_eq!(watch.read(true, 0, ms(1000), P), Some(false), "空闲读 1");
        assert_eq!(
            watch.read(false, 0, ms(5000), P),
            Some(true),
            "门控刚开、帧还没到：开着"
        );
        assert_eq!(watch.read(false, 1, ms(5040), P), Some(true));
        assert_eq!(watch.read(true, 9, ms(6000), P), Some(false), "关闭");
    }

    /// 防的回归（PR #215 评审）：帧率低到帧间隔超过 1 s 时，门控开着的帧间空档被当成空闲，把拍照电平学成空闲电平、极性反了。
    #[test]
    fn a_frame_gap_within_three_periods_keeps_the_learned_idle_level() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let period = Duration::from_secs(2);
        let mut watch = LineWatch::new(t0);
        assert_eq!(
            watch.read(true, 0, ms(1500), period),
            None,
            "低帧率下安静 1.5 s 还不算空闲"
        );
        assert_eq!(
            watch.read(true, 0, ms(6000), period),
            Some(false),
            "安静满三个帧周期：空闲读 1"
        );
        assert_eq!(watch.read(false, 1, ms(6100), period), Some(true));
        assert_eq!(
            watch.read(false, 1, ms(7600), period),
            Some(true),
            "帧间空档 1.5 s：门控仍开着"
        );
    }

    /// 防的回归（PR #215 评审）：一台相机轮询中断漏看一次门控开始，按它自己数的边沿算门控号，与其余相机错开一个门控、配错帧组。
    #[test]
    fn opens_seen_by_any_camera_share_one_index_across_the_group() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let gate = Gate::default();
        gate.observe(0, true, ms(3000));
        gate.observe(1, true, ms(3000));
        gate.edge(0, true, 10, ms(0));
        gate.edge(1, true, 20, ms(2));
        gate.edge(0, true, 11, ms(230));
        gate.edge(0, true, 12, ms(1000));
        gate.edge(1, true, 22, ms(1001));
        gate.edge(0, true, 13, ms(1060));
        assert_eq!(
            gate.opens(0, ms(3000)).unwrap()[..3],
            [(10, 0), (11, 1), (12, 2)]
        );
        assert_eq!(
            gate.opens(1, ms(3000)),
            Some(vec![(20, 0), (22, 2)]),
            "相机 1 漏了第 2 次，第 3 次仍记序号 2"
        );
        assert_eq!(
            gate.opens(0, ms(3000)).unwrap().last(),
            Some(&(13, 3)),
            "同一台相机 60 ms 后又看到一次开始：新的一次"
        );
        assert_eq!(gate.open_count(), 4);
    }
}

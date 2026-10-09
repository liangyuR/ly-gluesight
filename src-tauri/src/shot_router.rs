//! 帧身份与拍照点路由（P0 步 2）：按相机的触发计数认出一帧是本件哪个拍照点，不看到达顺序。
//! 纯逻辑，不碰 AppHandle。三台相机硬触发、帧交错到达时，按到达顺序定 k 会把帧落错拍照点。
//!
//! - 计数账（[`Ledgers`]）：每台相机本会话见到的最大触发计数。节拍对每一帧都记账，空闲时的游离帧、
//!   手动取的帧也记，否则下一件的基线会落后；
//! - 布防（[`ShotRouter::arm`]）：基线 = max(本会话见到的最大计数, 上一件收尾推算的下限)；本会话还没见过帧时
//!   用开流后的计数起点（模拟 / 回放为 0，海康按 [`Policy::counter_after_open`]，未经台架确认就拒绝布防）；
//! - 路由（[`ShotRouter::route`]）：本机序号 = 触发计数 − 基线，对应这台相机的第几个拍照点；
//! - 收尾（[`ShotRouter::close`]）：下一件的基线下限 = 基线 + PLC 报的本件已发触发数（S7 `cameraNTriggers`），
//!   上一件迟到的帧就落在基线以内，成了旧帧；没有 PLC 计数时按计划数假定。
//!
//! 只靠计数分不出来的情况（都会让这件判 ERR，不会判 OK）：本件中途 Line0 上多一个干扰触发，后面的帧整体后移，
//! 最后一帧成了超计划帧（96）；相机漏收一个触发脉冲，最后一个拍照点等不到帧（91）。没有 PLC 计数时，
//! 上一件实际触发少于计划（中途停了），下一件的基线会偏高，直到故障复位清掉下限（[`Ledgers::clear_floors`]）。
#![cfg_attr(not(test), allow(dead_code))] // 步 3 接进节拍前只有单测用到

use std::collections::BTreeMap;
use std::fmt;

use crate::frame::{CounterSource, Frame};
use crate::judge::fault;
use crate::recipe::ShotSpec;

/// 模拟、回放相机（重新）加载后的触发计数起点：第一帧是 1。
pub const SYNTHETIC_START: u64 = 0;

/// 路由要用的帧身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMeta {
    /// 相机组序号
    pub cam: u8,
    pub session: u64,
    pub source: CounterSource,
    /// 触发计数（来源不是触发计数时是帧计数，不可信）
    pub trigger: u64,
}

impl From<&Frame> for FrameMeta {
    fn from(f: &Frame) -> Self {
        Self { cam: f.cam, session: f.session, source: f.counter, trigger: f.trigger_counter }
    }
}

/// 一台相机在本配方里负责的拍照点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraPlan {
    /// 相机编号
    pub camera: String,
    /// 拍照点序号（配方 shots 的下标），按拍摄顺序：第 i 个是这台相机本件第 i+1 个触发
    pub shots: Vec<usize>,
}

/// 按相机拆开的拍照计划。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// 各拍照点的编号（日志用）
    pub shot_ids: Vec<String>,
    /// 参与相机，按第一次出现的拍照点排序（与 `Recipe::cameras` 一致）
    pub cameras: Vec<CameraPlan>,
}

impl Plan {
    /// 按拍摄顺序给出 (拍照点编号, 相机编号)。
    pub fn new<'a>(shots: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut plan = Plan { shot_ids: Vec::new(), cameras: Vec::new() };
        for (k, (id, camera)) in shots.into_iter().enumerate() {
            plan.shot_ids.push(id.to_string());
            match plan.cameras.iter_mut().find(|c| c.camera == camera) {
                Some(c) => c.shots.push(k),
                None => plan.cameras.push(CameraPlan { camera: camera.to_string(), shots: vec![k] }),
            }
        }
        plan
    }

    pub fn from_shots(shots: &[ShotSpec]) -> Self {
        Self::new(shots.iter().map(|s| (s.id.as_str(), s.camera.as_str())))
    }

    pub fn shot_count(&self) -> usize {
        self.shot_ids.len()
    }

    pub fn camera(&self, camera: &str) -> Option<&CameraPlan> {
        self.cameras.iter().find(|c| c.camera == camera)
    }

    /// 这台相机本件预期的帧数（不参与的相机为 0）。
    pub fn expected(&self, camera: &str) -> usize {
        self.camera(camera).map_or(0, |c| c.shots.len())
    }
}

/// 一台相机的计数账：只记当前会话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    pub session: u64,
    /// 本会话最近一帧的计数来源；还没见过帧为空
    pub source: Option<CounterSource>,
    /// 本会话见到的最大触发计数
    pub last: Option<u64>,
    pub frames: u64,
    /// 本会话里触发计数第一次回退：(之前的最大值, 回退到的值)
    pub backwards: Option<(u64, u64)>,
    /// 上一件收尾推算的下一件基线下限
    pub floor: Option<u64>,
}

impl Ledger {
    fn new(session: u64) -> Self {
        Self { session, source: None, last: None, frames: 0, backwards: None, floor: None }
    }
}

/// 各相机（按相机组序号）的计数账。
#[derive(Clone, Debug, Default)]
pub struct Ledgers {
    cams: BTreeMap<u8, Ledger>,
}

impl Ledgers {
    /// 每一帧都记：检测中、空闲时、手动取的都算。旧会话迟到的帧不记。
    pub fn observe(&mut self, f: &FrameMeta) {
        let l = self.cams.entry(f.cam).or_insert_with(|| Ledger::new(f.session));
        if f.session < l.session {
            return;
        }
        if f.session > l.session {
            *l = Ledger::new(f.session);
        }
        l.frames += 1;
        l.source = Some(f.source);
        match l.last {
            Some(last) if f.trigger < last => {
                l.backwards.get_or_insert((last, f.trigger));
            }
            _ => l.last = Some(f.trigger),
        }
    }

    pub fn get(&self, cam: u8) -> Option<&Ledger> {
        self.cams.get(&cam)
    }

    /// 记下下一件的基线下限；账上已是更新的会话时不记（那次打开的下限没意义了）。
    pub fn set_floor(&mut self, floor: &Floor) {
        let l = self.cams.entry(floor.cam).or_insert_with(|| Ledger::new(floor.session));
        if floor.session > l.session {
            *l = Ledger::new(floor.session);
        }
        if floor.session == l.session {
            l.floor = l.floor.max(Some(floor.baseline));
        }
    }

    /// 故障复位后不再用上一件推算的下限，只按见到的计数布防（复位时线上没有在途的触发）。
    pub fn clear_floors(&mut self) {
        for l in self.cams.values_mut() {
            l.floor = None;
        }
    }
}

/// 哪些计数来源可以认拍照点，以及开流后计数从几开始。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub trusted: Vec<CounterSource>,
    /// 海康相机打开后、第一个触发之前的触发计数（第一帧是它加一）。台架确认前为空：本次打开后没见过帧就拒绝布防
    pub counter_after_open: Option<u64>,
}

impl Policy {
    /// 生产（S7）：只认 Chunk 触发计数。
    pub fn production(counter_after_open: Option<u64>) -> Self {
        Self { trusted: vec![CounterSource::ChunkTrigger], counter_after_open }
    }

    /// 开发演示（Modbus / MC / 模拟）：模拟、回放的计数也认。
    pub fn development(counter_after_open: Option<u64>) -> Self {
        Self { trusted: vec![CounterSource::ChunkTrigger, CounterSource::Synthetic], counter_after_open }
    }

    pub fn trusts(&self, source: CounterSource) -> bool {
        self.trusted.contains(&source)
    }
}

/// 布防时一台相机的现状，由相机组给出：会话号不取自帧，重新打开后可能还没出过帧。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmCam {
    /// 相机组序号（帧里的 cam）
    pub cam: u8,
    /// 相机编号（配方里的 camera）
    pub camera: String,
    /// 此刻的会话号
    pub session: u64,
    /// 出第一帧之前就确定的计数来源：模拟、回放为 Synthetic；海康要看帧里的 Chunk，为空
    pub source: Option<CounterSource>,
}

/// 拒绝布防的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 计划里的相机没有给出现状
    NoCamera { camera: String },
    /// 两个相机编号对到同一个相机组序号
    SharedSlot { camera: String, other: String },
    Untrusted { camera: String, source: CounterSource },
    /// 本次打开后还没见过帧，开流后的计数起点又未知
    NoBaseline { camera: String },
    /// 本会话里触发计数回退过
    Backwards { camera: String, from: u64, to: u64 },
}

impl Refusal {
    pub fn fault(&self) -> u16 {
        fault::DEVICE_LOST
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoCamera { camera } => write!(f, "配方的相机 {camera} 不在相机组里"),
            Refusal::SharedSlot { camera, other } => write!(f, "相机 {camera} 与 {other} 对到了同一台相机"),
            Refusal::Untrusted { camera, source: CounterSource::Synthetic } => {
                write!(f, "相机 {camera} 是模拟 / 回放相机，生产节拍不能用它的计数认拍照点")
            }
            Refusal::Untrusted { camera, source } => {
                write!(f, "相机 {camera} 的帧只有{}，没有触发计数，认不出拍照点（相机需开启 Chunk 触发计数）", source.label())
            }
            Refusal::NoBaseline { camera } => write!(
                f,
                "相机 {camera} 本次打开后还没出过帧，开流后的触发计数起点未经确认，定不了本件第一帧；先让它出一帧（空跑或软触发）再开始"
            ),
            Refusal::Backwards { camera, from, to } => {
                write!(f, "相机 {camera} 的触发计数在本次打开后回退过（{from} → {to}），需要重新打开相机")
            }
        }
    }
}

/// 一帧的去向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// 本件拍照点 `shot`（配方 shots 下标），是这台相机本件第 `ordinal` 帧
    Bound { shot: usize, ordinal: u64 },
    /// 布防前的帧（上一件迟到、空闲游离）：计数不大于基线；`baseline` 为空表示帧来自布防前那次开流。只记日志
    Stale { counter: u64, baseline: Option<u64> },
    /// 同一触发计数又来一帧；`shot` 保留先到的那帧
    Duplicate { shot: usize, ordinal: u64 },
    /// 超出这台相机本件计划的帧数
    Extra { ordinal: u64, planned: usize },
    /// 相机检测中重新打开过，之后的帧对不上拍照点
    SessionChanged { armed: u64, now: u64 },
    UntrustedCounter(CounterSource),
    /// 不是本件计划里的相机
    UnknownCamera,
}

impl Route {
    /// 这一帧让整件判 ERR 时的故障码；绑定、旧帧、别的相机的帧不算本件故障。
    pub fn fault(&self) -> Option<u16> {
        match self {
            Route::Bound { .. } | Route::Stale { .. } | Route::UnknownCamera => None,
            Route::Duplicate { .. } | Route::Extra { .. } => Some(fault::EXTRA_FRAME),
            Route::SessionChanged { .. } | Route::UntrustedCounter(_) => Some(fault::DEVICE_LOST),
        }
    }
}

/// 收尾推算的下一件基线下限。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Floor {
    pub cam: u8,
    pub camera: String,
    pub session: u64,
    pub baseline: u64,
}

#[derive(Clone, Debug)]
struct Armed {
    cam: u8,
    camera: String,
    session: u64,
    baseline: u64,
    shots: Vec<usize>,
    /// 本件见到的本会话最大触发计数（不低于基线）
    high: u64,
}

/// 一件的拍照点路由。
#[derive(Clone, Debug)]
pub struct ShotRouter {
    cams: Vec<Armed>,
    shot_ids: Vec<String>,
    trusted: Vec<CounterSource>,
    /// 各拍照点绑定到的触发计数
    bound: Vec<Option<u64>>,
}

impl ShotRouter {
    /// 布防：给每台参与相机定基线。
    pub fn arm(plan: &Plan, ledgers: &Ledgers, policy: &Policy, cams: &[ArmCam]) -> Result<Self, Refusal> {
        let mut armed: Vec<Armed> = Vec::with_capacity(plan.cameras.len());
        for p in &plan.cameras {
            let camera = p.camera.clone();
            let Some(now) = cams.iter().find(|c| c.camera == p.camera) else { return Err(Refusal::NoCamera { camera }) };
            if let Some(other) = armed.iter().find(|a| a.cam == now.cam) {
                return Err(Refusal::SharedSlot { camera, other: other.camera.clone() });
            }
            // 账上的会话与相机此刻的不同：重新打开后还没见过帧，账上的数不算
            let ledger = ledgers.get(now.cam).filter(|l| l.session == now.session);
            let source = ledger.and_then(|l| l.source).or(now.source);
            if let Some(source) = source.filter(|s| !policy.trusts(*s)) {
                return Err(Refusal::Untrusted { camera, source });
            }
            if let Some((from, to)) = ledger.and_then(|l| l.backwards) {
                return Err(Refusal::Backwards { camera, from, to });
            }
            let baseline = match ledger.and_then(|l| l.last.max(l.floor)) {
                Some(b) => b,
                None if source == Some(CounterSource::Synthetic) => SYNTHETIC_START,
                None => match policy.counter_after_open {
                    Some(b) => b,
                    None => return Err(Refusal::NoBaseline { camera }),
                },
            };
            armed.push(Armed { cam: now.cam, camera, session: now.session, baseline, shots: p.shots.clone(), high: baseline });
        }
        Ok(Self { cams: armed, shot_ids: plan.shot_ids.clone(), trusted: policy.trusted.clone(), bound: vec![None; plan.shot_count()] })
    }

    pub fn route(&mut self, f: &FrameMeta) -> Route {
        let Some(a) = self.cams.iter_mut().find(|a| a.cam == f.cam) else { return Route::UnknownCamera };
        if f.session < a.session {
            return Route::Stale { counter: f.trigger, baseline: None };
        }
        if f.session > a.session {
            return Route::SessionChanged { armed: a.session, now: f.session };
        }
        if !self.trusted.contains(&f.source) {
            return Route::UntrustedCounter(f.source);
        }
        if f.trigger <= a.baseline {
            return Route::Stale { counter: f.trigger, baseline: Some(a.baseline) };
        }
        a.high = a.high.max(f.trigger);
        let ordinal = f.trigger - a.baseline;
        let Some(&shot) = usize::try_from(ordinal - 1).ok().and_then(|i| a.shots.get(i)) else {
            return Route::Extra { ordinal, planned: a.shots.len() };
        };
        if self.bound[shot].is_some() {
            return Route::Duplicate { shot, ordinal };
        }
        self.bound[shot] = Some(f.trigger);
        Route::Bound { shot, ordinal }
    }

    /// 这台相机本件的基线（布防前最后一个触发计数）。
    pub fn baseline(&self, cam: u8) -> Option<u64> {
        self.cams.iter().find(|a| a.cam == cam).map(|a| a.baseline)
    }

    /// 拍照点绑定到的触发计数。
    pub fn bound(&self, shot: usize) -> Option<u64> {
        self.bound.get(shot).copied().flatten()
    }

    /// 还没有帧的拍照点，按拍照点顺序。
    pub fn missing(&self) -> Vec<usize> {
        (0..self.bound.len()).filter(|&k| self.bound[k].is_none()).collect()
    }

    pub fn complete(&self) -> bool {
        self.bound.iter().all(Option::is_some)
    }

    fn camera_name(&self, cam: u8) -> String {
        self.cams.iter().find(|a| a.cam == cam).map_or_else(|| format!("#{}", cam as u32 + 1), |a| a.camera.clone())
    }

    fn shot_name(&self, shot: usize) -> &str {
        self.shot_ids.get(shot).map_or("?", String::as_str)
    }

    /// 给日志看的一句话。
    pub fn describe(&self, f: &FrameMeta, r: &Route) -> String {
        let camera = self.camera_name(f.cam);
        match *r {
            Route::Bound { shot, ordinal } => format!("{} · 相机 {camera} 本件第 {ordinal} 帧（触发计数 {}）", self.shot_name(shot), f.trigger),
            Route::Stale { counter, baseline: Some(b) } => {
                format!("相机 {camera} 触发计数 {counter} 不大于本件基线 {b}：上一件或布防前的帧，不算本件")
            }
            Route::Stale { .. } => format!("相机 {camera} 的帧来自布防前那次打开（会话 {}），不算本件", f.session),
            Route::Duplicate { shot, .. } => format!("相机 {camera} 触发计数 {} 重复：{} 已有帧", f.trigger, self.shot_name(shot)),
            Route::Extra { ordinal, planned } => format!("相机 {camera} 本件第 {ordinal} 帧超出计划的 {planned} 帧"),
            Route::SessionChanged { armed, now } => {
                format!("相机 {camera} 检测中重新打开过（会话 {armed} → {now}），之后的帧对不上拍照点")
            }
            Route::UntrustedCounter(s) => format!("相机 {camera} 的帧只有{}，认不出是哪个拍照点", s.label()),
            Route::UnknownCamera => format!("相机组序号 {} 不在本件计划里", f.cam),
        }
    }

    /// 缺帧时给结果的原因。
    pub fn missing_reason(&self) -> Option<String> {
        let missing = self.missing();
        if missing.is_empty() {
            return None;
        }
        let names: Vec<String> = missing
            .iter()
            .map(|&k| {
                let camera = self.cams.iter().find(|a| a.shots.contains(&k)).map_or("?", |a| a.camera.as_str());
                format!("{}（{camera}）", self.shot_name(k))
            })
            .collect();
        Some(format!("拍照点 {}没有收到帧", names.join("、")))
    }

    /// 本件结束：推算各相机下一件的基线下限，记进计数账，并返回给日志。
    /// `issued`：PLC 报的本件各相机已发触发数（S7 `cameraNTriggers`，按相机编号）。没有时按计划数假定
    /// （模拟节拍按计划触发，丢在传输里的帧也占了计数）；见到的更高计数优先。
    pub fn close(&self, ledgers: &mut Ledgers, issued: Option<&[(&str, u64)]>) -> Vec<Floor> {
        let floors: Vec<Floor> = self
            .cams
            .iter()
            .map(|a| {
                let n = issued.and_then(|list| list.iter().find(|(c, _)| *c == a.camera)).map_or(a.shots.len() as u64, |(_, n)| *n);
                Floor { cam: a.cam, camera: a.camera.clone(), session: a.session, baseline: (a.baseline + n).max(a.high) }
            })
            .collect();
        for floor in &floors {
            ledgers.set_floor(floor);
        }
        floors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CounterSource::{ChunkFrame, ChunkTrigger, SdkFrame, Synthetic};

    /// cam1 → cam2 → cam3 → cam1；相机组序号 0、1、2，会话 11、12、13。
    fn plan4() -> Plan {
        Plan::new([("P1", "cam1"), ("P2", "cam2"), ("P3", "cam3"), ("P4", "cam1")])
    }

    const SESSIONS: [u64; 3] = [11, 12, 13];

    fn arm_cams(source: Option<CounterSource>) -> Vec<ArmCam> {
        (0..3u8).map(|i| ArmCam { cam: i, camera: format!("cam{}", i + 1), session: SESSIONS[i as usize], source }).collect()
    }

    fn frame(cam: u8, trigger: u64) -> FrameMeta {
        FrameMeta { cam, session: SESSIONS[cam as usize], source: ChunkTrigger, trigger }
    }

    /// 布防前各相机空闲时出过一帧：cam1 计数 40、cam2 7、cam3 100（各不相同，串了相机就会露馅）。
    fn primed() -> Ledgers {
        let mut l = Ledgers::default();
        for f in [frame(0, 40), frame(1, 7), frame(2, 100)] {
            l.observe(&f);
        }
        l
    }

    fn prod() -> Policy {
        Policy::production(None)
    }

    fn armed() -> ShotRouter {
        ShotRouter::arm(&plan4(), &primed(), &prod(), &arm_cams(None)).unwrap()
    }

    /// 本件四帧（帧, 真正的拍照点）。
    fn part_frames(base: [u64; 3]) -> [(FrameMeta, usize); 4] {
        [
            (frame(0, base[0] + 1), 0),
            (frame(1, base[1] + 1), 1),
            (frame(2, base[2] + 1), 2),
            (frame(0, base[0] + 2), 3),
        ]
    }

    /// 送进路由；每帧都要么绑到它真正的拍照点，要么不绑。
    fn feed(router: &mut ShotRouter, ledgers: &mut Ledgers, frames: &[(FrameMeta, Option<usize>)]) -> Vec<Route> {
        frames
            .iter()
            .map(|(f, truth)| {
                ledgers.observe(f);
                let r = router.route(f);
                if let Route::Bound { shot, .. } = r {
                    assert_eq!(Some(shot), *truth, "{f:?} 绑错了拍照点：{r:?}");
                }
                r
            })
            .collect()
    }

    #[test]
    fn plan_groups_shots_by_camera_in_first_appearance_order() {
        let plan = plan4();
        let cams: Vec<_> = plan.cameras.iter().map(|c| (c.camera.as_str(), c.shots.clone())).collect();
        assert_eq!(cams, [("cam1", vec![0, 3]), ("cam2", vec![1]), ("cam3", vec![2])]);
        assert_eq!([plan.expected("cam1"), plan.expected("cam2"), plan.expected("cam3"), plan.expected("cam9")], [2, 1, 1, 0]);
        assert_eq!(plan.shot_count(), 4);
        let recipe = crate::recipe::builtin().into_iter().next().unwrap();
        let from = Plan::from_shots(&recipe.shots);
        assert_eq!(from, Plan::new(recipe.shots.iter().map(|s| (s.id.as_str(), s.camera.as_str()))));
        assert_eq!(from.cameras.iter().map(|c| c.camera.clone()).collect::<Vec<_>>(), recipe.cameras());
    }

    #[test]
    fn interleaved_frames_bind_to_their_own_shots_in_any_order() {
        let frames = part_frames([40, 7, 100]);
        // 四帧的全部 24 种到达顺序
        let orders: Vec<[usize; 4]> = (0..256usize)
            .map(|n| [n % 4, n / 4 % 4, n / 16 % 4, n / 64])
            .filter(|o| (0..4).all(|i| o.contains(&i)))
            .collect();
        assert_eq!(orders.len(), 24);
        for order in orders {
            let mut router = armed();
            let mut ledgers = primed();
            let input: Vec<_> = order.iter().map(|&i| (frames[i].0, Some(frames[i].1))).collect();
            let routes = feed(&mut router, &mut ledgers, &input);
            assert!(routes.iter().all(|r| matches!(r, Route::Bound { .. })), "{order:?}: {routes:?}");
            assert!(router.complete() && router.missing_reason().is_none());
        }
    }

    #[test]
    fn missing_first_or_last_frame_of_a_camera_keeps_the_others_in_place() {
        let frames = part_frames([40, 7, 100]);
        // cam1 的第一帧（P1）或最后一帧（P4）丢在传输里：计数照样占了
        for (lost, shot) in [(0, 0), (3, 3)] {
            let mut router = armed();
            let input: Vec<_> = (0..4).filter(|&i| i != lost).map(|i| (frames[i].0, Some(frames[i].1))).collect();
            feed(&mut router, &mut primed(), &input);
            assert_eq!(router.missing(), [shot]);
            assert!(!router.complete());
            assert_eq!(router.missing_reason().unwrap(), format!("拍照点 P{}（cam1）没有收到帧", shot + 1));
        }
    }

    #[test]
    fn missing_middle_frame_of_a_camera_keeps_later_frames_in_place() {
        // cam1 拍三次：P1、P3、P5
        let plan = Plan::new([("P1", "cam1"), ("P2", "cam2"), ("P3", "cam1"), ("P4", "cam3"), ("P5", "cam1")]);
        let all = [(frame(0, 41), 0), (frame(1, 8), 1), (frame(0, 42), 2), (frame(2, 101), 3), (frame(0, 43), 4)];
        for lost in [0, 2, 4] {
            let mut router = ShotRouter::arm(&plan, &primed(), &prod(), &arm_cams(None)).unwrap();
            // 倒着送，后面的帧先到
            let input: Vec<_> = all.iter().rev().filter(|(_, s)| *s != lost).map(|(f, s)| (*f, Some(*s))).collect();
            let routes = feed(&mut router, &mut primed(), &input);
            assert!(routes.iter().all(|r| matches!(r, Route::Bound { .. })));
            assert_eq!(router.missing(), [lost]);
        }
    }

    #[test]
    fn duplicate_counter_keeps_the_first_frame() {
        let mut router = armed();
        assert_eq!(router.route(&frame(1, 8)), Route::Bound { shot: 1, ordinal: 1 });
        let dup = router.route(&frame(1, 8));
        assert_eq!(dup, Route::Duplicate { shot: 1, ordinal: 1 });
        assert_eq!(dup.fault(), Some(fault::EXTRA_FRAME));
        assert_eq!(router.bound(1), Some(8));
        assert!(router.describe(&frame(1, 8), &dup).contains("重复"));
    }

    #[test]
    fn trailing_extra_trigger_is_an_extra_frame() {
        let mut router = armed();
        let mut ledgers = primed();
        let mut input: Vec<_> = part_frames([40, 7, 100]).iter().map(|(f, s)| (*f, Some(*s))).collect();
        input.push((frame(2, 102), None));
        input.push((frame(0, 43), None));
        let routes = feed(&mut router, &mut ledgers, &input);
        assert_eq!(routes[4], Route::Extra { ordinal: 2, planned: 1 });
        assert_eq!(routes[5], Route::Extra { ordinal: 3, planned: 2 });
        assert_eq!(routes[5].fault(), Some(fault::EXTRA_FRAME));
        assert!(router.complete());
    }

    #[test]
    fn extra_trigger_mid_part_cannot_pass() {
        // P1 与 P4 之间 cam1 多了一个干扰触发：计数分不出哪个是干扰，后面的帧整体后移，最后一帧必然超计划
        let mut router = armed();
        let routes: Vec<_> = [frame(0, 41), frame(0, 42), frame(1, 8), frame(2, 101), frame(0, 43)].iter().map(|f| router.route(f)).collect();
        assert!(routes.iter().any(|r| r.fault() == Some(fault::EXTRA_FRAME)), "{routes:?}");
    }

    #[test]
    fn missed_trigger_pulse_leaves_the_last_shot_missing() {
        // cam1 漏收了 P1 的脉冲：P4 的帧拿到计数 41。分不出来，但 P4 一定等不到帧，整件缺帧
        let mut router = armed();
        for f in [frame(0, 41), frame(1, 8), frame(2, 101)] {
            router.route(&f);
        }
        assert_eq!(router.missing(), [3]);
    }

    #[test]
    fn camera_reopened_mid_part_never_binds_new_session_frames() {
        let mut router = armed();
        let mut ledgers = primed();
        assert_eq!(router.route(&frame(0, 41)), Route::Bound { shot: 0, ordinal: 1 });
        // cam1 掉线重连：新会话，计数从头数（42 恰好等于 P4 在旧会话里该有的计数）
        let reopened = FrameMeta { cam: 0, session: 21, source: ChunkTrigger, trigger: 42 };
        ledgers.observe(&reopened);
        let r = router.route(&reopened);
        assert_eq!(r, Route::SessionChanged { armed: 11, now: 21 });
        assert_eq!(r.fault(), Some(fault::DEVICE_LOST));
        assert_eq!(router.bound(3), None);
        // 旧会话迟到的帧不动账
        ledgers.observe(&frame(0, 99));
        assert_eq!(ledgers.get(0).map(|l| (l.session, l.last)), Some((21, Some(42))));
        // 收尾的下限属于旧会话，不记；下一件按新会话见到的计数布防
        router.close(&mut ledgers, None);
        assert_eq!(ledgers.get(0).unwrap().floor, None);
        let mut cams = arm_cams(None);
        cams[0].session = 21;
        let next = ShotRouter::arm(&plan4(), &ledgers, &prod(), &cams).unwrap();
        assert_eq!(next.baseline(0), Some(42));
    }

    #[test]
    fn frames_from_before_a_reopen_are_stale() {
        // 布防前 cam2 刚重新打开（会话 22），旧会话的帧还在通道里
        let mut cams = arm_cams(None);
        cams[1].session = 22;
        let mut ledgers = primed();
        ledgers.observe(&FrameMeta { cam: 1, session: 22, source: ChunkTrigger, trigger: 0 });
        let mut router = ShotRouter::arm(&plan4(), &ledgers, &prod(), &cams).unwrap();
        let old = frame(1, 8);
        let r = router.route(&old);
        assert_eq!(r, Route::Stale { counter: 8, baseline: None });
        assert_eq!(r.fault(), None);
        assert!(router.describe(&old, &r).contains("布防前那次打开"));
        assert_eq!(router.route(&FrameMeta { cam: 1, session: 22, source: ChunkTrigger, trigger: 1 }), Route::Bound { shot: 1, ordinal: 1 });
    }

    /// 上一件 cam1 的最后一帧（计数 42）迟到，下一件布防之后才到。
    fn late_frame_case(issued: Option<&[(&str, u64)]>) {
        let mut ledgers = primed();
        let mut a = ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(None)).unwrap();
        let frames = part_frames([40, 7, 100]);
        let input: Vec<_> = frames[..3].iter().map(|(f, s)| (*f, Some(*s))).collect();
        feed(&mut a, &mut ledgers, &input);
        assert_eq!(a.missing(), [3]);
        let floors = a.close(&mut ledgers, issued);
        assert_eq!(floors.iter().map(|f| f.baseline).collect::<Vec<_>>(), [42, 8, 101]);

        let mut b = ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(None)).unwrap();
        assert_eq!([b.baseline(0), b.baseline(1), b.baseline(2)], [Some(42), Some(8), Some(101)]);
        let late = frame(0, 42);
        let input = [(late, None), (frame(0, 43), Some(0)), (frame(1, 9), Some(1)), (frame(2, 102), Some(2)), (frame(0, 44), Some(3))];
        let routes = feed(&mut b, &mut ledgers, &input);
        assert_eq!(routes[0], Route::Stale { counter: 42, baseline: Some(42) });
        assert!(b.complete());
    }

    #[test]
    fn late_frame_of_previous_part_is_stale_with_plc_counts() {
        late_frame_case(Some(&[("cam1", 2), ("cam2", 1), ("cam3", 1)]));
    }

    #[test]
    fn late_frame_of_previous_part_is_stale_without_plc_counts() {
        late_frame_case(None);
    }

    #[test]
    fn plc_counts_follow_a_part_that_stopped_early() {
        // 上一件 cam1 只发了 1 个触发（中途停了）：按 PLC 计数推下限，下一件的第一帧照样是 P1
        let mut ledgers = primed();
        let mut a = armed();
        feed(&mut a, &mut ledgers, &[(frame(0, 41), Some(0)), (frame(1, 8), Some(1)), (frame(2, 101), Some(2))]);
        a.close(&mut ledgers, Some(&[("cam1", 1), ("cam2", 1), ("cam3", 1)]));
        let mut b = ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(None)).unwrap();
        assert_eq!(b.route(&frame(0, 42)), Route::Bound { shot: 0, ordinal: 1 });
    }

    #[test]
    fn untrusted_source_is_refused_in_production_but_synthetic_is_fine_in_development() {
        let mut ledgers = Ledgers::default();
        for cam in 0..3u8 {
            ledgers.observe(&FrameMeta { source: Synthetic, ..frame(cam, 5) });
        }
        let refusal = ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(Some(Synthetic))).unwrap_err();
        assert_eq!(refusal, Refusal::Untrusted { camera: "cam1".into(), source: Synthetic });
        assert_eq!(refusal.fault(), fault::DEVICE_LOST);
        assert!(refusal.to_string().contains("模拟"));
        let mut router = ShotRouter::arm(&plan4(), &ledgers, &Policy::development(None), &arm_cams(Some(Synthetic))).unwrap();
        assert_eq!(router.route(&FrameMeta { source: Synthetic, ..frame(0, 6) }), Route::Bound { shot: 0, ordinal: 1 });

        // 帧计数、SDK 帧号两种策略都不认
        for source in [ChunkFrame, SdkFrame] {
            let mut ledgers = primed();
            ledgers.observe(&FrameMeta { source, ..frame(2, 101) });
            for policy in [prod(), Policy::development(Some(0))] {
                let refusal = ShotRouter::arm(&plan4(), &ledgers, &policy, &arm_cams(None)).unwrap_err();
                assert_eq!(refusal, Refusal::Untrusted { camera: "cam3".into(), source });
                assert!(refusal.to_string().contains("Chunk 触发计数"));
            }
        }
    }

    #[test]
    fn untrusted_frame_after_arm_is_not_bound() {
        // 本次打开后没见过帧、按开流起点布防的相机，来的帧却没有触发计数
        let mut router = ShotRouter::arm(&plan4(), &Ledgers::default(), &Policy::production(Some(0)), &arm_cams(None)).unwrap();
        let r = router.route(&FrameMeta { source: SdkFrame, ..frame(0, 1) });
        assert_eq!(r, Route::UntrustedCounter(SdkFrame));
        assert_eq!(r.fault(), Some(fault::DEVICE_LOST));
        assert_eq!(router.missing(), [0, 1, 2, 3]);
    }

    #[test]
    fn first_part_after_open_needs_a_known_start() {
        let none = Ledgers::default();
        // 海康：本次打开后还没见过帧，开流起点未确认 → 拒绝
        let refusal = ShotRouter::arm(&plan4(), &none, &prod(), &arm_cams(None)).unwrap_err();
        assert_eq!(refusal, Refusal::NoBaseline { camera: "cam1".into() });
        assert!(refusal.to_string().contains("还没出过帧"));
        // 账上有帧但属于上一次打开：同样不算
        let mut cams = arm_cams(None);
        cams[2].session = 23;
        assert_eq!(ShotRouter::arm(&plan4(), &primed(), &prod(), &cams).unwrap_err(), Refusal::NoBaseline { camera: "cam3".into() });
        // 台架确认开流后计数从 0 起：第一帧是 1
        let mut router = ShotRouter::arm(&plan4(), &none, &Policy::production(Some(0)), &arm_cams(None)).unwrap();
        assert_eq!(router.route(&frame(0, 1)), Route::Bound { shot: 0, ordinal: 1 });
        assert_eq!(router.route(&frame(0, 2)), Route::Bound { shot: 3, ordinal: 2 });
        // 模拟 / 回放：加载后从 0 起是自己定的，不用台架参数
        let mut router = ShotRouter::arm(&plan4(), &none, &Policy::development(None), &arm_cams(Some(Synthetic))).unwrap();
        assert_eq!(router.route(&FrameMeta { source: Synthetic, ..frame(2, 1) }), Route::Bound { shot: 2, ordinal: 1 });
    }

    #[test]
    fn idle_stray_frames_advance_the_ledger() {
        let mut ledgers = primed();
        let mut a = armed();
        let input: Vec<_> = part_frames([40, 7, 100]).iter().map(|(f, s)| (*f, Some(*s))).collect();
        feed(&mut a, &mut ledgers, &input);
        a.close(&mut ledgers, Some(&[("cam1", 2), ("cam2", 1), ("cam3", 1)]));
        // 空闲时 Line0 干扰，cam1 出了两帧
        for f in [frame(0, 43), frame(0, 44)] {
            ledgers.observe(&f);
        }
        assert_eq!(ledgers.get(0).map(|l| (l.last, l.frames, l.floor)), Some((Some(44), 5, Some(42))));
        let mut b = ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(None)).unwrap();
        assert_eq!(b.baseline(0), Some(44));
        assert_eq!(b.route(&frame(0, 45)), Route::Bound { shot: 0, ordinal: 1 });
        assert!(matches!(b.route(&frame(0, 44)), Route::Stale { .. }));
    }

    #[test]
    fn counter_going_backwards_in_a_session_refuses_arm() {
        let mut ledgers = primed();
        ledgers.observe(&frame(0, 3));
        assert_eq!(ledgers.get(0).unwrap().last, Some(40));
        let refusal = ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(None)).unwrap_err();
        assert_eq!(refusal, Refusal::Backwards { camera: "cam1".into(), from: 40, to: 3 });
        assert!(refusal.to_string().contains("40 → 3"));
        // 重新打开（新会话）后恢复
        let mut cams = arm_cams(None);
        cams[0].session = 31;
        ledgers.observe(&FrameMeta { cam: 0, session: 31, source: ChunkTrigger, trigger: 0 });
        assert!(ShotRouter::arm(&plan4(), &ledgers, &prod(), &cams).is_ok());
    }

    #[test]
    fn arm_checks_cameras_and_other_cameras_frames_are_ignored() {
        let mut cams = arm_cams(None);
        cams.remove(1);
        assert_eq!(ShotRouter::arm(&plan4(), &primed(), &prod(), &cams).unwrap_err(), Refusal::NoCamera { camera: "cam2".into() });
        let mut cams = arm_cams(None);
        cams[2].cam = 0;
        assert_eq!(
            ShotRouter::arm(&plan4(), &primed(), &prod(), &cams).unwrap_err(),
            Refusal::SharedSlot { camera: "cam3".into(), other: "cam1".into() }
        );
        let mut router = armed();
        let r = router.route(&FrameMeta { cam: 5, session: 50, source: ChunkTrigger, trigger: 1 });
        assert_eq!((r, r.fault()), (Route::UnknownCamera, None));
    }

    #[test]
    fn floors_only_apply_to_their_own_session_and_can_be_cleared() {
        let mut ledgers = primed();
        let a = armed();
        a.close(&mut ledgers, None);
        assert_eq!(ledgers.get(0).unwrap().floor, Some(42));
        // cam1 重新打开且还没出帧：下限属于旧会话，不用
        let mut cams = arm_cams(None);
        cams[0].session = 41;
        assert_eq!(ShotRouter::arm(&plan4(), &ledgers, &prod(), &cams).unwrap_err(), Refusal::NoBaseline { camera: "cam1".into() });
        // 故障复位清掉下限：只按见到的计数
        ledgers.clear_floors();
        assert_eq!(ShotRouter::arm(&plan4(), &ledgers, &prod(), &arm_cams(None)).unwrap().baseline(0), Some(40));
    }

    /// 简单的线性同余随机数，测试可复现。
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// 故障矩阵随机回归：每件按计划触发（PLC 发的数 = 计划数），帧随机丢在传输里、重复送、迟到到下一件布防之后，
    /// 三台相机的帧随机交错（同一台相机内保持顺序，海康的回调就是这样）；件与件之间随机有游离帧、相机重新打开。
    /// 任何一帧都不能绑到别的拍照点，缺的拍照点正好是没送到的那些。
    #[test]
    fn random_fault_matrix_never_binds_a_frame_to_a_wrong_shot() {
        let plan = Plan::new([("P1", "cam1"), ("P2", "cam2"), ("P3", "cam3"), ("P4", "cam1"), ("P5", "cam2"), ("P6", "cam1")]);
        let cam_of: Vec<u8> = (0..plan.shot_count()).map(|k| plan.cameras.iter().position(|c| c.shots.contains(&k)).unwrap() as u8).collect();
        // 各种去向出现的次数，确认这些故障真的走到了
        let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
        for seed in 1..=40u64 {
            let mut rng = Lcg(seed);
            let mut session = [11u64, 12, 13];
            let mut next_session = 100;
            let mut counter = [40u64, 7, 100];
            let mut ledgers = Ledgers::default();
            for cam in 0..3u8 {
                ledgers.observe(&FrameMeta { cam, session: session[cam as usize], source: ChunkTrigger, trigger: counter[cam as usize] });
            }
            // 迟到到下一件的帧，按相机排队：(帧, 真正的件号与拍照点)
            let mut carry: [Vec<(FrameMeta, Option<(u32, usize)>)>; 3] = Default::default();
            let policy = Policy::production(Some(0));
            for part in 0..60u32 {
                let cams: Vec<_> = (0..3u8).map(|i| ArmCam { cam: i, camera: format!("cam{}", i + 1), session: session[i as usize], source: None }).collect();
                let mut router = ShotRouter::arm(&plan, &ledgers, &policy, &cams).unwrap_or_else(|e| panic!("seed {seed} 件 {part}：{e}"));
                // 上一件迟到的帧排在各相机队列最前
                let mut queues = std::mem::take(&mut carry);
                let mut expect_missing = Vec::new();
                // 一台相机有一帧迟到，它后面的帧也跟着迟到（同一台相机按顺序交付）
                let mut late = [false; 3];
                for k in 0..plan.shot_count() {
                    let cam = cam_of[k] as usize;
                    counter[cam] += 1;
                    let f = FrameMeta { cam: cam as u8, session: session[cam], source: ChunkTrigger, trigger: counter[cam] };
                    let fate = rng.below(10);
                    if fate == 0 {
                        // 丢在传输里：计数占了，帧不来
                        expect_missing.push(k);
                        continue;
                    }
                    late[cam] |= fate == 2;
                    let copies = if fate == 1 { 2 } else { 1 };
                    let to = if late[cam] { &mut carry[cam] } else { &mut queues[cam] };
                    to.extend(std::iter::repeat((f, Some((part, k)))).take(copies));
                    if late[cam] {
                        expect_missing.push(k);
                    }
                }
                // 三台相机的队列随机交错
                while queues.iter().any(|q| !q.is_empty()) {
                    let ready: Vec<usize> = (0..3).filter(|&c| !queues[c].is_empty()).collect();
                    let cam = ready[rng.below(ready.len() as u64) as usize];
                    let (f, truth) = queues[cam].remove(0);
                    ledgers.observe(&f);
                    let r = router.route(&f);
                    if let Route::Bound { shot, .. } = r {
                        assert_eq!(truth, Some((part, shot)), "seed {seed} 件 {part}：{f:?} 绑到了 {}", plan.shot_ids[shot]);
                    }
                    let kind = match r {
                        Route::Bound { .. } => "绑定",
                        Route::Stale { baseline: Some(_), .. } => "上一件迟到",
                        Route::Stale { baseline: None, .. } => "旧会话迟到",
                        Route::Duplicate { .. } => "重复",
                        other => panic!("seed {seed} 件 {part}：{f:?} 不该是 {other:?}"),
                    };
                    *tally.entry(kind).or_default() += 1;
                }
                expect_missing.sort_unstable();
                expect_missing.dedup();
                assert_eq!(router.missing(), expect_missing, "seed {seed} 件 {part}");
                let issued: Vec<(&str, u64)> = plan.cameras.iter().map(|c| (c.camera.as_str(), c.shots.len() as u64)).collect();
                router.close(&mut ledgers, (part % 2 == 0).then_some(issued.as_slice()));
                // 件与件之间：游离帧（布防前就到了、记了账；排在迟到帧后面、布防后才到的游离帧计数分不出来，不在此列），
                // 或相机重新打开（计数从 0 起，还在路上的旧帧照样迟到）
                for cam in 0..3 {
                    match rng.below(12) {
                        0 if carry[cam].is_empty() => {
                            counter[cam] += 1;
                            ledgers.observe(&FrameMeta { cam: cam as u8, session: session[cam], source: ChunkTrigger, trigger: counter[cam] });
                        }
                        1 => {
                            session[cam] = next_session;
                            next_session += 1;
                            counter[cam] = 0;
                        }
                        _ => {}
                    }
                }
            }
        }
        assert_eq!(tally.len(), 4, "{tally:?}");
    }
}

//! 帧身份与拍照点路由（P0 步 2）：按相机的触发计数认出一帧是本件哪个拍照点，不看到达顺序。
//! 纯逻辑，不碰 AppHandle。三台相机硬触发、帧交错到达时，按到达顺序定 k 会把帧落错拍照点。
//!
//! - 计数账（[`Ledgers`]）：每台相机本会话见到的最大触发计数。节拍对每一帧都记账，空闲时的游离帧、
//!   手动取的帧也记，否则下一件的基线会落后；
//! - 布防（[`ShotRouter::arm`]）：基线 = max(本会话见到的最大计数, 上一件收尾推算的下限)；本会话还没见过帧时
//!   用开流后的计数起点（模拟 / 样本集为 0，海康按 [`Policy::counter_after_open`]，未经台架确认就拒绝布防）；
//! - 路由（[`ShotRouter::route`]）：本机序号 = 触发计数 − 基线，对应这台相机的第几个拍照点；
//! - 收尾（[`ShotRouter::close`]）：下一件的基线下限 = 基线 + PLC 报的本件已发触发数（S7 `cameraNTriggers`：
//!   partEnd 核对过的，或提前判 ERR 的件在 PLC 确认结果时读到的），上一件迟到的帧就落在基线以内，成了旧帧；
//!   没有 PLC 计数时按计划数假定。
//!
//! 只靠计数分不出来的情况（都会让这件判 ERR，不会判 OK）：本件中途 Line0 上多一个干扰触发，后面的帧整体后移，
//! 最后一帧成了超计划帧（96）；相机漏收一个触发脉冲，最后一个拍照点等不到帧（91），之后每件的基线都偏高一格：
//! 第一帧被当成上一件的迟到帧、最后一个拍照点等不到帧（91，原因写明"触发计数错位"）。这与"上一件最后一帧
//! 迟到到下一件"在计数上一模一样，自动下调基线会在后一种情况下把帧整体错绑、凑齐一件，所以不自动改，
//! 等故障复位清掉下限（[`Ledgers::clear_floors`]）后按实际见到的计数重新对齐。

use std::collections::BTreeMap;
use std::fmt;

use crate::frame::{CounterSource, Frame};
use crate::judge::fault;
use crate::recipe::ShotSpec;

/// 模拟、样本集相机（重新）加载后的触发计数起点：第一帧是 1。
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
    pub sources: Vec<CounterSource>,
    /// 本会话见到的最大触发计数
    pub last: Option<u64>,
    pub frames: u64,
    /// 本会话里触发计数第一次回退：(之前的最大值, 回退到的值)
    pub backwards: Option<(u64, u64)>,
    pub duplicate: Option<u64>,
    /// 上一件收尾推算的下一件基线下限
    pub floor: Option<u64>,
    seen: u128,
}

impl Ledger {
    fn new(session: u64) -> Self {
        Self { session, source: None, sources: Vec::new(), last: None, frames: 0,
            backwards: None, duplicate: None, floor: None, seen: 0 }
    }

    pub fn snapshot(session: u64, source: CounterSource, trigger: u64) -> Self {
        Self { source: Some(source), sources: vec![source], last: Some(trigger), seen: 1, ..Self::new(session) }
    }

    fn note_backwards(&mut self, from: u64, to: u64) {
        if self.backwards.is_none_or(|(old_from, old_to)| (to, from) > (old_to, old_from)) {
            self.backwards = Some((from, to));
        }
    }

    fn seen_at(&self, last: u64) -> u128 {
        self.last.and_then(|old| last.checked_sub(old))
            .and_then(|shift| u32::try_from(shift).ok())
            .and_then(|shift| self.seen.checked_shl(shift)).unwrap_or(0)
    }
}

/// 各相机（按相机组序号）的计数账。
#[derive(Clone, Debug, Default)]
pub struct Ledgers {
    cams: BTreeMap<u8, Ledger>,
}

impl Ledgers {
    /// 每一帧都记：检测中、空闲时、手动取的都算。旧会话迟到的帧不记。
    pub fn observe(&mut self, f: &FrameMeta) -> bool {
        let l = self.cams.entry(f.cam).or_insert_with(|| Ledger::new(f.session));
        if f.session < l.session {
            return false;
        }
        if f.session > l.session {
            *l = Ledger::new(f.session);
        }
        l.frames += 1;
        l.source = Some(f.source);
        if !l.sources.contains(&f.source) { l.sources.push(f.source); }
        let advanced = l.last.is_none_or(|last| f.trigger > last);
        match l.last {
            Some(last) if f.trigger <= last => {
                if let Some(bit) = u32::try_from(last - f.trigger).ok().and_then(|shift| 1u128.checked_shl(shift)) {
                    if l.seen & bit != 0 && l.floor.is_none_or(|floor| f.trigger > floor) {
                        l.duplicate = l.duplicate.max(Some(f.trigger));
                    }
                    l.seen |= bit;
                }
                if f.trigger < last && l.floor.is_none_or(|floor| f.trigger > floor) {
                    l.note_backwards(last, f.trigger);
                }
            }
            _ => {
                l.seen = l.seen_at(f.trigger) | 1;
                l.last = Some(f.trigger);
            }
        }
        advanced
    }

    pub fn get(&self, cam: u8) -> Option<&Ledger> {
        self.cams.get(&cam)
    }

    pub fn merge(&mut self, cam: u8, observed: &Ledger) {
        let current = self.cams.entry(cam).or_insert_with(|| Ledger::new(observed.session));
        if observed.session < current.session { return; }
        if observed.session > current.session { *current = Ledger::new(observed.session); }
        let last = current.last.max(observed.last);
        current.seen = last.map_or(0, |last| current.seen_at(last) | observed.seen_at(last));
        current.last = last;
        current.source = observed.source.or(current.source);
        for source in observed.sources.iter().copied().chain(observed.source) {
            if !current.sources.contains(&source) { current.sources.push(source); }
        }
        current.frames = current.frames.max(observed.frames);
        if let Some((from, to)) = observed.backwards.filter(|(_, to)| current.floor.is_none_or(|floor| *to > floor)) {
            current.note_backwards(from, to);
        }
        current.duplicate = current.duplicate.max(observed.duplicate.filter(|counter| current.floor.is_none_or(|floor| *counter > floor)));
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

    /// 离线产线（进程内 S7 服务）：模拟、样本集相机的计数也认；接真实 PLC 时不用。
    pub fn offline(counter_after_open: Option<u64>) -> Self {
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
    /// 出第一帧之前就确定的计数来源：模拟、样本集为 Synthetic；海康要看帧里的 Chunk，为空
    pub source: Option<CounterSource>,
    pub counter_after_open: Option<u64>,
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
                write!(f, "相机 {camera} 是模拟 / 样本集相机，生产节拍不能用它的计数认拍照点")
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
    /// 布防时本会话见到的最大触发计数
    seen: Option<u64>,
    /// 布防后才到、布防前没见过、却不大于基线的计数（上一件迟到帧，或基线偏高）
    stale: Vec<u64>,
    /// 超出本件计划的计数
    extra: Vec<u64>,
}

fn note(list: &mut Vec<u64>, counter: u64) {
    if let Err(i) = list.binary_search(&counter) { list.insert(i, counter); }
}

/// 递增计数写成区间：41–43、45。
fn runs(counters: &[u64]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < counters.len() {
        let mut j = i;
        while j + 1 < counters.len() && counters[j + 1] == counters[j] + 1 { j += 1; }
        out.push(if j == i { counters[i].to_string() } else { format!("{}–{}", counters[i], counters[j]) });
        i = j + 1;
    }
    out.join("、")
}

impl Armed {
    /// 本件应收的触发计数：基线 + 1 ..= 基线 + 计划数。
    fn expected(&self) -> String {
        let (first, last) = (self.baseline + 1, self.baseline + self.shots.len() as u64);
        if first == last { first.to_string() } else { format!("{first}–{last}") }
    }
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
            let untrusted = ledger.into_iter().flat_map(|l| l.sources.iter().copied())
                .chain(source).find(|s| !policy.trusts(*s));
            if let Some(source) = untrusted {
                return Err(Refusal::Untrusted { camera, source });
            }
            if let Some((from, to)) = ledger.and_then(|l| l.backwards) {
                return Err(Refusal::Backwards { camera, from, to });
            }
            let baseline = match ledger.and_then(|l| l.last.max(l.floor)) {
                Some(b) => b,
                None if source == Some(CounterSource::Synthetic) => SYNTHETIC_START,
                None => match now.counter_after_open.or(policy.counter_after_open) {
                    Some(b) => b,
                    None => return Err(Refusal::NoBaseline { camera }),
                },
            };
            armed.push(Armed { cam: now.cam, camera, session: now.session, baseline, shots: p.shots.clone(), high: baseline,
                seen: ledger.and_then(|l| l.last), stale: Vec::new(), extra: Vec::new() });
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
            if a.seen.is_none_or(|seen| f.trigger > seen) { note(&mut a.stale, f.trigger); }
            return Route::Stale { counter: f.trigger, baseline: Some(a.baseline) };
        }
        a.high = a.high.max(f.trigger);
        let ordinal = f.trigger - a.baseline;
        let Some(&shot) = usize::try_from(ordinal - 1).ok().and_then(|i| a.shots.get(i)) else {
            note(&mut a.extra, f.trigger);
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

    pub fn check_sessions(&self, cameras: &[ArmCam]) -> Result<(), String> {
        for armed in &self.cams {
            let now = cameras.iter().find(|camera| camera.cam == armed.cam && camera.camera == armed.camera)
                .ok_or_else(|| format!("相机 {} 已不在本件相机组中", armed.camera))?;
            if now.session != armed.session {
                return Err(format!("相机 {} 检测中已重连（会话 {} → {}）", armed.camera, armed.session, now.session));
            }
        }
        Ok(())
    }

    pub fn check_observed(&self, cam: u8, observed: &Ledger) -> Result<(), (u16, String)> {
        let Some(armed) = self.cams.iter().find(|armed| armed.cam == cam) else { return Ok(()) };
        let fail = |code, reason: String| Err((code, format!("相机 {} {reason}", armed.camera)));
        if observed.session != armed.session {
            return fail(fault::DEVICE_LOST, format!("检测中会话不一致（{} → {}）", armed.session, observed.session));
        }
        if let Some(source) = observed.sources.iter().copied().chain(observed.source).find(|source| !self.trusted.contains(source)) {
            return fail(fault::DEVICE_LOST, format!("本会话曾出现不可信的{}，需要重新打开相机", source.label()));
        }
        if let Some(counter) = observed.duplicate.filter(|counter| *counter > armed.baseline) {
            return fail(fault::EXTRA_FRAME, format!("本件触发计数 {counter} 重复，不能确认拍照计划完整"));
        }
        let limit = armed.baseline.saturating_add(armed.shots.len() as u64);
        if let Some(last) = observed.last.filter(|last| *last > limit) {
            return fail(fault::EXTRA_FRAME, format!(
                "触发计数错位：本件应收 {}，回调已到 {last}，超过本件计划截止计数 {limit} 共 {} 个（多为本件多了干扰触发），拍照点归属不可信",
                armed.expected(), last - limit));
        }
        if let Some((from, to)) = observed.backwards.filter(|(_, to)| *to > armed.baseline) {
            return fail(fault::DEVICE_LOST, format!("本件触发计数回退（{from} → {to}），需要重新打开相机"));
        }
        Ok(())
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

    pub fn triggers(&self) -> u64 {
        self.cams.iter().map(|a| a.high.saturating_sub(a.baseline)).sum()
    }

    fn camera_name(&self, cam: u8) -> String {
        self.cams.iter().find(|a| a.cam == cam).map_or_else(|| format!("#{}", cam as u32 + 1), |a| a.camera.clone())
    }

    fn shot_name(&self, shot: usize) -> &str {
        self.shot_ids.get(shot).map_or("?", String::as_str)
    }

    /// 给日志看的一句话；会让整件判 ERR 的去向同时是结果原因，写明应收与实际收到的计数。
    pub fn describe(&self, f: &FrameMeta, r: &Route) -> String {
        let camera = self.camera_name(f.cam);
        let expected = self.cams.iter().find(|a| a.cam == f.cam).map_or_else(|| "?".into(), Armed::expected);
        match *r {
            Route::Bound { shot, ordinal } => format!("{} · 相机 {camera} 本件第 {ordinal} 帧（触发计数 {}）", self.shot_name(shot), f.trigger),
            Route::Stale { counter, baseline: Some(b) } => {
                format!("相机 {camera} 触发计数 {counter} 不大于本件基线 {b}：上一件或布防前的帧，不算本件（本件若随后缺帧，即触发计数错位）")
            }
            Route::Stale { .. } => format!("相机 {camera} 的帧来自布防前那次打开（会话 {}），不算本件", f.session),
            Route::Duplicate { shot, .. } => format!("相机 {camera} 触发计数 {} 重复：{} 已有帧，同一计数又来一帧，拍照点归属不可信",
                f.trigger, self.shot_name(shot)),
            Route::Extra { ordinal, planned } => format!(
                "相机 {camera} 触发计数错位：本件应收 {expected}，收到 {}，超出计划 {} 个（本件第 {ordinal} 帧，计划 {planned} 帧；多为本件多了干扰触发），拍照点归属不可信",
                f.trigger, ordinal - planned as u64),
            Route::SessionChanged { armed, now } => {
                format!("相机 {camera} 检测中重新打开过（会话 {armed} → {now}），之后的帧对不上拍照点")
            }
            Route::UntrustedCounter(s) => format!("相机 {camera} 的帧只有{}，认不出是哪个拍照点", s.label()),
            Route::UnknownCamera => format!("相机组序号 {} 不在本件计划里", f.cam),
        }
    }

    /// 缺帧时给结果的原因：缺哪些拍照点，以及各相机应收、实际收到的触发计数和可能的原因。
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
        let notes: Vec<String> = self.cams.iter().filter_map(|a| self.counter_note(a)).collect();
        Some(format!("拍照点 {}没有收到帧：{}", names.join("、"), notes.join("；")))
    }

    /// 一台缺帧相机的计数对账。
    fn counter_note(&self, a: &Armed) -> Option<String> {
        let missing: Vec<u64> = (1..=a.shots.len() as u64).filter(|i| self.bound[a.shots[*i as usize - 1]].is_none())
            .map(|i| a.baseline + i).collect();
        if missing.is_empty() {
            return None;
        }
        let bound: Vec<u64> = a.shots.iter().filter_map(|&k| self.bound[k]).collect();
        let mut received: Vec<u64> = bound.iter().chain(&a.stale).chain(&a.extra).copied().collect();
        received.sort_unstable();
        received.dedup();
        let received = if received.is_empty() { "无".to_string() } else { runs(&received) };
        let (camera, expected) = (&a.camera, a.expected());
        if !a.stale.is_empty() {
            return Some(format!(
                "相机 {camera} 触发计数错位：本件应收 {expected}，实际收到 {received}；{} 不大于本件基线 {}，被当作上一件的迟到帧。多为上一件相机漏收了触发脉冲，之后每件都会这样判 ERR；检查触发线路后执行故障复位，按实际计数重新对齐",
                runs(&a.stale), a.baseline));
        }
        if !a.extra.is_empty() {
            return Some(format!("相机 {camera} 触发计数错位：本件应收 {expected}，实际收到 {received}；{} 超出本件计划（多为本件多了干扰触发）",
                runs(&a.extra)));
        }
        let last = bound.iter().max().copied();
        let skipped: Vec<u64> = missing.iter().copied().filter(|c| last.is_some_and(|l| *c < l)).collect();
        let trailing: Vec<u64> = missing.iter().copied().filter(|c| last.is_none_or(|l| *c > l)).collect();
        let mut note = format!("相机 {camera} {}：本件应收 {expected}，实际收到 {received}",
            if skipped.is_empty() { "缺帧" } else { "触发计数跳号" });
        if !skipped.is_empty() {
            note += &format!("；{} 相机已计数但帧没送到（传输丢帧或帧通道满）", runs(&skipped));
        }
        if !trailing.is_empty() {
            note += &format!("；{} 未到（帧丢在传输里，或相机漏收了触发脉冲，后者下一件会报触发计数错位）", runs(&trailing));
        }
        Some(note)
    }

    /// 本件结束：推算各相机下一件的基线下限，记进计数账，并返回给日志。
    /// `issued`：PLC 报的本件各相机已发触发数（S7 `cameraNTriggers`，按相机编号）。没有时按计划数假定
    /// （离线产线按计划触发，丢在传输里的帧也占了计数）；见到的更高计数优先。
    pub fn close(&self, ledgers: &mut Ledgers, issued: Option<&[(&str, u64)]>) -> Vec<Floor> {
        let floors: Vec<Floor> = self
            .cams
            .iter()
            .map(|a| {
                let n = issued.and_then(|list| list.iter().find(|(c, _)| *c == a.camera)).map_or(a.shots.len() as u64, |(_, n)| *n);
                Floor { cam: a.cam, camera: a.camera.clone(), session: a.session, baseline: a.baseline.saturating_add(n).max(a.high) }
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
    use CounterSource::{ChunkTrigger, SdkFrame, Synthetic};

    fn plan() -> Plan {
        Plan::new([("P1", "device1"), ("P2", "device1"), ("P3", "device1"), ("P4", "device1")])
    }

    fn camera() -> ArmCam {
        ArmCam { cam: 0, camera: "device1".into(), session: 11, source: Some(ChunkTrigger), counter_after_open: None }
    }

    fn frame(trigger: u64) -> FrameMeta {
        FrameMeta { cam: 0, session: 11, source: ChunkTrigger, trigger }
    }

    fn primed() -> Ledgers {
        let mut ledgers = Ledgers::default();
        ledgers.observe(&frame(40));
        ledgers
    }

    fn armed() -> ShotRouter {
        ShotRouter::arm(&plan(), &primed(), &Policy::production(None), &[camera()]).unwrap()
    }

    #[test]
    fn one_device_frames_bind_by_counter_instead_of_arrival_order() {
        let mut router = armed();
        assert_eq!(plan().cameras.len(), 1);
        for (counter, shot) in [(43, 2), (41, 0), (44, 3), (42, 1)] {
            assert_eq!(router.route(&frame(counter)), Route::Bound { shot, ordinal: counter - 40 });
        }
        assert!(router.complete());
        assert!(router.missing_reason().is_none());
    }

    #[test]
    fn a_missing_composite_frame_does_not_shift_later_shots() {
        let mut router = armed();
        assert_eq!(router.route(&frame(41)), Route::Bound { shot: 0, ordinal: 1 });
        assert_eq!(router.route(&frame(43)), Route::Bound { shot: 2, ordinal: 3 });
        assert_eq!(router.route(&frame(44)), Route::Bound { shot: 3, ordinal: 4 });
        assert_eq!(router.missing(), [1]);
        assert!(!router.complete());
    }

    #[test]
    fn duplicate_counter_preserves_the_first_frame_and_cannot_complete_a_part() {
        let mut router = armed();
        router.route(&frame(41));
        let duplicate = router.route(&frame(41));
        assert_eq!(duplicate, Route::Duplicate { shot: 0, ordinal: 1 });
        assert_eq!(duplicate.fault(), Some(fault::EXTRA_FRAME));
        assert_eq!(router.bound(0), Some(41));
        assert_eq!(router.missing(), [1, 2, 3]);
    }

    #[test]
    fn an_extra_callback_is_rejected_before_the_actor_receives_it() {
        let router = armed();
        let mut callbacks = primed();
        callbacks.observe(&frame(45));
        assert_eq!(router.check_observed(0, callbacks.get(0).unwrap()).unwrap_err().0, fault::EXTRA_FRAME);
        assert_eq!(router.bound(0), None);
    }

    #[test]
    fn callbacks_reject_nonconsecutive_duplicates() {
        let router = armed();
        let mut callbacks = primed();
        for counter in [41, 42, 41] { callbacks.observe(&frame(counter)); }
        assert_eq!(router.check_observed(0, callbacks.get(0).unwrap()).unwrap_err().0, fault::EXTRA_FRAME);
    }

    #[test]
    fn reconnecting_during_a_part_rejects_new_and_stale_session_frames() {
        let mut router = armed();
        router.route(&frame(41));
        let reopened = FrameMeta { session: 12, trigger: 42, ..frame(42) };
        assert_eq!(router.route(&reopened), Route::SessionChanged { armed: 11, now: 12 });
        assert_eq!(router.bound(1), None);
        let mut ledgers = primed();
        ledgers.observe(&reopened);
        assert!(!ledgers.observe(&frame(99)));
        let next_camera = ArmCam { session: 12, ..camera() };
        let mut next = ShotRouter::arm(&plan(), &ledgers, &Policy::production(None), &[next_camera]).unwrap();
        assert_eq!(next.route(&frame(43)), Route::Stale { counter: 43, baseline: None });
        assert_eq!(next.route(&FrameMeta { session: 12, trigger: 43, ..frame(43) }), Route::Bound { shot: 0, ordinal: 1 });
    }

    #[test]
    fn late_frames_of_the_previous_part_cannot_satisfy_the_next_part() {
        let mut ledgers = primed();
        let mut first = armed();
        first.route(&frame(41));
        ledgers.observe(&frame(41));
        first.close(&mut ledgers, Some(&[("device1", 4)]));
        let mut next = ShotRouter::arm(&plan(), &ledgers, &Policy::production(None), &[camera()]).unwrap();
        assert_eq!(next.baseline(0), Some(44));
        assert_eq!(next.route(&frame(44)), Route::Stale { counter: 44, baseline: Some(44) });
        assert_eq!(next.route(&frame(45)), Route::Bound { shot: 0, ordinal: 1 });
    }

    #[test]
    fn production_requires_a_confirmed_baseline_and_trusted_trigger_counter() {
        assert!(matches!(ShotRouter::arm(&plan(), &Ledgers::default(), &Policy::production(None), &[camera()]), Err(Refusal::NoBaseline { .. })));
        let mut router = ShotRouter::arm(&plan(), &Ledgers::default(), &Policy::production(Some(0)), &[camera()]).unwrap();
        let untrusted = router.route(&FrameMeta { source: SdkFrame, ..frame(1) });
        assert_eq!(untrusted, Route::UntrustedCounter(SdkFrame));
        assert_eq!(untrusted.fault(), Some(fault::DEVICE_LOST));
        let synthetic = ArmCam { source: Some(Synthetic), ..camera() };
        assert!(matches!(ShotRouter::arm(&plan(), &Ledgers::default(), &Policy::production(Some(0)), &[synthetic.clone()]), Err(Refusal::Untrusted { .. })));
        let mut offline = ShotRouter::arm(&plan(), &Ledgers::default(), &Policy::offline(None), &[synthetic]).unwrap();
        assert_eq!(offline.route(&FrameMeta { source: Synthetic, ..frame(1) }), Route::Bound { shot: 0, ordinal: 1 });
    }

    #[test]
    fn backwards_counters_refuse_arming_until_a_new_session() {
        let mut ledgers = primed();
        ledgers.observe(&frame(39));
        assert!(matches!(ShotRouter::arm(&plan(), &ledgers, &Policy::production(None), &[camera()]), Err(Refusal::Backwards { .. })));
        ledgers.observe(&FrameMeta { session: 12, trigger: 0, ..frame(0) });
        assert!(ShotRouter::arm(&plan(), &ledgers, &Policy::production(None), &[ArmCam { session: 12, ..camera() }]).is_ok());
    }

    #[test]
    fn a_missed_trigger_keeps_the_inferred_floor_until_explicit_reset() {
        let mut ledgers = primed();
        ledgers.set_floor(&Floor { cam: 0, camera: "device1".into(), session: 11, baseline: 41 });
        let mut shifted = ShotRouter::arm(&plan(), &ledgers, &Policy::production(None), &[camera()]).unwrap();
        assert!(matches!(shifted.route(&frame(41)), Route::Stale { .. }));
        assert_eq!(shifted.route(&frame(42)), Route::Bound { shot: 0, ordinal: 1 });
        assert_eq!(shifted.route(&frame(43)), Route::Bound { shot: 1, ordinal: 2 });
        assert_eq!(shifted.route(&frame(44)), Route::Bound { shot: 2, ordinal: 3 });
        assert_eq!(shifted.missing(), [3]);
        assert!(!shifted.complete());
        ledgers.clear_floors();
        let reset = ShotRouter::arm(&plan(), &ledgers, &Policy::production(None), &[camera()]).unwrap();
        assert_eq!(reset.baseline(0), Some(40));
    }
}

//! 帧身份与计数账：纯逻辑，不碰 AppHandle。
//!
//! - 计数账（[`Ledgers`]）：每台相机本会话见到的最大触发计数与 SDK 帧计数。节拍对每一帧都记账，空闲时的游离帧、
//!   手动取的帧也记，否则下一件的基线会落后；
//! - 接件（[`arm_frames`]）：按 SDK 帧计数定每台计划相机的基线，帧归属由 `frame_group::Grouper` 按帧号配组；
//!   本会话没见过帧、开流计数起点又未知时照常接件，由本件首帧定基线；
//! - 收尾：下一件的帧计数下限记在 [`Ledger::frame_floor`]，= max(本件基线, 这台相机本件见到的最高帧计数)，
//!   PLC 报的触发数与计划数都不参与（D-SB-7）。

use std::collections::BTreeMap;
use std::fmt;

use crate::frame::{CounterSource, Frame};
use crate::judge::fault;

/// 模拟、样本集相机（重新）加载后的触发计数起点：第一帧是 1。
pub const SYNTHETIC_START: u64 = 0;

/// 计数账要用的帧身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMeta {
    /// 相机组序号
    pub cam: u8,
    pub session: u64,
    pub source: CounterSource,
    /// 触发计数（来源不是触发计数时是帧计数）
    pub trigger: u64,
    /// SDK 帧计数（`Frame::frame_counter`）：按它配组（D-SB-10）
    pub frame: u64,
}

impl From<&Frame> for FrameMeta {
    fn from(f: &Frame) -> Self {
        Self { cam: f.cam, session: f.session, source: f.counter, trigger: f.trigger_counter, frame: f.frame_counter }
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
    /// 本会话见到的最大 SDK 帧计数：接件与示教采集的配组基线（D-SB-10）
    pub last_frame: Option<u64>,
    pub frames: u64,
    /// 上一件收尾推算的下一件帧计数基线下限（D-SB-7）
    pub frame_floor: Option<u64>,
}

impl Ledger {
    fn new(session: u64) -> Self {
        Self {
            session,
            source: None,
            sources: Vec::new(),
            last: None,
            last_frame: None,
            frames: 0,
            frame_floor: None,
        }
    }

    /// 模拟 / 样本集相机的现状：帧计数与触发计数相同。
    pub fn snapshot(session: u64, source: CounterSource, trigger: u64) -> Self {
        Self {
            source: Some(source),
            sources: vec![source],
            last: Some(trigger),
            last_frame: Some(trigger),
            ..Self::new(session)
        }
    }
}

/// 各相机（按相机组序号）的计数账。
#[derive(Clone, Debug, Default)]
pub struct Ledgers {
    cams: BTreeMap<u8, Ledger>,
}

impl Ledgers {
    /// 每一帧都记：检测中、空闲时、手动取的都算。旧会话迟到的帧不记。触发计数比本会话见过的都大时返回 true。
    pub fn observe(&mut self, f: &FrameMeta) -> bool {
        let l = self.cams.entry(f.cam).or_insert_with(|| Ledger::new(f.session));
        if f.session < l.session {
            return false;
        }
        if f.session > l.session {
            *l = Ledger::new(f.session);
        }
        l.frames += 1;
        l.last_frame = l.last_frame.max(Some(f.frame));
        l.source = Some(f.source);
        if !l.sources.contains(&f.source) { l.sources.push(f.source); }
        let advanced = l.last.is_none_or(|last| f.trigger > last);
        if advanced {
            l.last = Some(f.trigger);
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
        current.last = current.last.max(observed.last);
        current.last_frame = current.last_frame.max(observed.last_frame);
        current.source = observed.source.or(current.source);
        for source in observed.sources.iter().copied().chain(observed.source) {
            if !current.sources.contains(&source) { current.sources.push(source); }
        }
        current.frames = current.frames.max(observed.frames);
    }

    /// 收尾：记下下一件的帧计数基线下限（`Floor::baseline` 是帧计数）；账上已是更新的会话时不记。
    pub fn set_frame_floor(&mut self, floor: &Floor) {
        let l = self.cams.entry(floor.cam).or_insert_with(|| Ledger::new(floor.session));
        if floor.session > l.session {
            *l = Ledger::new(floor.session);
        }
        if floor.session == l.session {
            l.frame_floor = l.frame_floor.max(Some(floor.baseline));
        }
    }

    /// 故障复位后不再用上一件推算的下限，只按见到的计数接件（复位时线上没有在途的触发）。
    pub fn clear_floors(&mut self) {
        for l in self.cams.values_mut() {
            l.frame_floor = None;
        }
    }
}

/// 哪些计数来源可信，以及开流后计数从几开始。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub trusted: Vec<CounterSource>,
    /// 海康相机打开后、第一个触发之前的计数（第一帧是它加一）。台架确认前为空：由首帧定基线
    pub counter_after_open: Option<u64>,
}

impl Policy {
    /// 生产（D-SB-10）：按 SDK 帧计数配组，海康三种来源（Chunk 触发计数、Chunk 帧计数、SDK 帧号）的帧计数都认。
    pub fn burst_production(counter_after_open: Option<u64>) -> Self {
        Self { trusted: vec![CounterSource::ChunkTrigger, CounterSource::ChunkFrame, CounterSource::SdkFrame], counter_after_open }
    }

    /// 离线产线：另外认模拟、样本集相机的计数。
    pub fn burst_offline(counter_after_open: Option<u64>) -> Self {
        Self { trusted: vec![CounterSource::ChunkTrigger, CounterSource::ChunkFrame, CounterSource::SdkFrame, CounterSource::Synthetic], counter_after_open }
    }

    pub fn trusts(&self, source: CounterSource) -> bool {
        self.trusted.contains(&source)
    }
}

/// 接件时一台相机的现状，由相机组给出：会话号不取自帧，重新打开后可能还没出过帧。
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

/// 本件不能检测的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 计划里的相机没有给出现状
    NoCamera { camera: String },
    /// 两个相机编号对到同一个相机组序号
    SharedSlot { camera: String, other: String },
    Untrusted { camera: String, source: CounterSource },
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
            Refusal::SharedSlot { camera, other } => {
                write!(f, "相机 {camera} 与 {other} 对到了同一台相机")
            }
            Refusal::Untrusted {
                camera,
                source: CounterSource::Synthetic,
            } => {
                write!(
                    f,
                    "相机 {camera} 是模拟 / 样本集相机，生产节拍不能用它的计数配组"
                )
            }
            Refusal::Untrusted { camera, source } => {
                write!(f, "相机 {camera} 的帧只有{}，帧号不可信", source.label())
            }
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

/// 接件时一台计划相机的帧计数基线：frame_counter ≤ baseline 的帧属于上一件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameBaseline {
    /// 相机组序号（帧里的 cam）
    pub cam: u8,
    pub camera: String,
    /// 接件时的会话号
    pub session: u64,
    /// 本会话还没见过帧、开流计数起点又未知时为空：本件这台相机的第一帧定基线（首帧计数 − 1）
    pub baseline: Option<u64>,
}

/// 接件（D-SB-7、D-SB-10）：按计划相机顺序给出每台相机的帧计数基线 =
/// max(本会话见到的最大 SDK 帧计数, 上一件收尾推算的帧计数下限)；本会话还没见过帧时
/// 取开流后的计数起点（模拟 / 样本集为 0，海康按 `counter_after_open`），仍未知时留空，由本件首帧定基线：
/// Line0 高电平门控下软件取不了帧，而本会话接件前没有帧，接件后这台相机的第一帧必属本件。
/// 本会话出现过 policy 不信任的计数来源时拒绝。
pub fn arm_frames(cameras: &[String], ledgers: &Ledgers, policy: &Policy, cams: &[ArmCam]) -> Result<Vec<FrameBaseline>, Refusal> {
    let mut armed: Vec<FrameBaseline> = Vec::with_capacity(cameras.len());
    for id in cameras {
        let camera = id.clone();
        let Some(now) = cams.iter().find(|c| &c.camera == id) else { return Err(Refusal::NoCamera { camera }) };
        if let Some(other) = armed.iter().find(|a| a.cam == now.cam) {
            return Err(Refusal::SharedSlot { camera, other: other.camera.clone() });
        }
        let ledger = ledgers.get(now.cam).filter(|l| l.session == now.session);
        let source = ledger.and_then(|l| l.source).or(now.source);
        if let Some(source) = ledger.into_iter().flat_map(|l| l.sources.iter().copied()).chain(source).find(|s| !policy.trusts(*s)) {
            return Err(Refusal::Untrusted { camera, source });
        }
        let baseline = match ledger.and_then(|l| l.last_frame.max(l.frame_floor)) {
            Some(b) => Some(b),
            None if source == Some(CounterSource::Synthetic) => Some(SYNTHETIC_START),
            None => now.counter_after_open.or(policy.counter_after_open),
        };
        armed.push(FrameBaseline { cam: now.cam, camera, session: now.session, baseline });
    }
    Ok(armed)
}

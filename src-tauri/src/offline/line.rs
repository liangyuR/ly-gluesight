use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ly_plc::{DataType, PlcConfig};
use tokio::time::{sleep, Instant};

use super::s7_server::{BitAddr, Memory, SharedMemory};
use super::Scenario;
use crate::handshake::{Contract, PROTOCOL_VERSION};
use crate::inspection::{state, tag};
use crate::plan::{PathSegment, SegmentKind};
use crate::segmentation::synthetic;

const FIELDS: &[&str] = &[
    tag::PLC_VERSION,
    tag::PLC_HEARTBEAT,
    tag::PLC_STATE,
    tag::PLC_REASON,
    tag::REQUEST_SEQ,
    tag::PRODUCT_CODE,
    tag::SN,
    tag::PC_VERSION,
    tag::PC_HEARTBEAT,
    tag::PC_STATE,
    tag::PC_MODE,
    tag::SEQ_ECHO,
    tag::RESULT_CODE,
    tag::FAULT_CODE,
    tag::SN_ECHO,
];
const FAULTED: &str = "视觉撤下就绪或置故障（pcState 为 0、90 或 99），PLC 撤销本件请求";
const ABORTED: &str = "已立即停止，PLC 撤销本件请求";

#[derive(Clone, Copy)]
struct Slot {
    db: u16,
    byte: u32,
    bit: Option<u8>,
    len: u32,
    /// 字符串字段（S7 STRING）的头字节数：声明长度 + 实际长度
    text_header: Option<usize>,
}

/// 握手字段在 DB 中的位置，全部从 PLC 配置的握手契约解析。
pub struct Layout {
    slots: HashMap<&'static str, Slot>,
}

impl Layout {
    pub fn from_config(config: &PlcConfig) -> Result<Self, String> {
        let contract = Contract::validate(config)?;
        let mut slots = HashMap::new();
        for &field in FIELDS {
            let id = contract.point_id(field)?;
            let point = config.points.iter().find(|p| p.id == id).ok_or_else(|| format!("握手点位 {id} 不存在"))?;
            let location = ly_plc::resolve(&config.connection, &point.address, point.data_type)?;
            let text_header = (point.data_type == DataType::String).then_some(2);
            slots.insert(field, Slot { db: location.area.index, byte: location.start, bit: location.bit, len: location.len, text_header });
        }
        Ok(Self { slots })
    }

    /// 心跳 Int 低字节的最低位：翻转后 plcHeartbeat 在 0/1 间交替。
    pub fn heartbeat(&self) -> BitAddr {
        let slot = self.slots[tag::PLC_HEARTBEAT];
        BitAddr {
            db: slot.db,
            byte: slot.byte + slot.len - 1,
            bit: 0,
        }
    }

    /// 按点表分配 DB（至少 1024 字节），PLC 协议版本预置为当前版本，字符串字段预置为空串（S7 STRING 带正确的声明长度）。
    pub fn memory(&self) -> SharedMemory {
        let mut extents = BTreeMap::<u16, usize>::new();
        for slot in self.slots.values() {
            let end = (slot.byte + slot.len) as usize;
            extents.entry(slot.db).and_modify(|e| *e = (*e).max(end)).or_insert(end);
        }
        let mut memory = Memory::default();
        for (db, end) in extents {
            memory.create(db, end.max(1024));
        }
        self.set(&mut memory, tag::PLC_VERSION, PROTOCOL_VERSION as u32);
        for (field, slot) in &self.slots {
            if slot.text_header.is_some() { self.set_text(&mut memory, field, ""); }
        }
        Arc::new(Mutex::new(memory))
    }

    pub(crate) fn get(&self, memory: &Memory, field: &str) -> u32 {
        let slot = self.slots[field];
        match slot.bit {
            Some(bit) => memory.bit(slot.db, slot.byte, bit).unwrap_or(false) as u32,
            None => memory.read(slot.db, slot.byte, slot.len as usize)
                .map(|bytes| bytes.iter().fold(0u32, |value, byte| (value << 8) | *byte as u32)).unwrap_or(0),
        }
    }

    pub(crate) fn get_text(&self, memory: &Memory, field: &str) -> String {
        let slot = self.slots[field];
        let header = slot.text_header.unwrap_or(2);
        memory.read(slot.db, slot.byte, slot.len as usize).ok()
            .and_then(|bytes| bytes.get(header..header + bytes[header - 1] as usize).map(|chars| chars.iter().map(|&b| b as char).collect()))
            .unwrap_or_default()
    }

    pub(crate) fn set_text(&self, memory: &mut Memory, field: &str, value: &str) {
        let slot = self.slots[field];
        let header = slot.text_header.unwrap_or(2);
        let mut bytes = vec![0u8; slot.len as usize];
        if header == 2 { bytes[0] = (slot.len - 2) as u8; }
        bytes[header - 1] = value.len() as u8;
        bytes[header..header + value.len()].copy_from_slice(value.as_bytes());
        let _ = memory.write(slot.db, slot.byte, &bytes);
    }

    pub(crate) fn set(&self, memory: &mut Memory, field: &str, value: u32) {
        let slot = self.slots[field];
        let _ = match slot.bit {
            Some(bit) => memory.set_bit(slot.db, slot.byte, bit, value != 0),
            None => memory.write(slot.db, slot.byte, &value.to_be_bytes()[4 - slot.len as usize..]),
        };
    }
}

#[derive(Clone, Debug)]
pub struct Timing {
    pub poll: Duration,
    pub end_delay: Duration,
    pub ack_delay: Duration,
    pub settle: Duration,
    pub arm_timeout: Duration,
    pub result_timeout: Duration,
    pub ready_timeout: Duration,
    pub release_timeout: Duration,
}

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
/// 已有整件 ΣN_exp（整件标定）时把待标定的段按估算比例缩放，使合计等于 ΣN_exp——正常件的帧数才落在
/// ΣN_exp ± Σtol 内，不会被当成帧数异常。
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

/// 虚拟产线的一件：按脉冲剖面发脉冲串（[`Line::run_burst`]），`lost` 只丢指定相机的指定帧，
/// `early_end_after` 为 PLC 提前终止前发出的脉冲数。
#[derive(Clone, Debug)]
pub struct PartOrder {
    pub sn: String,
    pub product_code: u16,
    pub profile: Option<PulseProfile>,
    /// (计划相机下标, seq)
    pub lost: Vec<(usize, u64)>,
    pub early_end_after: Option<u64>,
}

impl PartOrder {
    /// 没有脉冲剖面的一件；需要剖面时再填 `profile`。
    pub fn new(sn: String, product_code: u16) -> Self {
        Self {
            sn,
            product_code,
            profile: None,
            lost: Vec::new(),
            early_end_after: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub request_seq: u32,
    pub sn: String,
    pub result_code: u16,
    pub fault_code: u16,
}

enum Wait {
    Done,
    Faulted,
    Aborted,
    Timeout,
}

/// 虚拟产线：扮演 PLC 程序与机器人，直接读写进程内 S7 服务的 DB，按生产通讯协议 7 推进一件。
pub struct Line {
    memory: SharedMemory,
    layout: Layout,
}

impl Line {
    pub fn new(memory: SharedMemory, layout: Layout) -> Self {
        Self {
            memory,
            layout,
        }
    }

    pub(crate) fn read(&self, field: &str) -> u32 {
        self.layout.get(&self.memory.lock().unwrap(), field)
    }

    fn read_text(&self, field: &str) -> String {
        self.layout.get_text(&self.memory.lock().unwrap(), field)
    }

    fn write(&self, values: &[(&str, u32)]) {
        let mut memory = self.memory.lock().unwrap();
        for (field, value) in values { self.layout.set(&mut memory, field, *value); }
    }

    fn write_request(&self, sn: &str, values: &[(&str, u32)]) {
        let mut memory = self.memory.lock().unwrap();
        self.layout.set_text(&mut memory, tag::SN, sn);
        for (field, value) in values { self.layout.set(&mut memory, field, *value); }
    }

    fn release(&self) {
        self.write(&[
            (tag::PLC_REASON, 0),
            (tag::PLC_STATE, state::PLC_IDLE as u32),
        ]);
    }

    /// 故障注入：像操作员在现场按住或松开复位一样保持复位请求（plcState=99）。
    pub fn hold_reset_request(&self, on: bool) {
        let mut memory = self.memory.lock().unwrap();
        let current = self.layout.get(&memory, tag::PLC_STATE);
        if on && current == state::PLC_IDLE as u32 {
            self.layout
                .set(&mut memory, tag::PLC_STATE, state::PLC_RESET as u32);
        } else if !on && current == state::PLC_RESET as u32 {
            self.layout
                .set(&mut memory, tag::PLC_STATE, state::PLC_IDLE as u32);
        }
    }

    fn ready(&self) -> bool {
        self.read(tag::PC_STATE) == state::PC_READY as u32
    }

    /// 视觉撤下就绪、置故障或请求复位：在途件按协议放弃。
    fn pc_stopped(&self) -> bool {
        matches!(
            self.read(tag::PC_STATE) as u16,
            state::PC_NOT_READY | state::PC_FAULT | state::PC_RESET
        )
    }

    async fn wait(&self, timing: &Timing, timeout: Duration, abort: &AtomicBool, fault_stops: bool, done: impl Fn(&Self) -> bool) -> Wait {
        let deadline = Instant::now() + timeout;
        loop {
            if abort.load(Ordering::SeqCst) {
                return Wait::Aborted;
            }
            if done(self) {
                return Wait::Done;
            }
            if fault_stops && self.pc_stopped() {
                return Wait::Faulted;
            }
            if Instant::now() >= deadline {
                return Wait::Timeout;
            }
            sleep(timing.poll).await;
        }
    }

    async fn until(&self, timing: &Timing, timeout: Duration, abort: &AtomicBool, what: &str, done: impl Fn(&Self) -> bool) -> Result<(), String> {
        match self.wait(timing, timeout, abort, true, done).await {
            Wait::Done => Ok(()),
            Wait::Faulted => Err(FAULTED.into()),
            Wait::Aborted => Err(ABORTED.into()),
            Wait::Timeout => Err(format!("{} ms 内未等到{what}", timeout.as_millis())),
        }
    }

    async fn hold(&self, timing: &Timing, duration: Duration, abort: &AtomicBool) -> Result<(), String> {
        match self.wait(timing, duration, abort, true, |_| false).await {
            Wait::Faulted => Err(FAULTED.into()),
            Wait::Aborted => Err(ABORTED.into()),
            Wait::Done | Wait::Timeout => Ok(()),
        }
    }

    /// 等到 `deadline`（脉冲时钟）：轮询间隔不长于剩余时间，定频脉冲不被轮询周期拉长。
    async fn hold_until(&self, timing: &Timing, deadline: Instant, abort: &AtomicBool) -> Result<(), String> {
        loop {
            if abort.load(Ordering::SeqCst) {
                return Err(ABORTED.into());
            }
            if self.pc_stopped() {
                return Err(FAULTED.into());
            }
            let now = Instant::now();
            if now >= deadline { return Ok(()); }
            sleep(timing.poll.min(deadline - now)).await;
        }
    }

    /// 开工前确认视觉就绪；视觉未就绪或故障时像操作员一样在现场复位：置 plcState=99，等 pcState=10 后回 0。
    /// 进入离线的自动复位与运行可能同时在等，别的驱动已提交请求就让出。
    pub async fn ensure_ready(&self, timing: &Timing, abort: &AtomicBool) -> Result<(), String> {
        match self.wait(timing, timing.ready_timeout, abort, true, Self::ready).await {
            Wait::Done => return Ok(()),
            Wait::Aborted => return Err(ABORTED.into()),
            Wait::Timeout => {
                return Err(format!(
                    "{} ms 内视觉未置 pcState=10，查看在线检测页的提示",
                    timing.ready_timeout.as_millis()
                ))
            }
            Wait::Faulted => {}
        }
        sleep(timing.settle).await;
        self.hold_reset_request(true);
        let reset = self
            .wait(timing, timing.ready_timeout, abort, false, |line| {
                line.ready() || line.read(tag::PLC_STATE) == state::PLC_REQUEST as u32
            })
            .await;
        self.hold_reset_request(false);
        match reset {
            Wait::Done => Ok(()),
            Wait::Aborted => Err(ABORTED.into()),
            _ => Err("PLC 已置复位请求（99），PC 未回就绪".into()),
        }
    }

    /// 一件（规格 §0/§6）：写请求（plcState=10）→ 布防后相机使能拉高，PLC 按剖面定频发脉冲串，每个脉冲同时触发三台相机
    /// → 使能拉低后置 plcState=20 → 等结果、核对回显后置 30。
    /// - `order.lost` 只丢指定相机的指定帧（相机已计数、帧没送到）；
    /// - `order.early_end_after`：PLC 发到这么多脉冲就提前置 20（提前终止）；
    /// - countMismatch：协议 7 不报触发计数，段配方与正常相同；motionTimeout：不置 20；ackMismatch：PLC 核对回显不符，置 90（plcReason=5）；
    /// - notInspected：机器人等不到触发许可照常涂胶、置“本件未检”，PLC 不提交请求，返回 `Ok(None)`，PC 没有这一件。
    pub async fn run_burst(&self, timing: &Timing, order: &PartOrder, scenario: Scenario,
        fire: impl FnMut(&Pulse), abort: &AtomicBool) -> Result<Option<Outcome>, String> {
        let profile = order.profile.as_ref().ok_or("段配方的一件缺少脉冲剖面")?;
        if scenario == Scenario::NotInspected {
            let gluing = profile.period() * u32::try_from(profile.counts().iter().sum::<u64>()).unwrap_or(u32::MAX);
            self.hold(timing, gluing, abort).await?;
            return Ok(None);
        }
        let result = self.drive_burst(timing, order, profile, scenario, fire, abort).await;
        if result.is_err() { self.release(); }
        result.map(Some)
    }

    /// 写请求（数据先写，最后置 plcState=10），返回本件 requestSeq。
    fn request(&self, order: &PartOrder) -> u32 {
        let seq = self.read(tag::REQUEST_SEQ) % i32::MAX as u32 + 1;
        self.write_request(
            &order.sn,
            &[
                (tag::PLC_VERSION, PROTOCOL_VERSION as u32),
                (tag::REQUEST_SEQ, seq),
                (tag::PRODUCT_CODE, order.product_code as u32),
                (tag::PLC_REASON, 0),
                (tag::PLC_STATE, state::PLC_REQUEST as u32),
            ],
        );
        seq
    }

    fn published(&self, seq: u32) -> bool {
        self.read(tag::PC_STATE) == state::PC_RESULT as u32 && self.read(tag::SEQ_ECHO) == seq
    }

    /// 等布防或拒绝结果；视觉已布防（没有直接发布拒绝结果）时返回 true。
    async fn armed(&self, timing: &Timing, seq: u32, abort: &AtomicBool) -> Result<bool, String> {
        self.until(
            timing,
            timing.arm_timeout,
            abort,
            "布防或拒绝结果",
            |line| {
                (line.read(tag::PC_STATE) == state::PC_ARMED as u32
                    && line.read(tag::SEQ_ECHO) == seq)
                    || line.published(seq)
            },
        )
        .await?;
        Ok(!self.published(seq))
    }

    /// 运动结束：运动超时工况不置 plcState=20。
    async fn end_motion(&self, timing: &Timing, scenario: Scenario, abort: &AtomicBool) -> Result<(), String> {
        if scenario != Scenario::MotionTimeout {
            self.hold(timing, timing.end_delay, abort).await?;
            self.write(&[(tag::PLC_STATE, state::PLC_MOTION_END as u32)]);
        }
        Ok(())
    }

    /// 等本件结果，核对回显后置 30（ackMismatch 时按回显不符置 90），等视觉回就绪后回空闲。
    async fn conclude(
        &self,
        timing: &Timing,
        scenario: Scenario,
        seq: u32,
        abort: &AtomicBool,
    ) -> Result<Outcome, String> {
        self.until(
            timing,
            timing.result_timeout,
            abort,
            "本件结果",
            move |line| line.published(seq),
        )
        .await?;
        let outcome = Outcome {
            request_seq: seq,
            sn: self.read_text(tag::SN_ECHO),
            result_code: self.read(tag::RESULT_CODE) as u16,
            fault_code: self.read(tag::FAULT_CODE) as u16,
        };
        self.hold(timing, timing.ack_delay, abort).await?;
        if scenario == Scenario::AckMismatch || outcome.sn != self.read_text(tag::SN) {
            self.write(&[
                (tag::PLC_REASON, 5),
                (tag::PLC_STATE, state::PLC_FAULT as u32),
            ]);
            let _ = self
                .wait(timing, timing.release_timeout, abort, true, |_| false)
                .await;
            return Err("PLC 核对回显不符，置故障（plcReason=5）".into());
        }
        self.write(&[(tag::PLC_STATE, state::PLC_CONFIRMED as u32)]);
        self.until(
            timing,
            timing.release_timeout,
            abort,
            "视觉回就绪（pcState=10）",
            Self::ready,
        )
        .await?;
        self.release();
        Ok(outcome)
    }

    async fn drive_burst(&self, timing: &Timing, order: &PartOrder, profile: &PulseProfile, scenario: Scenario,
        mut fire: impl FnMut(&Pulse), abort: &AtomicBool) -> Result<Outcome, String> {
        let seq = self.request(order);
        if self.armed(timing, seq, abort).await? {
            let stop = order.early_end_after.unwrap_or(u64::MAX);
            let period = profile.period();
            let start = Instant::now();
            for pulse in profile.pulses(&order.lost).iter().take_while(|pulse| pulse.seq <= stop) {
                self.hold_until(timing, start + period * u32::try_from(pulse.seq).unwrap_or(u32::MAX), abort).await?;
                fire(pulse);
            }
            self.end_motion(timing, scenario, abort).await?;
        }
        self.conclude(timing, scenario, seq, abort).await
    }
}

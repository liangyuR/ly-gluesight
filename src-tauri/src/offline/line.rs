use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ly_plc::{DataType, PlcConfig};
use tokio::time::{sleep, Instant};

use super::s7_server::{BitAddr, Memory, SharedMemory};
use super::Scenario;
use crate::handshake::{Contract, PROTOCOL_VERSION};
use crate::inspection::tag;
use crate::plan::{PathSegment, SegmentKind};
use crate::segmentation::synthetic;

const FIELDS: &[&str] = &[
    tag::PART_START, tag::PART_END, tag::RESULT_ACK, tag::FAULT_RESET, tag::PLC_HEARTBEAT, tag::PROTOCOL_VERSION,
    tag::REQUEST_SEQ, tag::PART_SN, tag::PRODUCT_CODE, tag::SHOT_COUNT, tag::PLAN_VERSION, tag::PLAN_RESERVED,
    tag::DEVICE_SHOTS, tag::DEVICE_TRIGGERS, tag::ACK_SEQ, tag::VISION_READY, tag::ARMED, tag::BUSY, tag::DONE,
    tag::VISION_FAULT, tag::PC_HEARTBEAT, tag::PC_PROTOCOL_VERSION, tag::ACCEPTED_SEQ, tag::ACCEPTED_PLAN_RESERVED,
    tag::RESULT_SEQ, tag::RESULT_SN, tag::RESULT_CODE, tag::FAULT_CODE,
];
const FAULTED: &str = "视觉置位 visionFault（握手故障），PLC 撤销本件请求";
const ABORTED: &str = "已立即停止，PLC 撤销本件请求";
/// 虚拟 PLC 按住 faultReset 等视觉就绪的时长，超过就松开再按一次
const RESET_RETRY: Duration = Duration::from_secs(1);

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

    pub fn heartbeat(&self) -> BitAddr {
        let slot = self.slots[tag::PLC_HEARTBEAT];
        BitAddr { db: slot.db, byte: slot.byte, bit: slot.bit.unwrap_or(0) }
    }

    /// 按点表分配 DB（至少 1024 字节），PLC 协议版本预置为当前版本，字符串字段预置为空串（S7 STRING 带正确的声明长度）。
    pub fn memory(&self) -> SharedMemory {
        let mut extents = BTreeMap::<u16, usize>::new();
        for slot in self.slots.values() {
            let end = (slot.byte + slot.len) as usize;
            extents.entry(slot.db).and_modify(|e| *e = (*e).max(end)).or_insert(end);
        }
        let mut memory = Memory::default();
        for (db, end) in extents { memory.create(db, end.max(1024)); }
        self.set(&mut memory, tag::PROTOCOL_VERSION, PROTOCOL_VERSION as u32);
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
    pub shot_interval: Duration,
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
/// 种子化的首尾 ±`head_tail_jitter` 与弯道 ±`corner_jitter` 波动。波动的取法与分段回归入口
/// [`synthetic::frame_counts`] 相同，同一剖面与种子在离线产线与回归序列里帧数、真值边界一致。
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
    /// 同一剖面在分段回归入口里的描述；`missing` 为缺帧 (相机, seq)。
    pub fn spec(&self, missing: Vec<(usize, u64)>) -> synthetic::SequenceSpec {
        synthetic::SequenceSpec { segments: self.segments.clone(), head_tail_jitter: self.head_tail_jitter, corner_jitter: self.corner_jitter, missing }
    }

    /// 各段实际脉冲数（含波动）。
    pub fn counts(&self) -> Vec<u64> {
        synthetic::frame_counts(&self.spec(Vec::new()), self.seed)
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

/// PLC 下发的一件：计划字段取自生效配方编译出的 PLC 拍照计划。
/// 旧拍照点配方 `profile` 为空，虚拟机器人按 `device_shots` 精确触发（[`Line::run_part`]）；
/// 段配方带脉冲剖面，按剖面发脉冲串（[`Line::run_burst`]），`lost` 只丢指定相机的指定帧，
/// `early_end_after` 为 PLC 提前终止前发出的脉冲数。
#[derive(Clone, Debug)]
pub struct PartOrder {
    pub sn: String,
    pub product_code: u16,
    pub plan_version: u32,
    pub shot_count: u16,
    pub device_shots: u16,
    pub profile: Option<PulseProfile>,
    /// (计划相机下标, seq)
    pub lost: Vec<(usize, u64)>,
    pub early_end_after: Option<u64>,
}

impl PartOrder {
    /// 旧拍照点配方的一件：没有脉冲剖面。
    pub fn legacy(sn: String, product_code: u16, plan_version: u32, shot_count: u16, device_shots: u16) -> Self {
        Self { sn, product_code, plan_version, shot_count, device_shots, profile: None, lost: Vec::new(), early_end_after: None }
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

/// 虚拟产线：扮演 PLC 程序与机器人，直接读写进程内 S7 服务的 DB，严格按一期握手推进一件。
pub struct Line {
    memory: SharedMemory,
    layout: Layout,
}

impl Line {
    pub fn new(memory: SharedMemory, layout: Layout) -> Self {
        Self { memory, layout }
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
        self.layout.set_text(&mut memory, tag::PART_SN, sn);
        for (field, value) in values { self.layout.set(&mut memory, field, *value); }
    }

    fn release(&self) {
        self.write(&[(tag::PART_START, 0), (tag::PART_END, 0), (tag::RESULT_ACK, 0)]);
    }

    /// 故障注入：像操作员按住或松开复位按钮一样直接保持 faultReset 电平。
    pub fn hold_fault_reset(&self, level: bool) {
        self.write(&[(tag::FAULT_RESET, level as u32)]);
    }

    fn ready(&self) -> bool {
        self.read(tag::VISION_READY) == 1 && self.read(tag::VISION_FAULT) == 0
    }

    /// 视觉已就绪或别的驱动已发出 partStart 时不再按；判断与按下在同一把锁内，不会落在刚发出的 partStart 之后
    fn press_reset(&self) -> bool {
        let mut memory = self.memory.lock().unwrap();
        let get = |field| self.layout.get(&memory, field);
        let settled = get(tag::PART_START) == 1 || (get(tag::VISION_READY) == 1 && get(tag::VISION_FAULT) == 0);
        if !settled { self.layout.set(&mut memory, tag::FAULT_RESET, 1); }
        !settled
    }

    async fn wait(&self, timing: &Timing, timeout: Duration, abort: &AtomicBool, fault_stops: bool, done: impl Fn(&Self) -> bool) -> Wait {
        let deadline = Instant::now() + timeout;
        loop {
            if abort.load(Ordering::SeqCst) { return Wait::Aborted; }
            if done(self) { return Wait::Done; }
            if fault_stops && self.read(tag::VISION_FAULT) == 1 { return Wait::Faulted; }
            if Instant::now() >= deadline { return Wait::Timeout; }
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
            if abort.load(Ordering::SeqCst) { return Err(ABORTED.into()); }
            if self.read(tag::VISION_FAULT) == 1 { return Err(FAULTED.into()); }
            let now = Instant::now();
            if now >= deadline { return Ok(()); }
            sleep(timing.poll.min(deadline - now)).await;
        }
    }

    /// 开工前确认视觉就绪；视觉处于故障时像操作员一样释放输入并发 faultReset 上升沿。
    pub async fn ensure_ready(&self, timing: &Timing, abort: &AtomicBool) -> Result<(), String> {
        match self.wait(timing, timing.ready_timeout, abort, true, Self::ready).await {
            Wait::Done => return Ok(()),
            Wait::Aborted => return Err(ABORTED.into()),
            Wait::Timeout => return Err(format!("{} ms 内视觉未置 visionReady，查看在线检测页的提示", timing.ready_timeout.as_millis())),
            Wait::Faulted => {}
        }
        self.release();
        // 复位被拒（如心跳故障后视觉尚未观察到新的心跳翻转）时像操作员一样松开再按，直到就绪或超时；
        // 进入离线的自动复位与运行、整圈采集可能同时在按，别的驱动已发出 partStart 就让出，不在在途件上再按
        let deadline = Instant::now() + timing.ready_timeout;
        loop {
            self.write(&[(tag::FAULT_RESET, 0)]);
            sleep(timing.settle).await;
            if !self.press_reset() { return Ok(()); }
            let window = RESET_RETRY.min(deadline.saturating_duration_since(Instant::now()));
            let ready = self.wait(timing, window, abort, false, |line| line.ready() || line.read(tag::PART_START) == 1).await;
            self.write(&[(tag::FAULT_RESET, 0)]);
            match ready {
                Wait::Done => return Ok(()),
                Wait::Aborted => return Err(ABORTED.into()),
                _ if Instant::now() >= deadline => return Err("PLC 已发 faultReset，视觉仍未就绪：查看在线检测页的故障原因".into()),
                _ => {}
            }
        }
    }

    pub async fn run_part(&self, timing: &Timing, order: &PartOrder, scenario: Scenario,
        trigger: impl FnMut(usize, bool), abort: &AtomicBool) -> Result<Outcome, String> {
        let result = self.drive(timing, order, scenario, trigger, abort).await;
        if result.is_err() { self.release(); }
        result
    }

    /// 段配方的一件（规格 §0/§6）：写请求 → 布防后相机使能拉高，PLC 按剖面定频发脉冲串，每个脉冲同时触发三台相机，
    /// deviceTriggers 写实际脉冲数 → 使能拉低后置 partEnd → 等结果、回 ACK。
    /// - `order.lost` 只丢指定相机的指定帧（相机已计数、帧没送到）；
    /// - `order.early_end_after`：PLC 发到这么多脉冲就提前置 partEnd（提前终止）；
    /// - countMismatch：PLC 多计 2 个脉冲（只告警的计数偏差）；motionTimeout：不发 partEnd；ackMismatch：ACK 序号错位；
    /// - notInspected：机器人等不到触发许可照常涂胶、置“本件未检”，PLC 不提交 partStart，返回 `Ok(None)`，PC 没有这一件。
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

    /// 写开始请求，返回本件 requestSeq。
    fn request(&self, order: &PartOrder) -> u32 {
        let seq = self.read(tag::REQUEST_SEQ) % i32::MAX as u32 + 1;
        self.write_request(&order.sn, &[
            (tag::PROTOCOL_VERSION, PROTOCOL_VERSION as u32), (tag::REQUEST_SEQ, seq),
            (tag::PRODUCT_CODE, order.product_code as u32), (tag::SHOT_COUNT, order.shot_count as u32),
            (tag::PLAN_VERSION, order.plan_version), (tag::PLAN_RESERVED, 0), (tag::DEVICE_SHOTS, order.device_shots as u32),
            (tag::DEVICE_TRIGGERS, 0), (tag::PART_END, 0), (tag::RESULT_ACK, 0), (tag::FAULT_RESET, 0), (tag::PART_START, 1),
        ]);
        seq
    }

    fn published(&self, seq: u32) -> bool {
        self.read(tag::DONE) == 1 && self.read(tag::RESULT_SEQ) == seq
    }

    /// 等布防或拒绝结果；视觉已布防（没有直接发布拒绝结果）时返回 true。
    async fn armed(&self, timing: &Timing, seq: u32, abort: &AtomicBool) -> Result<bool, String> {
        self.until(timing, timing.arm_timeout, abort, "布防或拒绝结果",
            |line| (line.read(tag::ARMED) == 1 && line.read(tag::ACCEPTED_SEQ) == seq) || line.published(seq)).await?;
        Ok(!self.published(seq))
    }

    /// 运动结束：运动超时工况不发 partEnd。
    async fn end_motion(&self, timing: &Timing, scenario: Scenario, abort: &AtomicBool) -> Result<(), String> {
        if scenario != Scenario::MotionTimeout {
            self.hold(timing, timing.end_delay, abort).await?;
            self.write(&[(tag::PART_END, 1)]);
        }
        Ok(())
    }

    /// 等本件结果、回 ACK（ackMismatch 时序号错位）、等视觉清除 done / busy 后释放输入。
    async fn conclude(&self, timing: &Timing, scenario: Scenario, seq: u32, abort: &AtomicBool) -> Result<Outcome, String> {
        self.until(timing, timing.result_timeout, abort, "本件结果", move |line| line.published(seq)).await?;
        let outcome = Outcome { request_seq: seq, sn: self.read_text(tag::RESULT_SN),
            result_code: self.read(tag::RESULT_CODE) as u16, fault_code: self.read(tag::FAULT_CODE) as u16 };
        self.hold(timing, timing.ack_delay, abort).await?;
        let ack = if scenario == Scenario::AckMismatch { seq.wrapping_add(1) } else { seq };
        self.write(&[(tag::ACK_SEQ, ack), (tag::RESULT_ACK, 1)]);
        self.until(timing, timing.release_timeout, abort, "视觉清除 done / busy",
            |line| line.read(tag::DONE) == 0 && line.read(tag::BUSY) == 0).await?;
        self.release();
        Ok(outcome)
    }

    /// 旧拍照点配方：按计划精确触发 device_shots 次，第 k 次（0 起）由 `trigger(k, 传输丢帧)` 交给相机。
    async fn drive(&self, timing: &Timing, order: &PartOrder, scenario: Scenario,
        mut trigger: impl FnMut(usize, bool), abort: &AtomicBool) -> Result<Outcome, String> {
        let seq = self.request(order);
        if self.armed(timing, seq, abort).await? {
            let n = order.device_shots as usize;
            let mut issued = 0;
            for k in 0..n {
                self.hold(timing, timing.shot_interval, abort).await?;
                if scenario == Scenario::CountMismatch && k + 1 == n { continue; }
                trigger(k, scenario.lost_frame(n) == Some(k));
                issued += 1;
                self.write(&[(tag::DEVICE_TRIGGERS, issued)]);
            }
            self.end_motion(timing, scenario, abort).await?;
        }
        self.conclude(timing, scenario, seq, abort).await
    }

    async fn drive_burst(&self, timing: &Timing, order: &PartOrder, profile: &PulseProfile, scenario: Scenario,
        mut fire: impl FnMut(&Pulse), abort: &AtomicBool) -> Result<Outcome, String> {
        let seq = self.request(order);
        if self.armed(timing, seq, abort).await? {
            let extra = if scenario == Scenario::CountMismatch { 2 } else { 0 };
            let stop = order.early_end_after.unwrap_or(u64::MAX);
            let period = profile.period();
            let start = Instant::now();
            let mut issued = 0u32;
            for pulse in profile.pulses(&order.lost).iter().take_while(|pulse| pulse.seq <= stop) {
                self.hold_until(timing, start + period * u32::try_from(pulse.seq).unwrap_or(u32::MAX), abort).await?;
                fire(pulse);
                issued += 1;
                self.write(&[(tag::DEVICE_TRIGGERS, issued + extra)]);
            }
            self.end_motion(timing, scenario, abort).await?;
        }
        self.conclude(timing, scenario, seq, abort).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{new_segment, KindDefaults};

    /// 离线脉冲剖面取自配方：各段已标定用 N_exp；有段待标定时按段长 × 频率 / 速度估算（没有段长按类型取默认帧数），
    /// 已有整件标定时缩放到 ΣN_exp、已标定的段不动——否则正常件的帧数落在 ΣN_exp ± Σtol 之外，被当成帧数异常。
    #[test]
    fn nominal_profile_keeps_calibrated_segments_and_sums_to_part_expected_total() {
        use SegmentKind::{Corner as C, Straight as S};
        let segment = |kind, length_mm: Option<f64>, expected_frames: Option<u32>| PathSegment { length_mm, expected_frames, ..new_segment(1, kind, &KindDefaults::default()) };
        for (case, segments, part_total, expected) in [
            ("各段已标定", vec![segment(S, None, Some(12)), segment(C, Some(30.0), Some(4))], None, vec![12, 4]),
            ("待标定、没有整件标定：按段长估算，没有段长取类型默认", vec![segment(S, Some(100.0), None), segment(C, None, None)], None,
                vec![40, DEFAULT_CORNER_FRAMES]),
            ("待标定、有整件标定：已标定段不动，其余按估算比例分到合计", vec![segment(S, None, Some(10)), segment(C, Some(25.0), None), segment(S, Some(75.0), None)],
                Some(30), vec![10, 5, 15]),
        ] {
            let frames = nominal_frames(&segments, part_total, 20.0);
            assert_eq!(frames.iter().map(|&(_, n)| n).collect::<Vec<_>>(), expected, "{case}");
        }
    }

    /// 进入离线的自动复位还在重按时，运行或整圈采集见到就绪发出 partStart（落在它松开等待或按住等待期间），
    /// 这一件结束、视觉还没置回就绪；自动复位此后再按，会把在途件或件间的视觉打成“在途工件不能直接复位”。
    #[tokio::test]
    async fn reset_retry_once_another_driver_started_a_part_never_presses_fault_reset_again() {
        let timing = Timing {
            poll: Duration::from_millis(2), settle: Duration::from_millis(60), ready_timeout: Duration::from_secs(5),
            shot_interval: Duration::ZERO, end_delay: Duration::ZERO, ack_delay: Duration::ZERO,
            arm_timeout: Duration::ZERO, result_timeout: Duration::ZERO, release_timeout: Duration::ZERO,
        };
        for during_settle in [true, false] {
            let layout = Layout::from_config(&crate::inspection::default_plc_config()).unwrap();
            let line = Line::new(layout.memory(), layout);
            line.write(&[(tag::VISION_FAULT, 1)]);
            let abort = AtomicBool::new(false);
            let other = async {
                while line.read(tag::FAULT_RESET) == 0 { sleep(timing.poll).await; }
                if during_settle {
                    while line.read(tag::FAULT_RESET) == 1 { sleep(timing.poll).await; }
                }
                line.write(&[(tag::VISION_FAULT, 0), (tag::FAULT_RESET, 0), (tag::PART_START, 1)]);
                let started = Instant::now();
                while started.elapsed() < timing.settle * 2 + RESET_RETRY {
                    if started.elapsed() >= Duration::from_millis(100) { line.write(&[(tag::PART_START, 0)]); }
                    assert_eq!(line.read(tag::FAULT_RESET), 0, "during_settle={during_settle}：别的驱动发出 partStart 后不能再按 faultReset");
                    sleep(timing.poll).await;
                }
            };
            let (ready, ()) = tokio::join!(line.ensure_ready(&timing, &abort), other);
            assert!(ready.is_ok(), "during_settle={during_settle}: {ready:?}");
        }
    }
}

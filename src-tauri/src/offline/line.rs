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

/// PLC 下发的一件：计划字段取自生效配方编译出的 PLC 拍照计划。
#[derive(Clone, Debug)]
pub struct PartOrder {
    pub sn: String,
    pub product_code: u16,
    pub plan_version: u32,
    pub shot_count: u16,
    pub device_shots: u16,
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

    async fn drive(&self, timing: &Timing, order: &PartOrder, scenario: Scenario,
        mut trigger: impl FnMut(usize, bool), abort: &AtomicBool) -> Result<Outcome, String> {
        let seq = self.read(tag::REQUEST_SEQ) % i32::MAX as u32 + 1;
        self.write_request(&order.sn, &[
            (tag::PROTOCOL_VERSION, PROTOCOL_VERSION as u32), (tag::REQUEST_SEQ, seq),
            (tag::PRODUCT_CODE, order.product_code as u32), (tag::SHOT_COUNT, order.shot_count as u32),
            (tag::PLAN_VERSION, order.plan_version), (tag::PLAN_RESERVED, 0), (tag::DEVICE_SHOTS, order.device_shots as u32),
            (tag::DEVICE_TRIGGERS, 0), (tag::PART_END, 0), (tag::RESULT_ACK, 0), (tag::FAULT_RESET, 0), (tag::PART_START, 1),
        ]);
        let published = move |line: &Self| line.read(tag::DONE) == 1 && line.read(tag::RESULT_SEQ) == seq;
        self.until(timing, timing.arm_timeout, abort, "布防或拒绝结果",
            |line| (line.read(tag::ARMED) == 1 && line.read(tag::ACCEPTED_SEQ) == seq) || published(line)).await?;
        if !published(self) {
            let n = order.device_shots as usize;
            let mut issued = 0;
            for k in 0..n {
                self.hold(timing, timing.shot_interval, abort).await?;
                if scenario == Scenario::CountMismatch && k + 1 == n { continue; }
                trigger(k, scenario.lost_frame(n) == Some(k));
                issued += 1;
                self.write(&[(tag::DEVICE_TRIGGERS, issued)]);
            }
            if scenario != Scenario::MotionTimeout {
                self.hold(timing, timing.end_delay, abort).await?;
                self.write(&[(tag::PART_END, 1)]);
            }
        }
        self.until(timing, timing.result_timeout, abort, "本件结果", published).await?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

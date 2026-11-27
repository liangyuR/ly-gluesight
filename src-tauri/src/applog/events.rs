use super::Level;

#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    pub ev: &'static str,
    pub level: Level,
}

impl Event {
    const fn new(ev: &'static str, level: Level) -> Self {
        Self { ev, level }
    }
}

pub const APP_CONFIG: Event = Event::new("配置摘要", Level::Info);
pub const ALARM_RAISED: Event = Event::new("报警", Level::Error);
pub const ALARM_RAISED_SUMMARY: Event = Event::new("报警汇总", Level::Error);
pub const FRAME_STRAY: Event = Event::new("游离帧", Level::Warn);
pub const FRAME_STRAY_SUMMARY: Event = Event::new("游离帧汇总", Level::Warn);
pub const FRAME_LATE: Event = Event::new("迟到帧", Level::Warn);
pub const FRAME_LATE_SUMMARY: Event = Event::new("迟到帧汇总", Level::Warn);
pub const FRAME_AFTER_DEADLINE: Event = Event::new("截止后帧", Level::Warn);
pub const FRAME_AFTER_DEADLINE_SUMMARY: Event = Event::new("截止后帧汇总", Level::Warn);
pub const FRAME_UNROUTED: Event = Event::new("帧归属", Level::Warn);
pub const FRAME_UNROUTED_SUMMARY: Event = Event::new("帧归属汇总", Level::Warn);
pub const FRAME_ROUTE_FAULT: Event = Event::new("帧归属故障", Level::Error);
pub const FRAME_ROUTE_FAULT_SUMMARY: Event = Event::new("帧归属故障汇总", Level::Error);
pub const FRAME_DROPPED: Event = Event::new("丢帧", Level::Warn);
pub const FRAME_DROPPED_SUMMARY: Event = Event::new("丢帧汇总", Level::Warn);
pub const FRAME_BUDGET_DROPPED: Event = Event::new("图像内存满丢帧", Level::Error);
pub const FRAME_BUDGET_DROPPED_SUMMARY: Event = Event::new("图像内存满丢帧汇总", Level::Error);
pub const CAMERA_RECONNECT_FAILED: Event = Event::new("相机重连失败", Level::Warn);
pub const CAMERA_RECONNECT_FAILED_SUMMARY: Event = Event::new("相机重连失败汇总", Level::Warn);
pub const S7_READY: Event = Event::new("S7 通讯就绪", Level::Info);
pub const S7_PART_SKIPPED: Event = Event::new("S7 未接件", Level::Warn);
pub const S7_RESULT_CLEARED: Event = Event::new("S7 清除旧结果", Level::Warn);
pub const FAULT_ENTER: Event = Event::new("故障", Level::Error);
pub const FAULT_REPEAT: Event = Event::new("故障重复", Level::Warn);
pub const FAULT_CLEARED: Event = Event::new("故障解除", Level::Info);
pub const OP_CAPTURE_IMPORT: Event = Event::new("操作：历史件导入为采集轮次", Level::Info);
pub const OP_CAMERA: Event = Event::new("操作：相机", Level::Info);
pub const OP_PLC: Event = Event::new("操作：PLC", Level::Info);
pub const OP_MAINTENANCE: Event = Event::new("操作：维护模式", Level::Warn);
pub const OP_RECIPE_DELETE: Event = Event::new("操作：删除配方", Level::Info);
pub const OP_RECIPE_PACKAGE: Event = Event::new("操作：配方回传包", Level::Info);

//! 运行日志的稳定事件编码：Agent 按 code 与 data 分析日志，ev 与 msg 只给人看。
//! 每个编码须同时登记在 docs/architecture/agent-logging-codes.md，`data` 的键只增不改。

use super::Level;

#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    pub code: &'static str,
    pub ev: &'static str,
    pub level: Level,
}

pub const APP_START: Event = Event {
    code: "app.start",
    ev: "启动",
    level: Level::Info,
};
pub const APP_CONFIG: Event = Event {
    code: "app.config",
    ev: "配置摘要",
    level: Level::Info,
};
pub const APP_PANIC: Event = Event {
    code: "app.panic",
    ev: "程序异常",
    level: Level::Error,
};
pub const APP_LOG_WRITE_FAILED: Event = Event {
    code: "app.log_write_failed",
    ev: "日志写入失败",
    level: Level::Warn,
};
pub const ALARM_RAISED: Event = Event {
    code: "alarm.raised",
    ev: "报警",
    level: Level::Error,
};
pub const ALARM_RAISED_SUMMARY: Event = Event {
    code: "alarm.raised.summary",
    ev: "报警汇总",
    level: Level::Error,
};
pub const FRAME_STRAY: Event = Event {
    code: "frame.stray",
    ev: "游离帧",
    level: Level::Warn,
};
pub const FRAME_STRAY_SUMMARY: Event = Event {
    code: "frame.stray.summary",
    ev: "游离帧汇总",
    level: Level::Warn,
};
pub const FRAME_LATE: Event = Event {
    code: "frame.late",
    ev: "迟到帧",
    level: Level::Warn,
};
pub const FRAME_LATE_SUMMARY: Event = Event {
    code: "frame.late.summary",
    ev: "迟到帧汇总",
    level: Level::Warn,
};
pub const FRAME_AFTER_DEADLINE: Event = Event {
    code: "frame.after_deadline",
    ev: "截止后帧",
    level: Level::Warn,
};
pub const FRAME_AFTER_DEADLINE_SUMMARY: Event = Event {
    code: "frame.after_deadline.summary",
    ev: "截止后帧汇总",
    level: Level::Warn,
};
pub const FRAME_UNROUTED: Event = Event {
    code: "frame.unrouted",
    ev: "帧归属",
    level: Level::Warn,
};
pub const FRAME_UNROUTED_SUMMARY: Event = Event {
    code: "frame.unrouted.summary",
    ev: "帧归属汇总",
    level: Level::Warn,
};
pub const FRAME_ROUTE_FAULT: Event = Event {
    code: "frame.route_fault",
    ev: "帧归属故障",
    level: Level::Error,
};
pub const FRAME_ROUTE_FAULT_SUMMARY: Event = Event {
    code: "frame.route_fault.summary",
    ev: "帧归属故障汇总",
    level: Level::Error,
};
pub const FRAME_DROPPED: Event = Event {
    code: "frame.dropped",
    ev: "丢帧",
    level: Level::Warn,
};
pub const FRAME_DROPPED_SUMMARY: Event = Event {
    code: "frame.dropped.summary",
    ev: "丢帧汇总",
    level: Level::Warn,
};
pub const CAMERA_RECONNECT_FAILED: Event = Event {
    code: "camera.reconnect_failed",
    ev: "相机重连失败",
    level: Level::Warn,
};
pub const CAMERA_RECONNECT_FAILED_SUMMARY: Event = Event {
    code: "camera.reconnect_failed.summary",
    ev: "相机重连失败汇总",
    level: Level::Warn,
};
pub const PLC_ENGINE_WARN: Event = Event {
    code: "plc.engine.warn",
    ev: "PLC 引擎告警",
    level: Level::Warn,
};
pub const PLC_ENGINE_WARN_SUMMARY: Event = Event {
    code: "plc.engine.warn.summary",
    ev: "PLC 引擎告警汇总",
    level: Level::Warn,
};
pub const PLC_ENGINE_ERROR: Event = Event {
    code: "plc.engine.error",
    ev: "PLC 引擎错误",
    level: Level::Error,
};
pub const PLC_ENGINE_ERROR_SUMMARY: Event = Event {
    code: "plc.engine.error.summary",
    ev: "PLC 引擎错误汇总",
    level: Level::Error,
};

pub const S7_FAULT_OUTPUT: Event = Event {
    code: "s7.fault_output",
    ev: "S7 故障输出",
    level: Level::Warn,
};
pub const S7_RESET_REQUEST: Event = Event {
    code: "s7.reset.request",
    ev: "S7 复位请求",
    level: Level::Info,
};
pub const S7_RESET_DONE: Event = Event {
    code: "s7.reset.done",
    ev: "S7 复位完成",
    level: Level::Info,
};
pub const S7_ACK: Event = Event {
    code: "s7.ack",
    ev: "S7 结果确认",
    level: Level::Info,
};
pub const S7_RECONNECT_FORCED: Event = Event {
    code: "s7.reconnect.forced",
    ev: "S7 主动断开",
    level: Level::Error,
};
pub const FAULT_ENTER: Event = Event {
    code: "fault.enter",
    ev: "故障",
    level: Level::Error,
};
pub const FAULT_REPEAT: Event = Event {
    code: "fault.repeat",
    ev: "故障重复",
    level: Level::Warn,
};
pub const FAULT_CLEARED: Event = Event {
    code: "fault.cleared",
    ev: "故障解除",
    level: Level::Info,
};
pub const OP_RESET: Event = Event {
    code: "op.reset",
    ev: "操作：复位",
    level: Level::Info,
};
pub const OP_CAPTURE_IMPORT: Event = Event {
    code: "op.capture.import",
    ev: "操作：历史件导入为采集轮次",
    level: Level::Info,
};
pub const OP_CAMERA: Event = Event {
    code: "op.camera",
    ev: "操作：相机",
    level: Level::Info,
};
pub const OP_PLC: Event = Event {
    code: "op.plc",
    ev: "操作：PLC",
    level: Level::Info,
};
pub const OP_SITE: Event = Event {
    code: "op.site",
    ev: "操作：站点模式",
    level: Level::Info,
};
pub const OP_MAINTENANCE: Event = Event {
    code: "op.maintenance",
    ev: "操作：维护模式",
    level: Level::Warn,
};
pub const OP_RECIPE_DELETE: Event = Event {
    code: "op.recipe.delete",
    ev: "操作：删除配方",
    level: Level::Info,
};

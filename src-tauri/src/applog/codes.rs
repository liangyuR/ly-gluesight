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

pub const TEACH_REQUEST: Event = Event {
    code: "teach.request",
    ev: "示教请求",
    level: Level::Info,
};
pub const TEACH_PC_PUBLISHED: Event = Event {
    code: "teach.pc_published",
    ev: "示教 PC 发布",
    level: Level::Info,
};
pub const TEACH_PLC_CHANGED: Event = Event {
    code: "teach.plc_changed",
    ev: "示教 PLC 变化",
    level: Level::Info,
};
pub const TEACH_ACCEPTED: Event = Event {
    code: "teach.accepted",
    ev: "示教已接受",
    level: Level::Info,
};
pub const TEACH_ENDED: Event = Event {
    code: "teach.ended",
    ev: "示教采集结束",
    level: Level::Info,
};
pub const TEACH_REJECTED: Event = Event {
    code: "teach.rejected",
    ev: "示教被拒绝",
    level: Level::Warn,
};
pub const TEACH_RELEASED: Event = Event {
    code: "teach.released",
    ev: "示教已释放",
    level: Level::Info,
};
pub const TEACH_CANCEL: Event = Event {
    code: "teach.cancel",
    ev: "示教取消",
    level: Level::Warn,
};
pub const TEACH_FINISH: Event = Event {
    code: "teach.finish",
    ev: "示教结果上报",
    level: Level::Info,
};
pub const TEACH_AWAITING_ECHO: Event = Event {
    code: "teach.awaiting_echo",
    ev: "等待握手编号回显",
    level: Level::Warn,
};
pub const TEACH_TIMEOUT: Event = Event {
    code: "teach.timeout",
    ev: "示教超时",
    level: Level::Warn,
};
pub const TEACH_VIOLATION: Event = Event {
    code: "teach.violation",
    ev: "示教寄存器不合规",
    level: Level::Error,
};
pub const TEACH_RESET_BEGIN: Event = Event {
    code: "teach.reset.begin",
    ev: "示教复位开始",
    level: Level::Info,
};
pub const TEACH_RESET_DONE: Event = Event {
    code: "teach.reset.done",
    ev: "示教复位完成",
    level: Level::Info,
};
pub const TEACH_FAULT: Event = Event {
    code: "teach.fault",
    ev: "示教故障",
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
pub const OP_CAPTURE_START: Event = Event {
    code: "op.capture.start",
    ev: "操作：开始整圈采集",
    level: Level::Info,
};
pub const OP_CAPTURE_STOP: Event = Event {
    code: "op.capture.stop",
    ev: "操作：取消整圈采集",
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
pub const OP_RECIPE_DELETE: Event = Event {
    code: "op.recipe.delete",
    ev: "操作：删除配方",
    level: Level::Info,
};

#[cfg(test)]
const ALL: &[&Event] = &[
    &APP_START,
    &APP_CONFIG,
    &APP_PANIC,
    &APP_LOG_WRITE_FAILED,
    &ALARM_RAISED,
    &ALARM_RAISED_SUMMARY,
    &FRAME_STRAY,
    &FRAME_STRAY_SUMMARY,
    &FRAME_LATE,
    &FRAME_LATE_SUMMARY,
    &FRAME_AFTER_DEADLINE,
    &FRAME_AFTER_DEADLINE_SUMMARY,
    &FRAME_UNROUTED,
    &FRAME_UNROUTED_SUMMARY,
    &FRAME_ROUTE_FAULT,
    &FRAME_ROUTE_FAULT_SUMMARY,
    &FRAME_DROPPED,
    &FRAME_DROPPED_SUMMARY,
    &CAMERA_RECONNECT_FAILED,
    &CAMERA_RECONNECT_FAILED_SUMMARY,
    &PLC_ENGINE_WARN,
    &PLC_ENGINE_WARN_SUMMARY,
    &PLC_ENGINE_ERROR,
    &PLC_ENGINE_ERROR_SUMMARY,
    &TEACH_REQUEST,
    &TEACH_PC_PUBLISHED,
    &TEACH_PLC_CHANGED,
    &TEACH_ACCEPTED,
    &TEACH_ENDED,
    &TEACH_REJECTED,
    &TEACH_RELEASED,
    &TEACH_CANCEL,
    &TEACH_FINISH,
    &TEACH_AWAITING_ECHO,
    &TEACH_TIMEOUT,
    &TEACH_VIOLATION,
    &TEACH_RESET_BEGIN,
    &TEACH_RESET_DONE,
    &TEACH_FAULT,
    &S7_FAULT_OUTPUT,
    &S7_RESET_REQUEST,
    &S7_RESET_DONE,
    &S7_ACK,
    &S7_RECONNECT_FORCED,
    &FAULT_ENTER,
    &FAULT_REPEAT,
    &FAULT_CLEARED,
    &OP_RESET,
    &OP_CAPTURE_START,
    &OP_CAPTURE_STOP,
    &OP_CAMERA,
    &OP_PLC,
    &OP_SITE,
    &OP_RECIPE_DELETE,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn level_name(level: Level) -> &'static str {
        match level {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    /// 编码是给 Agent 的契约：代码里的编码表与文档逐条一致（编码、事件名、级别），且编码不重复。
    #[test]
    fn every_code_is_unique_and_registered_in_the_code_table_document() {
        let doc = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/architecture/agent-logging-codes.md"
        ));
        let mut documented: Vec<(String, String, String)> = doc
            .lines()
            .filter(|line| line.starts_with("| `"))
            .map(|line| {
                let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
                (
                    cells[0].trim_matches('`').to_string(),
                    cells[1].to_string(),
                    cells[2].to_string(),
                )
            })
            .collect();
        let mut coded: Vec<(String, String, String)> = ALL
            .iter()
            .map(|e| {
                (
                    e.code.to_string(),
                    e.ev.to_string(),
                    level_name(e.level).to_string(),
                )
            })
            .collect();
        documented.sort();
        coded.sort();
        let mut codes: Vec<&str> = ALL.iter().map(|e| e.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len(), "编码重复");
        assert_eq!(documented, coded);
    }
}

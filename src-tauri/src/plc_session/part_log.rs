use ly_plc::{now_ms, LogEntry, PlcEngine};
use serde::Serialize;
use serde_json::json;

use super::Pending;

#[derive(Serialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum PartEvent<'a> {
    Received { observed_at: i64 },
    Ended,
    ResultWritten { result_code: u16, fault_code: u16 },
    ResultFailed { reason: &'a str },
    Interrupted { reason: &'a str },
    Skipped { reason: &'a str, observed_at: i64 },
}

impl PartEvent<'_> {
    pub(super) fn record(self, engine: &PlcEngine, part: &Pending) {
        self.log(
            engine,
            part.request.request_seq,
            part.started_at,
            Some(&part.request.sn),
            Some(part.request.product_code),
        );
    }

    pub(super) fn log(
        self,
        engine: &PlcEngine,
        seq: u32,
        received_at: i64,
        sn: Option<&str>,
        product_code: Option<u16>,
    ) {
        let (level, message) = match self {
            Self::Received { .. } => ("info", "PC 已接件".into()),
            Self::Ended => ("info", "PC 读到工件结束".into()),
            Self::ResultWritten { result_code, fault_code } => (
                "info",
                format!("结果已写入并读回，resultSeq={seq}，resultCode={result_code}，faultCode={fault_code}"),
            ),
            Self::ResultFailed { reason } => ("error", format!("结果未能确认写入：{reason}")),
            Self::Interrupted { reason } => ("warn", format!("工件中断，结果未回写：{reason}")),
            Self::Skipped { reason, .. } => ("warn", format!("已跳过工件：{reason}")),
        };
        let mut payload = json!(self);
        payload["receivedAt"] = json!(received_at);
        payload["sn"] = json!(sn);
        payload["productCode"] = json!(product_code);
        engine.logbook().log(LogEntry {
            id: 0,
            ts: now_ms(),
            level: level.into(),
            category: "part".into(),
            point_id: None,
            point_name: None,
            message,
            old_value: None,
            new_value: Some(payload.to_string()),
            context: Some(format!("partSeq={seq}")),
        });
    }
}

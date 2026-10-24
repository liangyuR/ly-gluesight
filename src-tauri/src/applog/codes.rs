//! 运行日志的稳定事件编码：Agent 按 code 与 data 分析日志，ev 与 msg 只给人看。
//! 每个编码须同时登记在 docs/architecture/agent-logging-codes.md，`data` 的键只增不改。

use super::Level;

#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    pub code: &'static str,
    pub ev: &'static str,
    pub level: Level,
}

pub const APP_START: Event = Event { code: "app.start", ev: "启动", level: Level::Info };
pub const APP_CONFIG: Event = Event { code: "app.config", ev: "配置摘要", level: Level::Info };
pub const APP_PANIC: Event = Event { code: "app.panic", ev: "程序异常", level: Level::Error };
pub const APP_LOG_WRITE_FAILED: Event = Event { code: "app.log_write_failed", ev: "日志写入失败", level: Level::Warn };

#[cfg(test)]
const ALL: &[&Event] = &[&APP_START, &APP_CONFIG, &APP_PANIC, &APP_LOG_WRITE_FAILED];

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
        let doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/architecture/agent-logging-codes.md"));
        let mut documented: Vec<(String, String, String)> = doc
            .lines()
            .filter(|line| line.starts_with("| `"))
            .map(|line| {
                let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
                (cells[0].trim_matches('`').to_string(), cells[1].to_string(), cells[2].to_string())
            })
            .collect();
        let mut coded: Vec<(String, String, String)> = ALL.iter().map(|e| (e.code.to_string(), e.ev.to_string(), level_name(e.level).to_string())).collect();
        documented.sort();
        coded.sort();
        let mut codes: Vec<&str> = ALL.iter().map(|e| e.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len(), "编码重复");
        assert_eq!(documented, coded);
    }
}

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 帧录制：把一件工件的整帧图像与元数据落盘，供历史复测。离线期间临时为保留全部。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordMode {
    #[default]
    Off,
    /// 只留 NG / ERR 件
    Failed,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProductSource {
    Plc,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Timeouts {
    pub arm_ms: u64,
    pub motion_ms: u64,
    pub drain_ms: u64,
    pub proc_ms: u64,
    pub ack_ms: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { arm_ms: 500, motion_ms: 30_000, drain_ms: 1000, proc_ms: 3000, ack_ms: 5000 }
    }
}

/// 布防预算范围：上限远小于 PLC 等布防（协议 7 默认 10 s），PC 必须先于 PLC 超时给出布防或拒收；默认值依据见 docs/decisions/r2-plc.md R2P-1。
const ARM_MS_RANGE: std::ops::RangeInclusive<u64> = 100..=5000;

impl Timeouts {
    pub fn arm(&self) -> Duration {
        Duration::from_millis(self.arm_ms)
    }
    pub fn motion(&self) -> Duration {
        Duration::from_millis(self.motion_ms)
    }
    pub fn drain(&self) -> Duration {
        Duration::from_millis(self.drain_ms)
    }
    pub fn proc(&self) -> Duration {
        Duration::from_millis(self.proc_ms)
    }
    pub fn ack(&self) -> Duration {
        Duration::from_millis(self.ack_ms)
    }
}

/// 维护模式（在线投运）：开启后每件都布防采图，结果码写 0，PLC 按未检放行；项目级，重启后保持。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Maintenance {
    pub since: i64,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CycleSettings {
    pub product_source: ProductSource,
    pub manual_recipe_id: Option<String>,
    pub timeouts: Timeouts,
    pub history_days: u32,
    pub record: RecordMode,
    /// 帧录制最多保留多少件、总共多少 GB，超出删最旧的
    pub record_keep: u32,
    pub record_max_gb: f32,
    pub offline_record_keep: u32,
    pub offline_record_max_gb: f32,
    /// 只由维护模式命令改写，保存检测设置时沿用当前值
    pub maintenance: Option<Maintenance>,
}

impl Default for CycleSettings {
    fn default() -> Self {
        Self {
            product_source: ProductSource::Plc,
            manual_recipe_id: None,
            timeouts: Timeouts::default(),
            history_days: 180,
            record: RecordMode::Off,
            record_keep: 100,
            record_max_gb: 20.0,
            offline_record_keep: 300,
            offline_record_max_gb: 10.0,
            maintenance: None,
        }
    }
}

impl CycleSettings {
    /// 读不了的文件先备份，返回说明（节拍启动后写日志）。取值不符合当前范围的旧设置备份后按默认值重建。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match crate::fsio::read_json::<Self>(path) {
            Ok(None) => (Self::default(), None),
            Ok(Some(s)) => match s.validate() {
                Ok(()) => (s, None),
                Err(why) => (Self::default(), Some(Self::rebuild(path, &why))),
            },
            Err(note) => (Self::default(), Some(note)),
        }
    }

    fn rebuild(path: &Path, why: &str) -> String {
        let done = match crate::fsio::backup(path, "invalid") {
            Ok(backup) => match Self::default().save(path) {
                Ok(()) => format!("原文件已备份为 {}，已按默认值重建", backup.display()),
                Err(e) => format!("原文件已备份为 {}，重建失败（{e}），先按默认值运行", backup.display()),
            },
            Err(e) => format!("备份失败（{e}），原文件未改，先按默认值运行"),
        };
        format!("{} 的检测设置不符合当前取值范围（{why}），{done}", path.display())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::fsio::write_atomic(path, &serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
    }

    pub fn validate(&self) -> Result<(), String> {
        let t = &self.timeouts;
        if t.drain_ms < 200 {
            return Err("收尾等待不能小于 200 ms".into());
        }
        if !(1..=3650).contains(&self.history_days) {
            return Err("记录保留天数需在 1–3650 之间".into());
        }
        if t.motion_ms < 1000 || t.proc_ms < 200 || t.ack_ms < 500 {
            return Err("超时参数过小".into());
        }
        if !ARM_MS_RANGE.contains(&t.arm_ms) {
            return Err(format!("布防预算需在 {}–{} ms 之间：须明显小于 PLC 布防等待（10 s）", ARM_MS_RANGE.start(), ARM_MS_RANGE.end()));
        }
        if !(1..=100_000).contains(&self.record_keep) {
            return Err("帧录制保留件数需在 1–100000 之间".into());
        }
        if !(self.record_max_gb >= 0.5 && self.record_max_gb <= 10_000.0) {
            return Err("帧录制总大小需在 0.5–10000 GB 之间".into());
        }
        if !(1..=100_000).contains(&self.offline_record_keep) {
            return Err("离线原图保留件数需在 1–100000 之间".into());
        }
        if !(self.offline_record_max_gb >= 0.5 && self.offline_record_max_gb <= 10_000.0) {
            return Err("离线原图总大小需在 0.5–10000 GB 之间".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_budget_outside_100_to_5000_ms_is_rejected_so_pc_gives_up_before_plc_arm_timeout() {
        for (arm_ms, valid) in [(99, false), (100, true), (5000, true), (5001, false), (10_000, false)] {
            let settings = CycleSettings { timeouts: Timeouts { arm_ms, ..Timeouts::default() }, ..CycleSettings::default() };
            assert_eq!(settings.validate().is_ok(), valid, "arm_ms={arm_ms}");
        }
    }
}

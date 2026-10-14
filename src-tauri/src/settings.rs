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
        Self { arm_ms: 200, motion_ms: 30_000, drain_ms: 1000, proc_ms: 3000, ack_ms: 5000 }
    }
}

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
        }
    }
}

impl CycleSettings {
    /// 读不了的文件先备份，返回说明（节拍启动后写日志）。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match crate::fsio::read_json(path) {
            Ok(s) => (s.unwrap_or_default(), None),
            Err(note) => (Self::default(), Some(note)),
        }
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
    fn offline_retention_outside_supported_limits_is_rejected() {
        for (keep, max_gb, valid) in [
            (0, 10.0, false), (100_001, 10.0, false),
            (300, 0.49, false), (300, 10_001.0, false),
            (300, f32::NAN, false), (300, f32::INFINITY, false),
            (1, 0.5, true), (100_000, 10_000.0, true),
        ] {
            let settings = CycleSettings { offline_record_keep: keep, offline_record_max_gb: max_gb, ..CycleSettings::default() };
            assert_eq!(settings.validate().is_ok(), valid, "keep={keep}, max_gb={max_gb}");
        }
    }
}

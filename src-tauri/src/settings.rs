use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 帧录制：把一件工件的整帧图像与元数据落盘，供历史复测。离线期间临时为保留全部。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordMode {
    Off,
    /// 只留 NG / ERR 件
    Failed,
    #[default]
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
    pub motion_ms: u64,
    pub drain_ms: u64,
    pub proc_ms: u64,
    pub ack_ms: u64,
    /// 看到 Line0 门控关闭后再等多久就收尾（D-SB-54，帧从曝光到 PC 收到约 39 ms）；0 = 不按门控提前，固定等满 drain_ms
    pub gate_guard_ms: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            motion_ms: 1_200_000,
            drain_ms: 1000,
            proc_ms: 3000,
            ack_ms: 5000,
            gate_guard_ms: 80,
        }
    }
}

impl Timeouts {
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
    /// 按门控提前收尾的余量；关掉时为 None。
    pub fn gate_guard(&self) -> Option<Duration> {
        (self.gate_guard_ms > 0).then(|| Duration::from_millis(self.gate_guard_ms))
    }
}

/// 维护模式（在线投运）：开启后每件都采图，结果码写 0，PLC 按未检放行；项目级，重启后保持。
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
    /// PC 启动或 PLC 重连后、没有未结事务时自动复位握手；关闭时须在线检测页“软件复位”、PLC 复位信号或 PLC 置 99
    pub auto_reset: bool,
}

impl Default for CycleSettings {
    fn default() -> Self {
        Self {
            product_source: ProductSource::Plc,
            manual_recipe_id: None,
            timeouts: Timeouts::default(),
            history_days: 180,
            record: RecordMode::All,
            record_keep: 100,
            record_max_gb: 100.0,
            offline_record_keep: 300,
            offline_record_max_gb: 100.0,
            maintenance: None,
            auto_reset: false,
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

    /// 写项目的 `cycle.json`；历史保留天数是本机设置（`save_history_days`），不写进项目。
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(object) = value.as_object_mut() {
            object.remove("historyDays");
        }
        crate::fsio::write_atomic(path, &serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?)
    }

    pub fn validate(&self) -> Result<(), String> {
        let t = &self.timeouts;
        if t.drain_ms < 200 {
            return Err("收尾等待不能小于 200 ms".into());
        }
        if t.gate_guard_ms != 0 && !(40..=t.drain_ms).contains(&t.gate_guard_ms) {
            return Err("门控关闭后的收尾余量需为 0（不用）或 40 ms 到收尾等待之间".into());
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

/// 本机检测历史设置（资料目录 `config/history.json`）：历史保留天数，所有项目共用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryFile {
    history_days: u32,
}

/// 读本机历史保留天数；文件还没有时用 `seed`（当前项目旧 `cycle.json` 里的值）建立。读不了或越界时用 `seed` 并给出说明。
pub fn load_history_days(path: &Path, seed: u32) -> (u32, Option<String>) {
    match crate::fsio::read_json::<HistoryFile>(path) {
        Ok(Some(file)) if (1..=3650).contains(&file.history_days) => (file.history_days, None),
        Ok(Some(file)) => (seed, Some(format!("{} 的历史保留天数 {} 越界，先按 {seed} 天运行", path.display(), file.history_days))),
        Ok(None) => match save_history_days(path, seed) {
            Ok(()) => (seed, None),
            Err(error) => (seed, Some(format!("本机历史保留天数写入失败（{error}），先按 {seed} 天运行"))),
        },
        Err(note) => (seed, Some(note)),
    }
}

pub fn save_history_days(path: &Path, days: u32) -> Result<(), String> {
    crate::fsio::write_atomic(path, &serde_json::to_string_pretty(&HistoryFile { history_days: days }).map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline::test_dir::TestDir;

    /// 防的回归：现场旧版本的检测设置里还有已删除的 `recordBayer`；读入时报错会把整份设置备份后按默认值重建，
    /// 录制策略、保留上限等现场值全部丢失。
    #[test]
    fn old_settings_with_record_bayer_load_without_rebuild() {
        let dir = TestDir::new("settings-tests", "record-bayer");
        let path = dir.join("cycle-settings.json");
        std::fs::write(
            &path,
            r#"{"record":"all","recordKeep":42,"recordBayer":true}"#,
        )
        .unwrap();
        let (settings, note) = CycleSettings::load(&path);
        assert_eq!(
            (settings.record, settings.record_keep, note),
            (RecordMode::All, 42, None)
        );
    }
}

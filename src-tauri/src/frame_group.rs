//! 三台相机按 SDK 帧号配组（规格 §3）：同一触发序号的三帧配成一组，缺帧标 incomplete，丢帧只告警。
//!
//! 口径（docs/decisions/segment-burst.md）：seq = frame_counter − 该相机基线（从 1 起）；帧组数 = 各相机最大 seq 的最大值；
//! 单台相机的帧数只用于丢帧告警，不参与容差比较；下一件每台相机的基线 = max(原基线, 本件见到的最高 frame_counter)。

// 骨架（WP00）：函数体由 A2 填写，填完删掉 unused_variables；C3/D1 接入调用方后删掉 dead_code。
#![allow(dead_code, unused_variables)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// 一件（或一轮示教采集）的配组器。相机下标与计划相机顺序一致。
#[derive(Clone, Debug)]
pub struct Grouper {
    /// 每台相机的基线：frame_counter ≤ 基线的帧属于上一件
    baselines: Vec<u64>,
    /// 每台相机已接受的 seq
    seqs: Vec<BTreeSet<u64>>,
    /// 每台相机见到的最高 frame_counter（含 Duplicate，不含 Stale）
    max_counters: Vec<u64>,
}

/// 收到一帧后的归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    Accepted { seq: u64 },
    /// 同一相机同一 seq 已有帧：保留先到的那帧
    Duplicate,
    /// frame_counter 不大于基线：上一件的迟到帧，不计入本件
    Stale,
}

/// 一个帧组：同一 seq 下三台相机是否都到了。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameGroup {
    pub seq: u64,
    /// 按相机顺序
    pub present: Vec<bool>,
    /// 有任一相机缺帧
    pub incomplete: bool,
}

/// 帧数相对 N_exp ± tol 的位置；越界只告警（帧数异常），不改结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CountStatus {
    Normal,
    Low,
    High,
}

/// 丢帧告警的依据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LossReason {
    /// 与 PLC deviceTriggers 相差超过 1
    Triggers { device_triggers: u32 },
    /// 与另一台相机的帧数相差超过 1
    Cameras { other: usize, other_count: u32 },
}

/// 某台相机疑似丢帧：只告警，不升级故障、不拦停。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LossAlarm {
    /// 计划相机下标
    pub camera: usize,
    /// 这台相机的帧数
    pub count: u32,
    pub reason: LossReason,
}

impl Grouper {
    /// baselines 按计划相机顺序，取布防时各相机的 frame_counter 基线。
    pub fn new(baselines: Vec<u64>) -> Self {
        unimplemented!()
    }

    pub fn offer(&mut self, cam: usize, frame_counter: u64) -> Offer {
        unimplemented!()
    }

    /// seq 取 1..=各相机最大 seq 的最大值；长度即帧组数。末尾缺帧的相机同样标 incomplete。
    pub fn groups(&self) -> Vec<FrameGroup> {
        unimplemented!()
    }

    /// 每台相机已接受的帧数。
    pub fn camera_counts(&self) -> Vec<u32> {
        unimplemented!()
    }

    /// 该相机见到的最高 frame_counter，用作下一件基线；没见到帧时返回基线。
    pub fn max_counter(&self, cam: usize) -> u64 {
        unimplemented!()
    }
}

/// 丢帧判定：某台相机帧数与 deviceTriggers 相差 > 1，或与另一台相机相差 > 1 → 告警；
/// device_triggers 为 None 时只比较相机之间。
pub fn loss_alarms(counts: &[u32], device_triggers: Option<u32>) -> Vec<LossAlarm> {
    unimplemented!()
}

/// actual ∈ [expected − tol, expected + tol] 为 Normal。
pub fn count_status(actual: u32, expected: u32, tol: u32) -> CountStatus {
    unimplemented!()
}

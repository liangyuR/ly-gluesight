use serde::{Deserialize, Serialize};

/// 离线产线的工况：前几种只影响模拟相机合成图与模拟测量，后几种改变虚拟 PLC / 机器人的握手行为与脉冲串。
/// 名字与前端 `src/features/cycle/types.ts` 的 `Scenario` 联合类型一致（camelCase）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scenario {
    Normal,
    Excursion,
    Gap,
    /// 触发已发出并计数，帧在传输中丢失（三台相机同一帧都丢）
    LostFrame,
    /// 机器人不报运动结束（PLC 不置 20）
    MotionTimeout,
    /// PLC 核对 seqEcho / snEcho 不符，置 90（plcReason=5）
    AckMismatch,
    /// 三台相机之一（第 2 台）第 min(3, 总数) 帧传输丢失，其余相机照常
    CameraLost,
    /// 起头 / 收尾各 ±1 帧的脉冲数波动
    PulseJitter,
    /// 弯道减速，每个弯道段 ±2 帧波动
    CornerSlowdown,
    /// 机器人等不到触发许可，照常涂胶并置“本件未检”；PLC 不提交请求，PC 没有这一件
    NotInspected,
    /// PLC 提前报运动结束（提前终止），只发出约一半脉冲
    EarlyEnd,
    Random,
}

impl Scenario {
    /// 只改变虚拟 PLC / 机器人握手与脉冲串、不依赖合成图的工况；现场原图样本集只能用这些。
    pub fn handshake(self) -> bool {
        matches!(
            self,
            Scenario::Normal
                | Scenario::LostFrame
                | Scenario::MotionTimeout
                | Scenario::AckMismatch
                | Scenario::CameraLost
                | Scenario::PulseJitter
                | Scenario::CornerSlowdown
                | Scenario::NotInspected
                | Scenario::EarlyEnd
        )
    }

    /// 本工况在脉冲剖面上加的 (首尾波动, 弯道波动)。
    pub fn jitter(self) -> (i32, i32) {
        match self {
            Scenario::PulseJitter => (1, 0),
            Scenario::CornerSlowdown => (0, 2),
            _ => (0, 0),
        }
    }

    /// 本件 `total` 个脉冲里传输丢失的 (计划相机下标, seq)。
    pub fn lost_frames(self, total: u64) -> Vec<(usize, u64)> {
        match self {
            Scenario::CameraLost if total > 0 => vec![(1, 3.min(total))],
            Scenario::LostFrame if total > 0 => {
                (0..3).map(|camera| (camera, 4.min(total))).collect()
            }
            _ => Vec::new(),
        }
    }

    /// 提前终止前 PLC 发出的脉冲数——约一半，且少于 ΣN_exp − Σtol（帧组数低于正常下限才算疑似提前终止）。
    pub fn early_end_after(self, total: u64, tolerance_total: u32) -> Option<u64> {
        (self == Scenario::EarlyEnd).then(|| (total / 2).min(total.saturating_sub(u64::from(tolerance_total) + 1)).max(1))
    }

    /// 连续运行用的随机工况只在不打断握手的几种之间选。
    pub fn resolve(self, seed: u32) -> Scenario {
        if self != Scenario::Random {
            return self;
        }
        match seed % 25 {
            0 => Scenario::Gap,
            1 => Scenario::LostFrame,
            3 | 4 => Scenario::Excursion,
            _ => Scenario::Normal,
        }
    }
}

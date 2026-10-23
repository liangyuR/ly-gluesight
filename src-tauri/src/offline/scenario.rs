use serde::{Deserialize, Serialize};

use crate::recipe::Recipe;

/// 离线产线的工况：前几种只影响模拟相机合成图与模拟测量，后几种改变虚拟 PLC / 机器人的握手行为。
/// cameraLost 起的五种是段配方（分段连拍）的脉冲串工况，旧拍照点配方不能选（[`Scenario::burst_only`]）。
/// 名字与前端 `src/features/cycle/types.ts` 的 `Scenario` 联合类型一致（camelCase）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scenario {
    Normal,
    Excursion,
    Gap,
    /// 触发已发出并计数，帧在传输中丢失（段配方：三台相机同一帧都丢）
    LostFrame,
    /// 一帧定位失败、胶正常：该帧有胶站没有位置基准，整件 ERR 92
    LocateFail,
    /// 定位失败的那一帧里还断胶：NG 优先于基准无效，整件 NG_GAP
    LocateFailGap,
    /// 旧拍照点配方：机器人少发一次触发，PLC 报的设备触发数与计划不符；
    /// 段配方：PLC 多计 2 个脉冲，deviceTriggers 比相机帧数多 2（只告警）
    CountMismatch,
    /// 机器人不发 partEnd
    MotionTimeout,
    /// PLC 回 ACK 时序号与 resultSeq 不匹配
    AckMismatch,
    /// 段配方：三台相机之一（第 2 台）第 min(3, 总数) 帧传输丢失，其余相机照常
    CameraLost,
    /// 段配方：起头 / 收尾各 ±1 帧的脉冲数波动
    PulseJitter,
    /// 段配方：弯道减速，每个弯道段 ±2 帧波动
    CornerSlowdown,
    /// 段配方：机器人等不到触发许可，照常涂胶并置“本件未检”；PLC 不提交 partStart，PC 没有这一件
    NotInspected,
    /// 段配方：PLC 提前置 partEnd（提前终止），只发出约一半脉冲
    EarlyEnd,
    Random,
}

impl Scenario {
    /// 只改变虚拟 PLC / 机器人握手与脉冲串、不依赖合成图的工况；现场原图样本集只能用这些。
    pub fn handshake(self) -> bool {
        matches!(self, Scenario::Normal | Scenario::LostFrame | Scenario::CountMismatch | Scenario::MotionTimeout | Scenario::AckMismatch)
            || self.burst_only()
    }

    /// 只对段配方有意义的脉冲串工况。
    pub fn burst_only(self) -> bool {
        matches!(self, Scenario::CameraLost | Scenario::PulseJitter | Scenario::CornerSlowdown | Scenario::NotInspected | Scenario::EarlyEnd)
    }

    pub fn lost_frame(self, n: usize) -> Option<usize> {
        (self == Scenario::LostFrame && n > 0).then(|| 3.min(n - 1))
    }

    /// 段配方：本工况在脉冲剖面上加的 (首尾波动, 弯道波动)。
    pub fn jitter(self) -> (i32, i32) {
        match self {
            Scenario::PulseJitter => (1, 0),
            Scenario::CornerSlowdown => (0, 2),
            _ => (0, 0),
        }
    }

    /// 段配方：本件 `total` 个脉冲里传输丢失的 (计划相机下标, seq)。
    pub fn lost_frames(self, total: u64) -> Vec<(usize, u64)> {
        match self {
            Scenario::CameraLost if total > 0 => vec![(1, 3.min(total))],
            Scenario::LostFrame => self.lost_frame(total as usize).map_or_else(Vec::new, |k| (0..3).map(|camera| (camera, k as u64 + 1)).collect()),
            _ => Vec::new(),
        }
    }

    /// 段配方：提前终止前 PLC 发出的脉冲数——约一半，且少于 ΣN_exp − Σtol（帧组数低于正常下限才算疑似提前终止）。
    pub fn early_end_after(self, total: u64, tolerance_total: u32) -> Option<u64> {
        (self == Scenario::EarlyEnd).then(|| (total / 2).min(total.saturating_sub(u64::from(tolerance_total) + 1)).max(1))
    }

    pub fn locate_fail_frame(self, n: usize) -> Option<usize> {
        (matches!(self, Scenario::LocateFail | Scenario::LocateFailGap) && n > 0).then(|| 4.min(n - 1))
    }

    /// 断胶场景：第二段（只有一段时第一段）中部缺一段，比这段允许的断口长 2 mm；
    /// 定位失败带断胶时缺在定位失败那一帧的段上。
    pub fn gap_points(self, recipe: &Recipe) -> Vec<usize> {
        let segment = match self {
            Scenario::Gap => recipe.segments.get(1).or(recipe.segments.first()),
            Scenario::LocateFailGap => self.locate_fail_frame(recipe.shot_count()).and_then(|k| recipe.shot_segment(k)),
            _ => None,
        };
        let Some(g) = segment else { return Vec::new() };
        let n = (((g.max_gap_len + 2.0) / recipe.spacing).ceil() as usize).min(g.count);
        let from = g.first + (g.count - n) / 2;
        (from..from + n).collect()
    }

    /// 连续运行用的随机工况只在不打断握手的几种之间选。
    pub fn resolve(self, seed: u32) -> Scenario {
        if self != Scenario::Random {
            return self;
        }
        match seed % 25 {
            0 => Scenario::Gap,
            1 => Scenario::LostFrame,
            2 => Scenario::LocateFail,
            3 | 4 => Scenario::Excursion,
            _ => Scenario::Normal,
        }
    }
}

use serde::{Deserialize, Serialize};

use crate::recipe::Recipe;

/// 离线产线的工况：前几种只影响模拟相机合成图与模拟测量，后几种改变虚拟 PLC / 机器人的握手行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scenario {
    Normal,
    Excursion,
    Gap,
    /// 触发已发出并计数，帧在传输中丢失
    LostFrame,
    /// 一帧定位失败、胶正常：该帧有胶站没有位置基准，整件 ERR 92
    LocateFail,
    /// 定位失败的那一帧里还断胶：NG 优先于基准无效，整件 NG_GAP
    LocateFailGap,
    /// 机器人少发一次触发，PLC 报的设备触发数与计划不符
    CountMismatch,
    /// 机器人不发 partEnd
    MotionTimeout,
    /// PLC 回 ACK 时序号与 resultSeq 不匹配
    AckMismatch,
    Random,
}

impl Scenario {
    /// 只改变虚拟 PLC / 机器人握手、不依赖合成图的工况；现场原图样本集只能用这些。
    pub fn handshake(self) -> bool {
        matches!(self, Scenario::Normal | Scenario::LostFrame | Scenario::CountMismatch | Scenario::MotionTimeout | Scenario::AckMismatch)
    }

    pub fn lost_frame(self, n: usize) -> Option<usize> {
        (self == Scenario::LostFrame && n > 0).then(|| 3.min(n - 1))
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

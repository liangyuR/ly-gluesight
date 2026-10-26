use serde::{Deserialize, Serialize};

/// 声明顺序即严重程度，整件结果取各段最严重的一个。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Ok,
    OkWithExcursion,
    NgPosition,
    NgWidth,
    NgAbsolute,
    NgGap,
    ErrInspect,
}

impl Verdict {
    pub fn plc_code(self) -> u16 {
        match self {
            Verdict::Ok => 1,
            Verdict::OkWithExcursion => 2,
            Verdict::NgPosition => 11,
            Verdict::NgAbsolute => 12,
            Verdict::NgGap => 13,
            Verdict::NgWidth => 14,
            Verdict::ErrInspect => 90,
        }
    }
}

pub mod fault {
    pub const MISSING_FRAME: u16 = 91;
    pub const INVALID_POINTS: u16 = 93;
    pub const NO_RECIPE: u16 = 95;
    pub const MOTION_TIMEOUT: u16 = 97;
    pub const DEVICE_LOST: u16 = 98;
    pub const PROCESS_TIMEOUT: u16 = 99;
    pub const PLAN_MISMATCH: u16 = 100;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointState {
    Pending,
    /// 有胶。d：胶条中线相对位置基准的横向偏移，None 为有胶、无位置基准（ST_NO_REF，如定位失败）；
    /// w：胶宽（没测时为 NaN）
    Measured { d: Option<f32>, w: f32 },
    /// 超出检测范围：位姿把示教胶路沿切向搬动后，这一站落在搬过去的胶路两端之外，几何上本来就测不了。
    /// 不判胶偏、不判断胶、不计入有胶比例，也不计入测不了、无基准的连续长度；测到了胶宽（w 为 Some）时照常判胶宽。
    OutOfRange { w: Option<f32> },
    /// 沿示教中线找了、没有胶
    Gap,
    /// 该点测不了（被遮挡、出了图像、胶边被截断）
    Invalid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentResult {
    pub verdict: Verdict,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub excursion_len: f32,
    #[serde(default)]
    pub w_min: Option<f32>,
    #[serde(default)]
    pub w_max: Option<f32>,
    #[serde(default)]
    pub w_excursion_len: f32,
    /// 位置基准无效：有位置判定的段里无基准的站连续超过 2 mm 或有胶站全都没有基准，或整段超出检测范围。
    /// 本段位置判不了（ERR）；本段或别处另有 NG 时取 NG。
    #[serde(default)]
    pub reference_invalid: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GapRun {
    pub segment: usize,
    pub s0: f32,
    pub s1: f32,
    pub len: f32,
    /// 缺胶点所在的拍照点
    pub frames: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Judgement {
    pub verdict: Verdict,
    pub plc_code: u16,
    pub fault_code: u16,
    pub reason: String,
    pub segments: Vec<SegmentResult>,
    pub gaps: Vec<GapRun>,
}

impl Judgement {
    pub fn error(fault_code: u16, reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::ErrInspect,
            plc_code: Verdict::ErrInspect.plc_code(),
            fault_code,
            reason: reason.into(),
            segments: Vec::new(),
            gaps: Vec::new(),
        }
    }
}

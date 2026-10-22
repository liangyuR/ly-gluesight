//! 段模型判胶（规格 §4）：按区域类型分流。每帧独立指标（有胶 / 断胶、胶宽、溢胶）三类区域都跑；
//! 胶偏只在直线段；结果标“检测不完整”“未检区域”“胶偏未判”“帧数异常”“疑似提前终止”。
//!
//! 与旧拍照点模型的 `judge::Judgement` 并存：旧配方仍用 `judge`，段配方用这里的 `BurstJudgement`。

// 骨架（WP00）：函数体由 A4 填写，填完删掉 unused_variables；C3 接入调用方后删掉 dead_code。
#![allow(dead_code, unused_variables)]

use serde::{Deserialize, Serialize};

use crate::frame_group::{CountStatus, FrameGroup, LossAlarm};
use crate::judge::Verdict;
use crate::plan::PathSegment;
use crate::segmentation::Region;

/// 一帧的测量状态。除 Measured 外都只记“检测不完整”，不影响 OK/NG。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FrameState {
    Measured,
    /// 相机没出图，或回调通道满、内存预算满被丢弃
    Missing,
    /// 测量失败（lyFlow 运行失败等），原因在 `FrameMetrics.error`
    Error,
    /// 没测：测量队列满或超时
    Unmeasured,
    /// 胶条走出本相机的走廊（弯道帧常见，默认做法 4）：记检测不完整，不判 NG
    OutOfCorridor,
}

/// 一台相机一帧的独立指标；示教轮次写进 features.jsonl，供分段算法和标定使用。
/// 除 state / error 外的数值只在 state = Measured 时有意义。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameMetrics {
    /// 计划相机下标
    pub camera: usize,
    pub seq: u64,
    pub state: FrameState,
    #[serde(default)]
    pub error: Option<String>,
    pub width_min: Option<f64>,
    pub width_max: Option<f64>,
    pub width_mean: Option<f64>,
    /// 沿走廊逐站胶宽（mm，None = 该站无胶或测不了），用来算帧内超差长度
    #[serde(default)]
    pub widths: Vec<Option<f32>>,
    /// 走廊站距（mm）
    #[serde(default)]
    pub spacing_mm: f64,
    /// 帧内最长连续缺胶（mm）
    pub gap_max_mm: f64,
    /// 帧内有胶站占比（0–1）
    pub present_ratio: f64,
    /// 帧内胶中心相对走廊的法向偏移均值（mm），即 AbsoluteX 测量量
    pub x_mm: Option<f64>,
}

/// 区域内 Measured 帧的胶宽统计（mm）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WidthStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

/// 直线段胶偏统计（去掉区域首尾各一帧后）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OffsetStat {
    /// x 均值 − x_ref
    pub mean_shift: f64,
    /// 各帧 x 与均值的最大偏差
    pub max_dev: f64,
}

/// 胶偏未判的原因（结果标“胶偏未判：…”）。Corner 区域本来就不判，不标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OffsetUnjudged {
    /// 未分段（分段算法未就绪）
    Unsegmented,
    /// 分段不确定（边界置信度不足）
    LowConfidence,
    /// 测量量未实现（ToEdge，默认做法 2）
    MeasureNotImplemented,
    /// 启用了胶偏但标准件 x_ref 未标定
    NoReference,
}

/// 一个区域的判定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegionResult {
    pub region: Region,
    pub verdict: Verdict,
    /// 中文摘要（如“溢胶”“胶宽不均”），OK 时为空
    pub reason: String,
    /// 区域帧组数 = `Region::frame_count()`
    pub frame_count: u32,
    /// 比较用的预期帧组数：单段区域取该段 N_exp，覆盖全部段的未分段区域取 expected_total；无从比较为 None
    pub expected: Option<u32>,
    pub tolerance: u32,
    /// expected 为 None 时为 None
    pub count_status: Option<CountStatus>,
    /// 区域内各相机帧数，按计划相机顺序
    pub camera_frames: Vec<u32>,
    pub width: Option<WidthStats>,
    pub offset: Option<OffsetStat>,
    pub offset_unjudged: Option<OffsetUnjudged>,
    /// 区域内缺帧（含非 Measured 帧）的帧组 seq
    pub incomplete_groups: Vec<u64>,
}

/// 整件标记：都只告警，不改 OK/NG。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Flags {
    /// 检测不完整：有帧组缺帧或有帧没测成
    pub incomplete: bool,
    /// 帧数异常：有区域帧组数超出 N_exp ± tol
    pub frame_count_anomaly: bool,
    pub offset_unjudged: bool,
    /// 含未分段区域
    pub unsegmented: bool,
    /// 疑似提前终止：整件帧组数 < expected_total − tolerance_total（默认做法 3）
    pub short_part: bool,
}

/// 未检区域：段内 [0, a) 与 (b, length_mm]；length 未知时 to_mm 为 None。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Uninspected {
    pub segment: u32,
    pub from_mm: f64,
    pub to_mm: Option<f64>,
}

/// 本轮没有算法的检测项，结果中标“未检项”（默认做法 1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UncheckedItem {
    Bubble,
    Stringing,
}

/// 段配方一件的判定结果。verdict / plc_code / fault_code / reason 的含义与 `judge::Judgement` 相同。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BurstJudgement {
    pub verdict: Verdict,
    pub plc_code: u16,
    pub fault_code: u16,
    pub reason: String,
    /// 整件帧组数
    pub group_count: u32,
    /// 整件帧组数相对 expected_total ± tolerance_total；未标定为 None
    pub count_status: Option<CountStatus>,
    pub regions: Vec<RegionResult>,
    pub flags: Flags,
    /// 丢帧告警，原样带出
    pub alarms: Vec<LossAlarm>,
    pub uninspected: Vec<Uninspected>,
    pub unchecked: Vec<UncheckedItem>,
}

impl BurstJudgement {
    /// 判不了（ERR）：如三台相机整件一帧都没有（91）、会话变化（98）。
    pub fn error(fault_code: u16, reason: impl Into<String>) -> Self {
        unimplemented!()
    }
}

/// 判一件。cameras 是计划相机（`FrameMetrics.camera` 的下标基准，`OffsetRef.camera` 按它找下标）；
/// expected_total 由 `plan::expected_total` 算出；三台相机取最差。
pub fn judge(
    segments: &[PathSegment],
    expected_total: Option<u32>,
    regions: &[Region],
    groups: &[FrameGroup],
    metrics: &[FrameMetrics],
    cameras: &[String],
    loss: &[LossAlarm],
) -> BurstJudgement {
    unimplemented!()
}

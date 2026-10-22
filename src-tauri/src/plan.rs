//! 段计划（分段连拍飞拍，规格 §1）：配方里的段列表、各段与整件的标定统计、计划身份与导出。
//!
//! 与旧拍照点模型并存：`RecipeDoc.segments` 为空的配方仍按拍照点模型走旧校验、旧节拍、旧发布，
//! 不经过本模块；非空即段模型。`recipe::Segment`（旧模型的测量段）与这里的 `PathSegment` 同名不同义。
//! 契约与口径见 docs/decisions/segment-burst.md。

// 骨架（WP00）：函数体由 A1 填写，填完删掉 unused_variables；C1 接入调用方后删掉 dead_code。
#![allow(dead_code, unused_variables)]

use serde::{Deserialize, Serialize};

/// 段类型：直线段判胶偏，弯道段不判。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SegmentKind {
    Straight,
    Corner,
}

/// 胶宽（mm），每帧独立判。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WidthCheck {
    pub enabled: bool,
    pub nominal_mm: f64,
    pub tol_upper: f64,
    pub tol_lower: f64,
    /// 一帧内允许的累计超差长度（mm）：不超过时为 OkWithExcursion
    pub max_excursion_mm: f64,
    /// 均匀性：区域内各帧 width_mean 的极差上限（mm），只在 Straight 区域判；None 不判
    #[serde(default)]
    pub max_range_mm: Option<f64>,
}

/// 断胶，每帧独立判。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BreakCheck {
    pub enabled: bool,
    /// 帧内允许的最长连续缺胶（mm）
    pub max_gap_mm: f64,
    /// 帧内有胶站的最低占比（0–1）
    pub min_present: f64,
}

/// 溢胶：每帧最大胶宽超过上限判 NG_WIDTH，原因“溢胶”（决策补充默认做法 1）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverflowCheck {
    pub enabled: bool,
    pub max_width_mm: f64,
}

/// 段级胶偏（只在 Straight 段）：去掉区域首尾各一帧后，x 均值 − x_ref 判整段偏移，最大偏差判局部跑偏。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OffsetCheck {
    pub enabled: bool,
    pub max_mean_shift_mm: f64,
    pub max_local_dev_mm: f64,
}

/// 一段的检测项与允差。`gap` 在 JSON 中叫 `break`（关键字避让）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SegmentChecks {
    pub width: WidthCheck,
    #[serde(rename = "break")]
    pub gap: BreakCheck,
    pub overflow: OverflowCheck,
    /// 只有 Straight 段可以有；Corner 段必须为 None（validate_segments 拒绝）
    #[serde(default)]
    pub offset: Option<OffsetCheck>,
}

/// 胶偏测量量。本轮只实现 AbsoluteX；选 ToEdge 的段结果标“胶偏未判：测量量未实现”，x_ref 也不计算（默认做法 2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OffsetMeasure {
    ToEdge,
    AbsoluteX,
}

/// 胶偏参考（只在 Straight 段）。x_ref / spread 由标准件多轮采集算出，不能手填。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OffsetRef {
    pub measure: OffsetMeasure,
    /// 用哪台相机的帧测（三台相机几何不同，不能共用 x_ref）；必须属于计划相机，默认第一台
    pub camera: String,
    /// 标准件多轮 x 均值（mm）；None = 未标定（ToEdge 恒为 None）
    #[serde(default)]
    pub x_ref: Option<f64>,
    /// 标准件多轮 x 的 max − min（mm），与 x_ref 同时有值
    #[serde(default)]
    pub spread: Option<f64>,
}

/// 多轮采集的帧组数统计，供容差建议与追溯。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SegmentCalib {
    pub rounds: u32,
    pub min: u32,
    pub max: u32,
    pub mean: f64,
}

/// 检测区间 [a_mm, b_mm]（沿本段胶路），区间外记“未检区域”。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectRange {
    pub a_mm: f64,
    pub b_mm: f64,
}

/// 沿胶路的一段（规格 §1 的 Segment）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathSegment {
    /// 1..=N，沿胶路顺序连续
    pub index: u32,
    pub kind: SegmentKind,
    /// 机器人工程师提供的段长（mm），只用于估算和未检区域
    #[serde(default)]
    pub length_mm: Option<f64>,
    /// N_exp：只能由多轮采集写入，None = 待标定（待分段算法）
    #[serde(default)]
    pub expected_frames: Option<u32>,
    /// 帧数容差，默认 1
    pub tolerance: u32,
    #[serde(default)]
    pub calib: Option<SegmentCalib>,
    pub checks: SegmentChecks,
    #[serde(default)]
    pub offset_ref: Option<OffsetRef>,
    #[serde(default)]
    pub inspect_range: Option<InspectRange>,
}

/// 整件标定：多轮采集统计整件帧组数（不依赖分段）。本轮各段 N_exp 可以为 None，
/// 多段配方凭它得到 expected_total 并发布（决策补充“多段配方的发布与运行”）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartCalib {
    /// 各轮整件帧组数均值四舍五入
    pub expected_total: u32,
    pub calib: SegmentCalib,
}

/// 一种段类型的默认容差与检测项；新建段时取用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KindDefault {
    pub tolerance: u32,
    pub checks: SegmentChecks,
}

/// 配方级默认参数：直线段与弯道段各一套。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KindDefaults {
    pub straight: KindDefault,
    pub corner: KindDefault,
}

impl Default for KindDefaults {
    /// 容差默认 1（默认做法 7）；限值沿用旧模型 `recipe::default_limits` 的胶宽 4.0 ± 1.5 mm、
    /// 允许超差 5 mm、断胶 6 mm、有胶比例 0.8，溢胶上限取旧胶宽绝对上限 8 mm，胶偏 ±2 mm。
    fn default() -> Self {
        let checks = |offset: Option<OffsetCheck>| SegmentChecks {
            width: WidthCheck { enabled: true, nominal_mm: 4.0, tol_upper: 1.5, tol_lower: 1.5, max_excursion_mm: 5.0, max_range_mm: None },
            gap: BreakCheck { enabled: true, max_gap_mm: 6.0, min_present: 0.8 },
            overflow: OverflowCheck { enabled: true, max_width_mm: 8.0 },
            offset,
        };
        Self {
            straight: KindDefault {
                tolerance: 1,
                checks: checks(Some(OffsetCheck { enabled: true, max_mean_shift_mm: 2.0, max_local_dev_mm: 2.0 })),
            },
            corner: KindDefault { tolerance: 1, checks: checks(None) },
        }
    }
}

/// 计划身份：相等即沿用 plan_version，不等则配方库分配新版本。
/// 组成：产品代码 + 各段 (index, kind, expected_frames) + 有序相机 + expected_total。
/// 限值、容差、x_ref、inspect_range、length_mm 不参与（规格 §1）；N_exp / expected_total 参与（默认做法 6）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanIdentity {
    product_code: u16,
    segments: Vec<(u32, SegmentKind, Option<u32>)>,
    cameras: Vec<String>,
    expected_total: Option<u32>,
}

/// 导出给 PLC / 机器人工程师的一段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSegment {
    pub index: u32,
    pub kind: SegmentKind,
    pub length_mm: Option<f64>,
    /// None = 待标定（本轮多段配方常见）
    pub expected_frames: Option<u32>,
    pub tolerance: u32,
    pub inspect_range: Option<InspectRange>,
}

/// 计划导出 JSON（规格 §1）：PLC 的 shotCount 与 deviceShots 都填 expected_total。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanExport {
    /// 等于 `handshake::PROTOCOL_VERSION`
    pub protocol_version: u16,
    pub recipe_id: String,
    pub product_code: u16,
    pub plan_version: u32,
    pub cameras: Vec<String>,
    pub segments: Vec<ExportSegment>,
    /// ΣN_exp；未标定为 None
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
}

/// 段列表的模型约束（规格 §1/§5）：index 从 1 连续；cameras 恰好 3 个且不重复；Corner 段不得带 offset / offset_ref；
/// offset_ref.camera 属于 cameras；inspect_range 满足 0 ≤ a < b；tolerance ≤ 1000；ΣN_exp ≤ 32767。
pub fn validate_segments(segments: &[PathSegment], cameras: &[String]) -> Result<(), String> {
    unimplemented!()
}

/// 整件预期帧组数：各段 N_exp 都已标定时为其和；否则取整件标定（PartCalib）；都没有为 None。
pub fn expected_total(segments: &[PathSegment], part: Option<&PartCalib>) -> Option<u32> {
    unimplemented!()
}

/// 整件容差 = Σ各段 tolerance。
pub fn tolerance_total(segments: &[PathSegment]) -> u32 {
    unimplemented!()
}

/// `expected_total` 由调用方先用 [`expected_total`] 算好传入。
pub fn plan_identity(product_code: u16, segments: &[PathSegment], cameras: &[String], expected_total: Option<u32>) -> PlanIdentity {
    unimplemented!()
}

/// 未分段区域用的较宽一档：上限取大、下限取小、max_gap 取大、min_present 取小、溢胶上限取大，
/// max_range_mm 与 offset 置 None（未分段不判均匀性和胶偏）。
pub fn wider_checks(checks: &[&SegmentChecks]) -> SegmentChecks {
    unimplemented!()
}

/// 新建一段：tolerance 与 checks 取对应 kind 的默认值，expected_frames / calib / offset_ref 为 None。
pub fn new_segment(index: u32, kind: SegmentKind, defaults: &KindDefaults) -> PathSegment {
    unimplemented!()
}

/// 计划导出：不要求已标定（未标定时 expected_total 为 None）。
pub fn export(
    recipe_id: &str,
    product_code: u16,
    plan_version: u32,
    segments: &[PathSegment],
    cameras: &[String],
    part: Option<&PartCalib>,
) -> PlanExport {
    unimplemented!()
}

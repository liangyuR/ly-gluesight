//! 段计划（分段连拍飞拍，规格 §1）：配方里的段列表、各段与整件的标定统计、计划身份与导出。
//!
//! 与旧拍照点模型并存：`RecipeDoc.segments` 为空的配方仍按拍照点模型走旧校验、旧节拍、旧发布，
//! 不经过本模块；非空即段模型。`recipe::Segment`（旧模型的测量段）与这里的 `PathSegment` 同名不同义。
//! 契约与口径见 docs/decisions/segment-burst.md。

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
    /// 检测相机（D-SB-41）：计划相机的子集，按段判定时只用这几台；空 = 全部计划相机（旧数据与新建段的默认）
    #[serde(default)]
    pub cameras: Vec<String>,
}

/// 一段实际参与判定的相机（按计划相机顺序）：段没指定时为全部计划相机。
pub fn segment_cameras(segment: &PathSegment, plan_cameras: &[String]) -> Vec<String> {
    plan_cameras
        .iter()
        .filter(|camera| segment.cameras.is_empty() || segment.cameras.contains(camera))
        .cloned()
        .collect()
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
/// 组成：产品代码 + 各段 (index, kind, expected_frames, 检测相机) + 有序相机 + expected_total（段检测相机按 D-SB-41 进入身份）。
/// 限值、容差、x_ref、inspect_range、length_mm 不参与（规格 §1）；N_exp / expected_total 参与（默认做法 6）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanIdentity {
    product_code: u16,
    segments: Vec<(u32, SegmentKind, Option<u32>, Vec<String>)>,
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

/// 段计划的相机数：三台独立相机共用一个触发（规格 §0/§1）。
pub const PLAN_CAMERAS: usize = 3;
/// 单段帧数容差上限（D-SB-31）。
pub const MAX_TOLERANCE: u32 = 1000;
/// ΣN_exp 上限：PLC shotCount 为 Int（D-SB-31）。
pub const MAX_EXPECTED_TOTAL: u32 = i16::MAX as u32;

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn non_negative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

/// 启用的检测项限值合法（停用项不校验）：胶宽名义值 > 0，上下允差、允许超差长度、均匀性极差上限 ≥ 0；
/// 断胶长度 ≥ 0、有胶比例 ∈ [0, 1]；溢胶上限 > 0；胶偏两个限值 ≥ 0。`what` 是报错主语。
fn validate_checks(checks: &SegmentChecks, what: &str) -> Result<(), String> {
    let width = &checks.width;
    if width.enabled {
        if !positive(width.nominal_mm) {
            return Err(format!("{what}：胶宽名义值 {} mm 需为正数", width.nominal_mm));
        }
        let tolerances = [width.tol_upper, width.tol_lower, width.max_excursion_mm];
        if !(tolerances.into_iter().all(non_negative) && width.max_range_mm.is_none_or(non_negative)) {
            return Err(format!("{what}：胶宽上下允差、允许超差长度与均匀性极差上限需为不小于 0 的有限数"));
        }
    }
    if checks.gap.enabled {
        if !non_negative(checks.gap.max_gap_mm) {
            return Err(format!("{what}：允许断胶长度需为不小于 0 的有限数"));
        }
        if !(0.0..=1.0).contains(&checks.gap.min_present) {
            return Err(format!("{what}：最低有胶比例需在 0–100% 之间"));
        }
    }
    if checks.overflow.enabled && !positive(checks.overflow.max_width_mm) {
        return Err(format!("{what}：溢胶胶宽上限 {} mm 需为正数", checks.overflow.max_width_mm));
    }
    if let Some(offset) = checks.offset.as_ref().filter(|offset| offset.enabled) {
        if !(non_negative(offset.max_mean_shift_mm) && non_negative(offset.max_local_dev_mm)) {
            return Err(format!("{what}：胶偏整段偏移与局部跑偏限值需为不小于 0 的有限数"));
        }
    }
    Ok(())
}

/// 一段或一种段类型默认参数共用的约束：tolerance ≤ 1000；Corner 不得带 offset 检测；启用的检测项限值合法。
fn validate_kind_settings(kind: SegmentKind, tolerance: u32, checks: &SegmentChecks, what: &str) -> Result<(), String> {
    if tolerance > MAX_TOLERANCE {
        return Err(format!("{what}的帧数容差 {tolerance} 超过上限 {MAX_TOLERANCE}"));
    }
    if kind == SegmentKind::Corner && checks.offset.is_some() {
        return Err(format!("{what}：弯道段不判胶偏，不能配置胶偏检测"));
    }
    validate_checks(checks, what)
}

/// 配方的直线 / 弯道默认参数：`new_segment` 从这里取值，默认非法就会生成非法段，按段的同一套约束校验。
pub fn validate_defaults(defaults: &KindDefaults) -> Result<(), String> {
    validate_kind_settings(SegmentKind::Straight, defaults.straight.tolerance, &defaults.straight.checks, "直线段默认参数")?;
    validate_kind_settings(SegmentKind::Corner, defaults.corner.tolerance, &defaults.corner.checks, "弯道段默认参数")
}

/// 段列表的模型约束（规格 §1/§5）：至少一段（空段列表是旧拍照点配方，不经过本函数）；index 从 1 连续；cameras 恰好 3 个且不重复；Corner 段不得带 offset / offset_ref；
/// 段检测相机是计划相机的子集且不重复（空为全部）；offset_ref.camera 属于该段的检测相机；启用的检测项限值合法（见 `validate_checks`）；length_mm 有值时 > 0；
/// inspect_range 满足 0 ≤ a < b，有段长时 b ≤ length_mm；tolerance ≤ 1000；ΣN_exp ≤ 32767。
pub fn validate_segments(segments: &[PathSegment], cameras: &[String]) -> Result<(), String> {
    if cameras.len() != PLAN_CAMERAS {
        return Err(format!("段配方需要恰好 {PLAN_CAMERAS} 台相机共用一个触发，当前 {} 台", cameras.len()));
    }
    if cameras.iter().enumerate().any(|(i, id)| id.trim().is_empty() || cameras[..i].contains(id)) {
        return Err(format!("段配方的 {PLAN_CAMERAS} 台相机设备 ID 必须非空且互不相同：{cameras:?}"));
    }
    if segments.is_empty() {
        return Err("段配方至少需要一段".into());
    }
    let mut frames = 0u64;
    for (k, segment) in segments.iter().enumerate() {
        let index = segment.index;
        if index as usize != k + 1 {
            return Err(format!("第 {} 段的序号为 {index}：段序号必须从 1 起沿胶路连续递增", k + 1));
        }
        if segment.kind == SegmentKind::Corner && segment.offset_ref.is_some() {
            return Err(format!("第 {index} 段是弯道段，不判胶偏，不能配置胶偏参考"));
        }
        validate_kind_settings(
            segment.kind,
            segment.tolerance,
            &segment.checks,
            &format!("第 {index} 段"),
        )?;
        if segment
            .cameras
            .iter()
            .enumerate()
            .any(|(i, id)| !cameras.contains(id) || segment.cameras[..i].contains(id))
        {
            return Err(format!(
                "第 {index} 段检测相机 {:?} 须是计划相机 {cameras:?} 中互不相同的几台",
                segment.cameras
            ));
        }
        if let Some(offset_ref) = &segment.offset_ref {
            if !segment_cameras(segment, cameras).contains(&offset_ref.camera) {
                return Err(format!(
                    "第 {index} 段胶偏参考相机 {} 不在该段检测相机中",
                    offset_ref.camera
                ));
            }
        }
        if let Some(length) = segment.length_mm {
            if !positive(length) {
                return Err(format!("第 {index} 段段长 {length} mm 无效：需为正数"));
            }
        }
        if let Some(range) = &segment.inspect_range {
            if !(range.a_mm.is_finite() && range.b_mm.is_finite() && 0.0 <= range.a_mm && range.a_mm < range.b_mm) {
                return Err(format!("第 {index} 段检测区间 [{}, {}] mm 无效：要求 0 ≤ a < b", range.a_mm, range.b_mm));
            }
            if let Some(length) = segment.length_mm.filter(|&length| range.b_mm > length) {
                return Err(format!("第 {index} 段检测区间终点 {} mm 超出段长 {length} mm", range.b_mm));
            }
        }
        frames += u64::from(segment.expected_frames.unwrap_or(0));
    }
    if frames > MAX_EXPECTED_TOTAL as u64 {
        return Err(format!("各段预期帧数之和 {frames} 超过 PLC shotCount 上限 {MAX_EXPECTED_TOTAL}"));
    }
    Ok(())
}

/// 整件预期帧组数：各段 N_exp 都已标定时为其和；否则取整件标定（PartCalib）；都没有为 None。
pub fn expected_total(segments: &[PathSegment], part: Option<&PartCalib>) -> Option<u32> {
    let summed = (!segments.is_empty())
        .then(|| segments.iter().try_fold(0u32, |sum, segment| segment.expected_frames.map(|n| sum.saturating_add(n))))
        .flatten();
    summed.or_else(|| part.map(|part| part.expected_total))
}

/// 整件容差 = Σ各段 tolerance。
pub fn tolerance_total(segments: &[PathSegment]) -> u32 {
    segments.iter().fold(0u32, |sum, segment| sum.saturating_add(segment.tolerance))
}

/// `expected_total` 由调用方先用 [`expected_total`] 算好传入。
pub fn plan_identity(product_code: u16, segments: &[PathSegment], cameras: &[String], expected_total: Option<u32>) -> PlanIdentity {
    PlanIdentity {
        product_code,
        segments: segments
            .iter()
            .map(|segment| {
                (
                    segment.index,
                    segment.kind,
                    segment.expected_frames,
                    segment_cameras(segment, cameras),
                )
            })
            .collect(),
        cameras: cameras.to_vec(),
        expected_total,
    }
}

/// 未分段区域用的较宽一档：上限取大、下限取小、max_gap 取大、min_present 取小、溢胶上限取大，
/// max_range_mm 与 offset 置 None（未分段不判均匀性和胶偏）。
///
/// 某项只要有一段停用，合并后即停用（停用是最宽的一档：不知道哪些帧属于停用该项的段）。
/// 胶宽名义值沿用第一段，上下允差按合并后的绝对上下限反推。`checks` 不能为空（未分段区域至少覆盖一段）。
#[cfg_attr(not(test), allow(dead_code))] // A4 burst_judge 接入后去掉
pub fn wider_checks(checks: &[&SegmentChecks]) -> SegmentChecks {
    let first = checks.first().expect("wider_checks：未分段区域至少覆盖一段");
    let fold = |init: f64, pick: fn(f64, f64) -> f64, value: &dyn Fn(&SegmentChecks) -> f64| {
        checks.iter().map(|c| value(c)).fold(init, pick)
    };
    let nominal = first.width.nominal_mm;
    let upper = fold(f64::NEG_INFINITY, f64::max, &|c| c.width.nominal_mm + c.width.tol_upper);
    let lower = fold(f64::INFINITY, f64::min, &|c| c.width.nominal_mm - c.width.tol_lower);
    SegmentChecks {
        width: WidthCheck {
            enabled: checks.iter().all(|c| c.width.enabled),
            nominal_mm: nominal,
            tol_upper: upper - nominal,
            tol_lower: nominal - lower,
            max_excursion_mm: fold(f64::NEG_INFINITY, f64::max, &|c| c.width.max_excursion_mm),
            max_range_mm: None,
        },
        gap: BreakCheck {
            enabled: checks.iter().all(|c| c.gap.enabled),
            max_gap_mm: fold(f64::NEG_INFINITY, f64::max, &|c| c.gap.max_gap_mm),
            min_present: fold(f64::INFINITY, f64::min, &|c| c.gap.min_present),
        },
        overflow: OverflowCheck {
            enabled: checks.iter().all(|c| c.overflow.enabled),
            max_width_mm: fold(f64::NEG_INFINITY, f64::max, &|c| c.overflow.max_width_mm),
        },
        offset: None,
    }
}

/// 新建一段：tolerance 与 checks 取对应 kind 的默认值，expected_frames / calib / offset_ref 为 None。
pub fn new_segment(index: u32, kind: SegmentKind, defaults: &KindDefaults) -> PathSegment {
    let default = match kind {
        SegmentKind::Straight => &defaults.straight,
        SegmentKind::Corner => &defaults.corner,
    };
    PathSegment {
        index,
        kind,
        length_mm: None,
        expected_frames: None,
        tolerance: default.tolerance,
        calib: None,
        checks: default.checks.clone(),
        offset_ref: None,
        inspect_range: None,
        cameras: Vec::new(),
    }
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
    PlanExport {
        protocol_version: crate::handshake::PROTOCOL_VERSION,
        recipe_id: recipe_id.to_owned(),
        product_code,
        plan_version,
        cameras: cameras.to_vec(),
        segments: segments
            .iter()
            .map(|segment| ExportSegment {
                index: segment.index,
                kind: segment.kind,
                length_mm: segment.length_mm,
                expected_frames: segment.expected_frames,
                tolerance: segment.tolerance,
                inspect_range: segment.inspect_range.clone(),
            })
            .collect(),
        expected_total: expected_total(segments, part),
        tolerance_total: tolerance_total(segments),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SegmentKind::{Corner, Straight};

    /// 一份合法的段计划：直线 12 帧（带胶偏参考）、弯道 4 帧、直线 9 帧，三台相机。
    struct Plan {
        product: u16,
        segments: Vec<PathSegment>,
        cameras: Vec<String>,
        part: Option<PartCalib>,
    }

    type Edit = fn(&mut Plan);

    fn plan() -> Plan {
        let defaults = KindDefaults::default();
        let mut segments: Vec<_> =
            [Straight, Corner, Straight].into_iter().enumerate().map(|(i, kind)| new_segment(i as u32 + 1, kind, &defaults)).collect();
        for (segment, n) in segments.iter_mut().zip([12, 4, 9]) {
            segment.expected_frames = Some(n);
        }
        segments[0].offset_ref = Some(OffsetRef { measure: OffsetMeasure::AbsoluteX, camera: "camA".into(), x_ref: Some(0.2), spread: Some(0.1) });
        Plan { product: 7, segments, cameras: ["camA", "camB", "camC"].map(String::from).to_vec(), part: None }
    }

    fn edited(edit: Edit) -> Plan {
        let mut p = plan();
        edit(&mut p);
        p
    }

    fn identity(p: &Plan) -> PlanIdentity {
        plan_identity(p.product, &p.segments, &p.cameras, expected_total(&p.segments, p.part.as_ref()))
    }

    fn part(total: u32) -> PartCalib {
        PartCalib { expected_total: total, calib: SegmentCalib { rounds: 10, min: total - 1, max: total + 1, mean: total as f64 } }
    }

    /// 规格 §1：段序列 / 类型 / 相机绑定 / 产品代码变化换计划号；N_exp 与整件帧组数也换（默认做法 6）；
    /// 限值、容差、x_ref、检测区间、段长、标定统计变化沿用。
    #[test]
    fn plan_identity_changes_only_with_sequence_kind_frames_cameras_or_product() {
        let base = identity(&plan());
        let cases: [(&str, bool, Edit); 16] = [
            ("段类型", true, |p| p.segments[2].kind = Corner),
            ("段顺序", true, |p| {
                p.segments.swap(0, 1);
                for (i, segment) in p.segments.iter_mut().enumerate() {
                    segment.index = i as u32 + 1;
                }
            }),
            ("增加一段", true, |p| p.segments.push(new_segment(4, Corner, &KindDefaults::default()))),
            ("相机顺序", true, |p| p.cameras.swap(0, 1)),
            ("换一台相机", true, |p| p.cameras[2] = "camD".into()),
            ("各段 N_exp 重分配但总数不变", true, |p| {
                p.segments[0].expected_frames = Some(13);
                p.segments[1].expected_frames = Some(3);
            }),
            ("产品代码", true, |p| p.product = 8),
            ("段检测相机", true, |p| {
                p.segments[1].cameras = vec!["camB".into()]
            }),
            (
                "段检测相机列全三台（与不指定相同）",
                false,
                |p| p.segments[1].cameras = p.cameras.clone(),
            ),
            ("容差", false, |p| p.segments[0].tolerance = 3),
            ("胶宽、断胶、溢胶、胶偏限值", false, |p| {
                let checks = &mut p.segments[0].checks;
                checks.width.tol_upper = 2.5;
                checks.gap.max_gap_mm = 9.0;
                checks.overflow.max_width_mm = 10.0;
                checks.offset.as_mut().unwrap().max_mean_shift_mm = 3.0;
            }),
            ("x_ref", false, |p| p.segments[0].offset_ref.as_mut().unwrap().x_ref = Some(0.5)),
            ("检测区间", false, |p| p.segments[1].inspect_range = Some(InspectRange { a_mm: 5.0, b_mm: 40.0 })),
            ("段长", false, |p| p.segments[2].length_mm = Some(120.0)),
            ("标定统计", false, |p| p.segments[0].calib = Some(SegmentCalib { rounds: 12, min: 11, max: 13, mean: 12.1 })),
            ("各段已标定时的整件标定", false, |p| p.part = Some(part(30))),
        ];
        for (case, changes, edit) in cases {
            assert_eq!(identity(&edited(edit)) != base, changes, "{case}");
        }
        let recalibrated = |total| {
            let mut p = plan();
            p.segments.iter_mut().for_each(|segment| segment.expected_frames = None);
            p.part = Some(part(total));
            identity(&p)
        };
        assert_ne!(recalibrated(50), recalibrated(51), "各段待标定时整件重新标定");
    }

    /// 规格 §1 + 决策补充第二节：各段都标定时取 ΣN_exp；有段待标定时取整件标定，都没有为 null。
    #[test]
    fn any_uncalibrated_segment_falls_back_to_part_calib_or_none_otherwise_sum() {
        let mut p = plan();
        assert_eq!(expected_total(&p.segments, Some(&part(30))), Some(25), "各段都已标定：取各段之和");
        p.segments[1].expected_frames = None;
        assert_eq!(expected_total(&p.segments, Some(&part(30))), Some(30), "有段待标定：取整件标定");
        assert_eq!(expected_total(&p.segments, None), None, "有段待标定且没有整件标定");
    }

    /// 规格 §1 段模型约束；检测限值按旧 ShotLimits 的范围规则校验（D-SB-1 新路径不丢旧约束），停用的检测项不校验限值。
    #[test]
    fn invalid_segment_plans_are_rejected() {
        let check = |p: &Plan| validate_segments(&p.segments, &p.cameras);
        assert_eq!(check(&plan()), Ok(()));
        let accepted: [(&str, Edit); 3] = [
            ("容差 1000、ΣN_exp 32767 恰在上限", |p| {
                p.segments[0].tolerance = MAX_TOLERANCE;
                p.segments[0].expected_frames = Some(MAX_EXPECTED_TOTAL - 4 - 9);
            }),
            ("检测区间终点恰为段长", |p| {
                p.segments[0].length_mm = Some(100.0);
                p.segments[0].inspect_range = Some(InspectRange { a_mm: 10.0, b_mm: 100.0 });
            }),
            ("停用的检测项不校验限值", |p| {
                let checks = &mut p.segments[0].checks;
                checks.width = WidthCheck { enabled: false, nominal_mm: 0.0, tol_upper: -1.0, tol_lower: -1.0, max_excursion_mm: -1.0, max_range_mm: Some(-1.0) };
                checks.gap = BreakCheck { enabled: false, max_gap_mm: -1.0, min_present: 1.2 };
                checks.overflow = OverflowCheck { enabled: false, max_width_mm: 0.0 };
                checks.offset = Some(OffsetCheck { enabled: false, max_mean_shift_mm: -1.0, max_local_dev_mm: -1.0 });
            }),
        ];
        for (case, edit) in accepted {
            assert_eq!(check(&edited(edit)), Ok(()), "{case}");
        }
        let cases: [(&str, Edit); 32] = [
            ("没有段", |p| p.segments.clear()),
            ("序号不从 1 起", |p| p.segments.iter_mut().for_each(|segment| segment.index += 1)),
            ("序号跳号", |p| p.segments[2].index = 4),
            ("相机只有 2 台", |p| {
                p.cameras.pop();
            }),
            ("相机 4 台", |p| p.cameras.push("camD".into())),
            ("相机重复", |p| p.cameras[2] = "camA".into()),
            ("相机 ID 为空", |p| p.cameras[1] = " ".into()),
            ("弯道段带胶偏检测", |p| {
                p.segments[1].checks.offset = p.segments[0].checks.offset.clone()
            }),
            ("弯道段带胶偏参考", |p| {
                p.segments[1].offset_ref = p.segments[0].offset_ref.clone()
            }),
            ("胶偏参考相机不在计划内", |p| {
                p.segments[0].offset_ref.as_mut().unwrap().camera = "camD".into()
            }),
            ("胶偏参考相机不在该段检测相机中", |p| {
                p.segments[0].cameras = vec!["camB".into()]
            }),
            ("段检测相机不在计划内", |p| {
                p.segments[1].cameras = vec!["camD".into()]
            }),
            ("段检测相机重复", |p| {
                p.segments[1].cameras = vec!["camB".into(), "camB".into()]
            }),
            ("检测区间 a = b", |p| {
                p.segments[0].inspect_range = Some(InspectRange {
                    a_mm: 10.0,
                    b_mm: 10.0,
                })
            }),
            ("检测区间 a < 0", |p| {
                p.segments[0].inspect_range = Some(InspectRange {
                    a_mm: -1.0,
                    b_mm: 10.0,
                })
            }),
            ("检测区间不是有限值", |p| {
                p.segments[0].inspect_range = Some(InspectRange {
                    a_mm: 0.0,
                    b_mm: f64::NAN,
                })
            }),
            ("容差超过 1000", |p| {
                p.segments[1].tolerance = MAX_TOLERANCE + 1
            }),
            ("ΣN_exp 超过 32767", |p| {
                p.segments[0].expected_frames = Some(MAX_EXPECTED_TOTAL - 4 - 9 + 1)
            }),
            ("胶宽名义值为 0", |p| {
                p.segments[1].checks.width.nominal_mm = 0.0
            }),
            ("胶宽上允差为负", |p| {
                p.segments[1].checks.width.tol_upper = -0.1
            }),
            ("胶宽下允差为负", |p| {
                p.segments[1].checks.width.tol_lower = -0.1
            }),
            ("胶宽允许超差长度为负", |p| {
                p.segments[1].checks.width.max_excursion_mm = -1.0
            }),
            ("胶宽均匀性极差上限为负", |p| {
                p.segments[0].checks.width.max_range_mm = Some(-0.1)
            }),
            ("胶宽限值不是有限值", |p| {
                p.segments[0].checks.width.tol_upper = f64::INFINITY
            }),
            ("允许断胶长度为负", |p| {
                p.segments[1].checks.gap.max_gap_mm = -1.0
            }),
            ("有胶比例大于 1", |p| {
                p.segments[1].checks.gap.min_present = 1.2
            }),
            ("有胶比例小于 0", |p| {
                p.segments[1].checks.gap.min_present = -0.1
            }),
            ("溢胶上限为 0", |p| {
                p.segments[1].checks.overflow.max_width_mm = 0.0
            }),
            ("胶偏整段偏移限值为负", |p| {
                p.segments[0]
                    .checks
                    .offset
                    .as_mut()
                    .unwrap()
                    .max_mean_shift_mm = -1.0
            }),
            ("胶偏局部跑偏限值为负", |p| {
                p.segments[0]
                    .checks
                    .offset
                    .as_mut()
                    .unwrap()
                    .max_local_dev_mm = -1.0
            }),
            ("段长为 0", |p| p.segments[2].length_mm = Some(0.0)),
            ("检测区间终点超出段长", |p| {
                p.segments[2].length_mm = Some(100.0);
                p.segments[2].inspect_range = Some(InspectRange { a_mm: 10.0, b_mm: 100.5 });
            }),
        ];
        for (case, edit) in cases {
            assert!(check(&edited(edit)).is_err(), "{case}");
        }
    }
}

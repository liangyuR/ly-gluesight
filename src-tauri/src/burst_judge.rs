//! 段模型判胶（规格 §4）：按区域类型分流。每帧独立指标（有胶 / 断胶、胶宽、溢胶）三类区域都跑；
//! 胶偏只在直线段；结果标“检测不完整”“未检区域”“胶偏未判”“帧数异常”“疑似提前终止”。
//!
//! 与旧拍照点模型的 `judge::Judgement` 并存：旧配方仍用 `judge`，段配方用这里的 `BurstJudgement`。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::frame_group::{count_status, CountStatus, FrameGroup, LossAlarm};
use crate::judge::{fault, Verdict};
use crate::plan::{tolerance_total, wider_checks, OffsetCheck, OffsetMeasure, PathSegment, SegmentChecks, SegmentKind};
use crate::segmentation::{Region, RegionKind, UnsegReason};

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
    /// 这一帧所在段不由这台相机检测，按设计不测（D-SB-45）：不判定，也不计检测不完整
    NotInspected,
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
    /// 测量用的是第几段的走廊（`PathSegment.index`，D-SB-45）；没测或旧记录为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corridor: Option<u32>,
    /// 逐帧定向测得的胶条方向（度，图像坐标，D-SB-47）；按示教走廊测的帧为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f64>,
    /// 逐帧定向没找到胶条、按预测段示教方向回退测量（D-SB-47）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub heading_fallback: bool,
    /// 胶中心到零件边距离的均值（mm，D-SB-48 ToEdge 测量量）；没找边或没找到边为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_mm: Option<f64>,
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
    /// 测量量未实现（ToEdge 没配找边参数，D-SB-48）
    MeasureNotImplemented,
    /// 启用了胶偏但标准件 x_ref 未标定
    NoReference,
    /// 去掉区域首尾各一帧后，参考相机没有测成、带 x 的帧（缺帧、测量失败或区域太短）
    NoFrames,
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

/// 未检区域：本段跳过的帧组 from_seq..=to_seq（`skip_head` / `skip_tail`）。
/// 2026-10-26 之前的记录按毫米检测区间写 from_mm / to_mm（判定其实没排除这些帧），读旧记录时保留原样。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Uninspected {
    pub segment: u32,
    #[serde(default)]
    pub from_seq: Option<u64>,
    #[serde(default)]
    pub to_seq: Option<u64>,
    #[serde(default)]
    pub from_mm: Option<f64>,
    #[serde(default)]
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
        Self {
            verdict: Verdict::ErrInspect,
            plc_code: Verdict::ErrInspect.plc_code(),
            fault_code,
            reason: reason.into(),
            group_count: 0,
            count_status: None,
            regions: Vec::new(),
            flags: Flags::default(),
            alarms: Vec::new(),
            uninspected: Vec::new(),
            unchecked: Vec::new(),
        }
    }
}

/// 判一件。cameras 是计划相机（`FrameMetrics.camera` 的下标基准，`OffsetRef.camera` 按它找下标）；
/// expected_total 由 `plan::expected_total` 算出；区域内检测相机取最差（D-SB-41：直线 / 弯道区域只用本段检测相机，
/// 过渡区用两侧段检测相机的并集，未分段区域用全部计划相机）。
///
/// - 每帧独立指标（断胶、有胶比例、胶宽、溢胶）三类区域都跑；只判 Measured 帧，其余状态只记检测不完整。
/// - 区域允差：直线 / 弯道区域用该段 checks，未分段区域用所覆盖各段的 [`wider_checks`]。
/// - 胶宽均匀性与胶偏只在直线区域判；未分段区域覆盖的直线段要判胶偏时记胶偏未判，弯道区域不判也不标。
/// - 过渡区（D-SB-40）按未分段区域判每帧指标，不判也不标胶偏，不算“含未分段”。
/// - 帧数越界、缺帧、疑似提前终止只进 flags，不改 OK/NG；帧数只核对整件与未分段区域，直线 / 弯道区域不逐段核对。
/// - 三台相机整件没有一帧测成时判 ERR 91；区域引用了配方里没有的段时判 ERR 93。
pub fn judge(
    segments: &[PathSegment],
    expected_total: Option<u32>,
    regions: &[Region],
    groups: &[FrameGroup],
    metrics: &[FrameMetrics],
    cameras: &[String],
    loss: &[LossAlarm],
) -> BurstJudgement {
    let group_count = u32::try_from(groups.iter().map(|g| g.seq).max().unwrap_or(0)).unwrap_or(u32::MAX);
    let tol_total = tolerance_total(segments);
    let part_status = expected_total.map(|expected| count_status(group_count, expected, tol_total));
    let short_part = part_status == Some(CountStatus::Low);
    // 每个 (seq, 相机) 一帧；不是计划相机的帧不参与
    let frames: BTreeMap<(u64, usize), &FrameMetrics> =
        metrics.iter().filter(|m| m.camera < cameras.len()).map(|m| ((m.seq, m.camera), m)).collect();

    if !frames.values().any(|m| m.state == FrameState::Measured) {
        let reason = if frames.values().all(|m| m.state == FrameState::Missing) { "三台相机整件没有收到帧" } else { "三台相机整件没有一帧测成" };
        return BurstJudgement {
            group_count,
            count_status: part_status,
            flags: Flags { incomplete: true, short_part, ..Flags::default() },
            alarms: loss.to_vec(),
            ..BurstJudgement::error(fault::MISSING_FRAME, reason)
        };
    }
    if regions.iter().any(|region| covered(segments, region).is_empty()) {
        return BurstJudgement::error(fault::INVALID_POINTS, "分段区域与配方的段不一致");
    }

    let incomplete_seqs = groups.iter().filter(|g| g.incomplete).map(|g| g.seq).collect();
    let part = PartInput { segments, expected_total, tol_total, incomplete_seqs, frames: &frames, cameras };
    let results: Vec<RegionResult> = regions.iter().map(|region| judge_region(&part, region)).collect();

    let verdict = results.iter().map(|r| r.verdict).max().unwrap_or(Verdict::Ok);
    let reason = match verdict {
        Verdict::Ok => format!("{group_count} 个帧组全部合格"),
        _ => results
            .iter()
            .filter(|r| r.verdict == verdict)
            .map(|r| format!("{}：{}", region_label(&r.region), r.reason))
            .collect::<Vec<_>>()
            .join("；"),
    };
    let anomalous = |status: Option<CountStatus>| matches!(status, Some(CountStatus::Low | CountStatus::High));
    let flags = Flags {
        // 区域首尾相接覆盖全部帧组，各区域只看自己的检测相机（D-SB-41）：没选中的相机缺帧、没测成不算检测不完整
        incomplete: results.iter().any(|r| !r.incomplete_groups.is_empty()),
        frame_count_anomaly: anomalous(part_status) || results.iter().any(|r| anomalous(r.count_status)),
        offset_unjudged: results.iter().any(|r| r.offset_unjudged.is_some()),
        unsegmented: results.iter().any(|r| matches!(r.region.kind, RegionKind::Unsegmented { reason, .. } if reason != UnsegReason::Transition)),
        short_part,
    };
    BurstJudgement {
        verdict,
        plc_code: verdict.plc_code(),
        fault_code: 0,
        reason,
        group_count,
        count_status: part_status,
        regions: results,
        flags,
        alarms: loss.to_vec(),
        uninspected: uninspected(regions),
        unchecked: vec![UncheckedItem::Bubble, UncheckedItem::Stringing],
    }
}

/// 一项不合格（或允许范围内的超差）。声明顺序即区域摘要里的先后。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Finding {
    Gap,
    Sparse,
    Overflow,
    Width,
    Uneven,
    Shift,
    LocalDev,
    Excursion,
}

impl Finding {
    fn verdict(self) -> Verdict {
        match self {
            Finding::Gap | Finding::Sparse => Verdict::NgGap,
            // 溢胶沿用 NG_WIDTH（默认做法 1），胶宽不均也是 NG_WIDTH（D-SB-16）
            Finding::Overflow | Finding::Width | Finding::Uneven => Verdict::NgWidth,
            Finding::Shift | Finding::LocalDev => Verdict::NgPosition,
            Finding::Excursion => Verdict::OkWithExcursion,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Finding::Gap => "断胶",
            Finding::Sparse => "胶条断续",
            Finding::Overflow => "溢胶",
            Finding::Width => "胶宽超差",
            Finding::Uneven => "胶宽不均",
            Finding::Shift => "整段胶偏",
            Finding::LocalDev => "局部跑偏",
            Finding::Excursion => "胶宽局部超差（允许范围内）",
        }
    }
}

/// 一帧的独立判定（只对 Measured 帧调用）。
///
/// 胶宽超差长度按 `widths × spacing_mm` 累计，不超过 `max_excursion_mm` 为允许范围内的超差；
/// 没有逐站胶宽（或站距未知）时量不出长度，帧内极值超限即按超过允许长度算。
fn judge_frame(frame: &FrameMetrics, checks: &SegmentChecks) -> BTreeSet<Finding> {
    let mut found = BTreeSet::new();
    let gap = &checks.gap;
    if gap.enabled {
        if frame.gap_max_mm > gap.max_gap_mm {
            found.insert(Finding::Gap);
        }
        if frame.present_ratio < gap.min_present {
            found.insert(Finding::Sparse);
        }
    }
    let stations = || frame.widths.iter().flatten().map(|&w| f64::from(w));
    let width = &checks.width;
    if width.enabled {
        let (lower, upper) = (width.nominal_mm - width.tol_lower, width.nominal_mm + width.tol_upper);
        let out = |w: f64| w < lower || w > upper;
        let length_known = !frame.widths.is_empty() && frame.spacing_mm > 0.0;
        let excursion = if !length_known {
            if frame.width_min.is_some_and(out) || frame.width_max.is_some_and(out) { f64::INFINITY } else { 0.0 }
        } else {
            stations().filter(|&w| out(w)).count() as f64 * frame.spacing_mm
        };
        if excursion > width.max_excursion_mm {
            found.insert(Finding::Width);
        } else if excursion > 0.0 {
            found.insert(Finding::Excursion);
        }
    }
    let overflow = &checks.overflow;
    let width_max = frame.width_max.or_else(|| stations().reduce(f64::max));
    if overflow.enabled && width_max.is_some_and(|w| w > overflow.max_width_mm) {
        found.insert(Finding::Overflow);
    }
    found
}

/// 一帧单独的结论（在线帧格显示用）：只看这一帧的独立指标，不含区域级的均匀性与胶偏。
pub fn frame_verdict(frame: &FrameMetrics, checks: &SegmentChecks) -> Verdict {
    judge_frame(frame, checks).iter().map(|f| f.verdict()).max().unwrap_or(Verdict::Ok)
}

/// 直线段胶偏（xs 已去掉区域首尾各一帧，非空）：x 均值 − x_ref 判整段偏移，各帧 x 与均值的最大偏差判局部跑偏。
fn straight_offset(xs: &[f64], x_ref: f64, check: &OffsetCheck) -> (OffsetStat, BTreeSet<Finding>) {
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    let stat = OffsetStat { mean_shift: mean - x_ref, max_dev: xs.iter().map(|x| (x - mean).abs()).fold(0.0, f64::max) };
    let mut found = BTreeSet::new();
    if stat.mean_shift.abs() > check.max_mean_shift_mm {
        found.insert(Finding::Shift);
    }
    if stat.max_dev > check.max_local_dev_mm {
        found.insert(Finding::LocalDev);
    }
    (stat, found)
}

/// 整件共用的输入。
struct PartInput<'a> {
    segments: &'a [PathSegment],
    expected_total: Option<u32>,
    tol_total: u32,
    /// 配组标了 incomplete 的 seq
    incomplete_seqs: BTreeSet<u64>,
    frames: &'a BTreeMap<(u64, usize), &'a FrameMetrics>,
    cameras: &'a [String],
}

/// 区域覆盖的段（按配方顺序）。
fn covered<'a>(segments: &'a [PathSegment], region: &Region) -> Vec<&'a PathSegment> {
    let (first, last) = match region.kind {
        RegionKind::Straight { segment } | RegionKind::Corner { segment } => (segment, segment),
        RegionKind::Unsegmented { first_segment, last_segment, .. } => (first_segment, last_segment),
        RegionKind::Skipped { segment } => (segment, segment),
    };
    segments.iter().filter(|segment| (first..=last).contains(&segment.index)).collect()
}

/// 区域参与判定的相机下标（计划相机顺序）。
fn region_cameras(part: &PartInput, region: &Region, covered: &[&PathSegment]) -> BTreeSet<usize> {
    let all = || (0..part.cameras.len()).collect();
    let pick = |segments: &[&PathSegment]| {
        segments
            .iter()
            .flat_map(|segment| crate::plan::segment_cameras(segment, part.cameras))
            .filter_map(|id| part.cameras.iter().position(|camera| *camera == id))
            .collect()
    };
    match region.kind {
        RegionKind::Straight { .. } | RegionKind::Corner { .. } => pick(covered),
        RegionKind::Unsegmented {
            reason: UnsegReason::Transition,
            ..
        } => pick(covered),
        RegionKind::Unsegmented { .. } => all(),
        // 跳过的帧不判定：不取任何相机，区域里既没有结论也没有检测不完整
        RegionKind::Skipped { .. } => BTreeSet::new(),
    }
}

fn judge_region(part: &PartInput, region: &Region) -> RegionResult {
    let covered = covered(part.segments, region);
    let allowed = region_cameras(part, region, &covered);
    let checks = match region.kind {
        RegionKind::Unsegmented { .. } => wider_checks(&covered.iter().map(|segment| &segment.checks).collect::<Vec<_>>()),
        _ => covered[0].checks.clone(),
    };
    let straight = matches!(region.kind, RegionKind::Straight { .. });
    let in_region: Vec<&FrameMetrics> = part
        .frames
        .range((region.first_seq, 0)..=(region.last_seq, usize::MAX))
        .map(|(_, &m)| m)
        .filter(|m| allowed.contains(&m.camera))
        .collect();
    let measured: Vec<&FrameMetrics> = in_region.iter().copied().filter(|m| m.state == FrameState::Measured).collect();

    let mut findings: BTreeSet<Finding> = measured.iter().flat_map(|m| judge_frame(m, &checks)).collect();
    // 均匀性：只在直线区域判（D-SB-16）
    if straight && checks.width.enabled {
        let means: Vec<f64> = measured.iter().filter_map(|m| m.width_mean).collect();
        if let (Some(limit), Some(spread)) = (checks.width.max_range_mm, spread(&means)) {
            if spread > limit {
                findings.insert(Finding::Uneven);
            }
        }
    }

    let (offset, offset_unjudged) = match region.kind {
        RegionKind::Straight { .. } => {
            let (offset, unjudged, found) = region_offset(covered[0], &checks, region, &measured, part.cameras);
            findings.extend(found);
            (offset, unjudged)
        }
        // 过渡区本来就不判胶偏（D-SB-40），不标胶偏未判
        RegionKind::Corner { .. }
        | RegionKind::Skipped { .. }
        | RegionKind::Unsegmented {
            reason: UnsegReason::Transition,
            ..
        } => (None, None),
        RegionKind::Unsegmented { reason, .. } => {
            let would_judge = covered
                .iter()
                .any(|segment| segment.kind == SegmentKind::Straight && segment.checks.offset.as_ref().is_some_and(|o| o.enabled));
            let unjudged = match reason {
                UnsegReason::LowConfidence => OffsetUnjudged::LowConfidence,
                UnsegReason::AlgorithmPending | UnsegReason::Transition => {
                    OffsetUnjudged::Unsegmented
                }
            };
            (None, would_judge.then_some(unjudged))
        }
    };

    let verdict = findings.iter().map(|f| f.verdict()).max().unwrap_or(Verdict::Ok);
    let reason = findings.iter().filter(|f| f.verdict() == verdict).map(|f| f.label()).collect::<Vec<_>>().join("、");

    let frame_count = region.frame_count();
    let covers_all = part.segments.first().map(|s| s.index) == covered.first().map(|s| s.index)
        && part.segments.last().map(|s| s.index) == covered.last().map(|s| s.index);
    let (expected, tolerance) = match region.kind {
        RegionKind::Straight { .. } | RegionKind::Corner { .. } => (covered[0].expected_frames, covered[0].tolerance),
        RegionKind::Unsegmented { .. } if covers_all => (part.expected_total, part.tol_total),
        RegionKind::Unsegmented { .. } => (None, covered.iter().fold(0u32, |sum, s| sum.saturating_add(s.tolerance))),
        RegionKind::Skipped { .. } => (None, 0),
    };

    let cams = part.cameras.len();
    // 各相机收到的帧数（全部计划相机，供追溯；不参与判定）
    let camera_frames = (0..cams)
        .map(|c| {
            (region.first_seq..=region.last_seq)
                .filter(|&seq| {
                    part.frames
                        .get(&(seq, c))
                        .is_some_and(|m| m.state != FrameState::Missing)
                })
                .count() as u32
        })
        .collect();
    // 缺帧组：区域检测相机里某台这一组没有测成的帧（缺帧、测量失败、没测、走出走廊）；用全部相机时配组标了 incomplete 也算。
    // 按设计不测的帧（NotInspected：未分段区域里不检测预测段的相机）不算
    let all_cameras = allowed.len() == cams;
    let incomplete_groups = (region.first_seq..=region.last_seq)
        .filter(|seq| {
            (all_cameras && part.incomplete_seqs.contains(seq))
                || allowed.iter().any(|&c| {
                    part.frames.get(&(*seq, c)).is_none_or(|m| {
                        !matches!(m.state, FrameState::Measured | FrameState::NotInspected)
                    })
                })
        })
        .collect();

    RegionResult {
        region: region.clone(),
        verdict,
        reason,
        frame_count,
        expected,
        tolerance,
        // 直线 / 弯道区域的帧数由比例边界推算并扣掉过渡区，不与本段 N_exp 核对（D-SB-40）
        count_status: match region.kind {
            RegionKind::Straight { .. }
            | RegionKind::Corner { .. }
            | RegionKind::Skipped { .. } => None,
            RegionKind::Unsegmented { .. } => {
                expected.map(|expected| count_status(frame_count, expected, tolerance))
            }
        },
        camera_frames,
        width: width_stats(&measured),
        offset,
        offset_unjudged,
        incomplete_groups,
    }
}

/// 直线区域的胶偏：只用 `offset_ref.camera` 那台相机的 Measured 帧，去掉区域首尾各一帧（按 seq）。
/// AbsoluteX 用每帧的 x 与 x_ref 比；ToEdge 用每帧胶边距（D-SB-48），参考值取图纸胶边距，没填时取标准件得出的 x_ref。
/// 没启用胶偏不判也不标；ToEdge 没配找边参数标测量量未实现；没有参考值（或参考相机不在计划内）标未标定；
/// 去掉首尾后没有可用的测量量时标没有可用帧（不静默放过）。
fn region_offset(
    segment: &PathSegment,
    checks: &SegmentChecks,
    region: &Region,
    measured: &[&FrameMetrics],
    cameras: &[String],
) -> (Option<OffsetStat>, Option<OffsetUnjudged>, BTreeSet<Finding>) {
    let Some(check) = checks.offset.as_ref().filter(|check| check.enabled) else { return (None, None, BTreeSet::new()) };
    let unjudged = |why| (None, Some(why), BTreeSet::new());
    let Some(reference) = &segment.offset_ref else {
        return unjudged(OffsetUnjudged::NoReference);
    };
    let (target, value): (Option<f64>, fn(&FrameMetrics) -> Option<f64>) =
        match (reference.measure, &reference.edge) {
            (OffsetMeasure::ToEdge, None) => {
                return unjudged(OffsetUnjudged::MeasureNotImplemented)
            }
            (OffsetMeasure::ToEdge, Some(edge)) => {
                (edge.nominal_mm.or(reference.x_ref), |m| m.edge_mm)
            }
            (OffsetMeasure::AbsoluteX, _) => (reference.x_ref, |m| m.x_mm),
        };
    let (Some(x_ref), Some(camera)) = (
        target,
        cameras.iter().position(|id| *id == reference.camera),
    ) else {
        return unjudged(OffsetUnjudged::NoReference);
    };
    let xs: Vec<f64> = measured
        .iter()
        .filter(|m| m.camera == camera && m.seq != region.first_seq && m.seq != region.last_seq)
        .filter_map(|m| value(m))
        .collect();
    if xs.is_empty() {
        return unjudged(OffsetUnjudged::NoFrames);
    }
    let (stat, found) = straight_offset(&xs, x_ref, check);
    (Some(stat), None, found)
}

/// max − min；空为 None。
fn spread(values: &[f64]) -> Option<f64> {
    let min = values.iter().copied().reduce(f64::min)?;
    let max = values.iter().copied().reduce(f64::max)?;
    Some(max - min)
}

fn width_stats(measured: &[&FrameMetrics]) -> Option<WidthStats> {
    let min = measured.iter().filter_map(|m| m.width_min).reduce(f64::min)?;
    let max = measured.iter().filter_map(|m| m.width_max).reduce(f64::max)?;
    let means: Vec<f64> = measured.iter().filter_map(|m| m.width_mean).collect();
    let mean = if means.is_empty() { (min + max) / 2.0 } else { means.iter().sum::<f64>() / means.len() as f64 };
    Some(WidthStats { min, max, mean })
}

fn region_label(region: &Region) -> String {
    match region.kind {
        RegionKind::Straight { segment } => format!("第 {segment} 段（直线）"),
        RegionKind::Corner { segment } => format!("第 {segment} 段（弯道）"),
        RegionKind::Unsegmented {
            first_segment,
            last_segment,
            reason: UnsegReason::Transition,
        } => {
            format!("第 {first_segment}–{last_segment} 段边界过渡区")
        }
        RegionKind::Unsegmented { first_segment, last_segment, .. } if first_segment == last_segment => {
            format!("未分段区域（第 {first_segment} 段）")
        }
        RegionKind::Unsegmented { first_segment, last_segment, .. } => format!("未分段区域（第 {first_segment}–{last_segment} 段）"),
        RegionKind::Skipped { segment } => format!("第 {segment} 段（跳过）"),
    }
}

/// 未检区域：分段解析出的跳过区域（各段 `skip_head` / `skip_tail`），按帧组号给出。
fn uninspected(regions: &[Region]) -> Vec<Uninspected> {
    regions
        .iter()
        .filter_map(|region| match region.kind {
            RegionKind::Skipped { segment } => Some(Uninspected {
                segment,
                from_seq: Some(region.first_seq),
                to_seq: Some(region.last_seq),
                from_mm: None,
                to_mm: None,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{new_segment, KindDefaults, OffsetRef};
    use SegmentKind::{Corner, Straight};

    const CAMS: usize = 3;

    fn cameras() -> Vec<String> {
        ["camA", "camB", "camC"].map(String::from).to_vec()
    }

    /// 默认允差的段：胶宽 4.0 ± 1.5、允许超差 5 mm、断胶 6 mm、有胶比例 0.8、溢胶 8 mm；直线段胶偏 ±2 mm。
    fn plan(kinds: &[SegmentKind]) -> Vec<PathSegment> {
        let defaults = KindDefaults::default();
        kinds.iter().enumerate().map(|(i, &kind)| new_segment(i as u32 + 1, kind, &defaults)).collect()
    }

    fn reference(x_ref: Option<f64>, measure: OffsetMeasure) -> Option<OffsetRef> {
        Some(OffsetRef {
            measure,
            camera: "camA".into(),
            x_ref,
            spread: x_ref.map(|_| 0.1),
            edge: None,
        })
    }

    /// 合格帧：10 站胶宽都是 4.0（站距 1 mm），无断口，胶中心 x = 0。
    fn ok(camera: usize, seq: u64) -> FrameMetrics {
        FrameMetrics {
            camera,
            seq,
            state: FrameState::Measured,
            error: None,
            width_min: Some(4.0),
            width_max: Some(4.0),
            width_mean: Some(4.0),
            widths: vec![Some(4.0); 10],
            spacing_mm: 1.0,
            gap_max_mm: 0.0,
            corridor: None,
            present_ratio: 1.0,
            x_mm: Some(0.0),
            heading_deg: None,
            heading_fallback: false,
            edge_mm: None,
        }
    }

    /// 没测成的帧：数值都是测不到时的样子，若被当成 Measured 判会是断胶。
    fn failed(camera: usize, seq: u64, state: FrameState) -> FrameMetrics {
        FrameMetrics {
            state,
            width_min: None,
            width_max: None,
            width_mean: None,
            widths: Vec::new(),
            present_ratio: 0.0,
            x_mm: None,
            ..ok(camera, seq)
        }
    }

    /// seq 1..=n 三台相机全部合格。
    fn frames(n: u64) -> Vec<FrameMetrics> {
        (1..=n).flat_map(|seq| (0..CAMS).map(move |camera| ok(camera, seq))).collect()
    }

    fn at(metrics: &mut [FrameMetrics], camera: usize, seq: u64) -> &mut FrameMetrics {
        metrics.iter_mut().find(|m| m.camera == camera && m.seq == seq).unwrap()
    }

    /// 逐站胶宽，同时更新帧内极值与均值。
    fn set_widths(frame: &mut FrameMetrics, widths: &[f64]) {
        frame.widths = widths.iter().map(|&w| Some(w as f32)).collect();
        frame.width_min = widths.iter().copied().reduce(f64::min);
        frame.width_max = widths.iter().copied().reduce(f64::max);
        frame.width_mean = Some(widths.iter().sum::<f64>() / widths.len() as f64);
    }

    /// 与 Grouper 同一口径：到了的帧（不论测没测成）算 present，Missing 与没有记录算缺帧。
    fn groups_of(metrics: &[FrameMetrics]) -> Vec<FrameGroup> {
        let total = metrics.iter().map(|m| m.seq).max().unwrap_or(0);
        (1..=total)
            .map(|seq| {
                let present: Vec<bool> = (0..CAMS)
                    .map(|camera| metrics.iter().any(|m| m.camera == camera && m.seq == seq && m.state != FrameState::Missing))
                    .collect();
                FrameGroup { seq, incomplete: present.contains(&false), present }
            })
            .collect()
    }

    fn region(kind: RegionKind, first_seq: u64, last_seq: u64) -> Region {
        Region { kind, first_seq, last_seq }
    }

    /// 覆盖全部段的未分段区域。
    fn whole(segments: &[PathSegment], reason: UnsegReason, last_seq: u64) -> Region {
        let kind = RegionKind::Unsegmented { first_segment: 1, last_segment: segments.len() as u32, reason };
        region(kind, 1, last_seq)
    }

    fn run(segments: &[PathSegment], expected_total: Option<u32>, regions: &[Region], metrics: &[FrameMetrics]) -> BurstJudgement {
        judge(segments, expected_total, regions, &groups_of(metrics), metrics, &cameras(), &[])
    }

    /// 规格 §3/§4 未分段：每帧独立指标照常判，允差取所覆盖段中较宽一档；不判胶偏，按未分段原因标“胶偏未判”。
    /// 直线段胶宽上限 5.5、溢胶 8；弯道段放宽到 6.5、9。帧内 9 站 6.0 mm 加 1 站尖峰。
    #[test]
    fn unsegmented_region_judges_frames_with_wider_limits_and_flags_offset_unjudged() {
        let mut segments = plan(&[Straight, Corner]);
        segments[0].offset_ref = reference(Some(0.0), OffsetMeasure::AbsoluteX);
        segments[1].checks.width.tol_upper = 2.5;
        segments[1].checks.overflow.max_width_mm = 9.0;
        for (case, reason, peak, verdict, unjudged) in [
            ("较宽允差内：只有尖峰一站超差", UnsegReason::AlgorithmPending, 8.5, Verdict::OkWithExcursion, OffsetUnjudged::Unsegmented),
            ("超过较宽的溢胶上限", UnsegReason::LowConfidence, 9.5, Verdict::NgWidth, OffsetUnjudged::LowConfidence),
        ] {
            let mut metrics = frames(6);
            set_widths(at(&mut metrics, 0, 3), &[6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, peak]);
            // 若按直线段判胶偏，x 偏 5 mm 必然 NG
            metrics.iter_mut().filter(|m| m.camera == 0).for_each(|m| m.x_mm = Some(5.0));
            let result = run(&segments, None, &[whole(&segments, reason, 6)], &metrics);
            assert_eq!(result.verdict, verdict, "{case}");
            assert_eq!((&result.regions[0].offset, result.regions[0].offset_unjudged), (&None, Some(unjudged)), "{case}");
            assert!(result.flags.offset_unjudged && result.flags.unsegmented, "{case}");
        }
    }

    /// 规格 §0/§3：某台相机这一组没有测成的帧（缺帧、测量失败、没测、走出走廊）→ 该组与整件标检测不完整，
    /// 其余相机照常判，不影响 OK/NG。
    #[test]
    fn missing_camera_frames_mark_incomplete_without_changing_verdict() {
        for (case, state, overflow_elsewhere, verdict) in [
            ("缺帧（没有记录）", None, false, Verdict::Ok),
            ("缺帧", Some(FrameState::Missing), false, Verdict::Ok),
            ("测量失败", Some(FrameState::Error), false, Verdict::Ok),
            ("没测", Some(FrameState::Unmeasured), false, Verdict::Ok),
            ("走出走廊", Some(FrameState::OutOfCorridor), false, Verdict::Ok),
            ("缺帧且其余相机溢胶", None, true, Verdict::NgWidth),
        ] {
            let segments = plan(&[Straight, Corner]);
            let mut metrics = frames(3);
            metrics.retain(|m| (m.camera, m.seq) != (1, 2));
            if let Some(state) = state {
                metrics.push(failed(1, 2, state));
            }
            if overflow_elsewhere {
                set_widths(at(&mut metrics, 0, 3), &[4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 9.5]);
            }
            let result = run(&segments, None, &[whole(&segments, UnsegReason::AlgorithmPending, 3)], &metrics);
            assert_eq!(result.verdict, verdict, "{case}");
            assert_eq!(result.regions[0].incomplete_groups, vec![2], "{case}");
            assert!(result.flags.incomplete, "{case}");
        }
    }

    /// 规格 §2：帧数超出容差只记帧数异常；默认做法 3：整件帧组数 < ΣN_exp − Σtol 另标疑似提前终止。结论都不变。
    /// 两段各容差 1：整件 14 ± 2。D-SB-40：直线 / 弯道区域的帧数是比例边界推算并扣掉过渡区的结果，不与本段 N_exp 比。
    #[test]
    fn frame_count_anomaly_comes_from_part_total_not_from_segment_regions() {
        let unsegmented = |n| vec![whole(&plan(&[Straight, Corner]), UnsegReason::AlgorithmPending, n)];
        for (case, regions, anomaly, short_part) in [
            (
                "未分段：整件 17 组高于上限 16",
                unsegmented(17),
                true,
                false,
            ),
            ("未分段：整件 11 组低于下限 12", unsegmented(11), true, true),
            (
                "直线段区域 12 组、本段 N_exp 10 ± 1，整件 15 组正常",
                vec![region(RegionKind::Straight { segment: 1 }, 1, 12), region(RegionKind::Corner { segment: 2 }, 13, 15)],
                false,
                false,
            ),
        ] {
            let mut segments = plan(&[Straight, Corner]);
            segments
                .iter_mut()
                .zip([10, 4])
                .for_each(|(segment, n)| segment.expected_frames = Some(n));
            let groups = regions.last().unwrap().last_seq;
            let result = run(&segments, Some(14), &regions, &frames(groups));
            assert_eq!(result.verdict, Verdict::Ok, "{case}");
            assert_eq!(
                (result.flags.frame_count_anomaly, result.flags.short_part),
                (anomaly, short_part),
                "{case}"
            );
        }
    }

    /// D-SB-40：边界过渡区按两侧段中较宽的允差判每帧指标，不判胶偏、不标胶偏未判，也不算“含未分段”。
    /// 直线段胶宽上限 5.5、溢胶 8；弯道段 6.5、9；过渡区一帧 9 站 6.0 mm 加 1 站 8.5 mm、x 偏 5 mm。
    #[test]
    fn transition_region_judges_frames_with_wider_limits_without_offset_or_unsegmented_flag() {
        let mut segments = plan(&[Straight, Corner]);
        segments[0].offset_ref = reference(Some(0.0), OffsetMeasure::AbsoluteX);
        segments[1].checks.width.tol_upper = 2.5;
        segments[1].checks.overflow.max_width_mm = 9.0;
        let mut metrics = frames(9);
        set_widths(
            at(&mut metrics, 0, 5),
            &[6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 8.5],
        );
        at(&mut metrics, 0, 5).x_mm = Some(5.0);
        let transition = RegionKind::Unsegmented {
            first_segment: 1,
            last_segment: 2,
            reason: UnsegReason::Transition,
        };
        let regions = [
            region(RegionKind::Straight { segment: 1 }, 1, 3),
            region(transition, 4, 6),
            region(RegionKind::Corner { segment: 2 }, 7, 9),
        ];
        let result = run(&segments, None, &regions, &metrics);
        assert_eq!(
            (result.regions[1].verdict, result.verdict),
            (Verdict::OkWithExcursion, Verdict::OkWithExcursion)
        );
        assert_eq!(
            (&result.regions[1].offset, result.regions[1].offset_unjudged),
            (&None, None)
        );
        assert!(!result.flags.offset_unjudged && !result.flags.unsegmented);
    }

    /// D-SB-41：直线 / 弯道区域只用本段检测相机——没选中的相机这一帧断胶、缺帧都不影响结论与检测不完整；
    /// 过渡区用两侧段检测相机的并集，选中相机的断胶照常判 NG。
    #[test]
    fn segment_regions_judge_only_their_cameras_and_transitions_use_the_union() {
        let mut segments = plan(&[Straight, Corner]);
        segments[0].cameras = vec![cameras()[0].clone()];
        segments[1].cameras = vec![cameras()[1].clone()];
        let transition = RegionKind::Unsegmented {
            first_segment: 1,
            last_segment: 2,
            reason: UnsegReason::Transition,
        };
        let regions = [
            region(RegionKind::Straight { segment: 1 }, 1, 3),
            region(transition, 4, 5),
            region(RegionKind::Corner { segment: 2 }, 6, 8),
        ];
        for (case, camera, seq, verdict, incomplete) in [
            ("段 1 没选中的 cam3 断胶", 2, 2, Verdict::Ok, false),
            ("段 2 没选中的 cam1 缺帧", 0, 7, Verdict::Ok, false),
            (
                "过渡区里 cam2（段 2 选中）断胶",
                1,
                4,
                Verdict::NgGap,
                false,
            ),
            ("段 1 选中的 cam1 缺帧", 0, 3, Verdict::Ok, true),
        ] {
            let mut metrics = frames(8);
            if case.contains("缺帧") {
                metrics.retain(|m| (m.camera, m.seq) != (camera, seq));
            } else {
                at(&mut metrics, camera, seq).gap_max_mm = 10.0;
            }
            let result = run(&segments, None, &regions, &metrics);
            assert_eq!(
                (result.verdict, result.flags.incomplete),
                (verdict, incomplete),
                "{case}"
            );
        }
    }

    /// 段首尾跳过的帧不判定：其中的断胶不判 NG、缺帧不算检测不完整，结果按帧组号列为未检区域。
    #[test]
    fn skipped_region_is_not_judged_and_listed_as_uninspected() {
        let segments = plan(&[Straight]);
        let regions = [
            region(RegionKind::Skipped { segment: 1 }, 1, 2),
            region(RegionKind::Straight { segment: 1 }, 3, 6),
        ];
        let mut metrics = frames(6);
        at(&mut metrics, 0, 1).gap_max_mm = 10.0;
        metrics.retain(|m| (m.camera, m.seq) != (1, 2));
        let result = run(&segments, None, &regions, &metrics);
        assert_eq!(
            (result.verdict, result.flags.incomplete),
            (Verdict::Ok, false)
        );
        assert_eq!(
            result.uninspected,
            vec![Uninspected {
                segment: 1,
                from_seq: Some(1),
                to_seq: Some(2),
                from_mm: None,
                to_mm: None
            }]
        );
    }

    /// 决策 D-SB-14：三台相机取最差——任一相机任一帧 NG，所在区域与整件即 NG（不沿用旧模型“一个视图合格即合格”）。
    #[test]
    fn any_camera_frame_ng_makes_region_ng() {
        let segments = plan(&[Straight, Corner]);
        let mut metrics = frames(4);
        at(&mut metrics, 2, 3).gap_max_mm = 10.0;
        let result = run(&segments, None, &[whole(&segments, UnsegReason::AlgorithmPending, 4)], &metrics);
        assert_eq!((result.regions[0].verdict, result.verdict), (Verdict::NgGap, Verdict::NgGap));
    }

    /// 规格 §4 每帧独立指标：断口超长、有胶比例低 → 断胶；胶宽超差累计超过允许长度 → 胶宽 NG（多处短超差累加），
    /// 不超过为允许范围内的超差；溢胶 → 胶宽 NG 且原因写“溢胶”（默认做法 1）。
    #[test]
    fn frame_exceeding_gap_presence_width_or_overflow_limit_is_ng_with_matching_reason() {
        type Edit = fn(&mut FrameMetrics);
        let cases: [(&str, Edit, Verdict); 5] = [
            ("断口 7 mm > 6 mm", |m| m.gap_max_mm = 7.0, Verdict::NgGap),
            ("有胶比例 0.7 < 0.8", |m| m.present_ratio = 0.7, Verdict::NgGap),
            ("两处各 3 mm 超差，累计 6 mm > 5 mm", |m| set_widths(m, &[6.0, 6.0, 6.0, 4.0, 4.0, 6.0, 6.0, 6.0, 4.0, 4.0]), Verdict::NgWidth),
            ("一处 3 mm 超差 ≤ 5 mm", |m| set_widths(m, &[6.0, 6.0, 6.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0]), Verdict::OkWithExcursion),
            ("溢胶：最大胶宽 9 mm > 8 mm，超差只有 2 mm", |m| set_widths(m, &[4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 9.0, 9.0]), Verdict::NgWidth),
        ];
        for (k, (case, edit, verdict)) in cases.into_iter().enumerate() {
            let segments = plan(&[Corner]);
            let mut metrics = frames(3);
            edit(at(&mut metrics, 0, 2));
            let result = run(&segments, None, &[region(RegionKind::Corner { segment: 1 }, 1, 3)], &metrics);
            assert_eq!(result.verdict, verdict, "{case}");
            assert_eq!(result.regions[0].reason.contains("溢胶"), k == 4, "{case}");
        }
    }

    #[test]
    fn to_edge_offset_compares_edge_distance_with_the_drawing_value_first_and_frames_without_an_edge_are_unjudged(
    ) {
        // 防的回归（D-SB-48）：ToEdge 仍按 x（胶相对胶嘴）判、图纸胶边距被标准件 x_ref 盖掉、没配找边参数时当作已判，
        // 或参考相机一帧都没找到边时静默判 OK
        use crate::plan::{EdgeParams, EdgePolarity};
        let region = Region {
            kind: crate::segmentation::RegionKind::Straight { segment: 1 },
            first_seq: 1,
            last_seq: 6,
        };
        let edge = |nominal_mm| EdgeParams {
            polarity: EdgePolarity::BrightToDark,
            search_mm: 20.0,
            contrast_min: 40.0,
            nominal_mm,
        };
        let mut frames = frames(6);
        for seq in 1..=6 {
            at(&mut frames, 0, seq).edge_mm = Some(5.0);
        }
        let judge_with = |edge: Option<EdgeParams>, frames: &[FrameMetrics]| {
            let mut segment = plan(&[Straight]).remove(0);
            segment.offset_ref = Some(OffsetRef {
                measure: OffsetMeasure::ToEdge,
                camera: "camA".into(),
                x_ref: Some(1.0),
                spread: Some(0.1),
                edge,
            });
            let measured: Vec<&FrameMetrics> = frames.iter().collect();
            let (stat, unjudged, found) =
                region_offset(&segment, &segment.checks, &region, &measured, &cameras());
            (
                stat.map(|s| s.mean_shift),
                unjudged,
                found.contains(&Finding::Shift),
            )
        };
        assert_eq!(
            judge_with(Some(edge(Some(5.0))), &frames),
            (Some(0.0), None, false),
            "按图纸 5 mm 比，不按 x 比"
        );
        assert_eq!(
            judge_with(Some(edge(None)), &frames),
            (Some(4.0), None, true),
            "没填图纸值时用 x_ref"
        );
        assert_eq!(
            judge_with(None, &frames).1,
            Some(OffsetUnjudged::MeasureNotImplemented)
        );
        frames.iter_mut().for_each(|m| m.edge_mm = None);
        assert_eq!(
            judge_with(Some(edge(Some(5.0))), &frames).1,
            Some(OffsetUnjudged::NoFrames)
        );
    }
}

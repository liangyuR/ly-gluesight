//! 段配方的候选工作区（D2）：每台计划相机的走廊示教、多轮标定（≥ N 轮得出整件 ΣN_exp 与单段 N_exp、容差建议）、
//! 容差确认、标准件轮次的胶偏参考 x_ref、段列表编辑，以及 `WorkspaceView.plan`。
//!
//! 只服务段配方（`RecipeDoc.segments` 非空）。旧拍照点配方的单帧示教、采用整圈采集、组选图仍走 workflow.rs，行为不变。
//! 规则见 docs/decisions/segment-burst.md（第二节、第六节口径与 D-SB-18/20/21/36）：
//! - N_exp、标定统计、整件标定与 x_ref 只能由多轮标定写入；段的序列或类型变化时一并清空；
//! - 标定轮次（不含标准件轮次）不少于配方的 `calibration_rounds`；帧数 = 帧组数；
//! - 本轮没有分段算法：每轮帧组数写整件标定，单段计划同时写这一段的 N_exp 与 calib；多段计划只有每个标定轮次都确认了
//!   手动分段时才按边界逐段标定，否则各段保持待标定；
//! - 建议容差只放在视图里，操作员确认（workspace_confirm_tolerance）后才写进配方；
//! - x_ref 只用标准件轮次、只给直线段（AbsoluteX），每轮去掉首尾各一帧取胶中心 x 均值，再取各轮均值与极差。

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use super::*;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::calibration::{self, CalibError, CalibOutcome};
use crate::frame_group::FrameGroup;
use crate::plan::{OffsetMeasure, OffsetRef, PartCalib, PathSegment, PlanExport, SegmentKind};
use crate::recipe::CameraInspect;
use crate::recipe_capture::{CaptureCamera, CaptureRoundV2, CaptureState};
use crate::segmentation::{Proportional, SegInput, SegOutcome, Segmenter};
use progress::StepStatus;

/// 每轮标定的每帧特征与分段结果放在候选目录 `calibration/<轮次>/` 下（供分段算法与重新标定使用）。
const CALIBRATION_DIR: &str = "calibration";
/// 一行一帧：`{camera, seq, metrics}`，按计划相机、帧组序号排列；1..=帧组数内每台相机每个 seq 都有一行（缺帧也有）。
pub(super) const FEATURES_FILE: &str = "features.jsonl";
/// 分段结果（`SegOutcome`）。
pub(super) const BOUNDARIES_FILE: &str = "boundaries.json";

/// 段配方一段、一台检测相机的示教帧（D-SB-45）：从完整的示教采集轮次冻结的一帧（原图存为候选 images/ 下的 PGM），
/// 这一段的走廊在它上面示教。旧版的相机级示教帧（`segment` 为空）已放弃，读入时丢弃。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraTeaching {
    pub camera: String,
    pub frame: FrozenImage,
    /// 来源采集轮次与帧组序号
    pub round: String,
    pub seq: u64,
    /// 所属段（`PathSegment.index`）；旧数据为空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<u32>,
}

/// 读入候选时丢弃旧版的相机级示教帧（D-SB-45 放弃相机走廊，不迁移）。返回是否丢弃过。
pub(super) fn drop_camera_teaching(w: &mut Workspace) -> bool {
    let before = w.camera_teaching.len();
    w.camera_teaching.retain(|t| t.segment.is_some());
    before != w.camera_teaching.len()
}

/// 一台相机的示教帧尺寸：它各段的示教帧同尺寸（示教时保证）；还没有示教帧时为 None。
fn camera_frame_size(teaching: &[CameraTeaching], camera: &str) -> Option<[u32; 2]> {
    teaching
        .iter()
        .find(|t| t.camera == camera && t.segment.is_some())
        .map(|t| t.frame.size)
}

/// 各计划相机的示教帧尺寸（计划相机顺序），建走廊测量图用；有相机还没有示教帧时报错。
pub(super) fn teaching_sizes(w: &Workspace) -> Result<Vec<[u32; 2]>, String> {
    w.doc
        .plan_cameras()
        .iter()
        .map(|camera| {
            camera_frame_size(&w.camera_teaching, camera)
                .ok_or_else(|| format!("相机 {camera} 还没有示教帧：先完成走廊示教"))
        })
        .collect()
}

/// 加入多轮标定的一轮（D-SB-36）。帧组数用于 N_exp；标准件轮次另用于胶偏参考 x_ref。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationRound {
    pub round_id: String,
    /// 标准件轮次：只用于 x_ref，不计入 N_exp 的轮数
    pub standard: bool,
    pub created_at: i64,
    /// 加入时的计划相机顺序；下面几项按它排列
    pub cameras: Vec<String>,
    /// 各相机采集时的设备配置：后加入的轮次必须相同（同一设备）
    pub camera_config: Vec<Value>,
    /// 帧组数 = 各相机最大 seq 的最大值
    pub group_count: u64,
    pub camera_frames: Vec<u32>,
    pub incomplete_groups: u32,
    pub loss_alarms: u32,
    /// 测量每帧特征时各相机的走廊身份（走廊、当量、检测参数、站距）：走廊变了，这一轮的 x 不再可比
    pub corridors: Vec<Value>,
    /// 各相机去掉首尾各一帧后、测成的帧的胶中心 x 均值（mm）；没有可用帧为 None
    pub x_means: Vec<Option<f64>>,
    /// 同口径的胶边距均值（mm，D-SB-48 ToEdge）；没找边的相机为 None
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edge_means: Vec<Option<f64>>,
    /// 逐帧定向的帧数统计（D-SB-47）：段号 → [定向测量的帧数, 其中回退的帧数]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub heading: BTreeMap<u32, [u32; 2]>,
}

/// 候选的多轮标定：加入的轮次（含标准件轮次）。标定结果写进配方（N_exp、calib、partCalib、x_ref），视图由配方与轮次算出。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Calibration {
    pub rounds: Vec<CalibrationRound>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationRoundView {
    pub round_id: String,
    pub standard: bool,
    pub group_count: u64,
    /// 按当前计划相机顺序
    pub camera_frames: Vec<u32>,
    pub incomplete_groups: u32,
    pub loss_alarms: u32,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XRefView {
    pub x_ref: f64,
    pub spread: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentCalibView {
    pub index: u32,
    pub kind: SegmentKind,
    /// None = 本段未能标定，原因见 pending
    pub outcome: Option<CalibOutcome>,
    pub pending: Option<String>,
    /// 配方里当前的容差
    pub tolerance: u32,
    pub x_ref: Option<XRefView>,
    pub x_ref_pending: Option<String>,
    /// 逐帧定向的提示（D-SB-47）：检测窗口能否覆盖每帧后移量、定向回退比例
    pub hints: Vec<String>,
}

/// 定向回退比例超过它就提示（窗口内胶路跟不住，多见于弯道）。
const HEADING_FALLBACK_HINT: f64 = 0.2;

/// 开启逐帧定向的段的标定提示（D-SB-47）：每帧后移量 ≈ 段长 ÷ N_exp，检测窗口短于它时相邻帧之间有胶测不到，
/// 超过 3 倍时同一处胶被重复测；各轮定向回退的帧超过 20% 时提示缩短窗口或减小起点。不阻断发布。
pub(super) fn heading_hints(
    doc: &RecipeDoc,
    segment: &PathSegment,
    rounds: &[CalibrationRound],
) -> Vec<String> {
    if segment.heading.is_none() {
        return Vec::new();
    }
    let mut hints = Vec::new();
    let cameras = crate::plan::segment_cameras(segment, &doc.plan_cameras());
    match (
        segment.length_mm,
        segment.expected_frames.filter(|&n| n > 0),
    ) {
        (None, _) => hints.push("段长未填，无法核对检测窗口能否覆盖每帧后移量".to_string()),
        (Some(_), None) => {}
        (Some(length), Some(n)) => {
            let travel = length / f64::from(n);
            for inspect in doc.inspect.iter().filter(|i| cameras.contains(&i.camera)) {
                let Some(window) = &inspect.window else {
                    continue;
                };
                if window.length_mm < travel {
                    hints.push(format!(
                        "相机 {} 检测窗口长 {:.1} mm，短于每帧后移量约 {travel:.1} mm：相邻帧之间有胶测不到，断胶可能漏检",
                        inspect.camera, window.length_mm
                    ));
                } else if window.length_mm > 3.0 * travel {
                    hints.push(format!(
                        "相机 {} 检测窗口长 {:.1} mm，超过每帧后移量约 {travel:.1} mm 的 3 倍：同一处胶会被测多次",
                        inspect.camera, window.length_mm
                    ));
                }
            }
        }
    }
    let [measured, fallback] = rounds
        .iter()
        .filter_map(|r| r.heading.get(&segment.index))
        .fold([0u32; 2], |a, b| [a[0] + b[0], a[1] + b[1]]);
    if measured > 0 && f64::from(fallback) > HEADING_FALLBACK_HINT * f64::from(measured) {
        hints.push(format!(
            "定向回退 {fallback}/{measured} 帧（{:.0}%）：窗口内胶路跟不住，建议缩短检测窗口或减小起点",
            100.0 * f64::from(fallback) / f64::from(measured)
        ));
    }
    hints
}

/// 多轮标定（TS `CalibrationView`）：由配方当前的标定结果与已加入的轮次算出。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationView {
    pub min_rounds: u32,
    pub rounds: Vec<CalibrationRoundView>,
    /// 整件帧组数统计 → PartCalib
    pub part: Option<CalibOutcome>,
    pub segments: Vec<SegmentCalibView>,
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
    /// 已标定且计划身份与生产不同：发布时分配新计划版本，PLC 要同步 shotCount
    pub plan_version_change: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraTeachingView {
    pub camera: String,
    pub mm_per_px: Option<f64>,
    /// 走廊示教完成：有像素当量，且它检测的每一段都有已保存的走廊（D-SB-45）
    pub ready: bool,
    /// 已选示教帧的段，按段号
    pub segments: Vec<SegmentTeachingView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentTeachingView {
    pub segment: u32,
    pub frame: FrozenImage,
    pub round: String,
    pub seq: u64,
    /// 走廊已保存且完整落在示教帧内
    pub saved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepIdV2 { Capture, Segments, Corridor, Calibration, Validation, Publish }

/// 阻塞原因与“下一步”跳转的页面（TS `ProgressTargetV2`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "page", rename_all = "camelCase")]
pub enum TargetV2 {
    Capture,
    /// 契约里的段列表页；段列表能保存即合法，目前没有指向它的阻塞原因
    #[allow(dead_code)]
    Segments,
    Corridor {
        camera: String,
        segment: Option<u32>,
    },
    Calibration,
    Validation,
    #[allow(dead_code)]
    Camera,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerV2 { pub message: String, pub target: TargetV2 }

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepV2 {
    pub id: StepIdV2,
    pub label: &'static str,
    pub optional: bool,
    pub status: StepStatus,
    pub detail: String,
    pub blockers: Vec<BlockerV2>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextV2 { pub step: StepIdV2, pub label: String, pub target: TargetV2 }

/// 段配方的进度（TS `ProgressV2`）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressV2 { pub steps: Vec<StepV2>, pub next: Option<NextV2> }

/// 候选工作区里段配方的计划部分（TS `WorkspacePlanView`）；旧拍照点配方没有。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePlanView {
    pub segments: Vec<PathSegment>,
    pub cameras: Vec<String>,
    pub segment_defaults: crate::plan::KindDefaults,
    pub calibration_rounds: u32,
    pub part_calib: Option<PartCalib>,
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
    /// expected_total 非空：满足发布的帧数门槛
    pub calibrated: bool,
    pub camera_teaching: Vec<CameraTeachingView>,
    pub calibration: Option<CalibrationView>,
    pub production_export: Option<PlanExport>,
    pub candidate_export: Option<PlanExport>,
    pub candidate_export_error: Option<String>,
    /// 候选 expectedTotal 与生产不同：生效后 PLC 要同步 shotCount / deviceShots
    pub shot_count_changed: bool,
    pub progress: ProgressV2,
}

/// features.jsonl 的一行。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureLine<'a> {
    camera: &'a str,
    seq: u64,
    metrics: &'a FrameMetrics,
}

fn require_burst(doc: &RecipeDoc) -> Result<(), String> {
    if doc.is_burst() { Ok(()) } else { Err("旧拍照点配方没有段计划：请用单帧示教、整圈采集与原有标定步骤".into()) }
}

fn kind_default(doc: &RecipeDoc, kind: SegmentKind) -> u32 {
    match kind {
        SegmentKind::Straight => doc.segment_defaults.straight.tolerance,
        SegmentKind::Corner => doc.segment_defaults.corner.tolerance,
    }
}

/// 一台相机的走廊身份：测每帧特征用到的当量、检测参数、站距与各段走廊；不是计划相机为 null。
pub(super) fn corridor_tag(doc: &RecipeDoc, camera: &str) -> Value {
    doc.inspect
        .iter()
        .find(|i| i.camera == camera)
        .map_or(Value::Null, |i| {
            json!([
                i.mm_per_px,
                i.detect.as_ref().unwrap_or(&doc.detect),
                doc.spacing,
                i.segments,
                i.window,
                doc.segments
                    .iter()
                    .map(|s| (s.index, &s.heading))
                    .collect::<Vec<_>>()
            ])
        })
}

fn path_saved(corridor: &[[f32; 2]], size: [u32; 2]) -> bool {
    let length: f32 = corridor
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .sum();
    corridor.len() >= 2 && length >= 1.0 && vision::corridor_inside(corridor, size)
}

/// 段走廊可用于测量：已选这一段的示教帧，走廊已保存且落在那一帧内（与 `vision::plan_corridor` 的前置相同）。
pub(super) fn segment_corridor_saved(
    inspect: &CameraInspect,
    segment: u32,
    teaching: &[CameraTeaching],
) -> bool {
    let frame = teaching
        .iter()
        .find(|t| t.camera == inspect.camera && t.segment == Some(segment));
    match (inspect.segment_corridor(segment), frame) {
        (Some(s), Some(t)) => path_saved(&s.corridor, t.frame.size),
        _ => false,
    }
}

/// 段走廊随段结构与段相机失效（D-SB-42/45）：段的类型序列变了，各相机的段走廊全部清空、返回 true；
/// 某段不再由某台相机检测时，去掉它在这一段的走廊。保存的每条段走廊都要有对应的段示教帧。
/// 调用方据此去掉不再适用的段示教帧（见 `update_doc`）。
pub(super) fn guard_segment_corridors(
    old: &RecipeDoc,
    new: &mut RecipeDoc,
    teaching: &[CameraTeaching],
) -> Result<bool, String> {
    if !new.is_burst() {
        return Ok(false);
    }
    let kinds = |doc: &RecipeDoc| {
        doc.segments
            .iter()
            .map(|s| (s.index, s.kind))
            .collect::<Vec<_>>()
    };
    if kinds(old) != kinds(new) {
        new.inspect.iter_mut().for_each(|i| i.segments.clear());
        return Ok(true);
    }
    let cameras = new.plan_cameras();
    let segments = new.segments.clone();
    for inspect in &mut new.inspect {
        let camera = inspect.camera.clone();
        inspect
            .segments
            .retain(|s| crate::plan::detects(&segments, s.segment, &camera, &cameras));
    }
    for inspect in &new.inspect {
        if let Some(s) = inspect.segments.iter().find(|s| {
            !teaching
                .iter()
                .any(|t| t.camera == inspect.camera && t.segment == Some(s.segment))
        }) {
            return Err(format!(
                "相机 {} 第 {} 段还没有选示教帧",
                inspect.camera, s.segment
            ));
        }
    }
    Ok(false)
}

/// 这条段示教帧在当前段列表下仍适用：段结构没变，且这台相机仍检测这一段。
pub(super) fn teaching_applies(
    doc: &RecipeDoc,
    teaching: &CameraTeaching,
    structure_changed: bool,
) -> bool {
    !structure_changed
        && teaching.segment.is_some_and(|segment| {
            crate::plan::detects(
                &doc.segments,
                segment,
                &teaching.camera,
                &doc.plan_cameras(),
            )
        })
}

/// 标定按段选走廊（D-SB-45）：按本轮已确认的手动分段，每帧只由所在段的检测相机用该段走廊测量，其余相机不测（None）。
/// 单段配方所有帧属于第 1 段；多段配方本轮没有确认手动分段时报错。按计划相机、seq（1..=帧组数）给出段号。
pub(super) fn calibration_corridors(
    w: &Workspace,
    round: &str,
    group_count: u64,
) -> Result<Vec<Vec<Option<u32>>>, String> {
    let segments = &w.doc.segments;
    let owner: Vec<u32> = if segments.len() == 1 {
        vec![segments[0].index; group_count as usize]
    } else {
        let counts = capture_review::confirmed_segment_counts(
            &w.doc,
            &w.capture_review.rounds,
            round,
            group_count,
        )
        .ok_or_else(|| {
            format!(
                "采集轮次 {round} 还没有确认手动分段：先在分段标定里确认这一轮的边界，再加入标定"
            )
        })?;
        segments
            .iter()
            .zip(counts)
            .flat_map(|(s, n)| std::iter::repeat_n(s.index, n as usize))
            .collect()
    };
    let cameras = w.doc.plan_cameras();
    Ok(cameras
        .iter()
        .map(|camera| {
            owner
                .iter()
                .map(|&segment| {
                    crate::plan::detects(segments, segment, camera, &cameras).then_some(segment)
                })
                .collect()
        })
        .collect())
}

/// 一轮里每台计划相机每个 seq 到没到（缺帧为 false），按计划相机、seq 排列，复测用（见 `burst_history::remeasure_by_rule`）。
pub(super) fn round_frames(cameras: &[String], round: &CaptureRoundV2) -> Vec<(usize, u64, bool)> {
    cameras
        .iter()
        .enumerate()
        .flat_map(|(index, camera)| {
            let source = round.cameras.iter().position(|c| &c.camera_id == camera);
            (1..=round.group_count).map(move |seq| {
                let missing = source.is_none_or(|j| {
                    round
                        .missing_frames
                        .iter()
                        .any(|m| m.camera == j && m.seq == seq)
                });
                (index, seq, !missing)
            })
        })
        .collect()
}

/// 没有测量值的一帧（缺帧、测量失败）。
pub(super) fn frame_without_metrics(
    camera: usize,
    seq: u64,
    state: FrameState,
    error: Option<String>,
) -> FrameMetrics {
    FrameMetrics {
        camera,
        seq,
        state,
        error,
        width_min: None,
        width_max: None,
        width_mean: None,
        widths: Vec::new(),
        spacing_mm: 0.0,
        gap_max_mm: 0.0,
        present_ratio: 0.0,
        x_mm: None,
        corridor: None,
        heading_deg: None,
        heading_fallback: false,
        edge_mm: None,
    }
}

// ───────────── 段列表与配方编辑 ─────────────

/// 清空全部 N_exp、标定统计、整件标定与 x_ref。
pub(super) fn clear_calibration(doc: &mut RecipeDoc) {
    for segment in &mut doc.segments {
        segment.expected_frames = None;
        segment.calib = None;
        if let Some(reference) = &mut segment.offset_ref { reference.x_ref = None; reference.spread = None; }
    }
    doc.part_calib = None;
}

/// 新建段配方候选（`workspace_create` 已清掉拍照点与相机参数）：计划相机补默认成像参数（不然过不了“检测相机要有曝光与增益”的校验），
/// 复制来的 N_exp、标定统计、整件标定与 x_ref 清空——它们只能由本配方的多轮标定得出。旧拍照点配方不经过。
pub(super) fn prepare_new_doc(doc: &mut RecipeDoc) {
    if !doc.is_burst() { return; }
    clear_calibration(doc);
    doc.sync_imaging();
}

/// 段配方的标定只能由多轮标定写入（D-SB-18/20）：段的类型序列不变时，N_exp、标定统计、整件标定不能改，
/// x_ref / spread 只能原样带过或清空；胶偏参考的测量量或相机变了，旧 x_ref 不再适用，一并清空。
/// 段的类型序列（段数、顺序、类型）变了，全部 N_exp、标定统计、整件标定与 x_ref 清空，等重新标定。
/// 两份都是旧拍照点配方时不检查（旧路径行为不变）。
pub(super) fn guard_calibration(old: &RecipeDoc, new: &mut RecipeDoc) -> Result<(), String> {
    if !old.is_burst() && !new.is_burst() { return Ok(()); }
    let kinds = |doc: &RecipeDoc| doc.segments.iter().map(|s| s.kind).collect::<Vec<_>>();
    if kinds(old) != kinds(new) {
        clear_calibration(new);
        return Ok(());
    }
    if new.part_calib != old.part_calib {
        return Err("整件预期帧组数只能由多轮标定写入，不能直接修改".into());
    }
    for (before, after) in old.segments.iter().zip(new.segments.iter_mut()) {
        if after.expected_frames != before.expected_frames || after.calib != before.calib {
            return Err(format!("第 {} 段的预期帧数与标定统计只能由多轮标定写入，不能直接修改", after.index));
        }
        let Some(reference) = after.offset_ref.as_mut() else {
            continue;
        };
        let edge = |r: &OffsetRef| {
            r.edge
                .as_ref()
                .map(|e| (e.polarity, e.search_mm, e.contrast_min))
        };
        let previous = before.offset_ref.as_ref().filter(|old| {
            old.measure == reference.measure
                && old.camera == reference.camera
                && edge(old) == edge(reference)
        });
        match previous {
            None => { reference.x_ref = None; reference.spread = None; }
            Some(old) => {
                let cleared = reference.x_ref.is_none() && reference.spread.is_none();
                if !cleared && (reference.x_ref, reference.spread) != (old.x_ref, old.spread) {
                    return Err(format!("第 {} 段的胶偏参考 x_ref 只能由标准件多轮采集得出，不能手填", after.index));
                }
            }
        }
    }
    Ok(())
}

/// 整体替换段列表（经 `update_doc`，同样受 `guard_calibration` 约束）。
pub(super) fn update_segments(w: &mut Workspace, segments: Vec<PathSegment>) -> Result<(), String> {
    require_burst(&w.doc)?;
    if segments.is_empty() { return Err("段配方至少需要一段，不能改成拍照点配方".into()); }
    let mut doc = w.doc.clone();
    doc.segments = segments;
    update_doc(w, doc)
}

/// 操作员确认第 `segment` 段（PathSegment.index）的帧数容差。
pub(super) fn confirm_tolerance(w: &mut Workspace, segment: u32, tolerance: u32) -> Result<(), String> {
    require_burst(&w.doc)?;
    if tolerance > crate::plan::MAX_TOLERANCE {
        return Err(format!("帧数容差 {tolerance} 超过上限 {}", crate::plan::MAX_TOLERANCE));
    }
    let mut doc = w.doc.clone();
    let target = doc.segments.iter_mut().find(|s| s.index == segment).ok_or_else(|| format!("没有第 {segment} 段"))?;
    if target.tolerance == tolerance { return Ok(()); }
    target.tolerance = tolerance;
    doc.build()?;
    w.doc = doc;
    w.changed();
    Ok(())
}

// ───────────── 走廊示教 ─────────────

/// 用来示教的一帧要来自本候选完整的段配方采集轮次，这台相机有这一帧，且采集后设备配置没变。返回这台相机的采集记录。
fn teaching_source<'a>(round: &'a CaptureRoundV2, recipe_id: &str, camera: &str, seq: u64, current_config: &Value) -> Result<&'a CaptureCamera, String> {
    if round.recipe_id != recipe_id || round.state != CaptureState::Complete {
        return Err(format!("采集轮次 {} 不属于当前候选或尚未完整完成，不能用于示教", round.round_id));
    }
    let (index, source) = round.cameras.iter().enumerate().find(|(_, c)| c.camera_id == camera)
        .ok_or_else(|| format!("采集轮次 {} 没有相机 {camera}", round.round_id))?;
    if !(1..=round.group_count).contains(&seq) {
        return Err(format!("采集轮次 {} 只有 {} 个帧组，没有第 {seq} 个", round.round_id, round.group_count));
    }
    if round.missing_frames.iter().any(|m| m.camera == index && m.seq == seq) {
        return Err(format!("相机 {camera} 缺第 {seq} 帧组的图像，请换一帧"));
    }
    if &source.camera_config != current_config {
        return Err(format!("采集后相机 {camera} 的参数已改变，请重新采集"));
    }
    Ok(source)
}

/// 段走廊的示教帧（D-SB-45）：这台相机是该段的检测相机；示教轮次已确认手动分段（单段配方不需要），帧组落在这一段内；
/// 与这台相机已有的示教帧同尺寸。同一相机同一段只留一帧，已画的走廊原样保留。相机还没有像素当量时用 `default_scale`
/// （设备标定的当量）。
pub(super) fn apply_segment_teaching(
    w: &mut Workspace,
    teaching: CameraTeaching,
    group_count: u64,
    default_scale: Option<f64>,
) -> Result<(), String> {
    require_burst(&w.doc)?;
    let (camera, segment) = (
        teaching.camera.clone(),
        teaching.segment.ok_or("段走廊的示教帧缺段号")?,
    );
    let position = w
        .doc
        .segments
        .iter()
        .position(|s| s.index == segment)
        .ok_or_else(|| format!("没有第 {segment} 段"))?;
    if !crate::plan::detects(&w.doc.segments, segment, &camera, &w.doc.plan_cameras()) {
        return Err(format!("相机 {camera} 不是第 {segment} 段的检测相机"));
    }
    let others = w
        .camera_teaching
        .iter()
        .filter(|t| t.camera == camera && t.segment.is_some_and(|s| s != segment));
    if others
        .map(|t| t.frame.size)
        .any(|size| size != teaching.frame.size)
    {
        return Err(format!("这一帧尺寸与相机 {camera} 其他段的示教帧不同"));
    }
    capture_review::require_available(w, &teaching.round, teaching.seq)?;
    if w.doc.segments.len() > 1 {
        let counts = capture_review::confirmed_segment_counts(
            &w.doc,
            &w.capture_review.rounds,
            &teaching.round,
            group_count,
        )
        .ok_or_else(|| {
            format!(
                "采集轮次 {} 还没有确认手动分段：先在分段标定里确认这一轮的边界",
                teaching.round
            )
        })?;
        let start = counts[..position]
            .iter()
            .map(|&n| u64::from(n))
            .sum::<u64>()
            + 1;
        let end = start + u64::from(counts[position]) - 1;
        if !(start..=end).contains(&teaching.seq) {
            return Err(format!(
                "第 {} 帧组不在第 {segment} 段（这一轮为帧组 {start}–{end}）",
                teaching.seq
            ));
        }
    }
    let mut doc = w.doc.clone();
    let inspect = doc
        .inspect
        .iter_mut()
        .find(|i| i.camera == camera)
        .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
    inspect.mm_per_px = inspect.mm_per_px.or(default_scale);
    doc.build()?;
    w.doc = doc;
    w.camera_teaching
        .retain(|t| !(t.camera == camera && t.segment == Some(segment)));
    w.camera_teaching.push(teaching);
    w.changed();
    Ok(())
}

/// 去掉一台相机在某一段的走廊与示教帧，这一段回到“待示教”。
pub(super) fn clear_segment_corridor(
    w: &mut Workspace,
    camera: &str,
    segment: u32,
) -> Result<(), String> {
    require_burst(&w.doc)?;
    let before = w.camera_teaching.len();
    w.camera_teaching
        .retain(|t| !(t.camera == camera && t.segment == Some(segment)));
    let mut removed = before != w.camera_teaching.len();
    if let Some(inspect) = w.doc.inspect.iter_mut().find(|i| i.camera == camera) {
        let count = inspect.segments.len();
        inspect.segments.retain(|s| s.segment != segment);
        removed |= count != inspect.segments.len();
    }
    if !removed {
        return Err(format!("相机 {camera} 第 {segment} 段没有走廊或示教帧"));
    }
    w.changed();
    Ok(())
}

/// 用采集轮次 `round` 里相机 `camera` 的第 `seq` 帧示教第 `segment` 段的走廊（D-SB-42）：冻结这一帧，走廊之后在这一帧上画、随候选保存。
#[tauri::command]
pub async fn workspace_teach_segment_corridor(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    segment: u32,
    round: String,
    seq: u64,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (frame, default_scale, group_count) =
            freeze_teaching_frame(&app, &id, revision, &camera, &round, seq)?;
        let host = app.state::<WorkspaceHost>();
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(&id).ok_or("候选配置不存在")?;
        current.expect(revision)?;
        let mut next = current.clone();
        apply_segment_teaching(
            &mut next,
            CameraTeaching {
                camera,
                frame,
                round,
                seq,
                segment: Some(segment),
            },
            group_count,
            default_scale,
        )?;
        host.save(&next)?;
        *current = next;
        view(&app, current.clone())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn workspace_clear_segment_corridor(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    segment: u32,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    clear_segment_corridor(&mut w, &camera, segment)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

/// 把采集轮次里的一帧冻结成示教帧（存为候选 images/ 下的 PGM），返回冻结记录、工位标定的像素当量与这一轮的帧组数。
fn freeze_teaching_frame(
    app: &AppHandle,
    id: &str,
    revision: u64,
    camera: &str,
    round: &str,
    seq: u64,
) -> Result<(FrozenImage, Option<f64>, u64), String> {
    let host = app.state::<WorkspaceHost>();
    let inspect = {
        let items = host.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        require_burst(&w.doc)?;
        w.doc.inspect.iter().find(|i| i.camera == camera).cloned().ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?
    };
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    let captured = capture.get_burst(round)?;
    let config = cycle.camera.configs().into_iter().find(|c| c.id == camera).ok_or_else(|| format!("相机 {camera} 已不在设备配置里"))?;
    let imaging = teaching_source(&captured, id, camera, seq, &state_value(&config))?.imaging;
    let image = crate::image_file::load(&capture.burst_frame_path(round, camera, seq)?)?;
    let calib = vision::camera_calib_path(app, &inspect)?;
    let default_scale = if config.source == CameraSource::Sim { Some(f64::from(crate::recipe::SIM_MM_PER_PX)) }
        else { vision::calib_info(&calib).and_then(|c| c.mm_per_px) };
    let sample_set = crate::offline::sample_set(app).map(|set| set.manifest().id.clone());
    let image_id = format!("corridor-{}-{revision}-{seq}-{}", ly_plc::now_ms(), host.capture_seq.fetch_add(1, Ordering::Relaxed));
    let path = image_file(&host, id, &image_id)?;
    std::fs::create_dir_all(path.parent().ok_or("示教图像目录无效")?).map_err(|e| e.to_string())?;
    crate::image_file::save_pgm_new(&path, &image)?;
    let frame = FrozenImage {
        id: image_id,
        view: 1,
        source: format!("capture:{round}#{seq}"),
        captured_at: captured.created_at,
        size: [image.width, image.height],
        camera: camera.to_string(),
        camera_tag: camera_state(&config, sample_set.as_deref()),
        calib_tag: calibration_state(&calib)?,
        geometry_tag: json!({"round": round, "camera": camera, "seq": seq}),
        exposure_us: Some(imaging.exposure_us), gain_db: Some(imaging.gain_db), history_id: None,
    };
    Ok((frame, default_scale, captured.group_count))
}

/// 段配方的走廊提取（决策记录 5.4）：在相机 `camera` 冻结的示教帧 `image_id` 上按选区提取，复用旧中线提取的边缘算法；
/// 检测极性取这台相机的检测参数，没有时取配方默认。旧配方仍用 `workspace_extract_centerline`。
#[tauri::command]
pub async fn workspace_extract_corridor(app: AppHandle, id: String, revision: u64, camera: String, image_id: String, roi: [u32; 4], guide: Option<super::centerline::BrushGuide>, point_spacing_px: Option<f32>) -> Result<CenterlineResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let frozen = |w: &Workspace| {
            w.camera_teaching
                .iter()
                .any(|t| t.camera == camera && t.frame.id == image_id)
        };
        let (path, polarity) = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选不存在")?;
            w.expect(revision)?;
            require_burst(&w.doc)?;
            if !frozen(w) {
                return Err("图像已变化，请重新框选".into());
            }
            let inspect = w
                .doc
                .inspect
                .iter()
                .find(|i| i.camera == camera)
                .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
            (
                image_file(&host, &id, &image_id)?,
                inspect.detect.as_ref().unwrap_or(&w.doc.detect).polarity,
            )
        };
        let image = crate::image_file::load(&path)?;
        let result = super::centerline::extract(&image, roi, polarity, guide.as_ref(), point_spacing_px.unwrap_or(20.0));
        let items = host.items.lock().unwrap();
        let current = items.get(&id).ok_or("候选不存在")?;
        current.expect(revision)?;
        if !frozen(current) {
            return Err("提取期间图像已变化".into());
        }
        match result {
            Ok(extracted) => Ok(CenterlineResult { path: extracted.path, message: extracted.notice.unwrap_or_else(|| "走廊已提取，请检查并人工修正后保存".into()), failure: None }),
            Err(super::centerline::ExtractionError::Failed(failure)) => Ok(CenterlineResult { path: Vec::new(), message: failure.message.clone(), failure: Some(failure) }),
            Err(super::centerline::ExtractionError::Invalid(message)) => Err(message),
        }
    }).await.map_err(|e| e.to_string())?
}

// ───────────── 多轮标定 ─────────────

/// 一轮能不能加入标定（规格 §2 同一工件程序、同一设备）：属于本候选、已完整完成、还没加入；相机集合等于计划相机；
/// 每台相机采集时的设备配置与当前设备相同，也与已加入的轮次相同。`current` 为当前设备配置（按相机编号）。
pub(super) fn check_calibration_devices(rounds: &[CalibrationRound], current: &BTreeMap<String, Value>) -> Result<(), String> {
    for round in rounds {
        for (index, camera) in round.cameras.iter().enumerate() {
            let mut config = current.get(camera).cloned().unwrap_or(Value::Null);
            if let Some(object) = config.as_object_mut() { object.remove("sampleSet"); }
            if config.is_null() || Some(&config) != round.camera_config.get(index) {
                return Err(format!("相机 {camera} 的参数与标定轮次 {} 不一致，请恢复采集参数或重新标定", round.round_id));
            }
        }
    }
    Ok(())
}

pub(super) fn check_round(doc: &RecipeDoc, rounds: &[CalibrationRound], round: &CaptureRoundV2, current: &BTreeMap<String, Value>) -> Result<(), String> {
    require_burst(doc)?;
    let id = &round.round_id;
    if round.recipe_id != doc.id { return Err(format!("采集轮次 {id} 属于配方 {}，不是当前候选 {}", round.recipe_id, doc.id)); }
    if round.state != CaptureState::Complete { return Err(format!("采集轮次 {id} 尚未完整完成，不能加入标定")); }
    if rounds.iter().any(|r| &r.round_id == id) { return Err(format!("采集轮次 {id} 已加入多轮标定")); }
    let cameras = doc.plan_cameras();
    let ids: BTreeSet<&str> = round.cameras.iter().map(|c| c.camera_id.as_str()).collect();
    if round.cameras.len() != cameras.len() || ids != cameras.iter().map(String::as_str).collect() {
        return Err(format!("采集轮次 {id} 的相机 {:?} 与计划相机 {cameras:?} 不同", round.camera_ids()));
    }
    if round.group_count == 0 { return Err(format!("采集轮次 {id} 没有帧组")); }
    for camera in &round.cameras {
        let camera_id = &camera.camera_id;
        if current.get(camera_id) != Some(&camera.camera_config) {
            return Err(format!("采集后相机 {camera_id} 的参数已改变，请重新采集"));
        }
        for other in rounds {
            let config = other.cameras.iter().position(|c| c == camera_id).and_then(|j| other.camera_config.get(j));
            if config.is_some_and(|config| *config != camera.camera_config) {
                return Err(format!("采集轮次 {id} 里相机 {camera_id} 的设备配置与已加入标定的轮次 {} 不同：多轮标定要求同一工件程序、同一设备", other.round_id));
            }
        }
    }
    Ok(())
}

/// 量一轮的每一帧：按计划相机、帧组序号 1..=帧组数排列；缺帧记 Missing，`measure` 失败的帧记 Error（规格 §0 所有帧都测）。
/// `measure(计划相机下标, 相机编号, seq)`。
pub(super) fn measure_round(cameras: &[String], round: &CaptureRoundV2, mut measure: impl FnMut(usize, &str, u64) -> Result<FrameMetrics, String>) -> Vec<FrameMetrics> {
    let mut metrics = Vec::with_capacity(cameras.len() * round.group_count as usize);
    for (index, camera) in cameras.iter().enumerate() {
        let source = round.cameras.iter().position(|c| &c.camera_id == camera);
        let missing: BTreeSet<u64> = round.missing_frames.iter().filter(|m| Some(m.camera) == source).map(|m| m.seq).collect();
        for seq in 1..=round.group_count {
            if source.is_none() || missing.contains(&seq) {
                metrics.push(frame_without_metrics(index, seq, FrameState::Missing, None));
                continue;
            }
            metrics.push(match measure(index, camera, seq) {
                Ok(m) => FrameMetrics { camera: index, seq, ..m },
                Err(error) => frame_without_metrics(index, seq, FrameState::Error, Some(error)),
            });
        }
    }
    metrics
}

/// 用比例分段器分段（D-SB-40；各段 N_exp 未全部标定时为未分段），结果写进 boundaries.json 供追溯。
pub(super) fn segment_round(
    segments: &[PathSegment],
    cameras: usize,
    group_count: u64,
    metrics: &[FrameMetrics],
) -> SegOutcome {
    let groups: Vec<FrameGroup> = (1..=group_count)
        .map(|seq| {
            let present: Vec<bool> = (0..cameras)
                .map(|camera| {
                    metrics.iter().any(|m| {
                        m.camera == camera && m.seq == seq && m.state != FrameState::Missing
                    })
                })
                .collect();
            FrameGroup {
                seq,
                incomplete: present.contains(&false),
                present,
            }
        })
        .collect();
    Proportional.segment(&SegInput {
        groups: &groups,
        segments,
    })
}

/// features.jsonl 读回的一行。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredFeature {
    camera: String,
    seq: u64,
    metrics: FrameMetrics,
}

/// 读一轮标定的每帧特征（`FEATURES_FILE`）：相机按当前计划相机重新编下标，不再是计划相机的行丢弃。
pub(super) fn read_features(dir: &Path, cameras: &[String]) -> Result<Vec<FrameMetrics>, String> {
    let text = std::fs::read_to_string(dir.join(FEATURES_FILE))
        .map_err(|e| format!("读取标定特征失败：{e}"))?;
    let mut metrics = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let stored: StoredFeature =
            serde_json::from_str(line).map_err(|e| format!("标定特征格式无效：{e}"))?;
        let Some(camera) = cameras.iter().position(|c| *c == stored.camera) else {
            continue;
        };
        metrics.push(FrameMetrics {
            camera,
            seq: stored.seq,
            ..stored.metrics
        });
    }
    Ok(metrics)
}

/// 写每帧特征与分段结果（文件格式见 FEATURES_FILE / BOUNDARIES_FILE）。
pub(super) fn write_features(dir: &Path, cameras: &[String], metrics: &[FrameMetrics], outcome: &SegOutcome) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建标定特征目录失败：{e}"))?;
    let mut text = String::new();
    for m in metrics {
        let camera = cameras.get(m.camera).ok_or("特征的相机下标超出计划相机")?;
        text.push_str(&serde_json::to_string(&FeatureLine { camera, seq: m.seq, metrics: m }).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    crate::fsio::write_atomic(&dir.join(FEATURES_FILE), &text)?;
    crate::fsio::write_atomic(&dir.join(BOUNDARIES_FILE), &serde_json::to_string_pretty(outcome).map_err(|e| e.to_string())?)
}

/// 一台相机去掉首尾各一帧（seq 1 与帧组数）后、测成的帧的测量量（胶中心 x 或胶边距）均值。
fn round_mean(
    metrics: &[FrameMetrics],
    camera: usize,
    group_count: u64,
    value: fn(&FrameMetrics) -> Option<f64>,
) -> Option<f64> {
    let xs: Vec<f64> = metrics
        .iter()
        .filter(|m| {
            m.camera == camera
                && m.seq > 1
                && m.seq < group_count
                && m.state == FrameState::Measured
        })
        .filter_map(value)
        .collect();
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// 由采集轮次与每帧特征得出要记下的一轮（按当前计划相机顺序）。
pub(super) fn calibration_round(doc: &RecipeDoc, round: &CaptureRoundV2, standard: bool, metrics: &[FrameMetrics]) -> CalibrationRound {
    let cameras = doc.plan_cameras();
    let source: Vec<Option<usize>> = cameras.iter().map(|c| round.cameras.iter().position(|r| &r.camera_id == c)).collect();
    CalibrationRound {
        round_id: round.round_id.clone(),
        standard,
        created_at: round.created_at,
        camera_config: source.iter().map(|j| j.map_or(Value::Null, |j| round.cameras[j].camera_config.clone())).collect(),
        group_count: round.group_count,
        camera_frames: source.iter().map(|j| j.and_then(|j| round.camera_frames.get(j).copied()).unwrap_or(0)).collect(),
        incomplete_groups: u32::try_from(round.incomplete_groups.len()).unwrap_or(u32::MAX),
        loss_alarms: u32::try_from(round.loss_alarms.len()).unwrap_or(u32::MAX),
        corridors: cameras.iter().map(|c| corridor_tag(doc, c)).collect(),
        x_means: (0..cameras.len())
            .map(|camera| round_mean(metrics, camera, round.group_count, |m| m.x_mm))
            .collect(),
        edge_means: (0..cameras.len())
            .map(|camera| round_mean(metrics, camera, round.group_count, |m| m.edge_mm))
            .collect(),
        heading: metrics
            .iter()
            .filter(|m| m.heading_deg.is_some() || m.heading_fallback)
            .fold(BTreeMap::new(), |mut acc, m| {
                if let Some(segment) = m.corridor {
                    let entry: &mut [u32; 2] = acc.entry(segment).or_default();
                    entry[0] += 1;
                    entry[1] += u32::from(m.heading_fallback);
                }
                acc
            }),
        cameras,
    }
}

/// 把一轮记进候选（同一轮不重复加入）。
pub(super) fn add_calibration_round(w: &mut Workspace, round: CalibrationRound) -> Result<(), String> {
    require_burst(&w.doc)?;
    if w.calibration.rounds.iter().any(|r| r.round_id == round.round_id) {
        return Err(format!("采集轮次 {} 已加入多轮标定", round.round_id));
    }
    w.calibration.rounds.push(round);
    w.changed();
    Ok(())
}

/// 胶偏参考：直线段配了 offset_ref 用它；没配但启用了胶偏检测时，按 AbsoluteX、计划第一台相机（D-SB-18 默认）。
pub(super) fn offset_reference(segment: &PathSegment, cameras: &[String]) -> Option<OffsetRef> {
    if segment.kind != SegmentKind::Straight {
        return None;
    }
    segment.offset_ref.clone().or_else(|| {
        segment
            .checks
            .offset
            .as_ref()
            .filter(|o| o.enabled)
            .and(cameras.first())
            .map(|camera| OffsetRef {
                measure: OffsetMeasure::AbsoluteX,
                camera: camera.clone(),
                x_ref: None,
                spread: None,
                edge: None,
            })
    })
}

/// 标准件轮次里这台相机可用的测量量均值（AbsoluteX 取 x，ToEdge 取胶边距）：走廊与当前相同（走廊变了，不可比）。
fn standard_x(rounds: &[CalibrationRound], reference: &OffsetRef, tag: &Value) -> Vec<f64> {
    rounds
        .iter()
        .filter(|r| r.standard)
        .filter_map(|r| {
            let j = r.cameras.iter().position(|c| *c == reference.camera)?;
            let means = match reference.measure {
                OffsetMeasure::AbsoluteX => &r.x_means,
                OffsetMeasure::ToEdge => &r.edge_means,
            };
            (r.corridors.get(j) == Some(tag))
                .then(|| means.get(j).copied().flatten())
                .flatten()
        })
        .collect()
}

/// 多段计划逐段标定的输入：每个标定轮次（不含标准件）按已确认的手动分段得出各段帧组数，按段排列（外层段、内层轮次）；
/// 有轮次没确认分段时返回这些轮次。
fn segment_counts(
    doc: &RecipeDoc,
    rounds: &[CalibrationRound],
    reviews: &BTreeMap<String, RoundReview>,
) -> Result<Vec<Vec<u32>>, Vec<String>> {
    let mut counts = vec![Vec::new(); doc.segments.len()];
    let mut unconfirmed = Vec::new();
    for round in rounds.iter().filter(|r| !r.standard) {
        match capture_review::confirmed_segment_counts(
            doc,
            reviews,
            &round.round_id,
            round.group_count,
        ) {
            Some(round_counts) => counts
                .iter_mut()
                .zip(round_counts)
                .for_each(|(segment, n)| segment.push(n)),
            None => unconfirmed.push(round.round_id.clone()),
        }
    }
    if unconfirmed.is_empty() {
        Ok(counts)
    } else {
        Err(unconfirmed)
    }
}

fn calib_error(error: CalibError) -> String {
    match error {
        CalibError::TooFewRounds { have, need } => format!("标定轮次不足：已加入 {have} 轮（不含标准件轮次），至少 {need} 轮"),
        CalibError::Empty => "没有可用于标定的轮次".into(),
    }
}

/// 应用多轮标定（D-SB-21）：标定轮次（不含标准件）帧组数做 `calibrate_counts`，轮数不少于配方的 `calibration_rounds`。
/// 结果写整件标定；单段计划同时写这一段的 N_exp 与 calib，并按标准件轮次写直线段的 x_ref（ToEdge 不写）；
/// 多段计划在每个标定轮次都确认了手动分段时逐段标定（各段 N_exp 之和即 ΣN_exp），否则各段保持待标定。
/// 容差不写，建议值在视图里，由操作员确认。失败时配方不变。
pub(super) fn apply_calibration(
    doc: &mut RecipeDoc,
    rounds: &[CalibrationRound],
    reviews: &BTreeMap<String, RoundReview>,
) -> Result<(), String> {
    require_burst(doc)?;
    let cameras = doc.plan_cameras();
    let planned: BTreeSet<&String> = cameras.iter().collect();
    if let Some(round) = rounds.iter().find(|r| r.cameras.len() != cameras.len() || r.cameras.iter().collect::<BTreeSet<_>>() != planned) {
        return Err(format!("标定轮次 {} 的相机 {:?} 与计划相机 {cameras:?} 不同，请移出后重新采集", round.round_id, round.cameras));
    }
    let counts: Vec<u32> = rounds.iter().filter(|r| !r.standard).map(|r| u32::try_from(r.group_count).unwrap_or(u32::MAX)).collect();
    let default: u32 = doc.segments.iter().map(|s| kind_default(doc, s.kind)).sum();
    let outcome = calibration::calibrate_counts(&counts, doc.calibration_rounds, default).map_err(calib_error)?;
    if !(1..=crate::plan::MAX_EXPECTED_TOTAL).contains(&outcome.n_exp) {
        return Err(format!("各轮帧组数均值为 {}，不能作为预期帧组数（需在 1–{} 之间）", outcome.n_exp, crate::plan::MAX_EXPECTED_TOTAL));
    }
    let reference = match doc.segments.as_slice() {
        [segment] => offset_reference(segment, &cameras).map(|mut reference| {
            let values = match (reference.measure, &reference.edge) {
                (OffsetMeasure::ToEdge, None) => Vec::new(),
                _ => standard_x(rounds, &reference, &corridor_tag(doc, &reference.camera)),
            };
            (reference.x_ref, reference.spread) = calibration::x_ref(&values).unzip();
            reference
        }),
        _ => None,
    };
    let per_segment = match doc.segments.len() {
        1 => None,
        _ => segment_counts(doc, rounds, reviews)
            .ok()
            .map(|counts| {
                counts
                    .iter()
                    .zip(&doc.segments)
                    .map(|(counts, segment)| {
                        calibration::calibrate_counts(
                            counts,
                            doc.calibration_rounds,
                            kind_default(doc, segment.kind),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(calib_error)?,
    };
    doc.part_calib = Some(PartCalib { expected_total: outcome.n_exp, calib: outcome.calib.clone() });
    if let [segment] = doc.segments.as_mut_slice() {
        segment.expected_frames = Some(outcome.n_exp);
        segment.calib = Some(outcome.calib);
        if reference.is_some() { segment.offset_ref = reference; }
    } else {
        for (k, segment) in doc.segments.iter_mut().enumerate() {
            let calibrated = per_segment.as_ref().map(|outcomes| &outcomes[k]);
            segment.expected_frames = calibrated.map(|o| o.n_exp);
            segment.calib = calibrated.map(|o| o.calib.clone());
        }
    }
    Ok(())
}

/// 多轮标定的视图：由配方当前的标定结果与已加入的轮次算出（建议容差 = max(该类段默认容差, N − min, max − N)）。
pub(super) fn calibration_view(
    doc: &RecipeDoc,
    rounds: &[CalibrationRound],
    reviews: &BTreeMap<String, RoundReview>,
    production: Option<&RecipeDoc>,
) -> CalibrationView {
    let cameras = doc.plan_cameras();
    let normal = rounds.iter().filter(|r| !r.standard).count() as u32;
    let single = doc.segments.len() == 1;
    let outcome = |n: u32, calib: &crate::plan::SegmentCalib, default: u32| CalibOutcome {
        n_exp: n, calib: calib.clone(), suggested_tolerance: calibration::suggest_tolerance(n, calib.min, calib.max, default),
    };
    let not_applied = || {
        if normal < doc.calibration_rounds {
            format!("还差 {} 轮", doc.calibration_rounds - normal)
        } else {
            "尚未应用标定".into()
        }
    };
    let unconfirmed = if single {
        Vec::new()
    } else {
        segment_counts(doc, rounds, reviews)
            .err()
            .unwrap_or_default()
    };
    let segments = doc
        .segments
        .iter()
        .map(|segment| {
            let calibrated = segment
                .expected_frames
                .zip(segment.calib.as_ref())
                .map(|(n, calib)| outcome(n, calib, kind_default(doc, segment.kind)));
            let pending = calibrated.is_none().then(|| {
                if unconfirmed.is_empty() {
                    not_applied()
                } else {
                    format!("待确认分段：{}", unconfirmed.join("、"))
                }
            });
            let x_ref = segment.offset_ref.as_ref().and_then(|r| {
                Some(XRefView {
                    x_ref: r.x_ref?,
                    spread: r.spread?,
                })
            });
            let x_ref_pending = match (segment.kind, &x_ref, offset_reference(segment, &cameras)) {
                (SegmentKind::Corner, ..) | (_, Some(_), _) => None,
                (_, None, None) => Some("未启用胶偏检测".into()),
                (_, None, Some(_)) if !single => Some("分段算法未就绪".into()),
                (_, None, Some(reference))
                    if reference.measure == OffsetMeasure::ToEdge && reference.edge.is_none() =>
                {
                    Some("测量量未实现：未配置找边参数".into())
                }
                (_, None, Some(reference))
                    if reference
                        .edge
                        .as_ref()
                        .is_some_and(|e| e.nominal_mm.is_some()) =>
                {
                    Some("按图纸胶边距判，不需要标准件".into())
                }
                (_, None, Some(reference)) => Some(if !rounds.iter().any(|r| r.standard) {
                    "还没有标准件轮次".into()
                } else if standard_x(rounds, &reference, &corridor_tag(doc, &reference.camera))
                    .is_empty()
                {
                    "标准件轮次的走廊已变化或没有测成的帧，请重新加入标准件轮次".into()
                } else {
                    not_applied()
                }),
            };
            SegmentCalibView {
                index: segment.index,
                kind: segment.kind,
                outcome: calibrated,
                pending,
                tolerance: segment.tolerance,
                x_ref,
                x_ref_pending,
                hints: heading_hints(doc, segment, rounds),
            }
        })
        .collect();
    let default: u32 = doc.segments.iter().map(|s| kind_default(doc, s.kind)).sum();
    let expected_total = doc.expected_total();
    CalibrationView {
        min_rounds: doc.calibration_rounds,
        rounds: rounds.iter().map(|r| CalibrationRoundView {
            round_id: r.round_id.clone(), standard: r.standard, group_count: r.group_count,
            camera_frames: cameras.iter().map(|c| r.cameras.iter().position(|x| x == c).and_then(|j| r.camera_frames.get(j).copied()).unwrap_or(0)).collect(),
            incomplete_groups: r.incomplete_groups, loss_alarms: r.loss_alarms, created_at: r.created_at,
        }).collect(),
        part: doc.part_calib.as_ref().map(|p| outcome(p.expected_total, &p.calib, default)),
        segments,
        expected_total,
        tolerance_total: crate::plan::tolerance_total(&doc.segments),
        plan_version_change: expected_total.is_some() && production.is_none_or(|p| p.plan_version == 0 || !p.same_plan(doc)),
    }
}

#[tauri::command]
pub async fn workspace_calibration_add_round(app: AppHandle, id: String, revision: u64, round: String, standard: bool) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || add_round(app, id, revision, round, standard)).await.map_err(|e| e.to_string())?
}

/// 把一轮加入标定：用已示教的走廊量整轮每一帧（锁外），写每帧特征与分段结果，再记进候选。
fn add_round(app: AppHandle, id: String, revision: u64, round_id: String, standard: bool) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let w = {
        let items = host.items.lock().unwrap();
        let w = items.get(&id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.clone()
    };
    require_burst(&w.doc)?;
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在处理采集或设备事务，请稍后加入标定")?;
    crate::camera::check_idle(&cycle)?;
    let capture = cycle.camera.recipe_capture();
    let round = capture.get_burst(&round_id)?;
    let current: BTreeMap<String, Value> = cycle.camera.configs().iter().map(|c| (c.id.clone(), state_value(c))).collect();
    check_round(&w.doc, &w.calibration.rounds, &round, &current)?;
    let recipe = w.doc.build()?;
    let cameras = w.doc.plan_cameras();
    let choice = calibration_corridors(&w, &round_id, round.group_count)?;
    let graphs = vision::CorridorSet::build(&recipe, &teaching_sizes(&w)?)?;
    let engine = app
        .state::<vision::VisionHost>()
        .engine()
        .ok_or("lyFlow 核心库未加载")?;
    let run = vision::unique_run_id(&format!("calibration-{id}-{round_id}"));
    let metrics =
        capture_review::measure_calibration_round(&w, &round, |camera, camera_id, seq| {
            let Some(segment) = choice[camera][(seq - 1) as usize] else {
                return Ok(frame_without_metrics(
                    camera,
                    seq,
                    FrameState::NotInspected,
                    None,
                ));
            };
            let graph = graphs
                .get(camera, segment)
                .ok_or_else(|| format!("第 {segment} 段相机 {camera_id} 还没有示教走廊"))?;
            let image =
                crate::image_file::load(&capture.burst_frame_path(&round_id, camera_id, seq)?)?;
            vision::measure_frame(
                &engine,
                graph,
                &image,
                seq,
                &format!("{run}-c{camera}-{seq}"),
            )
            .map(|m| m.metrics)
        });
    let outcome = segment_round(&w.doc.segments, cameras.len(), round.group_count, &metrics);
    let standard = standard || round.origin.as_ref().is_some_and(|o| o.standard());
    let entry = calibration_round(&w.doc, &round, standard, &metrics);
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let mut next = current.clone();
    add_calibration_round(&mut next, entry)?;
    write_features(&host.dir(&id).join(CALIBRATION_DIR).join(&round_id), &cameras, &metrics, &outcome)?;
    host.save(&next)?;
    *current = next;
    view(&app, current.clone())
}

#[tauri::command]
pub fn workspace_calibration_remove_round(app: AppHandle, id: String, revision: u64, round: String) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    remove_calibration_round(&mut w, &round)?;
    host.save(&w)?;
    // 只删本候选标定目录下这一轮的特征（轮次编号来自已加入的记录，只含字母、数字与短横线）
    if round.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { let _ = std::fs::remove_dir_all(host.dir(&id).join(CALIBRATION_DIR).join(&round)); }
    items.insert(id, w.clone());
    view(&app, w)
}

/// 已加入标定的一轮的每帧特征（加入标定时测量写下的），按计划相机、seq；只读，多轮标定页逐帧查看用。
#[tauri::command]
pub fn workspace_calibration_round_features(
    app: AppHandle,
    id: String,
    round: String,
) -> Result<Vec<FrameMetrics>, String> {
    let host = app.state::<WorkspaceHost>();
    let cameras = {
        let items = host.items.lock().unwrap();
        let w = items.get(&id).ok_or("候选配置不存在")?;
        require_burst(&w.doc)?;
        if !w.calibration.rounds.iter().any(|r| r.round_id == round) {
            return Err(format!("采集轮次 {round} 不在多轮标定里"));
        }
        w.doc.plan_cameras()
    };
    read_features(&host.dir(&id).join(CALIBRATION_DIR).join(&round), &cameras)
}

/// 标定轮次一帧按当前规则重测的结果（多轮标定页逐帧查看）：所用段与走廊、测到的胶边（px，无胶站为 None）与帧指标。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationFrameView {
    /// 所用走廊的段；这台相机不检测这一帧所在段时为 None，不测
    pub segment: Option<u32>,
    pub corridor: Vec<[f32; 2]>,
    /// 搜索半宽（px），叠图画搜索带用
    pub search_half_px: Option<f64>,
    pub edges: Vec<Option<[[f32; 2]; 2]>>,
    pub metrics: FrameMetrics,
}

/// 用当前段走廊重测已加入标定的一轮里一台相机的一帧（只读，不改候选与标定）：按本轮已确认的手动分段选走廊，
/// 与加入标定时同一规则（`calibration_corridors`）。有在途件或正在采集时拒绝，免得与生产争用测量。
#[tauri::command]
pub async fn workspace_calibration_frame(
    app: AppHandle,
    id: String,
    round: String,
    camera: String,
    seq: u64,
) -> Result<CalibrationFrameView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let w = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            items.get(&id).ok_or("候选配置不存在")?.clone()
        };
        require_burst(&w.doc)?;
        let entry = w
            .calibration
            .rounds
            .iter()
            .find(|r| r.round_id == round)
            .ok_or_else(|| format!("采集轮次 {round} 不在多轮标定里"))?;
        let cameras = w.doc.plan_cameras();
        let index = cameras
            .iter()
            .position(|c| *c == camera)
            .ok_or_else(|| format!("相机 {camera} 不是计划相机"))?;
        if !(1..=entry.group_count).contains(&seq) {
            return Err(format!(
                "这一轮只有 {} 个帧组，没有第 {seq} 个",
                entry.group_count
            ));
        }
        let segment =
            calibration_corridors(&w, &round, entry.group_count)?[index][(seq - 1) as usize];
        let Some(segment) = segment else {
            return Ok(CalibrationFrameView {
                segment: None,
                corridor: Vec::new(),
                search_half_px: None,
                edges: Vec::new(),
                metrics: frame_without_metrics(index, seq, FrameState::NotInspected, None),
            });
        };
        let recipe = w.doc.build()?;
        let inspect = &recipe.inspect[index];
        let corridor = inspect
            .segment_corridor(segment)
            .map(|s| s.corridor.clone())
            .unwrap_or_default();
        let search_half_px = inspect.mm_per_px.map(|scale| {
            f64::from(inspect.detect.as_ref().unwrap_or(&recipe.detect).search_mm) / scale
        });
        let graphs = vision::CorridorSet::build(&recipe, &teaching_sizes(&w)?)?;
        let graph = graphs
            .get(index, segment)
            .ok_or_else(|| format!("第 {segment} 段相机 {camera} 还没有示教走廊"))?;
        let cycle = app.state::<CycleHost>();
        crate::camera::check_idle(&cycle)?;
        let capture = cycle.camera.recipe_capture();
        let image = crate::image_file::load(&capture.burst_frame_path(&round, &camera, seq)?)?;
        let engine = app
            .state::<vision::VisionHost>()
            .engine()
            .ok_or("lyFlow 核心库未加载")?;
        let measured = vision::measure_frame(
            &engine,
            graph,
            &image,
            seq,
            &vision::unique_run_id(&format!("calibration-view-{id}-{round}-c{index}-{seq}")),
        )?;
        Ok(CalibrationFrameView {
            segment: Some(segment),
            corridor: if measured.line.is_empty() {
                corridor
            } else {
                measured.line.clone()
            },
            search_half_px,
            edges: measured.edges,
            metrics: FrameMetrics {
                camera: index,
                seq,
                ..measured.metrics
            },
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn remove_calibration_round(w: &mut Workspace, round: &str) -> Result<(), String> {
    require_burst(&w.doc)?;
    let before = w.calibration.rounds.len();
    w.calibration.rounds.retain(|r| r.round_id != round);
    if w.calibration.rounds.len() == before {
        return Err(format!("采集轮次 {round} 不在多轮标定里"));
    }
    clear_calibration(&mut w.doc);
    w.changed();
    Ok(())
}

/// 应用多轮标定（结果在 `plan.calibration`）。
#[tauri::command]
pub fn workspace_calibration_apply(app: AppHandle, id: String, revision: u64) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在处理采集或设备事务，请稍后应用标定")?;
    crate::camera::check_idle(&cycle)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    let configs = app.state::<CycleHost>().camera.configs();
    crate::camera::check_burst_cameras(&configs, &w.doc.plan_cameras())?;
    let current = configs.iter().map(|c| (c.id.clone(), state_value(c))).collect();
    check_calibration_devices(&w.calibration.rounds, &current)?;
    let mut doc = w.doc.clone();
    apply_calibration(&mut doc, &w.calibration.rounds, &w.capture_review.rounds)?;
    doc.build()?;
    if doc != w.doc { w.doc = doc; w.changed(); }
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

#[tauri::command]
pub fn workspace_confirm_tolerance(app: AppHandle, id: String, revision: u64, segment: u32, tolerance: u32) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    confirm_tolerance(&mut w, segment, tolerance)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

// 整体替换候选的段列表；拒绝直接改 N_exp、标定统计与 x_ref，段序列或类型变化时清空全部标定。
#[tauri::command]
pub fn workspace_update_segments(app: AppHandle, id: String, revision: u64, segments: Vec<PathSegment>) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选配置不存在")?.clone();
    w.expect(revision)?;
    update_segments(&mut w, segments)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

// ───────────── WorkspaceView.plan ─────────────

pub(super) fn camera_teaching_views(w: &Workspace) -> Vec<CameraTeachingView> {
    let open = untaught_pairs(w);
    w.doc
        .inspect
        .iter()
        .map(|inspect| {
            let mut segments: Vec<SegmentTeachingView> = w
                .camera_teaching
                .iter()
                .filter_map(|t| {
                    t.segment
                        .filter(|_| t.camera == inspect.camera)
                        .map(|segment| SegmentTeachingView {
                            segment,
                            frame: t.frame.clone(),
                            round: t.round.clone(),
                            seq: t.seq,
                            saved: segment_corridor_saved(inspect, segment, &w.camera_teaching),
                        })
                })
                .collect();
            segments.sort_by_key(|s| s.segment);
            CameraTeachingView {
                camera: inspect.camera.clone(),
                mm_per_px: inspect.mm_per_px,
                ready: inspect.mm_per_px.is_some()
                    && !open.iter().any(|(camera, _)| camera == &inspect.camera),
                segments,
            }
        })
        .collect()
}

/// 还没示教好走廊的（相机, 段）：按段顺序、段内按计划相机顺序；没选示教帧、走廊没保存或超出示教帧都算。
pub(super) fn untaught_pairs(w: &Workspace) -> Vec<(String, u32)> {
    let cameras = w.doc.plan_cameras();
    w.doc
        .segments
        .iter()
        .flat_map(|segment| {
            crate::plan::segment_cameras(segment, &cameras)
                .into_iter()
                .map(move |camera| (camera, segment.index))
        })
        .filter(|(camera, segment)| {
            !w.doc.inspect.iter().any(|i| {
                &i.camera == camera && segment_corridor_saved(i, *segment, &w.camera_teaching)
            })
        })
        .collect()
}

/// 段配方的进度：采集 → 段列表 → 走廊示教 → 多轮标定 → 验证（可选，D-SB-46）→ 发布。只读候选与已有完整采集轮数，不访问设备。
pub(super) fn progress_v2(w: &Workspace, teaching: &[CameraTeachingView], complete_rounds: usize, production_version: Option<u32>) -> ProgressV2 {
    let doc = &w.doc;
    let captured = complete_rounds > 0;
    let need_capture = || vec![BlockerV2 { message: "先完成一轮整圈采集".into(), target: TargetV2::Capture }];
    let capture = StepV2 { id: StepIdV2::Capture, label: "整圈采集", optional: false,
        status: if captured { StepStatus::Done } else { StepStatus::NotStarted },
        detail: if captured { format!("已有 {complete_rounds} 轮完整采集") } else { "三台相机同时采集整圈，按帧号配组".into() },
        blockers: Vec::new() };
    let straight = doc.segments.iter().filter(|s| s.kind == SegmentKind::Straight).count();
    let segments = StepV2 { id: StepIdV2::Segments, label: "段列表", optional: false, status: StepStatus::Done,
        detail: format!("{} 段（直线 {straight}、弯道 {}），帧数容差合计 ±{}", doc.segments.len(), doc.segments.len() - straight, crate::plan::tolerance_total(&doc.segments)),
        blockers: Vec::new() };
    let corridor = {
        let open = untaught_pairs(w);
        let total = doc
            .segments
            .iter()
            .map(|s| crate::plan::segment_cameras(s, &doc.plan_cameras()).len())
            .sum::<usize>();
        let blockers: Vec<_> = open
            .iter()
            .map(|(camera, segment)| BlockerV2 {
                message: format!("第 {segment} 段相机 {camera}：待示教走廊"),
                target: TargetV2::Corridor {
                    camera: camera.clone(),
                    segment: Some(*segment),
                },
            })
            .chain(
                teaching
                    .iter()
                    .filter(|t| t.mm_per_px.is_none())
                    .map(|t| BlockerV2 {
                        message: format!("相机 {}：缺像素当量", t.camera),
                        target: TargetV2::Corridor {
                            camera: t.camera.clone(),
                            segment: None,
                        },
                    }),
            )
            .chain(
                crate::heading::issues(&doc.segments, &doc.inspect)
                    .into_iter()
                    .map(|i| BlockerV2 {
                        message: i.message,
                        target: TargetV2::Corridor {
                            camera: i.camera,
                            segment: i.segment,
                        },
                    }),
            )
            .collect();
        let status = if !captured {
            StepStatus::NotStarted
        } else if blockers.is_empty() {
            StepStatus::Done
        } else if w.camera_teaching.is_empty() {
            StepStatus::NotStarted
        } else {
            StepStatus::InProgress
        };
        StepV2 {
            id: StepIdV2::Corridor,
            label: "走廊示教",
            optional: false,
            status,
            detail: format!("{} / {total} 条段走廊已示教", total - open.len()),
            blockers: if captured { blockers } else { need_capture() },
        }
    };
    let calibration = {
        let normal = w.calibration.rounds.iter().filter(|r| !r.standard).count() as u32;
        let min = doc.calibration_rounds;
        let total = doc.expected_total();
        let status = if total.is_some() { StepStatus::Done } else if normal > 0 { StepStatus::InProgress } else { StepStatus::NotStarted };
        let blockers = match total {
            Some(_) => Vec::new(),
            None if !captured => need_capture(),
            None => vec![BlockerV2 { message: if normal < min { format!("还差 {} 轮：至少 {min} 轮（不含标准件轮次）", min - normal) } else { format!("已有 {normal} 轮，请应用标定") },
                target: TargetV2::Calibration }],
        };
        let detail = match total {
            Some(total) => format!(
                "整件预期 {total} ± {} 个帧组{}",
                crate::plan::tolerance_total(&doc.segments),
                if doc.segments.iter().any(|s| s.expected_frames.is_none()) {
                    "；各段待逐段标定"
                } else {
                    ""
                }
            ),
            None => format!("{normal} / {min} 轮"),
        };
        StepV2 { id: StepIdV2::Calibration, label: "多轮标定", optional: false, status, detail, blockers }
    };
    let active = match &w.publish { PublishState::Active { version, revision, .. } if *revision == w.revision => Some(*version), _ => None };
    // D-SB-46：独立样本验证可选，不是发布前提；发布只要采集、走廊示教、多轮标定完成（设备与生产版本在发布时当场核对）
    let validation = {
        let current = w.validation.as_ref().filter(|v| v.revision == w.revision);
        let (status, detail) = match current {
            Some(v) if v.passed => (
                StepStatus::Done,
                format!("当前修订验证通过（{} 件样本）", v.samples.len()),
            ),
            Some(_) => (
                StepStatus::NotStarted,
                "可选：上次验证未通过，不影响发布".to_string(),
            ),
            None => (
                StepStatus::NotStarted,
                "可选：用独立整圈样本试验证，不影响发布".to_string(),
            ),
        };
        StepV2 {
            id: StepIdV2::Validation,
            label: "验证（可选）",
            optional: true,
            status,
            detail,
            blockers: Vec::new(),
        }
    };
    let publish = {
        let production =
            production_version.map_or("未发布".to_string(), |v| format!("生产 v{v}"));
        let pending = w
            .pending
            .as_ref()
            .map(|p| p.doc.version)
            .unwrap_or(w.doc.version);
        let open: Vec<BlockerV2> = [&capture, &corridor, &calibration]
            .into_iter()
            .filter(|s| s.status != StepStatus::Done)
            .map(|s| BlockerV2 {
                message: format!("先完成{}", s.label),
                target: s
                    .blockers
                    .first()
                    .map_or(TargetV2::Capture, |b| b.target.clone()),
            })
            .collect();
        let (status, detail, blockers) = match (&w.publish, active) {
            (_, Some(version)) => (StepStatus::Done, format!("v{version} 已生效"), Vec::new()),
            (PublishState::Waiting, _) => (
                StepStatus::InProgress,
                format!("v{pending} 待生效：等待当前工件结束"),
                Vec::new(),
            ),
            (PublishState::Failed { message }, _) => (
                StepStatus::Blocked,
                format!("v{pending} 生效失败，可重试或放弃该待生效版本"),
                vec![BlockerV2 {
                    message: message.clone(),
                    target: TargetV2::Validation,
                }],
            ),
            (PublishState::Uncertain { message }, _) => (
                StepStatus::Blocked,
                format!("v{pending} 生效状态不确定"),
                vec![BlockerV2 {
                    message: message.clone(),
                    target: TargetV2::Validation,
                }],
            ),
            _ if open.is_empty() => (
                StepStatus::NotStarted,
                format!("可发布 v{}（当前{production}）", w.doc.version),
                Vec::new(),
            ),
            _ => (
                StepStatus::NotStarted,
                format!("示教、标定完成后发布（当前{production}）"),
                open,
            ),
        };
        StepV2 { id: StepIdV2::Publish, label: "发布与生效", optional: false, status, detail, blockers }
    };
    let next = if capture.status != StepStatus::Done {
        Some(NextV2 { step: StepIdV2::Capture, label: "整圈采集".into(), target: TargetV2::Capture })
    } else if let Some(open) = [&corridor, &calibration].into_iter().find(|s| s.status != StepStatus::Done) {
        let target = open.blockers.first().map_or(TargetV2::Calibration, |b| b.target.clone());
        let label = match &target {
            TargetV2::Corridor {
                camera,
                segment: Some(segment),
            } => format!("示教第 {segment} 段相机 {camera} 的走廊"),
            TargetV2::Corridor {
                camera,
                segment: None,
            } => format!("设置相机 {camera} 的像素当量"),
            _ => open.label.to_string(),
        };
        Some(NextV2 {
            step: open.id,
            label,
            target,
        })
    } else {
        match publish.status {
            StepStatus::Done => None,
            StepStatus::InProgress => Some(NextV2 { step: StepIdV2::Publish, label: "等待工件边界生效".into(), target: TargetV2::Validation }),
            StepStatus::Blocked => Some(NextV2 { step: StepIdV2::Publish, label: "处理发布失败".into(), target: TargetV2::Validation }),
            _ => Some(NextV2 { step: StepIdV2::Publish, label: "发布生产配方".into(), target: TargetV2::Validation }),
        }
    };
    ProgressV2 { steps: vec![capture, segments, corridor, calibration, validation, publish], next }
}

/// 段配方的计划部分（决策记录 5.1 / 5.2 的 WorkspacePlanView）。`production` 为生产配方（可能是旧拍照点配方或还没发布）。
pub(super) fn plan_view(app: &AppHandle, w: &Workspace, layout: &Recipe, production: Option<&Recipe>, production_doc: Option<&RecipeDoc>, production_version: Option<u32>) -> WorkspacePlanView {
    let doc = &w.doc;
    let teaching = camera_teaching_views(w);
    let used: BTreeSet<&str> = w.camera_teaching.iter().map(|t| t.round.as_str()).chain(w.calibration.rounds.iter().map(|r| r.round_id.as_str())).collect();
    let complete_rounds = app.state::<CycleHost>().camera.recipe_capture().list(&doc.id)
        .map(|rounds| rounds.iter().filter(|r| r.state == CaptureState::Complete).count())
        .unwrap_or(used.len());
    let production_export = production.filter(|r| r.is_burst()).and_then(|r| PlanExport::from_recipe(r).ok());
    let (candidate_export, candidate_export_error) = match PlanExport::from_recipe(layout) {
        Ok(export) => (Some(export), None),
        Err(error) => (None, Some(error)),
    };
    let expected_total = doc.expected_total();
    WorkspacePlanView {
        segments: doc.segments.clone(),
        cameras: doc.plan_cameras(),
        segment_defaults: doc.segment_defaults.clone(),
        calibration_rounds: doc.calibration_rounds,
        part_calib: doc.part_calib.clone(),
        expected_total,
        tolerance_total: crate::plan::tolerance_total(&doc.segments),
        calibrated: expected_total.is_some(),
        calibration: Some(calibration_view(
            doc,
            &w.calibration.rounds,
            &w.capture_review.rounds,
            production_doc,
        )),
        shot_count_changed: production_export.as_ref().and_then(|e| e.expected_total)
            != expected_total,
        production_export,
        candidate_export,
        candidate_export_error,
        progress: progress_v2(w, &teaching, complete_rounds, production_version),
        camera_teaching: teaching,
    }
}

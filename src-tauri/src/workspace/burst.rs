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

/// 段配方一台计划相机的示教帧：从完整的示教采集轮次冻结的一帧（原图存为候选 images/ 下的 PGM），走廊在这一帧上示教。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraTeaching {
    pub camera: String,
    pub frame: FrozenImage,
    /// 来源采集轮次与帧组序号
    pub round: String,
    pub seq: u64,
    /// 段走廊的示教帧（D-SB-42）：`PathSegment.index`；为空是这台相机默认走廊的示教帧
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<u32>,
}

/// 这台相机默认走廊的示教帧（段走廊的示教帧 `segment` 非空，另记）。
pub(super) fn default_teaching<'a>(
    teaching: &'a [CameraTeaching],
    camera: &str,
) -> Option<&'a CameraTeaching> {
    teaching
        .iter()
        .find(|t| t.camera == camera && t.segment.is_none())
}

/// workspace_teach_camera 的草稿：与 `CameraInspect` 的可编辑部分一一对应（TS `CameraTeachDraft`）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraTeachDraft {
    pub corridor: Vec<[f32; 2]>,
    pub mm_per_px: Option<f64>,
    pub detect: Option<DetectParams>,
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
    /// 走廊已保存：至少两点、不短于 1 px，且完整落在示教帧内
    pub saved: bool,
    pub frame: Option<FrozenImage>,
    pub mm_per_px: Option<f64>,
    /// 已选示教帧的段走廊（D-SB-42），按段号
    pub segments: Vec<SegmentTeachingView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentTeachingView {
    pub segment: u32,
    pub frame: FrozenImage,
    pub round: String,
    pub seq: u64,
    /// 段走廊已保存且完整落在示教帧内；否则这一段测量用默认走廊
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
    Corridor { camera: String },
    Calibration,
    Validation,
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

/// 一台相机的走廊身份：测每帧特征用到的走廊、当量、检测参数与站距；不是计划相机为 null。
pub(super) fn corridor_tag(doc: &RecipeDoc, camera: &str) -> Value {
    doc.inspect
        .iter()
        .find(|i| i.camera == camera)
        .map_or(Value::Null, |i| {
            let mut tag = json!([
                i.corridor,
                i.mm_per_px,
                i.detect.as_ref().unwrap_or(&doc.detect),
                doc.spacing
            ]);
            if !i.segments.is_empty() {
                if let Some(items) = tag.as_array_mut() {
                    items.push(json!(i.segments));
                }
            }
            tag
        })
}

/// 走廊已示教且完整落在示教帧内（与 `vision::plan_corridor` 的前置相同）。
pub(super) fn corridor_saved(inspect: &CameraInspect, size: [u32; 2]) -> bool {
    path_saved(&inspect.corridor, size)
}

fn path_saved(corridor: &[[f32; 2]], size: [u32; 2]) -> bool {
    let length: f32 = corridor
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .sum();
    corridor.len() >= 2 && length >= 1.0 && vision::corridor_inside(corridor, size)
}

/// 段走廊可用于测量：已选段示教帧，走廊已保存且落在那一帧内。
fn segment_corridor_saved(
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

/// 段走廊随段结构失效（D-SB-42）：段的类型序列变了，各相机的段走廊清空，返回 true 让调用方一并去掉段示教帧；
/// 结构不变时每条段走廊都要有对应的段示教帧。
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
    for inspect in &new.inspect {
        if let Some(s) = inspect.segments.iter().find(|s| {
            !teaching
                .iter()
                .any(|t| t.camera == inspect.camera && t.segment == Some(s.segment))
        }) {
            return Err(format!(
                "相机 {} 第 {} 段还没有选段走廊的示教帧",
                inspect.camera, s.segment
            ));
        }
    }
    Ok(false)
}

/// 标定按段选走廊（D-SB-42）：本轮已确认手动分段时，落在某段、且这台相机在该段有可用段走廊的帧用段走廊，其余帧用默认走廊。
/// 按计划相机、seq（1..=帧组数）给出所用段走廊的段号，None 为默认走廊。
pub(super) fn corridor_segments(
    w: &Workspace,
    round: &str,
    group_count: u64,
) -> Vec<Vec<Option<u32>>> {
    let counts = capture_review::confirmed_segment_counts(
        &w.doc,
        &w.capture_review.rounds,
        round,
        group_count,
    );
    let owner: Vec<Option<u32>> = match &counts {
        Some(counts) => w
            .doc
            .segments
            .iter()
            .zip(counts)
            .flat_map(|(s, &n)| std::iter::repeat_n(Some(s.index), n as usize))
            .collect(),
        None => vec![None; group_count as usize],
    };
    w.doc
        .plan_cameras()
        .iter()
        .map(|camera| {
            let inspect = w.doc.inspect.iter().find(|i| &i.camera == camera);
            owner
                .iter()
                .map(|segment| {
                    segment.filter(|&s| {
                        inspect.is_some_and(|i| segment_corridor_saved(i, s, &w.camera_teaching))
                    })
                })
                .collect()
        })
        .collect()
}

/// 没有测量值的一帧（缺帧、测量失败）。
pub(super) fn frame_without_metrics(camera: usize, seq: u64, state: FrameState, error: Option<String>) -> FrameMetrics {
    FrameMetrics { camera, seq, state, error, width_min: None, width_max: None, width_mean: None, widths: Vec::new(),
        spacing_mm: 0.0, gap_max_mm: 0.0, present_ratio: 0.0, x_mm: None }
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
        let Some(reference) = after.offset_ref.as_mut() else { continue };
        let previous = before.offset_ref.as_ref().filter(|old| old.measure == reference.measure && old.camera == reference.camera);
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

/// 示教帧选定后写进候选：草稿写进这台相机的 inspect（当量缺省时沿用已有值或工位标定），记下示教帧。
pub(super) fn apply_camera_teaching(w: &mut Workspace, teaching: CameraTeaching, draft: CameraTeachDraft, default_scale: Option<f64>) -> Result<(), String> {
    require_burst(&w.doc)?;
    capture_review::require_available(w, &teaching.round, teaching.seq)?;
    if draft.corridor.len() > 10_000 || draft.corridor.iter().flatten().any(|v| !v.is_finite()) {
        return Err("走廊最多 10000 点，坐标必须为有限数".into());
    }
    let mut doc = w.doc.clone();
    let inspect = doc.inspect.iter_mut().find(|i| i.camera == teaching.camera)
        .ok_or_else(|| format!("相机 {} 不是段配方的计划相机", teaching.camera))?;
    inspect.corridor = draft.corridor;
    inspect.mm_per_px = draft.mm_per_px.or(inspect.mm_per_px).or(default_scale);
    inspect.detect = draft.detect;
    doc.build()?;
    w.doc = doc;
    w.camera_teaching
        .retain(|t| t.camera != teaching.camera || t.segment.is_some());
    w.camera_teaching.push(teaching);
    w.changed();
    Ok(())
}

/// 段走廊的示教帧（D-SB-42）：这台相机是该段的检测相机，已有默认走廊的示教帧（像素当量与检测参数按相机）且尺寸相同；
/// 示教轮次已确认手动分段，帧组落在这一段内。同一相机同一段只留一帧，已画的段走廊原样保留。
pub(super) fn apply_segment_teaching(
    w: &mut Workspace,
    teaching: CameraTeaching,
    group_count: u64,
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
    if !crate::plan::segment_cameras(&w.doc.segments[position], &w.doc.plan_cameras())
        .contains(&camera)
    {
        return Err(format!("相机 {camera} 不是第 {segment} 段的检测相机"));
    }
    let default = default_teaching(&w.camera_teaching, &camera)
        .ok_or_else(|| format!("相机 {camera} 还没有默认走廊的示教帧：先示教默认走廊"))?;
    if default.frame.size != teaching.frame.size {
        return Err(format!("这一帧尺寸与相机 {camera} 默认走廊的示教帧不同"));
    }
    capture_review::require_available(w, &teaching.round, teaching.seq)?;
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
    w.camera_teaching
        .retain(|t| !(t.camera == camera && t.segment == Some(segment)));
    w.camera_teaching.push(teaching);
    w.changed();
    Ok(())
}

/// 去掉一台相机在某一段的走廊与示教帧，这一段退回默认走廊。
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
        return Err(format!("相机 {camera} 第 {segment} 段没有单独示教的走廊"));
    }
    w.changed();
    Ok(())
}

/// 用采集轮次 `round` 里相机 `camera` 的第 `seq` 帧示教走廊：冻结这一帧，把草稿写进候选配方的 inspect。
#[tauri::command]
pub async fn workspace_teach_camera(app: AppHandle, id: String, revision: u64, camera: String, round: String, seq: u64, draft: CameraTeachDraft) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || teach_camera(app, id, revision, camera, round, seq, draft)).await.map_err(|e| e.to_string())?
}

fn teach_camera(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    round: String,
    seq: u64,
    draft: CameraTeachDraft,
) -> Result<WorkspaceView, String> {
    let (frame, default_scale, _) =
        freeze_teaching_frame(&app, &id, revision, &camera, &round, seq)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let mut next = current.clone();
    apply_camera_teaching(
        &mut next,
        CameraTeaching {
            camera,
            frame,
            round,
            seq,
            segment: None,
        },
        draft,
        default_scale,
    )?;
    host.save(&next)?;
    *current = next;
    view(&app, current.clone())
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
        let (frame, _, group_count) =
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
        features: metrics,
        segments,
    })
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

/// 一台相机去掉首尾各一帧（seq 1 与帧组数）后、测成的帧的胶中心 x 均值。
fn x_mean(metrics: &[FrameMetrics], camera: usize, group_count: u64) -> Option<f64> {
    let xs: Vec<f64> = metrics.iter()
        .filter(|m| m.camera == camera && m.seq > 1 && m.seq < group_count && m.state == FrameState::Measured)
        .filter_map(|m| m.x_mm).collect();
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
        x_means: (0..cameras.len()).map(|camera| x_mean(metrics, camera, round.group_count)).collect(),
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
    if segment.kind != SegmentKind::Straight { return None; }
    segment.offset_ref.clone().or_else(|| segment.checks.offset.as_ref().filter(|o| o.enabled).and(cameras.first())
        .map(|camera| OffsetRef { measure: OffsetMeasure::AbsoluteX, camera: camera.clone(), x_ref: None, spread: None }))
}

/// 标准件轮次里这台相机可用的 x 均值：走廊与当前相同（走廊变了，x 不可比）。
fn standard_x(rounds: &[CalibrationRound], camera: &str, tag: &Value) -> Vec<f64> {
    rounds.iter().filter(|r| r.standard).filter_map(|r| {
        let j = r.cameras.iter().position(|c| c == camera)?;
        (r.corridors.get(j) == Some(tag)).then(|| r.x_means.get(j).copied().flatten()).flatten()
    }).collect()
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
            let values = match reference.measure {
                OffsetMeasure::AbsoluteX => standard_x(rounds, &reference.camera, &corridor_tag(doc, &reference.camera)),
                OffsetMeasure::ToEdge => Vec::new(),
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
                (_, None, Some(reference)) if reference.measure == OffsetMeasure::ToEdge => {
                    Some("测量量未实现".into())
                }
                (_, None, Some(reference)) => Some(if !rounds.iter().any(|r| r.standard) {
                    "还没有标准件轮次".into()
                } else if standard_x(
                    rounds,
                    &reference.camera,
                    &corridor_tag(doc, &reference.camera),
                )
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
    let choice = corridor_segments(&w, &round_id, round.group_count);
    let graphs = recipe
        .inspect
        .iter()
        .enumerate()
        .map(|(camera, inspect)| {
            let teaching =
                default_teaching(&w.camera_teaching, &inspect.camera).ok_or_else(|| {
                    format!(
                        "相机 {} 还没有选示教帧：先完成走廊示教再加入标定",
                        inspect.camera
                    )
                })?;
            let default = vision::plan_corridor(inspect, &recipe, teaching.frame.size)?;
            let used: BTreeSet<u32> = choice
                .get(camera)
                .into_iter()
                .flatten()
                .flatten()
                .copied()
                .collect();
            let segments = used
                .into_iter()
                .map(|segment| {
                    let mut one = inspect.clone();
                    one.corridor = inspect
                        .segment_corridor(segment)
                        .map(|s| s.corridor.clone())
                        .unwrap_or_default();
                    vision::plan_corridor(&one, &recipe, teaching.frame.size)
                        .map(|graph| (segment, graph))
                        .map_err(|e| format!("第 {segment} 段走廊：{e}"))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            Ok((default, segments))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let engine = app
        .state::<vision::VisionHost>()
        .engine()
        .ok_or("lyFlow 核心库未加载")?;
    let run = vision::unique_run_id(&format!("calibration-{id}-{round_id}"));
    let metrics =
        capture_review::measure_calibration_round(&w, &round, |camera, camera_id, seq| {
            let image =
                crate::image_file::load(&capture.burst_frame_path(&round_id, camera_id, seq)?)?;
            let (default, segments) = &graphs[camera];
            let segment = choice[camera][(seq - 1) as usize];
            vision::measure_frame(
                &engine,
                segment.and_then(|s| segments.get(&s)).unwrap_or(default),
                &image,
                seq,
                &format!("{run}-c{camera}-{seq}"),
            )
            .map(|m| m.metrics)
        });
    let outcome = segment_round(&w.doc.segments, cameras.len(), round.group_count, &metrics);
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
    w.doc
        .inspect
        .iter()
        .map(|inspect| {
            let frame =
                default_teaching(&w.camera_teaching, &inspect.camera).map(|t| t.frame.clone());
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
                saved: frame
                    .as_ref()
                    .is_some_and(|f| corridor_saved(inspect, f.size)),
                frame,
                mm_per_px: inspect.mm_per_px,
                segments,
            }
        })
        .collect()
}

/// 没通过的验证项跳到哪一页（检查项名称见 `burst_release::burst_checks`）：走廊示教跳到第一台没示教好的相机。
fn validation_target(name: &str, teaching: &[CameraTeachingView]) -> TargetV2 {
    match name {
        "设备与采集" => TargetV2::Camera,
        "走廊示教" => teaching.iter().find(|t| !(t.saved && t.mm_per_px.is_some())).or(teaching.first())
            .map_or(TargetV2::Validation, |t| TargetV2::Corridor { camera: t.camera.clone() }),
        "段标定" | "标准件 x_ref" => TargetV2::Calibration,
        _ => TargetV2::Validation,
    }
}

/// 段配方的进度：采集 → 段列表 → 走廊示教 → 多轮标定 → 验证 → 发布。只读候选与已有完整采集轮数，不访问设备。
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
        let open: Vec<_> = teaching.iter().filter(|t| !(t.saved && t.mm_per_px.is_some())).collect();
        let blockers: Vec<_> = open.iter().map(|t| BlockerV2 {
            message: format!("相机 {}：{}", t.camera, if t.frame.is_none() { "待选示教帧" } else if !t.saved { "待示教走廊" } else { "缺像素当量" }),
            target: TargetV2::Corridor { camera: t.camera.clone() },
        }).collect();
        let status = if !captured { StepStatus::NotStarted } else if open.is_empty() { StepStatus::Done }
            else if teaching.iter().all(|t| t.frame.is_none()) { StepStatus::NotStarted } else { StepStatus::InProgress };
        StepV2 { id: StepIdV2::Corridor, label: "走廊示教", optional: false, status,
            detail: format!("{} / {} 台相机已示教走廊", teaching.len() - open.len(), teaching.len()),
            blockers: if captured { blockers } else { need_capture() } }
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
    let validation = {
        let current = w.validation.as_ref().filter(|v| v.revision == w.revision);
        let (status, detail, blockers) = if let Some(version) = active {
            (StepStatus::Done, format!("已随 v{version} 验证并发布"), Vec::new())
        } else if let Some(v) = current {
            if v.passed { (StepStatus::Done, format!("当前修订验证通过（{} 件样本）", v.samples.len()), Vec::new()) }
            else { (StepStatus::Blocked, "验证未通过".into(), v.checks.iter().filter(|c| !c.passed)
                .map(|c| BlockerV2 { message: format!("{}：{}", c.name, c.detail), target: validation_target(&c.name, teaching) }).collect()) }
        } else {
            let blockers = [&capture, &corridor, &calibration].into_iter().filter(|s| s.status != StepStatus::Done)
                .map(|s| BlockerV2 { message: format!("先完成{}", s.label), target: s.blockers.first().map_or(TargetV2::Capture, |b| b.target.clone()) }).collect();
            (StepStatus::NotStarted, if w.validation.is_some() { "候选已修改，需要重新验证" } else { "待独立正常整圈样本验证" }.into(), blockers)
        };
        StepV2 { id: StepIdV2::Validation, label: "验证", optional: false, status, detail, blockers }
    };
    let publish = {
        let production = production_version.map_or("未发布".to_string(), |v| format!("生产 v{v}"));
        let pending = w.pending.as_ref().map(|p| p.doc.version).unwrap_or(w.doc.version);
        let (status, detail, blockers) = match (&w.publish, active) {
            (_, Some(version)) => (StepStatus::Done, format!("v{version} 已生效"), Vec::new()),
            (PublishState::Waiting, _) => (StepStatus::InProgress, format!("v{pending} 待生效：等待当前工件结束"), Vec::new()),
            (PublishState::Failed { message }, _) => (StepStatus::Blocked, format!("v{pending} 生效失败，可重试或放弃该待生效版本"), vec![BlockerV2 { message: message.clone(), target: TargetV2::Validation }]),
            (PublishState::Uncertain { message }, _) => (StepStatus::Blocked, format!("v{pending} 生效状态不确定"), vec![BlockerV2 { message: message.clone(), target: TargetV2::Validation }]),
            _ if validation.status == StepStatus::Done => (StepStatus::NotStarted, format!("可发布 v{}（当前{production}）", w.doc.version), Vec::new()),
            _ => (StepStatus::NotStarted, format!("验证通过后发布（当前{production}）"), vec![BlockerV2 { message: "先通过当前修订的验证".into(), target: TargetV2::Validation }]),
        };
        StepV2 { id: StepIdV2::Publish, label: "发布与生效", optional: false, status, detail, blockers }
    };
    let next = if capture.status != StepStatus::Done {
        Some(NextV2 { step: StepIdV2::Capture, label: "整圈采集".into(), target: TargetV2::Capture })
    } else if let Some(open) = [&corridor, &calibration].into_iter().find(|s| s.status != StepStatus::Done) {
        let target = open.blockers.first().map_or(TargetV2::Calibration, |b| b.target.clone());
        let label = match &target { TargetV2::Corridor { camera } => format!("示教相机 {camera} 的走廊"), _ => open.label.to_string() };
        Some(NextV2 { step: open.id, label, target })
    } else if validation.status != StepStatus::Done {
        Some(NextV2 { step: StepIdV2::Validation, label: "运行整套验证".into(), target: validation.blockers.first().map_or(TargetV2::Validation, |b| b.target.clone()) })
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

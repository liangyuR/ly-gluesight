//! 段配方的候选工作区（D2）：每台计划相机的走廊示教、示教件基准（各段 N_exp 取示教件已确认分段的帧组数，示教件兼作标准件
//! 得出胶偏参考 x_ref）、段列表编辑，以及 `WorkspaceView.plan`。
//!
//! 只服务段配方（`RecipeDoc.segments` 非空）。旧拍照点配方的单帧示教、采用整圈采集、组选图仍走 workflow.rs，行为不变。
//! 规则见 docs/decisions/recipe-build-flow.md（RB-3）与 docs/decisions/segment-burst.md：
//! - N_exp、标定统计、整件标定只由示教件已确认的分段写入（分段保存时）；x_ref 只由测量示教件写入；段的序列或类型变化时一并清空；
//! - x_ref 只给单段配方的直线段：示教件去掉首尾各一帧（再去掉起涂、收尾帧）取胶中心 x（或胶边距）均值。

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use super::*;
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::calibration;
use crate::frame_group::FrameGroup;
use crate::plan::{
    OffsetMeasure, OffsetRef, PartCalib, PathSegment, PlanExport, SegmentCalib, SegmentKind,
};
use crate::recipe::CameraInspect;
use crate::recipe_capture::{CaptureCamera, CaptureRoundV2, CaptureState};
use crate::segmentation::{Proportional, SegInput, SegOutcome, Segmenter};
use progress::StepStatus;

/// 每轮标定的每帧特征与分段结果放在候选目录 `calibration/<轮次>/` 下（供分段算法与重新标定使用）。
pub(super) const CALIBRATION_DIR: &str = "calibration";
/// 一行一帧：`{camera, seq, metrics}`，按计划相机、帧组序号排列；1..=帧组数内每台相机每个 seq 都有一行（缺帧也有）。
pub(super) const FEATURES_FILE: &str = "features.jsonl";
/// 分段结果（`SegOutcome`）。
pub(super) const BOUNDARIES_FILE: &str = "boundaries.json";

/// 段配方一段、一台检测相机的示教帧（D-SB-45）：从完整的采集轮次冻结的一帧（原图存为候选 images/ 下的 PGM），
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

/// 各计划相机的示教帧尺寸（计划相机顺序），建走廊测量图用；检测某段的相机还没有示教帧时报错。
/// 不检测任何一段的相机没有走廊、不建图，尺寸记 [0, 0]。
pub(super) fn teaching_sizes(w: &Workspace) -> Result<Vec<[u32; 2]>, String> {
    let cameras = w.doc.plan_cameras();
    cameras
        .iter()
        .map(|camera| {
            let inspects = w
                .doc
                .segments
                .iter()
                .any(|s| crate::plan::segment_cameras(s, &cameras).contains(camera));
            match camera_frame_size(&w.camera_teaching, camera) {
                Some(size) => Ok(size),
                None if !inspects => Ok([0, 0]),
                None => Err(format!("相机 {camera} 还没有示教帧：先完成走廊示教")),
            }
        })
        .collect()
}

/// 各计划相机示教帧的通道数（计划相机顺序）：示教帧冻结时的相机标签是彩色模式为 3，否则为 1。
pub(super) fn teaching_channels(w: &Workspace) -> Vec<u8> {
    w.doc
        .plan_cameras()
        .iter()
        .map(|camera| {
            let color = w
                .camera_teaching
                .iter()
                .find(|t| &t.camera == camera && t.segment.is_some())
                .is_some_and(|t| {
                    t.frame.camera_tag.get("imageMode").and_then(Value::as_str) == Some("color")
                });
            if color {
                3
            } else {
                1
            }
        })
        .collect()
}

/// 没选测量方式也没有颜色模型的走廊按灰度测。彩色示教帧上新画的走廊、按胶色测量的段改了走廊点（旧模型随之去掉）时，
/// 记为按胶色测量、待取色，不悄悄回到灰度（color CL-3）。
pub(super) fn keep_color_measure(w: &Workspace, doc: &mut RecipeDoc) {
    for inspect in &mut doc.inspect {
        let before = w.doc.inspect.iter().find(|i| i.camera == inspect.camera);
        for s in &mut inspect.segments {
            if s.measure.is_some() || s.color_model.is_some() || s.corridor.is_empty() {
                continue;
            }
            let previous = before.and_then(|b| b.segment_corridor(s.segment));
            let color = match previous.filter(|p| !p.corridor.is_empty()) {
                Some(p) => p.measure_mode() == crate::recipe::MeasureMode::Color,
                None => w.camera_teaching.iter().any(|t| {
                    t.camera == inspect.camera
                        && t.segment == Some(s.segment)
                        && t.frame.camera_tag.get("imageMode").and_then(Value::as_str)
                            == Some("color")
                }),
            };
            if color {
                s.measure = Some(crate::recipe::MeasureMode::Color);
            }
        }
    }
}

/// 示教件的一次测量（RB-3）：用已示教的走廊按示教件分段量每一帧，得出胶偏参考 x_ref（示教件兼作标准件）、起涂收尾建议与定向统计。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationRound {
    pub round_id: String,
    /// 旧候选里标准件的标记，只为读入兼容
    #[serde(default, skip_serializing)]
    pub standard: bool,
    pub created_at: i64,
    /// 测量时的计划相机顺序；下面几项按它排列
    pub cameras: Vec<String>,
    /// 各相机采集时的设备配置：发布时要与当前设备相同
    pub camera_config: Vec<Value>,
    /// 帧组数 = 各相机最大 seq 的最大值
    pub group_count: u64,
    pub camera_frames: Vec<u32>,
    pub incomplete_groups: u32,
    pub loss_alarms: u32,
    /// 测量每帧特征时各相机的走廊身份（走廊、当量、检测参数、站距）：走廊变了，这次测量不再可比
    pub corridors: Vec<Value>,
    /// 各相机去掉首尾各一帧（单段配方再去掉起涂、收尾帧，D-SB-50）后、测成的帧的胶中心 x 均值（mm）；没有可用帧为 None
    pub x_means: Vec<Option<f64>>,
    /// 同口径的胶边距均值（mm，D-SB-48 ToEdge）；没找边的相机为 None
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edge_means: Vec<Option<f64>>,
    /// 逐帧定向的帧数统计（D-SB-47）：段号 → [定向测量的帧数, 其中没找到胶条的帧数]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub heading: BTreeMap<u32, [u32; 2]>,
}

/// 候选里示教件的测量（至多一次，旧候选可能留有标定件轮次）。N_exp 由示教件分段写入，x_ref 由这次测量写入。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Calibration {
    pub rounds: Vec<CalibrationRound>,
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
    /// 示教件这一段的帧组数；示教件分段未确认时为 None
    pub n_exp: Option<u32>,
    pub x_ref: Option<XRefView>,
    pub x_ref_pending: Option<String>,
    /// 逐帧定向的提示（D-SB-47）：检测窗口能否覆盖每帧后移量、没找到胶条的比例
    pub hints: Vec<String>,
}

/// 没找到胶条的帧占比超过它就提示（窗口内胶路跟不住，多见于弯道）。
const HEADING_FALLBACK_HINT: f64 = 0.2;

/// 开启逐帧定向的段的标定提示（D-SB-47）：每帧后移量 ≈ 段长 ÷ N_exp，检测窗口短于它时相邻帧之间有胶测不到，
/// 超过 3 倍时同一处胶被重复测；各轮定向没找到胶条的帧超过 20% 时提示缩短窗口或减小起点。不阻断发布。
pub(super) fn heading_hints(
    doc: &RecipeDoc,
    segment: &PathSegment,
    rounds: &[CalibrationRound],
) -> Vec<String> {
    if segment.heading.is_none() || segment.skip {
        return Vec::new();
    }
    let mut hints = Vec::new();
    let cameras = crate::plan::segment_cameras(segment, &doc.plan_cameras());
    match (
        segment.length_mm,
        segment.expected_frames.filter(|&n| n > 0),
    ) {
        (None, _) => hints.push("段长未填，无法核对测量区间能否覆盖每帧后移量".to_string()),
        (Some(_), None) => {}
        (Some(length), Some(n)) => {
            let travel = length / f64::from(n);
            // 本段覆盖与默认值各相机共用，只提示一次；用相机的测量区间时按相机提示
            let mut checked = Vec::new();
            for inspect in doc.inspect.iter().filter(|i| cameras.contains(&i.camera)) {
                let (window, source) = crate::heading::window(segment, inspect);
                let who = match source {
                    crate::heading::WindowSource::Segment => "本段".to_string(),
                    crate::heading::WindowSource::Camera => format!("相机 {}", inspect.camera),
                    crate::heading::WindowSource::Default => "默认".to_string(),
                };
                if checked.contains(&who) {
                    continue;
                }
                if window.length_mm < travel {
                    hints.push(format!(
                        "{who}测量区间长 {:.1} mm，短于每帧后移量约 {travel:.1} mm：相邻帧之间有胶测不到，断胶可能漏检",
                        window.length_mm
                    ));
                } else if window.length_mm > 3.0 * travel {
                    hints.push(format!(
                        "{who}测量区间长 {:.1} mm，超过每帧后移量约 {travel:.1} mm 的 3 倍：同一处胶会被测多次",
                        window.length_mm
                    ));
                }
                checked.push(who);
            }
        }
    }
    let [measured, fallback] = rounds
        .iter()
        .filter_map(|r| r.heading.get(&segment.index))
        .fold([0u32; 2], |a, b| [a[0] + b[0], a[1] + b[1]]);
    if measured > 0 && f64::from(fallback) > HEADING_FALLBACK_HINT * f64::from(measured) {
        hints.push(format!(
            "定向没找到胶条 {fallback}/{measured} 帧（{:.0}%）：窗口内胶路跟不住，建议缩短测量区间或减小跳过距离",
            100.0 * f64::from(fallback) / f64::from(measured)
        ));
    }
    hints
}

/// 示教件基准（TS `CalibrationView`）：由配方与示教件的测量算出。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationView {
    /// 示教件的测量轮次；还没测过为 None
    pub round_id: Option<String>,
    /// 示教件已按当前走廊测量；测过但走廊、当量或检测参数改了为 false
    pub measured: bool,
    pub segments: Vec<SegmentCalibView>,
    pub expected_total: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraTeachingView {
    pub camera: String,
    pub mm_per_px: Option<f64>,
    /// 这台相机工位标定当前的像素当量（设备标定；模拟相机为固定当量）；没有标定为空。与 `mm_per_px` 不同时示教页提示采用
    pub calib_mm_per_px: Option<f64>,
    /// 走廊示教完成：有像素当量，且它检测的每一段都有已保存的走廊（D-SB-45）
    pub ready: bool,
    /// 已选示教帧的段，按段号
    pub segments: Vec<SegmentTeachingView>,
    /// 定向用的出胶点（D-SB-47）：标定值，或由开启定向的各段走廊起点推出（起点相差过大时带提示）；推不出时为空
    pub nozzle: Option<crate::heading::Nozzle>,
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
pub enum StepIdV2 {
    Capture,
    Segments,
    Corridor,
    Calibration,
    Validation,
    Publish,
}

/// 阻塞原因与“下一步”跳转的页面（TS `ProgressTargetV2`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "page", rename_all = "camelCase")]
pub enum TargetV2 {
    Capture,
    /// 分段：在示教件上确认边界
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
pub struct BlockerV2 {
    pub message: String,
    pub target: TargetV2,
}

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
pub struct NextV2 {
    pub step: StepIdV2,
    pub label: String,
    pub target: TargetV2,
}

/// 段配方的进度（TS `ProgressV2`）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressV2 {
    pub steps: Vec<StepV2>,
    pub next: Option<NextV2>,
}

/// 候选工作区里段配方的计划部分（TS `WorkspacePlanView`）；旧拍照点配方没有。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePlanView {
    pub segments: Vec<PathSegment>,
    pub cameras: Vec<String>,
    pub segment_defaults: crate::plan::KindDefaults,
    pub part_calib: Option<PartCalib>,
    pub expected_total: Option<u32>,
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
    /// 示教件（还没选为空）
    pub pieces: Vec<super::pieces::PieceView>,
}

/// features.jsonl 的一行。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureLine<'a> {
    camera: &'a str,
    seq: u64,
    metrics: &'a FrameMetrics,
}

pub(super) fn require_burst(doc: &RecipeDoc) -> Result<(), String> {
    if doc.is_burst() {
        Ok(())
    } else {
        Err("旧拍照点配方没有段计划：请用单帧示教、整圈采集与原有标定步骤".into())
    }
}

/// 一台相机的走廊身份：测每帧特征用到的当量、配方检测参数、站距、各段走廊（含拖过的扇区方向）、测量区间与出胶点、
/// 各段的逐帧定向与检测参数覆盖；不是计划相机为 null。
/// 第 8 项是已删除的专家参数层，恒为两个空表，留着让身份与删除前相同；段覆盖只在有段设了覆盖时追加，已有的示教件测量
/// 不因这两处改动过期。
pub(super) fn corridor_tag(doc: &RecipeDoc, camera: &str) -> Value {
    doc.inspect
        .iter()
        .find(|i| i.camera == camera)
        .map_or(Value::Null, |i| {
            let mut tag = json!([
                i.mm_per_px,
                doc.detect,
                doc.spacing,
                i.segments,
                i.window,
                i.nozzle,
                doc.segments
                    .iter()
                    .map(|s| (s.index, &s.heading))
                    .collect::<Vec<_>>(),
                [{}, {}]
            ]);
            if doc
                .segments
                .iter()
                .any(|s| s.search_mm.is_some() || s.width_range.is_some())
            {
                tag.as_array_mut().unwrap().push(json!(doc
                    .segments
                    .iter()
                    .map(|s| (s.index, s.search_mm, s.width_range))
                    .collect::<Vec<_>>()));
            }
            tag
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

/// 段走廊随段结构与段相机失效（D-SB-42/45）：段的类型序列变了，各相机的段走廊全部清空、返回 true（`remapped` 时
/// 走廊已按示教件帧范围迁到新段号，不清空，见 `segment_remap`）；某段不再由某台相机检测时，去掉它在这一段的走廊。
/// 保存的每条段走廊都要有对应的段示教帧。调用方据此去掉不再适用的段示教帧（见 `update_doc`）。
pub(super) fn guard_segment_corridors(
    old: &RecipeDoc,
    new: &mut RecipeDoc,
    teaching: &[CameraTeaching],
    remapped: bool,
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
    if !remapped && kinds(old) != kinds(new) {
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

/// 测量按段选走廊（D-SB-45）：按本轮已确认的分段，每帧只由所在段的检测相机用该段走廊测量，其余相机不测（None）。
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
        .ok_or_else(|| format!("采集轮次 {round} 还没有确认分段：先在分段步骤确认示教件边界"))?;
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
        measurement_evidence: None,
        widths: Vec::new(),
        spacing_mm: 0.0,
        gap_max_mm: 0.0,
        present_ratio: 0.0,
        x_mm: None,
        corridor: None,
        heading_deg: None,
        heading_fallback: false,
        fallback_reason: None,
        edge_mm: None,
    }
}

// ───────────── 段列表与配方编辑 ─────────────

/// 清空全部 N_exp、标定统计、整件标定、笔结构与 x_ref。
pub(super) fn clear_calibration(doc: &mut RecipeDoc) {
    for segment in &mut doc.segments {
        segment.expected_frames = None;
        segment.calib = None;
    }
    doc.part_calib = None;
    doc.pass_frames = None;
    clear_reference(doc);
}

fn clear_reference(doc: &mut RecipeDoc) {
    for reference in doc
        .segments
        .iter_mut()
        .filter_map(|s| s.offset_ref.as_mut())
    {
        reference.x_ref = None;
        reference.spread = None;
    }
}

/// 候选每次改动后（`Workspace::changed`）按示教件重算基准（RB-3）：各段 N_exp = 示教件已确认分段的帧组数，标定统计记 1 件，
/// 整件 ΣN_exp 取各段之和；示教件分段未确认或已失效时清空。示教件没按当前走廊测量时 x_ref 清空，要重新测量。
pub(super) fn calibrate_from_teaching(w: &mut Workspace) {
    if !w.doc.is_burst() {
        return;
    }
    let counts = super::pieces::teaching_counts(w);
    let stat = |n: u32| SegmentCalib {
        rounds: 1,
        min: n,
        max: n,
        mean: f64::from(n),
    };
    for (k, segment) in w.doc.segments.iter_mut().enumerate() {
        let n = counts.as_ref().and_then(|counts| counts.get(k).copied());
        segment.expected_frames = n;
        segment.calib = n.map(stat);
    }
    // 笔结构随 N_exp 一起取自示教件（D-SB-62）：帧组数要与已确认的分段对得上
    let passes = w.teaching_piece.as_ref().map(|piece| piece.passes.clone());
    w.doc.pass_frames = passes.filter(|passes| {
        !passes.is_empty()
            && counts.as_ref().is_some_and(|counts| {
                counts.iter().map(|&n| u64::from(n)).sum::<u64>()
                    == passes.iter().map(|&n| u64::from(n)).sum::<u64>()
            })
    });
    w.doc.part_calib = None;
    if !super::pieces::teaching_measured(w) {
        clear_reference(&mut w.doc);
    }
}

/// 新建段配方候选：复制来的 N_exp、标定统计、整件标定与 x_ref 清空——它们只能由本配方的示教件得出。旧拍照点配方不经过。
pub(super) fn prepare_new_doc(doc: &mut RecipeDoc) {
    if !doc.is_burst() {
        return;
    }
    clear_calibration(doc);
}

/// 段配方的基准只能由示教件得出（RB-3）：保存时不采用传入的 N_exp、标定统计、整件标定、笔结构与 x_ref，沿用候选当前的值
/// （随后 `Workspace::changed` 按示教件重算 N_exp），界面拿着旧值保存也不会改动或被拒。段的类型序列（段数、顺序、类型）
/// 变了全部清空；胶偏参考的测量量、相机或找边参数变了，旧 x_ref 不再适用，清空。两份都是旧拍照点配方时不处理。
pub(super) fn guard_calibration(old: &RecipeDoc, new: &mut RecipeDoc) {
    if !old.is_burst() && !new.is_burst() {
        return;
    }
    let kinds = |doc: &RecipeDoc| doc.segments.iter().map(|s| s.kind).collect::<Vec<_>>();
    if kinds(old) != kinds(new) {
        clear_calibration(new);
        return;
    }
    new.part_calib.clone_from(&old.part_calib);
    new.pass_frames.clone_from(&old.pass_frames);
    let edge = |r: &OffsetRef| {
        r.edge
            .as_ref()
            .map(|e| (e.polarity, e.search_mm, e.contrast_min))
    };
    for (before, after) in old.segments.iter().zip(new.segments.iter_mut()) {
        after.expected_frames = before.expected_frames;
        after.calib.clone_from(&before.calib);
        let Some(reference) = after.offset_ref.as_mut() else {
            continue;
        };
        let previous = before.offset_ref.as_ref().filter(|old| {
            old.measure == reference.measure
                && old.camera == reference.camera
                && edge(old) == edge(reference)
        });
        (reference.x_ref, reference.spread) =
            previous.map_or((None, None), |o| (o.x_ref, o.spread));
    }
}

/// 整体替换段列表（经 `update_doc`，同样受 `guard_calibration` 约束）。
pub(super) fn update_segments(w: &mut Workspace, segments: Vec<PathSegment>) -> Result<(), String> {
    require_burst(&w.doc)?;
    if segments.is_empty() {
        return Err("段配方至少需要一段，不能改成拍照点配方".into());
    }
    let mut doc = w.doc.clone();
    doc.segments = segments;
    update_doc(w, doc)
}

// ───────────── 走廊示教 ─────────────

/// 用来示教的一帧要来自本候选完整的段配方采集轮次（示教件，调用方先核对），这台相机有这一帧，且采集后设备配置没变。返回这台相机的采集记录。
fn teaching_source<'a>(
    round: &'a CaptureRoundV2,
    recipe_id: &str,
    camera: &str,
    seq: u64,
    current_config: &Value,
) -> Result<&'a CaptureCamera, String> {
    if round.recipe_id != recipe_id || round.state != CaptureState::Complete {
        return Err(format!(
            "采集轮次 {} 不属于当前候选或尚未完整完成，不能用于示教",
            round.round_id
        ));
    }
    let (index, source) = round
        .cameras
        .iter()
        .enumerate()
        .find(|(_, c)| c.camera_id == camera)
        .ok_or_else(|| format!("采集轮次 {} 没有相机 {camera}", round.round_id))?;
    if !(1..=round.group_count).contains(&seq) {
        return Err(format!(
            "采集轮次 {} 只有 {} 个帧组，没有第 {seq} 个",
            round.round_id, round.group_count
        ));
    }
    if round
        .missing_frames
        .iter()
        .any(|m| m.camera == index && m.seq == seq)
    {
        return Err(format!("相机 {camera} 缺第 {seq} 帧组的图像，请换一帧"));
    }
    if &source.camera_config != current_config {
        return Err(format!("采集后相机 {camera} 的参数已改变，请重新采集"));
    }
    Ok(source)
}

/// 第 `segment` 段在采集轮次 `round` 里的帧组范围（含两端）：单段配方为整轮；多段配方按本轮已确认的手动分段，未确认时报错。
pub(super) fn segment_range(
    w: &Workspace,
    round: &str,
    group_count: u64,
    segment: u32,
) -> Result<(u64, u64), String> {
    let segments = &w.doc.segments;
    let position = segments
        .iter()
        .position(|s| s.index == segment)
        .ok_or_else(|| format!("没有第 {segment} 段"))?;
    if segments.len() == 1 {
        return Ok((1, group_count));
    }
    let counts = capture_review::confirmed_segment_counts(
        &w.doc,
        &w.capture_review.rounds,
        round,
        group_count,
    )
    .ok_or_else(|| {
        format!("采集轮次 {round} 还没有确认手动分段：先在分段标定里确认这一轮的边界")
    })?;
    let start = counts[..position]
        .iter()
        .map(|&n| u64::from(n))
        .sum::<u64>()
        + 1;
    Ok((start, start + u64::from(counts[position]) - 1))
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
    stage_segment_teaching(w, teaching, group_count, default_scale)?;
    w.changed();
    Ok(())
}

fn stage_segment_teaching(
    w: &mut Workspace,
    teaching: CameraTeaching,
    group_count: u64,
    default_scale: Option<f64>,
) -> Result<(), String> {
    require_burst(&w.doc)?;
    super::pieces::require_teaching_round(w, &teaching.round)?;
    let (camera, segment) = (
        teaching.camera.clone(),
        teaching.segment.ok_or("段走廊的示教帧缺段号")?,
    );
    if !w.doc.segments.iter().any(|s| s.index == segment) {
        return Err(format!("没有第 {segment} 段"));
    }
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
        let (start, end) = segment_range(w, &teaching.round, group_count, segment)?;
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
    super::pass_align::mark_retaught(w, segment, &camera);
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
    let prepared =
        prepare_segment_teaching(app.clone(), id, revision, None, camera, segment, round, seq)
            .await?;
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let mut next = prepared.workspace.clone();
        if !prepared.image.is_color() {
            let mut doc = next.doc.clone();
            gray_frame_measures_gray(&mut doc, &prepared.camera, prepared.segment);
            update_doc(&mut next, doc)?;
        }
        if next.revision == prepared.revision {
            next.changed();
        }
        let result = view(&app, next.clone())?;
        prepared.persist(&host, next)?;
        Ok(result)
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

/// 走廊示教的准备结果。准备与提取都只操作内存，提交时才写新图和候选。
pub struct PreparedSegmentTeaching {
    pub frame: FrozenImage,
    pub image: Arc<FrameImage>,
    pub workspace: Workspace,
    camera: String,
    segment: u32,
    bytes: Vec<u8>,
    revision: u64,
    instance_id: u64,
}

impl PreparedSegmentTeaching {
    pub fn stage(
        workspace: &Workspace,
        teaching: CameraTeaching,
        image: Arc<FrameImage>,
        bytes: Vec<u8>,
        group_count: u64,
        default_scale: Option<f64>,
    ) -> Result<Self, String> {
        workspace.expect(workspace.revision)?;
        if teaching.frame.size != [image.width, image.height] || !image.consistent() {
            return Err("示教帧尺寸与原图不一致".into());
        }
        let mut next = workspace.clone();
        stage_segment_teaching(&mut next, teaching.clone(), group_count, default_scale)?;
        Ok(Self {
            frame: teaching.frame,
            image,
            workspace: next,
            camera: teaching.camera,
            segment: teaching.segment.ok_or("段走廊的示教帧缺段号")?,
            bytes,
            revision: workspace.revision,
            instance_id: workspace.instance_id,
        })
    }

    fn corridor_workspace(
        &self,
        corridor: Vec<[f32; 2]>,
        color_model: Option<String>,
    ) -> Result<Workspace, String> {
        let mut base = self.workspace.clone();
        if !self.image.is_color() {
            gray_frame_measures_gray(&mut base.doc, &self.camera, self.segment);
        }
        with_segment_corridor(
            &base,
            self.revision,
            &self.camera,
            self.segment,
            self.frame.size,
            corridor,
            color_model,
        )
    }

    pub fn commit(
        self,
        host: &WorkspaceHost,
        corridor: Vec<[f32; 2]>,
        color_model: Option<String>,
    ) -> Result<Workspace, String> {
        if color_model.is_some() {
            require_usable_color_model(color_model.as_deref())?;
        }
        let next = self.corridor_workspace(corridor, color_model)?;
        self.persist(host, next)
    }

    fn persist(self, host: &WorkspaceHost, next: Workspace) -> Result<Workspace, String> {
        let id = &next.doc.id;
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(id).ok_or("候选配置不存在")?;
        current.expect(self.revision)?;
        if current.instance_id != self.instance_id {
            return Err("候选已重建，请重新加载后操作".into());
        }
        let path = image_file(host, id, &self.frame.id)?;
        std::fs::create_dir_all(path.parent().ok_or("示教图像目录无效")?)
            .map_err(|e| e.to_string())?;
        let bytes = if self.image.is_color() {
            self.bytes
        } else {
            let mut bytes =
                crate::image_file::pnm_header(self.image.width, self.image.height, 1, None)
                    .into_bytes();
            bytes.extend_from_slice(&self.image.pixels);
            bytes
        };
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("创建示教图 {} 失败，候选未改变：{e}", path.display()))?;
        let saved = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("写示教图 {} 失败：{e}", path.display()));
        drop(file);
        if let Err(error) = saved.and_then(|_| host.save(&next)) {
            return match std::fs::remove_file(&path) {
                Ok(()) => Err(format!("示教未提交，候选未改变：{error}")),
                Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(error),
                Err(cleanup) => Err(format!(
                    "示教未提交，候选未改变：{error}；新图 {} 清理失败：{cleanup}",
                    path.display()
                )),
            };
        }
        *current = next.clone();
        Ok(next)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_segment_teaching(
    app: AppHandle,
    id: String,
    revision: u64,
    expected_instance_id: Option<u64>,
    camera: String,
    segment: u32,
    round: String,
    seq: u64,
) -> Result<PreparedSegmentTeaching, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let workspace = workspace_clone(&app, &id)?;
        workspace.expect(revision)?;
        if expected_instance_id.is_some_and(|instance| workspace.instance_id != instance) {
            return Err("候选已重建，请重新加载后示教".into());
        }
        require_burst(&workspace.doc)?;
        super::pieces::require_teaching_round(&workspace, &round)?;
        let inspect = workspace
            .doc
            .inspect
            .iter()
            .find(|i| i.camera == camera)
            .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
        let cycle = app.state::<CycleHost>();
        let capture = cycle.camera.recipe_capture();
        let captured = capture.get_burst(&round)?;
        let config = cycle
            .camera
            .configs()
            .into_iter()
            .find(|c| c.id == camera)
            .ok_or_else(|| format!("相机 {camera} 已不在设备配置里"))?;
        let imaging = teaching_source(&captured, &id, &camera, seq, &state_value(&config))?.imaging;
        let source = capture.burst_frame_path(&round, &camera, seq)?;
        crate::image_file::checked_source_path(&source)?;
        let bytes =
            std::fs::read(&source).map_err(|e| format!("读取 {} 失败：{e}", source.display()))?;
        let image = Arc::new(
            crate::image_file::decode(&bytes)
                .map_err(|e| format!("解码 {} 失败：{e}", source.display()))?,
        );
        let calib = vision::camera_calib_path(&app, inspect)?;
        let default_scale = calib_scale(&app, inspect, config.source)?;
        let sample_set = None::<String>;
        let frame = FrozenImage {
            id: format!(
                "corridor-{}-{revision}-{seq}-{}",
                ly_plc::now_ms(),
                host.capture_seq.fetch_add(1, Ordering::Relaxed)
            ),
            view: 1,
            source: format!("capture:{round}#{seq}"),
            captured_at: captured.created_at,
            size: [image.width, image.height],
            camera: camera.clone(),
            camera_tag: camera_state(&config, sample_set.as_deref()),
            calib_tag: calibration_state(&calib)?,
            geometry_tag: json!({"round": round, "camera": camera, "seq": seq}),
            exposure_us: Some(imaging.exposure_us),
            gain_db: Some(imaging.gain_db),
            history_id: None,
        };
        PreparedSegmentTeaching::stage(
            &workspace,
            CameraTeaching {
                camera,
                frame,
                round,
                seq,
                segment: Some(segment),
            },
            image,
            bytes,
            captured.group_count,
            default_scale,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(super) async fn commit_segment_teaching(
    app: AppHandle,
    prepared: PreparedSegmentTeaching,
    corridor: Vec<[f32; 2]>,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepared.corridor_workspace(corridor.clone(), None)?;
        let inspect = prepared
            .workspace
            .doc
            .inspect
            .iter()
            .find(|i| i.camera == prepared.camera)
            .ok_or("示教相机不在候选中")?;
        let model = if prepared.image.is_color() && !gray_chosen(inspect, prepared.segment) {
            let scale = inspect.mm_per_px.ok_or("先填像素当量再取色")?;
            let doc = &prepared.workspace.doc;
            let detect = doc.detect.for_segment(&doc.segments, prepared.segment);
            let text = fit_color_model(
                &*lyflow(&app)?,
                &prepared.workspace.doc.id,
                &prepared.camera,
                &prepared.image,
                &corridor,
                &detect,
                scale,
            )?;
            Some(text)
        } else {
            None
        };
        let next = prepared.commit(&app.state::<WorkspaceHost>(), corridor, model)?;
        let revision = next.revision;
        view(&app, next).map_err(|e| format!("示教已保存为修订 {revision}，但刷新视图失败：{e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 相机工位标定的像素当量：模拟相机取固定当量，其余读它的设备标定文件。
fn calib_scale(
    app: &AppHandle,
    inspect: &CameraInspect,
    source: CameraSource,
) -> Result<Option<f64>, String> {
    if source == CameraSource::Sim {
        return Ok(Some(f64::from(crate::recipe::SIM_MM_PER_PX)));
    }
    Ok(vision::calib_info(&vision::camera_calib_path(app, inspect)?).and_then(|c| c.mm_per_px))
}

/// 示教取色：在相机 `camera` 第 `segment` 段冻结的示教帧上，沿已保存的走廊取胶色与两侧背景色，拟合颜色模型
/// （`glue.color_model`），写进这一段走廊的 `colorModel`；这一段之后按胶色测量。示教帧须是彩色。
/// 胶样本取走廊 ± 胶宽下限的一半，背景带从胶宽上限的一半外 4 px 起、宽 30 px。
#[tauri::command]
pub async fn workspace_fit_color_model(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    segment: u32,
) -> Result<WorkspaceView, String> {
    fit_workspace_color_model(app, id, revision, None, camera, segment).await
}

pub(super) async fn workspace_fit_color_model_at(
    app: AppHandle,
    id: String,
    revision: u64,
    instance_id: u64,
    camera: String,
    segment: u32,
) -> Result<WorkspaceView, String> {
    fit_workspace_color_model(app, id, revision, Some(instance_id), camera, segment).await
}

async fn fit_workspace_color_model(
    app: AppHandle,
    id: String,
    revision: u64,
    expected_instance_id: Option<u64>,
    camera: String,
    segment: u32,
) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let before = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选不存在")?;
            w.expect(revision)?;
            if expected_instance_id.is_some_and(|instance| w.instance_id != instance) {
                return Err("候选已重建，请重新加载后取色".into());
            }
            w.clone()
        };
        require_burst(&before.doc)?;
        let frame = before
            .camera_teaching
            .iter()
            .find(|t| t.camera == camera && t.segment == Some(segment))
            .map(|t| t.frame.id.as_str())
            .ok_or_else(|| format!("相机 {camera} 第 {segment} 段还没有示教帧"))?;
        let inspect = before
            .doc
            .inspect
            .iter()
            .find(|i| i.camera == camera)
            .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
        let corridor = inspect
            .segment_corridor(segment)
            .map(|s| s.corridor.as_slice())
            .filter(|c| c.len() >= 2)
            .ok_or_else(|| {
                format!("相机 {camera} 第 {segment} 段还没有保存走廊，先示教走廊再取色")
            })?;
        let scale = inspect.mm_per_px.ok_or("先填像素当量再取色")?;
        let detect = &before.doc.detect.for_segment(&before.doc.segments, segment);
        if gray_chosen(inspect, segment) {
            return Err(format!(
                "相机 {camera} 第 {segment} 段选了按灰度测量：先改为按胶色测量再取色"
            ));
        }
        let image = crate::image_file::load(&image_file(&host, &id, frame)?)?;
        if !image.is_color() {
            return Err(
                "示教帧是灰度图，不能取色：把相机图像模式改为彩色后重新选这一段的示教帧".into(),
            );
        }
        let text = fit_color_model(
            &*lyflow(&app)?,
            &id,
            &camera,
            &image,
            corridor,
            detect,
            scale,
        )?;
        let saved = host.save_fitted_color_model(&before, &camera, segment, text)?;
        let revision = saved.revision;
        view(&app, saved)
            .map_err(|e| format!("颜色模型已保存为修订 {revision}，但刷新视图失败：{e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

impl WorkspaceHost {
    pub fn save_fitted_color_model(
        &self,
        before: &Workspace,
        camera: &str,
        segment: u32,
        model: String,
    ) -> Result<Workspace, String> {
        let mut items = self.items.lock().unwrap();
        let mut w = items.get(&before.doc.id).ok_or("候选不存在")?.clone();
        w.expect(before.revision)?;
        if w.instance_id != before.instance_id {
            return Err("取色期间候选已重建，请重新取色".into());
        }
        let frame_id = before
            .camera_teaching
            .iter()
            .find(|t| t.camera == camera && t.segment == Some(segment))
            .map(|t| t.frame.id.as_str())
            .ok_or("开始取色时没有示教帧")?;
        let corridor = before
            .doc
            .inspect
            .iter()
            .find(|i| i.camera == camera)
            .and_then(|i| i.segment_corridor(segment))
            .map(|s| &s.corridor)
            .ok_or("开始取色时没有走廊")?;
        if !w
            .camera_teaching
            .iter()
            .any(|t| t.camera == camera && t.segment == Some(segment) && t.frame.id == frame_id)
        {
            return Err("取色期间示教帧已变化，请重新取色".into());
        }
        let mut doc = w.doc.clone();
        let target = doc
            .inspect
            .iter_mut()
            .find(|i| i.camera == camera)
            .and_then(|i| i.segments.iter_mut().find(|s| s.segment == segment))
            .filter(|s| &s.corridor == corridor)
            .ok_or("取色期间走廊已变化，请重新取色")?;
        target.color_model = Some(model);
        update_doc(&mut w, doc)?;
        self.save(&w)?;
        items.insert(before.doc.id.clone(), w.clone());
        Ok(w)
    }
}

pub(super) fn lyflow(app: &AppHandle) -> Result<Arc<vision::Engine>, String> {
    app.state::<vision::VisionHost>()
        .engine()
        .ok_or_else(|| "lyFlow 核心库未加载".into())
}

pub(super) fn fit_color_model(
    engine: &vision::Engine,
    id: &str,
    camera: &str,
    image: &FrameImage,
    corridor: &[[f32; 2]],
    detect: &crate::recipe::DetectParams,
    scale: f64,
) -> Result<String, String> {
    let params = color_teach_params(corridor, detect, scale)?;
    let result = engine.run_targets(
        vision::template::color_teach().text,
        &vision::unique_run_id(&format!("color-teach-{id}-{camera}")),
        image,
        &params,
        &["n_model".to_string()],
    )?;
    if result.status() != "ok" {
        return Err(format!("取色失败：{}", result.failure()));
    }
    let model = result.record("model").ok_or("取色没有输出颜色模型")?;
    serde_json::to_string(model).map_err(|e| e.to_string())
}

/// 取色图（`color_teach`）的参数：胶样本取走廊 ± 胶宽下限的一半，背景带从胶宽上限的一半外 4 px 起、宽 30 px。
pub(super) fn color_teach_params(
    points: &[[f32; 2]],
    detect: &crate::recipe::DetectParams,
    scale: f64,
) -> Result<Value, String> {
    let [lo, hi] = detect.width_range.map(|w| f64::from(w) / scale);
    let inner = 0.5 * hi + 4.0;
    Ok(serde_json::json!({
        "points": serde_json::to_string(points).map_err(|e| e.to_string())?,
        "glueHalf": (0.5 * lo).clamp(1.0, 40.0),
        "bgBand": [inner, inner + 30.0],
    }))
}

/// 段配方的走廊提取（决策记录 5.4）：在相机 `camera` 冻结的示教帧 `image_id` 上按选区找两侧胶边（彩色帧换算成亮度）；
/// 这台相机的检测极性（没有时取配方默认）只决定先试哪种，胶比背景暗还是亮按图自动判断（见 `centerline::extract`）。
/// 彩色帧的刷选走 [`workspace_snap_corridor`] 按胶色吸附。
#[tauri::command]
pub async fn workspace_extract_corridor(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    image_id: String,
    roi: [u32; 4],
    guide: Option<super::centerline::BrushGuide>,
    point_spacing_px: Option<f32>,
) -> Result<CenterlineResult, String> {
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
            if !w.doc.inspect.iter().any(|i| i.camera == camera) {
                return Err(format!("相机 {camera} 不是段配方的计划相机"));
            }
            (image_file(&host, &id, &image_id)?, w.doc.detect.polarity)
        };
        let image = crate::image_file::load(&path)?;
        let result = super::centerline::extract(
            &image,
            roi,
            polarity,
            guide.as_ref(),
            point_spacing_px.unwrap_or(20.0),
        );
        let items = host.items.lock().unwrap();
        let current = items.get(&id).ok_or("候选不存在")?;
        current.expect(revision)?;
        if !frozen(current) {
            return Err("提取期间图像已变化".into());
        }
        match result {
            Ok(extracted) => Ok(CenterlineResult {
                path: extracted.path,
                message: extracted
                    .notice
                    .unwrap_or_else(|| "走廊已提取，请检查并人工修正后保存".into()),
                failure: None,
            }),
            Err(super::centerline::ExtractionError::Failed(failure)) => Ok(CenterlineResult {
                path: Vec::new(),
                message: failure.message.clone(),
                failure: Some(failure),
            }),
            Err(super::centerline::ExtractionError::Invalid(message)) => Err(message),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把相机 `camera` 第 `segment` 段的走廊（连同颜色模型）写进 `workspace` 的副本：保留这一段拖过的扇区方向，
/// 按示教帧尺寸 `size` 核对走廊能建测量图。`base` 为提交所基于的修订，副本修订未前进时补一次。
fn with_segment_corridor(
    workspace: &Workspace,
    base: u64,
    camera: &str,
    segment: u32,
    size: [u32; 2],
    corridor: Vec<[f32; 2]>,
    color_model: Option<String>,
) -> Result<Workspace, String> {
    let mut next = workspace.clone();
    let mut doc = next.doc.clone();
    let inspect = doc
        .inspect
        .iter_mut()
        .find(|i| i.camera == camera)
        .ok_or("示教相机不在候选中")?;
    let previous = inspect.segment_corridor(segment).cloned();
    inspect.segments.retain(|s| s.segment != segment);
    inspect.segments.push(crate::recipe::SegmentCorridor {
        segment,
        corridor,
        sector_deg: previous.as_ref().and_then(|p| p.sector_deg),
        color_model,
        measure: previous.and_then(|p| p.measure),
    });
    inspect.segments.sort_by_key(|s| s.segment);
    let recipe = doc.build()?;
    let mut inspect = recipe
        .inspect
        .iter()
        .find(|i| i.camera == camera)
        .ok_or("示教相机不在候选中")?
        .clone();
    // 取色前先核对走廊几何：还没有模型时按灰度建图，不因“待取色”拦下
    if let Some(s) = inspect
        .segments
        .iter_mut()
        .find(|s| s.segment == segment && s.color_model.is_none())
    {
        s.measure = Some(crate::recipe::MeasureMode::Gray);
    }
    vision::plan_corridor(&inspect, segment, &recipe, size)?;
    update_doc(&mut next, doc)?;
    if next.revision == base {
        next.changed();
    }
    Ok(next)
}

/// 灰度示教帧只能按灰度测：去掉这一段的胶色选择与颜色模型，免得卡在取不了色的“待取色”（color CL-3）。
pub(super) fn gray_frame_measures_gray(doc: &mut RecipeDoc, camera: &str, segment: u32) {
    if let Some(s) = doc
        .inspect
        .iter_mut()
        .find(|i| i.camera == camera)
        .and_then(|i| i.segments.iter_mut().find(|s| s.segment == segment))
    {
        s.color_model = None;
        s.measure = None;
    }
}

/// 操作员明确选了按灰度测这一段：不取色、不按胶色吸附。
fn gray_chosen(inspect: &CameraInspect, segment: u32) -> bool {
    inspect
        .segment_corridor(segment)
        .is_some_and(|s| s.measure == Some(crate::recipe::MeasureMode::Gray))
}

fn require_usable_color_model(model: Option<&str>) -> Result<(), String> {
    let status = vision::color_model_status(model);
    if status.model_usable {
        return Ok(());
    }
    let reasons = status
        .issues
        .iter()
        .map(|i| i.message.as_str())
        .collect::<Vec<_>>()
        .join("；");
    Err(format!(
        "取色结果不可用：{reasons}；换一帧示教帧或调整笔划后重试，候选未保存"
    ))
}

#[derive(Serialize)]
pub struct SnapCorridorResult {
    /// 提交后的候选；吸附、取色或走廊核对没通过时为空，候选不变
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<WorkspaceView>,
    pub message: String,
}

/// 彩色示教帧上的刷选（与示教 MCP `teach_corridor` 按胶色吸附同一口径）：在相机 `camera` 第 `segment` 段冻结的示教帧
/// `image_id` 上，把笔划按胶色吸附到胶中心（偏移上限为笔刷半径），沿吸附后的走廊取色，走廊与颜色模型一次提交。
/// 吸附、取色或走廊核对没通过时候选不变，原因放在 `message`；候选修订或示教帧变了时报错。
#[tauri::command]
pub async fn workspace_snap_corridor(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    segment: u32,
    image_id: String,
    guide: super::centerline::BrushGuide,
) -> Result<SnapCorridorResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let frozen = |w: &Workspace| {
            w.camera_teaching.iter().any(|t| {
                t.camera == camera && t.segment == Some(segment) && t.frame.id == image_id
            })
        };
        let before = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选不存在")?;
            w.expect(revision)?;
            require_burst(&w.doc)?;
            if !frozen(w) {
                return Err("图像已变化，请重新刷选".into());
            }
            w.clone()
        };
        let fail = |message: String| Ok(SnapCorridorResult { view: None, message });
        let inspect = before
            .doc
            .inspect
            .iter()
            .find(|i| i.camera == camera)
            .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
        let Some(scale) = inspect.mm_per_px else {
            return fail(format!(
                "按胶色吸附要用像素当量把胶宽换成像素：{camera} 还没有像素当量，先在左侧“相机参数”里填写或采用设备标定当量，再刷选"
            ));
        };
        if gray_chosen(inspect, segment) {
            return Err(format!(
                "相机 {camera} 第 {segment} 段选了按灰度测量，刷选按两侧胶边提取，不按胶色吸附"
            ));
        }
        let detect = &before.doc.detect.for_segment(&before.doc.segments, segment);
        let radius = guide.radius_px;
        if !radius.is_finite() || !(3.0..=150.0).contains(&radius) || guide.points.len() < 2 {
            return Err("请沿胶路绘制笔划，笔刷半径需为 3–150 像素".into());
        }
        let image = crate::image_file::load(&image_file(&host, &id, &image_id)?)?;
        if !image.is_color() {
            return Err("示教帧是灰度图，不能按胶色吸附".into());
        }
        if !vision::corridor_inside(&guide.points, [image.width, image.height]) {
            return Err("笔划必须位于示教帧内".into());
        }
        let engine = lyflow(&app)?;
        let (corridor, moved) = match super::snap::snap_color(
            &engine,
            &id,
            &camera,
            detect,
            scale,
            &image,
            &guide.points,
            radius,
        ) {
            Ok(snapped) => snapped,
            Err(message) => return fail(message),
        };
        let model = match fit_color_model(&engine, &id, &camera, &image, &corridor, detect, scale) {
            Ok(model) => model,
            Err(message) => return fail(message),
        };
        if let Err(message) = require_usable_color_model(Some(&model)) {
            return fail(message);
        }
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(&id).ok_or("候选不存在")?;
        current.expect(revision)?;
        if current.instance_id != before.instance_id {
            return Err("吸附期间候选已重建，请重新刷选".into());
        }
        if !frozen(current) {
            return Err("吸附期间示教帧已变化，请重新刷选".into());
        }
        let next = match with_segment_corridor(
            current,
            revision,
            &camera,
            segment,
            [image.width, image.height],
            corridor,
            Some(model),
        ) {
            Ok(next) => next,
            Err(message) => return fail(message),
        };
        host.save(&next)?;
        *current = next.clone();
        drop(items);
        let note = moved.map_or_else(String::new, |s| format!("，平均移动 {s:.1} px"));
        let revision = next.revision;
        let view = view(&app, next)
            .map_err(|e| format!("走廊已保存为修订 {revision}，但刷新视图失败：{e}"))?;
        Ok(SnapCorridorResult {
            view: Some(view),
            message: format!("已按胶色吸附到胶中心{note}，走廊与胶色已保存，请核对叠图"),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// ───────────── 示教件测量 ─────────────

/// 示教件能不能测量：属于本候选、已完整完成；相机集合等于计划相机；每台相机采集时的设备配置与当前设备相同。
pub(super) fn check_round(
    doc: &RecipeDoc,
    round: &CaptureRoundV2,
    current: &BTreeMap<String, Value>,
) -> Result<(), String> {
    require_burst(doc)?;
    let id = &round.round_id;
    if round.recipe_id != doc.id {
        return Err(format!(
            "采集轮次 {id} 属于配方 {}，不是当前候选 {}",
            round.recipe_id, doc.id
        ));
    }
    if round.state != CaptureState::Complete {
        return Err(format!("采集轮次 {id} 尚未完整完成，不能测量"));
    }
    let cameras = doc.plan_cameras();
    let ids: BTreeSet<&str> = round.cameras.iter().map(|c| c.camera_id.as_str()).collect();
    if round.cameras.len() != cameras.len() || ids != cameras.iter().map(String::as_str).collect() {
        return Err(format!(
            "采集轮次 {id} 的相机 {:?} 与计划相机 {cameras:?} 不同",
            round.camera_ids()
        ));
    }
    if round.group_count == 0 {
        return Err(format!("采集轮次 {id} 没有帧组"));
    }
    for camera in &round.cameras {
        let camera_id = &camera.camera_id;
        if current.get(camera_id) != Some(&camera.camera_config) {
            return Err(format!("采集后相机 {camera_id} 的参数已改变，请重新采集"));
        }
    }
    Ok(())
}

/// 量一轮的每一帧：按计划相机、帧组序号 1..=帧组数排列；缺帧记 Missing，`measure` 失败的帧记 Error（规格 §0 所有帧都测）。
/// `measure(计划相机下标, 相机编号, seq)`。
pub(super) fn measure_round(
    cameras: &[String],
    round: &CaptureRoundV2,
    measure: impl FnMut(usize, &str, u64) -> Result<FrameMetrics, String>,
) -> Vec<FrameMetrics> {
    measure_round_with_operation(cameras, round, measure, None)
        .expect("无取消任务的测量总会返回帧结果")
}

pub(super) fn measure_round_with_operation(
    cameras: &[String],
    round: &CaptureRoundV2,
    mut measure: impl FnMut(usize, &str, u64) -> Result<FrameMetrics, String>,
    operation: Option<&crate::operation::Operation>,
) -> Result<Vec<FrameMetrics>, String> {
    let total = cameras.len() * round.group_count as usize;
    if let Some(operation) = operation {
        operation.measuring(total)?;
    }
    let mut metrics = Vec::with_capacity(total);
    for (index, camera) in cameras.iter().enumerate() {
        let source = round.cameras.iter().position(|c| &c.camera_id == camera);
        let missing: BTreeSet<u64> = round
            .missing_frames
            .iter()
            .filter(|m| Some(m.camera) == source)
            .map(|m| m.seq)
            .collect();
        for seq in 1..=round.group_count {
            if let Some(operation) = operation {
                operation.check()?;
            }
            let metric = if source.is_none() || missing.contains(&seq) {
                frame_without_metrics(index, seq, FrameState::Missing, None)
            } else {
                match measure(index, camera, seq) {
                    Ok(m) => FrameMetrics {
                        camera: index,
                        seq,
                        ..m
                    },
                    Err(error) => frame_without_metrics(index, seq, FrameState::Error, Some(error)),
                }
            };
            if let Some(operation) = operation {
                operation.advance(metric.state)?;
            }
            metrics.push(metric);
        }
    }
    Ok(metrics)
}

/// 用比例分段器分段（D-SB-40；各段 N_exp 未全部标定时为未分段），结果写进 boundaries.json 供追溯。
pub(super) fn segment_round(
    segments: &[PathSegment],
    cameras: usize,
    group_count: u64,
    metrics: &[FrameMetrics],
) -> SegOutcome {
    let mut groups = super::burst_history::frame_groups(metrics, cameras);
    groups.resize_with(group_count as usize, || FrameGroup {
        seq: 0,
        incomplete: true,
        present: vec![false; cameras],
    });
    for (index, group) in groups.iter_mut().enumerate() {
        group.seq = index as u64 + 1;
    }
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

/// 读示教件测量的每帧特征（`FEATURES_FILE`）：相机按当前计划相机重新编下标，不再是计划相机的行丢弃。
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
pub(super) fn write_features(
    dir: &Path,
    cameras: &[String],
    metrics: &[FrameMetrics],
    outcome: &SegOutcome,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建标定特征目录失败：{e}"))?;
    let mut text = String::new();
    for m in metrics {
        let camera = cameras.get(m.camera).ok_or("特征的相机下标超出计划相机")?;
        text.push_str(
            &serde_json::to_string(&FeatureLine {
                camera,
                seq: m.seq,
                metrics: m,
            })
            .map_err(|e| e.to_string())?,
        );
        text.push('\n');
    }
    crate::fsio::write_atomic(&dir.join(FEATURES_FILE), &text)?;
    crate::fsio::write_atomic(
        &dir.join(BOUNDARIES_FILE),
        &serde_json::to_string_pretty(outcome).map_err(|e| e.to_string())?,
    )
}

/// 一台相机去掉首尾 `trim`（各至少一帧）后、测成的帧的测量量（胶中心 x 或胶边距）均值。
fn round_mean(
    metrics: &[FrameMetrics],
    camera: usize,
    group_count: u64,
    trim: (u64, u64),
    value: fn(&FrameMetrics) -> Option<f64>,
) -> Option<f64> {
    let xs: Vec<f64> = metrics
        .iter()
        .filter(|m| {
            m.camera == camera
                && m.seq > trim.0
                && m.seq + trim.1 <= group_count
                && m.state == FrameState::Measured
        })
        .filter_map(value)
        .collect();
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// 由采集轮次与每帧特征得出要记下的一轮（按当前计划相机顺序）。
pub(super) fn calibration_round(
    doc: &RecipeDoc,
    round: &CaptureRoundV2,
    metrics: &[FrameMetrics],
) -> CalibrationRound {
    let cameras = doc.plan_cameras();
    let source: Vec<Option<usize>> = cameras
        .iter()
        .map(|c| round.cameras.iter().position(|r| &r.camera_id == c))
        .collect();
    // 单段配方的 x 均值另去掉本轮自己的起涂、收尾帧（D-SB-50）
    let edge = |end| match doc.segments.len() {
        1 => super::burst_skip::round_edge(
            doc,
            &round.round_id,
            round.group_count,
            &BTreeMap::new(),
            metrics,
            end,
        )
        .unwrap_or(0),
        _ => 0,
    };
    let trim = (
        u64::from(edge(super::burst_skip::SkipEnd::Head)).max(1),
        u64::from(edge(super::burst_skip::SkipEnd::Tail)).max(1),
    );
    CalibrationRound {
        round_id: round.round_id.clone(),
        standard: false,
        created_at: round.created_at,
        camera_config: source
            .iter()
            .map(|j| j.map_or(Value::Null, |j| round.cameras[j].camera_config.clone()))
            .collect(),
        group_count: round.group_count,
        camera_frames: source
            .iter()
            .map(|j| {
                j.and_then(|j| round.camera_frames.get(j).copied())
                    .unwrap_or(0)
            })
            .collect(),
        incomplete_groups: u32::try_from(round.incomplete_groups.len()).unwrap_or(u32::MAX),
        loss_alarms: u32::try_from(round.loss_alarms.len()).unwrap_or(u32::MAX),
        corridors: cameras.iter().map(|c| corridor_tag(doc, c)).collect(),
        x_means: (0..cameras.len())
            .map(|camera| round_mean(metrics, camera, round.group_count, trim, |m| m.x_mm))
            .collect(),
        edge_means: (0..cameras.len())
            .map(|camera| round_mean(metrics, camera, round.group_count, trim, |m| m.edge_mm))
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

/// 示教件测量里这台相机可用的测量量均值（AbsoluteX 取 x，ToEdge 取胶边距）：走廊与当前相同（走廊变了，不可比）。
fn standard_x(rounds: &[CalibrationRound], reference: &OffsetRef, tag: &Value) -> Vec<f64> {
    rounds
        .iter()
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

/// 用示教件的测量写单段配方直线段的胶偏参考（示教件兼作标准件，RB-3）：没配 offset_ref 但启用了胶偏时按默认参考写入；
/// ToEdge 没配找边参数时不取值。多段配方暂不计算 x_ref。
pub(super) fn apply_reference(doc: &mut RecipeDoc, rounds: &[CalibrationRound]) {
    let cameras = doc.plan_cameras();
    let Some(mut reference) = (doc.segments.len() == 1)
        .then(|| offset_reference(&doc.segments[0], &cameras))
        .flatten()
    else {
        return;
    };
    let values = match (reference.measure, &reference.edge) {
        (OffsetMeasure::ToEdge, None) => Vec::new(),
        _ => standard_x(rounds, &reference, &corridor_tag(doc, &reference.camera)),
    };
    (reference.x_ref, reference.spread) = calibration::x_ref(&values).unzip();
    doc.segments[0].offset_ref = Some(reference);
}

/// 示教件基准的视图：各段 N_exp、容差、x_ref 与定向提示。
pub(super) fn calibration_view(w: &Workspace) -> CalibrationView {
    let doc = &w.doc;
    let cameras = doc.plan_cameras();
    let single = doc.segments.len() == 1;
    let measured = super::pieces::teaching_measured(w);
    let rounds = &w.calibration.rounds;
    let segments = doc
        .segments
        .iter()
        .map(|segment| {
            let x_ref = segment.offset_ref.as_ref().and_then(|r| {
                Some(XRefView {
                    x_ref: r.x_ref?,
                    spread: r.spread?,
                })
            });
            let x_ref_pending = match (segment.kind, &x_ref, offset_reference(segment, &cameras)) {
                (SegmentKind::Corner, ..) | (_, Some(_), _) => None,
                (_, None, None) => Some("未启用胶偏检测".into()),
                (_, None, Some(_)) if !single => Some("多段配方暂不计算 x_ref".into()),
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
                    Some("按图纸胶边距判，不需要参考值".into())
                }
                (_, None, Some(_)) if !measured => Some("测量示教件后得出".into()),
                (_, None, Some(_)) => Some("示教件这台相机没有测成的帧：检查走廊后重新测量".into()),
            };
            SegmentCalibView {
                index: segment.index,
                kind: segment.kind,
                n_exp: segment.expected_frames,
                x_ref,
                x_ref_pending,
                hints: heading_hints(doc, segment, if measured { rounds } else { &[] }),
            }
        })
        .collect();
    let expected_total = doc.expected_total();
    CalibrationView {
        round_id: w
            .teaching_piece
            .as_ref()
            .filter(|t| rounds.iter().any(|r| r.round_id == t.round_id))
            .map(|t| t.round_id.clone()),
        measured,
        segments,
        expected_total,
    }
}

/// 示教件测量写下的每帧特征，按计划相机、seq；只读，试测页逐帧查看用。
#[tauri::command]
pub async fn workspace_calibration_round_features(
    app: AppHandle,
    id: String,
    round: String,
) -> Result<Vec<FrameMetrics>, String> {
    tauri::async_runtime::spawn_blocking(move || calibration_round_features(&app, &id, &round))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_calibration_round_overview(
    app: AppHandle,
    id: String,
    round: String,
) -> Result<Vec<FrameMetrics>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut frames = calibration_round_features(&app, &id, &round)?;
        for frame in &mut frames {
            frame.widths.clear();
            if let Some(evidence) = &mut frame.measurement_evidence {
                evidence.stations.clear();
            }
        }
        Ok(frames)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn calibration_round_features(
    app: &AppHandle,
    id: &str,
    round: &str,
) -> Result<Vec<FrameMetrics>, String> {
    let host = app.state::<WorkspaceHost>();
    let cameras = {
        let items = host.items.lock().unwrap();
        let w = items.get(id).ok_or("候选配置不存在")?;
        require_burst(&w.doc)?;
        if !w.calibration.rounds.iter().any(|r| r.round_id == round) {
            return Err(format!("采集轮次 {round} 还没有测量"));
        }
        w.doc.plan_cameras()
    };
    read_features(&host.dir(id).join(CALIBRATION_DIR).join(round), &cameras)
}

/// 示教件一帧按当前规则重测的结果（试测页逐帧查看）：所用段与走廊、测到的胶边（px，无胶站为 None）与帧指标。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationFrameView {
    /// 所用走廊的段；这台相机不检测这一帧所在段时为 None，不测
    pub segment: Option<u32>,
    pub corridor: Vec<[f32; 2]>,
    /// 搜索半宽（px），叠图画搜索带用
    pub search_half_px: Option<f64>,
    pub edges: Vec<Option<[[f32; 2]; 2]>>,
    /// 胶边距线（ToEdge，找到零件边的站：胶中心 → 零件边）；不找边时为空
    pub part_edges: Vec<[[f32; 2]; 2]>,
    pub metrics: FrameMetrics,
}

/// 用当前段走廊重测示教件里一台相机的一帧（只读，不改候选）：按示教件已确认的分段选走廊，
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
            .ok_or_else(|| format!("采集轮次 {round} 还没有测量"))?;
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
                part_edges: Vec::new(),
                metrics: frame_without_metrics(index, seq, FrameState::NotInspected, None),
            });
        };
        let recipe = w.doc.build()?;
        let graphs = vision::CorridorSet::build(&recipe, &teaching_sizes(&w)?)?
            .with_channels(&teaching_channels(&w));
        segment_frame_view(
            &app,
            &recipe,
            &graphs,
            (index, &camera),
            segment,
            (&round, seq),
            &format!("calibration-view-{id}-{round}-c{index}-{seq}"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 用第 `segment` 段的测量图（有逐帧定向时用定向图）量采集轮次 `round` 里计划相机 `index` 的第 `seq` 帧。
/// 有在途件或正在采集时拒绝，免得与生产争用测量。
fn segment_frame_view(
    app: &AppHandle,
    recipe: &crate::recipe::Recipe,
    graphs: &vision::CorridorSet,
    (index, camera): (usize, &str),
    segment: u32,
    (round, seq): (&str, u64),
    run: &str,
) -> Result<CalibrationFrameView, String> {
    let cycle = app.state::<CycleHost>();
    crate::camera::check_idle(&cycle)?;
    let capture = cycle.camera.recipe_capture();
    let image = crate::image_file::load(&capture.burst_frame_path(round, camera, seq)?)?;
    frame_view(
        app,
        recipe,
        graphs,
        (index, camera),
        segment,
        seq,
        &image,
        run,
    )
}

/// 计划相机 `index` 第 `segment` 段叠图用的示教走廊与搜索半宽（px）；相机下标越界时为空。
pub(super) fn corridor_overlay(
    recipe: &crate::recipe::Recipe,
    index: usize,
    segment: u32,
) -> (Vec<[f32; 2]>, Option<f64>) {
    let Some(inspect) = recipe.inspect.get(index) else {
        return (Vec::new(), None);
    };
    let corridor = inspect
        .segment_corridor(segment)
        .map(|s| s.corridor.clone())
        .unwrap_or_default();
    let search_half_px = inspect
        .mm_per_px
        .map(|scale| f64::from(recipe.segment_detect(segment).search_mm) / scale);
    (corridor, search_half_px)
}

/// 用第 `segment` 段的测量图（有逐帧定向时用定向图）量计划相机 `index` 的一帧图像，给出叠图用的走廊、搜索带与胶边。
#[allow(clippy::too_many_arguments)]
pub(super) fn frame_view(
    app: &AppHandle,
    recipe: &crate::recipe::Recipe,
    graphs: &vision::CorridorSet,
    (index, camera): (usize, &str),
    segment: u32,
    seq: u64,
    image: &FrameImage,
    run: &str,
) -> Result<CalibrationFrameView, String> {
    let (corridor, search_half_px) = corridor_overlay(recipe, index, segment);
    let graph = graphs
        .get(index, segment)
        .ok_or_else(|| format!("第 {segment} 段相机 {camera} 还没有示教走廊"))?;
    let engine = app
        .state::<vision::VisionHost>()
        .engine()
        .ok_or("lyFlow 核心库未加载")?;
    let measured = vision::measure_frame(&engine, graph, image, seq, &vision::unique_run_id(run))?;
    Ok(CalibrationFrameView {
        segment: Some(segment),
        corridor: if measured.line.is_empty() {
            corridor
        } else {
            measured.line.clone()
        },
        search_half_px,
        edges: measured.edges,
        part_edges: measured.part_edges,
        metrics: FrameMetrics {
            camera: index,
            seq,
            ..measured.metrics
        },
    })
}

/// 示教页试测一段：试测所用计划相机下标与这一段在示教轮次里的帧组范围（含两端）。
pub(super) struct TeachPreviewScope {
    pub index: usize,
    pub start: u64,
    pub end: u64,
}

/// 示教页试测（只读）的前置：段配方、段存在、相机是计划相机且检测这一段、这一段已保存走廊，
/// 轮次属于本候选且完整；范围同示教帧（`segment_range`）。
pub(super) fn teach_preview_scope(
    w: &Workspace,
    round: &CaptureRoundV2,
    camera: &str,
    segment: u32,
) -> Result<TeachPreviewScope, String> {
    require_burst(&w.doc)?;
    if !w.doc.segments.iter().any(|s| s.index == segment) {
        return Err(format!("没有第 {segment} 段"));
    }
    let cameras = w.doc.plan_cameras();
    let index = cameras
        .iter()
        .position(|c| c == camera)
        .ok_or_else(|| format!("相机 {camera} 不是段配方的计划相机"))?;
    if !crate::plan::detects(&w.doc.segments, segment, camera, &cameras) {
        return Err(format!("相机 {camera} 不是第 {segment} 段的检测相机"));
    }
    let saved = w
        .doc
        .inspect
        .iter()
        .find(|i| i.camera == camera)
        .is_some_and(|i| segment_corridor_saved(i, segment, &w.camera_teaching));
    if !saved {
        return Err(format!("第 {segment} 段相机 {camera} 还没有保存的走廊"));
    }
    super::pieces::require_teaching_round(w, &round.round_id)?;
    if round.recipe_id != w.doc.id || round.state != CaptureState::Complete {
        return Err(format!(
            "采集轮次 {} 不属于当前候选或尚未完整完成，不能用于试测",
            round.round_id
        ));
    }
    let (start, end) = segment_range(w, &round.round_id, round.group_count, segment)?;
    Ok(TeachPreviewScope { index, start, end })
}

fn frame_missing(round: &CaptureRoundV2, camera: &str, seq: u64) -> bool {
    round
        .cameras
        .iter()
        .position(|c| c.camera_id == camera)
        .is_none_or(|j| {
            round
                .missing_frames
                .iter()
                .any(|m| m.camera == j && m.seq == seq)
        })
}

/// 试测一段的每一帧（按 seq 升序覆盖整段）：缺帧记 Missing，帧组已屏蔽记 Unmeasured，`measure(seq)` 失败的帧记 Error，
/// 单帧失败不影响其余帧。
fn teach_preview_frames_operation(
    w: &Workspace,
    round: &CaptureRoundV2,
    camera: &str,
    scope: &TeachPreviewScope,
    mut measure: impl FnMut(u64) -> Result<FrameMetrics, String>,
    operation: Option<&crate::operation::Operation>,
) -> Result<Vec<FrameMetrics>, String> {
    if let Some(operation) = operation {
        operation.measuring((scope.end - scope.start + 1) as usize)?;
    }
    let index = scope.index;
    (scope.start..=scope.end)
        .map(|seq| {
            if let Some(operation) = operation {
                operation.check()?;
            }
            let metrics = if frame_missing(round, camera, seq) {
                frame_without_metrics(index, seq, FrameState::Missing, None)
            } else if capture_review::require_available(w, &round.round_id, seq).is_err() {
                frame_without_metrics(
                    index,
                    seq,
                    FrameState::Unmeasured,
                    Some("帧组已屏蔽".into()),
                )
            } else {
                match measure(seq) {
                    Ok(m) => FrameMetrics {
                        camera: index,
                        seq,
                        ..m
                    },
                    Err(error) => frame_without_metrics(index, seq, FrameState::Error, Some(error)),
                }
            };
            if let Some(operation) = operation {
                operation.advance(metrics.state)?;
            }
            Ok(metrics)
        })
        .collect()
}

/// 建测量图用的各计划相机示教帧尺寸：还没有示教帧的相机也没有走廊，不建图，尺寸不会被用到。
pub(super) fn taught_sizes(w: &Workspace) -> Vec<[u32; 2]> {
    w.doc
        .plan_cameras()
        .iter()
        .map(|camera| camera_frame_size(&w.camera_teaching, camera).unwrap_or_default())
        .collect()
}

fn workspace_clone(app: &AppHandle, id: &str) -> Result<Workspace, String> {
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    Ok(items.get(id).ok_or("候选配置不存在")?.clone())
}

/// 示教页用已保存的段走廊试测示教轮次 `round` 里这一段的第 `seq` 帧（只读，不改候选、标定与文件）。
#[tauri::command]
pub async fn workspace_teach_preview_frame(
    app: AppHandle,
    id: String,
    camera: String,
    segment: u32,
    round: String,
    seq: u64,
) -> Result<CalibrationFrameView, String> {
    preview_teaching_frame(app, id, None, camera, segment, round, seq).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn workspace_teach_preview_frame_at(
    app: AppHandle,
    id: String,
    revision: u64,
    instance_id: u64,
    camera: String,
    segment: u32,
    round: String,
    seq: u64,
) -> Result<CalibrationFrameView, String> {
    preview_teaching_frame(
        app,
        id,
        Some((revision, instance_id)),
        camera,
        segment,
        round,
        seq,
    )
    .await
}

async fn preview_teaching_frame(
    app: AppHandle,
    id: String,
    expected: Option<(u64, u64)>,
    camera: String,
    segment: u32,
    round: String,
    seq: u64,
) -> Result<CalibrationFrameView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let w = workspace_clone(&app, &id)?;
        if let Some((revision, instance_id)) = expected {
            w.expect(revision)?;
            if w.instance_id != instance_id {
                return Err("候选已重建，请重新加载后试测".into());
            }
        }
        let index = teach_frame_index(&app, &w, &camera, segment, &round, seq)?;
        let recipe = w.doc.build()?;
        let graphs = vision::CorridorSet::build(&recipe, &taught_sizes(&w))?
            .with_channels(&teaching_channels(&w));
        segment_frame_view(
            &app,
            &recipe,
            &graphs,
            (index, &camera),
            segment,
            (&round, seq),
            &format!("teach-preview-{id}-{round}-c{index}-{seq}"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 试测或调试示教件里第 `segment` 段的一帧：前置同 `teach_preview_scope`，帧组还要落在这一段内、不缺帧、没被屏蔽。返回计划相机下标。
fn teach_frame_index(
    app: &AppHandle,
    w: &Workspace,
    camera: &str,
    segment: u32,
    round: &str,
    seq: u64,
) -> Result<usize, String> {
    require_burst(&w.doc)?;
    let captured = app
        .state::<CycleHost>()
        .camera
        .recipe_capture()
        .get_burst(round)?;
    let scope = teach_preview_scope(w, &captured, camera, segment)?;
    if !(scope.start..=scope.end).contains(&seq) {
        return Err(format!(
            "第 {seq} 帧组不在第 {segment} 段（这一轮为帧组 {}–{}）",
            scope.start, scope.end
        ));
    }
    if frame_missing(&captured, camera, seq) {
        return Err(format!("相机 {camera} 缺第 {seq} 帧组的图像"));
    }
    capture_review::require_available(w, round, seq)?;
    Ok(scope.index)
}

/// 流程调试页（LF-6）的一帧：候选的测量图、计划相机下标与这一帧原图。有在途件或正在采集时拒绝，免得与生产争用测量。
pub(crate) fn flow_debug_frame(
    app: &AppHandle,
    id: &str,
    camera: &str,
    segment: u32,
    round: &str,
    seq: u64,
) -> Result<(vision::CorridorSet, usize, FrameImage), String> {
    let w = workspace_clone(app, id)?;
    let index = teach_frame_index(app, &w, camera, segment, round, seq)?;
    let recipe = w.doc.build()?;
    let graphs = vision::CorridorSet::build(&recipe, &taught_sizes(&w))?
        .with_channels(&teaching_channels(&w));
    let cycle = app.state::<CycleHost>();
    crate::camera::check_idle(&cycle)?;
    let image = crate::image_file::load(
        &cycle
            .camera
            .recipe_capture()
            .burst_frame_path(round, camera, seq)?,
    )?;
    Ok((graphs, index, image))
}

/// 示教页试测一段的结果：示教轮次、这一段的帧组范围（含两端）与每帧指标（按 seq 升序）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachPreviewSegment {
    pub round: String,
    pub start: u64,
    pub end: u64,
    pub frames: Vec<FrameMetrics>,
}

/// 区间测量保留完整配方和全段边界，from/to 只限定实际读取和测量的帧组，均含端点。
pub struct TeachPreviewRange {
    pub round: String,
    pub start: u64,
    pub end: u64,
    pub from: u64,
    pub to: u64,
    pub frames: Vec<FrameMetrics>,
    pub revision: u64,
    pub instance_id: u64,
    pub doc: RecipeDoc,
}

#[allow(clippy::too_many_arguments)]
pub fn measure_teaching_range(
    w: &Workspace,
    round: &CaptureRoundV2,
    camera: &str,
    segment: u32,
    from: Option<u64>,
    to: Option<u64>,
    expected_revision: Option<u64>,
    measure: impl FnMut(u64) -> Result<FrameMetrics, String>,
) -> Result<TeachPreviewRange, String> {
    measure_teaching_range_operation(
        w,
        round,
        camera,
        segment,
        from,
        to,
        expected_revision,
        measure,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn measure_teaching_range_operation(
    w: &Workspace,
    round: &CaptureRoundV2,
    camera: &str,
    segment: u32,
    from: Option<u64>,
    to: Option<u64>,
    expected_revision: Option<u64>,
    measure: impl FnMut(u64) -> Result<FrameMetrics, String>,
    operation: Option<&crate::operation::Operation>,
) -> Result<TeachPreviewRange, String> {
    if let Some(revision) = expected_revision {
        w.expect(revision)?;
    }
    let scope = teach_preview_scope(w, round, camera, segment)?;
    let window = preview_window(&scope, from, to)?;
    let frames = teach_preview_frames_operation(w, round, camera, &window, measure, operation)?;
    Ok(TeachPreviewRange {
        round: round.round_id.clone(),
        start: scope.start,
        end: scope.end,
        from: window.start,
        to: window.end,
        frames,
        revision: w.revision,
        instance_id: w.instance_id,
        doc: w.doc.clone(),
    })
}

fn preview_window(
    scope: &TeachPreviewScope,
    from: Option<u64>,
    to: Option<u64>,
) -> Result<TeachPreviewScope, String> {
    let start = from.unwrap_or(scope.start).max(scope.start);
    let end = to.unwrap_or(scope.end).min(scope.end);
    if start > end {
        return Err(format!(
            "测量范围与这一段的帧组 {}–{} 没有交集",
            scope.start, scope.end
        ));
    }
    Ok(TeachPreviewScope {
        index: scope.index,
        start,
        end,
    })
}

/// 示教页全段接口兼容；MCP 的区间入口与它共用测量流程。
#[tauri::command]
pub async fn workspace_teach_preview_segment(
    app: AppHandle,
    id: String,
    camera: String,
    segment: u32,
    round: String,
    task_id: Option<String>,
) -> Result<TeachPreviewSegment, String> {
    let result =
        preview_range_operation(app, id, camera, segment, round, None, None, None, task_id).await?;
    Ok(TeachPreviewSegment {
        round: result.round,
        start: result.start,
        end: result.end,
        frames: result.frames,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn workspace_teach_preview_range(
    app: AppHandle,
    id: String,
    camera: String,
    segment: u32,
    round: String,
    from: Option<u64>,
    to: Option<u64>,
    expected_revision: Option<u64>,
) -> Result<TeachPreviewRange, String> {
    preview_range_operation(
        app,
        id,
        camera,
        segment,
        round,
        from,
        to,
        expected_revision,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn preview_range_operation(
    app: AppHandle,
    id: String,
    camera: String,
    segment: u32,
    round: String,
    from: Option<u64>,
    to: Option<u64>,
    expected_revision: Option<u64>,
    task_id: Option<String>,
) -> Result<TeachPreviewRange, String> {
    let operation = crate::operation::Operation::start(&app, task_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let w = workspace_clone(&app, &id)?;
            if let Some(revision) = expected_revision {
                w.expect(revision)?;
            }
            require_burst(&w.doc)?;
            let cycle = app.state::<CycleHost>();
            let index = cycle.camera.recipe_capture().burst_frame_index(&round)?;
            let captured = index.round();
            let scope = teach_preview_scope(&w, &captured, &camera, segment)?;
            preview_window(&scope, from, to)?;
            let recipe = w.doc.build()?;
            let graphs = vision::CorridorSet::build(&recipe, &taught_sizes(&w))?
                .with_channels(&teaching_channels(&w));
            if graphs.get(scope.index, segment).is_none() {
                return Err(format!("第 {segment} 段相机 {camera} 还没有示教走廊"));
            }
            crate::camera::check_idle(&cycle)?;
            let engine = app
                .state::<vision::VisionHost>()
                .engine()
                .ok_or("lyFlow 核心库未加载")?;
            let result = measure_teaching_range_operation(
                &w,
                &captured,
                &camera,
                segment,
                from,
                to,
                expected_revision,
                |seq| {
                    crate::camera::check_idle(&cycle)?;
                    let image = crate::image_file::load(&index.path(&camera, seq)?)?;
                    let graph = graphs.get(scope.index, segment).ok_or("走廊测量图缺失")?;
                    vision::measure_frame(
                        &engine,
                        graph,
                        &image,
                        seq,
                        &format!("teach-preview-{id}-{round}-c{}-{seq}", scope.index),
                    )
                    .map(|measured| measured.metrics)
                },
                Some(operation),
            )?;
            operation.stage("summarizing")?;
            Ok(result)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// 整体替换候选的段列表；拒绝直接改 N_exp、标定统计与 x_ref，N_exp 随后按示教件分段重算。
#[tauri::command]
pub fn workspace_update_segments(
    app: AppHandle,
    id: String,
    revision: u64,
    segments: Vec<PathSegment>,
) -> Result<WorkspaceView, String> {
    let saved = app
        .state::<WorkspaceHost>()
        .update_segments(&id, revision, None, segments)?;
    view(&app, saved)
}

pub(super) fn workspace_update_segments_at(
    app: AppHandle,
    id: String,
    revision: u64,
    instance_id: u64,
    segments: Vec<PathSegment>,
) -> Result<WorkspaceView, String> {
    let saved =
        app.state::<WorkspaceHost>()
            .update_segments(&id, revision, Some(instance_id), segments)?;
    view(&app, saved)
}

impl WorkspaceHost {
    pub fn update_segments(
        &self,
        id: &str,
        revision: u64,
        expected_instance_id: Option<u64>,
        segments: Vec<PathSegment>,
    ) -> Result<Workspace, String> {
        let mut items = self.items.lock().unwrap();
        let mut w = items.get(id).ok_or("候选配置不存在")?.clone();
        w.expect(revision)?;
        if expected_instance_id.is_some_and(|instance| w.instance_id != instance) {
            return Err("候选已重建，请重新加载后修改段配置".into());
        }
        update_segments(&mut w, segments)?;
        self.save(&w)?;
        items.insert(id.to_string(), w.clone());
        Ok(w)
    }
}

// ───────────── WorkspaceView.plan ─────────────

pub(super) fn camera_teaching_views(w: &Workspace) -> Vec<CameraTeachingView> {
    let open = untaught_pairs(w);
    let cameras = w.doc.plan_cameras();
    w.doc
        .inspect
        .iter()
        .map(|inspect| {
            let nozzle = crate::heading::nozzle(&w.doc.segments, &cameras, inspect);
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
                calib_mm_per_px: None,
                ready: inspect.mm_per_px.is_some()
                    && !open.iter().any(|(camera, _)| camera == &inspect.camera),
                segments,
                nozzle,
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

/// 段配方的进度：示教件 → 分段 → 走廊示教 → 试测 → 验证（可选，D-SB-46）→ 发布（RB）。只读候选，不访问设备。
pub(super) fn progress_v2(
    w: &Workspace,
    teaching: &[CameraTeachingView],
    production_version: Option<u32>,
) -> ProgressV2 {
    let doc = &w.doc;
    let piece = w.teaching_piece.as_ref();
    let capture = StepV2 {
        id: StepIdV2::Capture,
        label: "示教件",
        optional: false,
        status: if piece.is_some() {
            StepStatus::Done
        } else {
            StepStatus::NotStarted
        },
        detail: piece.map_or_else(|| "等待选示教件".into(), |p| format!("示教件 SN {}", p.sn)),
        blockers: if piece.is_some() {
            Vec::new()
        } else {
            vec![BlockerV2 {
                message: "选示教件".into(),
                target: TargetV2::Capture,
            }]
        },
    };
    let segmented = super::pieces::teaching_counts(w).is_some();
    let straight = doc
        .segments
        .iter()
        .filter(|s| s.kind == SegmentKind::Straight)
        .count();
    let segments = StepV2 {
        id: StepIdV2::Segments,
        label: "分段",
        optional: false,
        status: if segmented {
            StepStatus::Done
        } else {
            StepStatus::NotStarted
        },
        detail: format!(
            "{} 段（直线 {straight}、弯道 {}）",
            doc.segments.len(),
            doc.segments.len() - straight,
        ),
        blockers: match (piece, segmented) {
            (None, _) => capture.blockers.clone(),
            (Some(_), false) => vec![BlockerV2 {
                message: "在示教件上确认分段".into(),
                target: TargetV2::Segments,
            }],
            (Some(_), true) => Vec::new(),
        },
    };
    // 走廊示教在示教件分段确认之后
    let need_segments = || segments.blockers.clone();
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
        let status = if !segmented {
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
            blockers: if segmented { blockers } else { need_segments() },
        }
    };
    // 试测（可选，不是发布前提）：测量示教件得出胶偏参考、起涂收尾建议与定向提示，并用历史件试测
    let calibration = {
        let measured = super::pieces::teaching_measured(w);
        StepV2 {
            id: StepIdV2::Calibration,
            label: "试测",
            optional: true,
            status: if measured {
                StepStatus::Done
            } else {
                StepStatus::NotStarted
            },
            detail: if measured {
                "示教件已按当前走廊测量".into()
            } else {
                "可选：测量示教件、用历史件试测".into()
            },
            blockers: Vec::new(),
        }
    };
    let active = match &w.publish {
        PublishState::Active {
            version, revision, ..
        } if *revision == w.revision => Some(*version),
        _ => None,
    };
    // D-SB-46：独立样本验证可选，不是发布前提；发布只要示教件、分段、走廊示教完成（设备与生产版本在发布时当场核对）
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
        let open: Vec<BlockerV2> = [&capture, &segments, &corridor, &calibration]
            .into_iter()
            .filter(|s| s.status != StepStatus::Done && !s.optional)
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
                format!("示教完成后发布（当前{production}）"),
                open,
            ),
        };
        StepV2 {
            id: StepIdV2::Publish,
            label: "发布与生效",
            optional: false,
            status,
            detail,
            blockers,
        }
    };
    let next = if capture.status != StepStatus::Done {
        Some(NextV2 {
            step: StepIdV2::Capture,
            label: "选示教件".into(),
            target: TargetV2::Capture,
        })
    } else if let Some(open) = [&segments, &corridor, &calibration]
        .into_iter()
        .find(|s| s.status != StepStatus::Done && !s.optional)
    {
        let target = open
            .blockers
            .first()
            .map_or(TargetV2::Calibration, |b| b.target.clone());
        let label = match &target {
            TargetV2::Corridor {
                camera,
                segment: Some(segment),
            } => format!("示教第 {segment} 段相机 {camera} 的走廊"),
            TargetV2::Corridor {
                camera,
                segment: None,
            } => format!("设置相机 {camera} 的像素当量"),
            TargetV2::Segments => "在示教件上确认分段".into(),
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
            StepStatus::InProgress => Some(NextV2 {
                step: StepIdV2::Publish,
                label: "等待工件边界生效".into(),
                target: TargetV2::Validation,
            }),
            StepStatus::Blocked => Some(NextV2 {
                step: StepIdV2::Publish,
                label: "处理发布失败".into(),
                target: TargetV2::Validation,
            }),
            _ => Some(NextV2 {
                step: StepIdV2::Publish,
                label: "发布生产配方".into(),
                target: TargetV2::Validation,
            }),
        }
    };
    ProgressV2 {
        steps: vec![
            capture,
            segments,
            corridor,
            calibration,
            validation,
            publish,
        ],
        next,
    }
}

/// 段配方的计划部分（决策记录 5.1 / 5.2 的 WorkspacePlanView）。`production` 为生产配方（可能是旧拍照点配方或还没发布）。
pub(super) fn plan_view(
    app: &AppHandle,
    w: &Workspace,
    layout: &Recipe,
    production: Option<&Recipe>,
    production_version: Option<u32>,
) -> WorkspacePlanView {
    let doc = &w.doc;
    let mut teaching = camera_teaching_views(w);
    let configs = app.state::<CycleHost>().camera.configs();
    for (item, inspect) in teaching.iter_mut().zip(&doc.inspect) {
        item.calib_mm_per_px = configs
            .iter()
            .find(|c| c.id == inspect.camera)
            .and_then(|config| calib_scale(app, inspect, config.source).ok().flatten());
    }
    let production_export = production
        .filter(|r| r.is_burst())
        .and_then(|r| PlanExport::from_recipe(r).ok());
    let (candidate_export, candidate_export_error) = match PlanExport::from_recipe(layout) {
        Ok(export) => (Some(export), None),
        Err(error) => (None, Some(error)),
    };
    let expected_total = doc.expected_total();
    WorkspacePlanView {
        segments: doc.segments.clone(),
        cameras: doc.plan_cameras(),
        segment_defaults: doc.segment_defaults.clone(),
        part_calib: doc.part_calib.clone(),
        expected_total,
        calibrated: expected_total.is_some(),
        calibration: Some(calibration_view(w)),
        shot_count_changed: production_export.as_ref().and_then(|e| e.expected_total)
            != expected_total,
        production_export,
        candidate_export,
        candidate_export_error,
        progress: progress_v2(w, &teaching, production_version),
        camera_teaching: teaching,
        pieces: super::pieces::plan_pieces(app, w),
    }
}

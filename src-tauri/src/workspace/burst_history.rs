//! 段配方的历史原图、复测与对照（D3）：原图与复测都按 (计划相机下标, 帧组序号) 读（决策记录 D-SB-29），用 burst_judge 重判。
//!
//! 只服务段配方记录（`PartDetail.burst` 非空）。旧拍照点记录仍走 workspace.rs 里按 (拍照点, 视角) 的 `history_image`、
//! `measure_images` 与 `reproduce::measure`，行为不变。
//! - 原图：生产记录读录制目录（`recorded::load_frame_verified`，核对 part.json 的帧身份）；候选离线调试记录读受管样本集
//!   （`history_samples::BurstSamples`，每台相机按清单顺序的拍照点即帧序列）。
//! - 复测：缺帧仍是缺帧、不读图；读图或测量失败记测量失败，只算检测不完整（D-SB-13），不判整件 ERR。
//! - 区域：按候选（或原发布包）的段重新划分，本轮只有 `Unsegmented`；按规则重判沿用记录里的区域（与历史重判同一口径）。

use super::*;
use crate::burst_judge::{self, BurstJudgement, FrameMetrics, FrameState};
use crate::frame_group::{FrameGroup, LossAlarm};
use crate::plan::PathSegment;
use crate::segmentation::{resolve_regions, Proportional, Region, SegInput, Segmenter};
use crate::store::{FrameRecord, PartDetail, PartDetailBurst, RecordSource};

/// 区域划分的置信度门槛，与在线节拍（cycle/burst.rs）相同：比例分段器在帧数容差内给 1、之外给 0。
const MIN_CONFIDENCE: f32 = 0.5;

/// 段配方的对照结果（TS `Comparison.burst`）：重判所用的段、计划相机、ΣN_exp 与区域判定。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonBurst {
    pub segments: Vec<PathSegment>,
    pub cameras: Vec<String>,
    pub expected_total: Option<u32>,
    pub judgement: BurstJudgement,
}

impl ComparisonBurst {
    fn of(recipe: &Recipe, judgement: BurstJudgement) -> Self {
        Self { segments: recipe.path_segments.clone(), cameras: recipe.plan_cameras(), expected_total: recipe.expected_total(), judgement }
    }
}

/// 结论摘要（旧 `Judgement` 的形状，决策记录 5.1）：verdict / plcCode / faultCode / reason 与区域判定相同，segments、gaps 为空。
pub(super) fn summary(judgement: &BurstJudgement) -> judge::Judgement {
    judge::Judgement {
        verdict: judgement.verdict,
        plc_code: judgement.plc_code,
        fault_code: judgement.fault_code,
        reason: judgement.reason.clone(),
        segments: Vec::new(),
        gaps: Vec::new(),
    }
}

/// 由每帧指标还原帧组（与在线 Grouper 同一口径）：到了的帧（不论测没测成）算 present，缺帧不算；帧组数 = 最大 seq。
pub(super) fn frame_groups(metrics: &[FrameMetrics], cameras: usize) -> Vec<FrameGroup> {
    let total = metrics.iter().map(|m| m.seq).max().unwrap_or(0);
    (1..=total).map(|seq| {
        let present: Vec<bool> = (0..cameras)
            .map(|camera| metrics.iter().any(|m| m.camera == camera && m.seq == seq && m.state != FrameState::Missing))
            .collect();
        FrameGroup { seq, incomplete: present.contains(&false), present }
    }).collect()
}

/// 按给定的段比例分段（与生产同一规则，D-SB-40）重新划分区域后判一件；丢帧告警原样带出。
pub(super) fn judge_frames(
    segments: &[PathSegment],
    boundary_guard: u32,
    expected_total: Option<u32>,
    cameras: &[String],
    metrics: &[FrameMetrics],
    loss: &[LossAlarm],
) -> BurstJudgement {
    let groups = frame_groups(metrics, cameras.len());
    let outcome = Proportional.segment(&SegInput {
        groups: &groups,
        features: metrics,
        segments,
    });
    let regions = resolve_regions(
        segments,
        &outcome,
        groups.len() as u64,
        MIN_CONFIDENCE,
        boundary_guard,
    );
    burst_judge::judge(
        segments,
        expected_total,
        &regions,
        &groups,
        metrics,
        cameras,
        loss,
    )
}

/// 历史帧记录按 (camera, seq) 逐帧重测：缺帧仍记缺帧、不读原图；其余帧交给 `measure`，失败记测量失败。
/// 结果的帧身份一律取帧记录的 (camera, seq)。
pub(super) fn remeasure(frames: &[FrameRecord], mut measure: impl FnMut(&FrameRecord) -> Result<FrameMetrics, String>) -> Vec<FrameMetrics> {
    frames.iter().map(|frame| {
        if frame.status == FrameState::Missing {
            return burst::frame_without_metrics(frame.camera, frame.seq, FrameState::Missing, None);
        }
        match measure(frame) {
            Ok(metrics) => FrameMetrics { camera: frame.camera, seq: frame.seq, ..metrics },
            Err(error) => burst::frame_without_metrics(frame.camera, frame.seq, FrameState::Error, Some(error)),
        }
    }).collect()
}

/// 记录里存的每帧指标；没存指标的帧按帧状态补一条没有测量值的指标。
fn stored_metrics(frames: &[FrameRecord]) -> Vec<FrameMetrics> {
    frames.iter().map(|frame| match &frame.metrics {
        Some(metrics) => FrameMetrics { camera: frame.camera, seq: frame.seq, ..metrics.clone() },
        None => burst::frame_without_metrics(frame.camera, frame.seq, frame.status, None),
    }).collect()
}

/// 录制原图齐：录制证据完整，且除缺帧外每帧都有原图引用（与前端历史详情 `rawComplete` 同一口径）。
pub(super) fn raw_complete(detail: &PartDetail, burst: &PartDetailBurst) -> bool {
    detail.recording.available && burst.frames.iter().all(|f| f.status == FrameState::Missing || f.raw_file.is_some())
}

/// 段配方候选离线调试记录的样本原图。
fn burst_samples(app: &AppHandle, detail: &PartDetail) -> Result<history_samples::BurstSamples, String> {
    let revision = detail.summary.recipe_revision.as_deref().ok_or("历史配方快照身份缺失")?;
    let recipe = app.state::<Store>().recipe_snapshot(revision)?.ok_or("历史配方快照缺失，不能确定离线样本原图")?;
    history_samples::BurstSamples::open(&crate::storage::root(app), detail, &recipe)
}

/// 段配方历史件第 camera 台计划相机第 seq 帧组的原图：生产记录读录制目录，候选离线调试记录读受管样本集。
pub(super) fn frame_image(app: &AppHandle, detail: &PartDetail, camera: usize, seq: u64) -> Result<FrameImage, String> {
    let burst = detail.burst.as_ref().ok_or("这条历史是旧拍照点记录，请按拍照点读原图")?;
    let frame = burst.frames.iter().find(|f| f.camera == camera && f.seq == seq).ok_or("没有这一帧的记录")?;
    if frame.status == FrameState::Missing { return Err("这台相机本帧组缺帧，没有原图".into()); }
    if detail.summary.source == RecordSource::OfflineCandidate {
        return burst_samples(app, detail)?.load(camera, seq);
    }
    let camera_id = burst.cameras.get(camera).ok_or("相机下标超出本件计划相机")?;
    let cycle_id = detail.summary.cycle_id.as_deref().ok_or("历史记录没有 cycleId，不能确定对应原图")?;
    recorded::load_frame_verified(app.state::<CycleHost>().recorders.root(detail.summary.source), cycle_id, camera_id, frame)
}

/// 段配方历史件一帧的录制原图（决策记录 5.4，camera 为计划相机下标）；未录制、录制队满丢弃或缺帧时报错。
#[tauri::command]
pub fn workspace_record_frame_image(app: AppHandle, history_id: i64, camera: usize, seq: u64) -> Result<tauri::ipc::Response, String> {
    let detail = app.state::<Store>().detail(history_id)?;
    Ok(preview_response(&frame_image(&app, &detail, camera, seq)?))
}

/// 用候选当前的走廊量帧：每台计划相机一条走廊测量图，按它冻结的示教帧尺寸建（与加入标定同一做法）。
pub(super) struct CandidateMeasurer {
    engine: Arc<vision::Engine>,
    graphs: Vec<vision::CorridorGraph>,
    run: String,
}

impl CandidateMeasurer {
    pub(super) fn new(app: &AppHandle, w: &Workspace, recipe: &Recipe) -> Result<Self, String> {
        let graphs = recipe
            .inspect
            .iter()
            .map(|inspect| {
                let teaching = super::burst::default_teaching(&w.camera_teaching, &inspect.camera)
                    .ok_or_else(|| {
                        format!("相机 {} 还没有选示教帧：先完成走廊示教", inspect.camera)
                    })?;
                vision::plan_corridor(inspect, recipe, teaching.frame.size)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let engine = app
            .state::<vision::VisionHost>()
            .engine()
            .ok_or("lyFlow 核心库未加载")?;
        Ok(Self {
            engine,
            graphs,
            run: vision::unique_run_id(&format!("candidate-{}", recipe.revision_id)),
        })
    }

    pub(super) fn measure(&self, camera: usize, seq: u64, image: &FrameImage) -> Result<FrameMetrics, String> {
        let graph = self.graphs.get(camera).ok_or("候选没有这台相机的走廊测量图")?;
        vision::measure_frame(&self.engine, graph, image, seq, &format!("{}-c{camera}-{seq}", self.run)).map(|m| m.metrics)
    }
}

/// `workspace_compare` 的段配方分支：`raw` 从原图用候选走廊复测整件；否则用记录里存的每帧指标按候选规则重判
/// （要求每帧指标的测量定义与计划身份不变，见 `store::same_measurement_layout`）。
pub(super) fn compare(app: &AppHandle, w: &Workspace, recipe: &Recipe, detail: &PartDetail, original: &Recipe, raw: bool) -> Result<(judge::Judgement, ComparisonBurst), String> {
    let burst = detail.burst.as_ref().ok_or("这条历史是旧拍照点记录，不能用段配方候选对照")?;
    if !original.is_burst() || !recipe.is_burst() {
        return Err("段配方记录只能与段配方候选对照".into());
    }
    let cameras = recipe.plan_cameras();
    let judgement = if raw {
        if original.plan_cameras() != cameras { return Err("历史原图的计划相机与候选不同；请重新采集样本".into()); }
        if detail.summary.source != RecordSource::OfflineCandidate && !raw_complete(detail, burst) {
            return Err("原图未完整保留，无法完成整件复测".into());
        }
        let measurer = CandidateMeasurer::new(app, w, recipe)?;
        let metrics = remeasure(&burst.frames, |frame| {
            measurer.measure(
                frame.camera,
                frame.seq,
                &frame_image(app, detail, frame.camera, frame.seq)?,
            )
        });
        judge_frames(
            &recipe.path_segments,
            recipe.boundary_guard,
            recipe.expected_total(),
            &cameras,
            &metrics,
            &burst.judgement.alarms,
        )
    } else {
        if !crate::store::same_measurement_layout(original, recipe) {
            return Err("保存的每帧指标与候选的走廊、当量、检测参数或计划身份不同，不能复用；请从原图复测".into());
        }
        if burst.judgement.verdict == Verdict::ErrInspect || burst.frames.is_empty() {
            return Err("原始结论为 ERR 或没有逐帧指标，没有可重判的区域；缺失的测量不能靠修改规则补齐".into());
        }
        let regions: Vec<Region> = burst.judgement.regions.iter().map(|r| r.region.clone()).collect();
        let metrics = stored_metrics(&burst.frames);
        burst_judge::judge(&recipe.path_segments, recipe.expected_total(), &regions, &frame_groups(&metrics, cameras.len()), &metrics, &cameras, &burst.judgement.alarms)
    };
    Ok((summary(&judgement), ComparisonBurst::of(recipe, judgement)))
}

/// `workspace_compare_original` 的段配方分支：按原发布包逐帧重现（见 `reproduce::measure_burst`）。
pub(super) fn reproduce_original(app: &AppHandle, detail: &PartDetail, prepared: &crate::production::Prepared, cycle_id: &str) -> Result<(judge::Judgement, ComparisonBurst), String> {
    let burst = detail.burst.as_ref().ok_or("这条历史是旧拍照点记录，不能按相机帧重现")?;
    if !raw_complete(detail, burst) { return Err("原图未完整保留，不能重现原发布测量；请查看录制状态".into()); }
    let judgement = reproduce::measure_burst(prepared, &burst.frames, &burst.judgement.alarms, cycle_id,
        |frame| frame_image(app, detail, frame.camera, frame.seq))?;
    Ok((summary(&judgement), ComparisonBurst::of(&prepared.recipe, judgement)))
}

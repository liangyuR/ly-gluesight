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
use crate::segmentation::{
    frame_corridors, resolve_regions, segment_part, Proportional, Region, SegInput, Segmenter,
};
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
    /// 这次对照用的每帧指标（复测为重测结果，按规则重判为记录里存的指标）；之前保存的对照没有这一项
    #[serde(default)]
    pub frames: Vec<FrameMetrics>,
    /// 这次对照建走廊测量图用的各计划相机示教帧尺寸（候选对照取当时的走廊示教）：叠图按它重建，不依赖候选现在的示教；
    /// 按原发布包重现的对照与之前保存的对照为空
    #[serde(default)]
    pub sizes: Vec<[u32; 2]>,
    /// 逐帧结论（帧格颜色）：按这次对照的段、区域与逐帧指标算出（`burst_judge::frame_verdicts`），读出时重算
    #[serde(default, skip_deserializing)]
    pub frame_verdicts: Vec<burst_judge::FrameVerdict>,
    /// 从原图复测时没有可用走廊、没测的段（D-SB-57）；其余对照与之前保存的对照为空
    #[serde(default)]
    pub unmeasured: Vec<UnmeasuredSegment>,
}

impl ComparisonBurst {
    /// 按这次对照的段、区域与逐帧指标算出逐帧结论。
    pub(super) fn fill_verdicts(&mut self) {
        let regions: Vec<Region> = self
            .judgement
            .regions
            .iter()
            .map(|r| r.region.clone())
            .collect();
        self.frame_verdicts =
            burst_judge::frame_verdicts(&self.segments, &regions, &self.frames, &self.cameras);
    }

    fn of(
        recipe: &Recipe,
        judgement: BurstJudgement,
        frames: Vec<FrameMetrics>,
        sizes: Vec<[u32; 2]>,
    ) -> Self {
        let mut burst = Self {
            segments: recipe.path_segments.clone(),
            cameras: recipe.plan_cameras(),
            expected_total: recipe.expected_total(),
            judgement,
            frames,
            sizes,
            frame_verdicts: Vec::new(),
            unmeasured: Vec::new(),
        };
        burst.fill_verdicts();
        burst
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
    boundary_refine: bool,
    expected_total: Option<u32>,
    cameras: &[String],
    metrics: &[FrameMetrics],
    loss: &[LossAlarm],
) -> BurstJudgement {
    let groups = frame_groups(metrics, cameras.len());
    let outcome = segment_part(
        &SegInput {
            groups: &groups,
            segments,
        },
        metrics,
        boundary_refine,
    );
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

/// 补回维护试判件丢失的区域判定（MM-10）：按记录的配方快照、已存每帧指标与丢帧告警重新划分区域并判定（与在线同一规则）；
/// 结论与摘要相同才写回，回写码沿用摘要的 0。返回补回件数与跳过的原因。
pub(crate) fn repair_shadow_regions(store: &Store) -> Result<(usize, Vec<String>), String> {
    let (mut repaired, mut skipped) = (0, Vec::new());
    for id in store.shadow_parts_without_regions()? {
        let detail = match store.detail(id) {
            Ok(detail) => detail,
            Err(error) => {
                skipped.push(format!("记录 #{id} 读取失败，未补回：{error}"));
                continue;
            }
        };
        let Some(burst) = detail.burst.as_ref() else {
            continue;
        };
        let s = &detail.summary;
        let Some(recipe) = s
            .recipe_revision
            .as_deref()
            .map(|r| store.recipe_snapshot(r))
            .transpose()?
            .flatten()
        else {
            skipped.push(format!("记录 #{id} 的原配方快照缺失"));
            continue;
        };
        let metrics: Vec<FrameMetrics> = burst
            .frames
            .iter()
            .filter_map(|f| f.metrics.clone())
            .collect();
        let judged = judge_frames(
            &recipe.path_segments,
            recipe.boundary_guard,
            recipe.boundary_refine,
            recipe.expected_total(),
            &burst.cameras,
            &metrics,
            &burst.judgement.alarms,
        );
        if (judged.verdict, judged.fault_code, judged.reason.as_str())
            != (s.verdict, s.fault_code, s.reason.as_str())
        {
            skipped.push(format!(
                "记录 #{id} 重判结论（{}）与原结论（{}）不同，未补回",
                judged.reason, s.reason
            ));
            continue;
        }
        let judgement = BurstJudgement {
            plc_code: s.plc_code,
            ..judged
        };
        let regions: Vec<(usize, u64, Option<usize>)> = burst
            .frames
            .iter()
            .map(|f| {
                (
                    f.camera,
                    f.seq,
                    judgement
                        .regions
                        .iter()
                        .position(|r| (r.region.first_seq..=r.region.last_seq).contains(&f.seq)),
                )
            })
            .collect();
        if store.repair_burst_regions(id, &judgement, &regions)? {
            repaired += 1;
        }
    }
    Ok((repaired, skipped))
}

/// 按生产规则从原图复测一件（D-SB-45）：由各帧到没到（`frames` 为 (计划相机下标, seq, 到了)）得出帧组数，按比例分段得出区域，
/// 再按 `frame_corridors` 逐帧选走廊交给 `measure(相机, seq, 段号)`。判定用不到的帧记 NotInspected、不读图；缺帧仍记缺帧；
/// 读图或测量失败记测量失败。结果的帧身份一律取输入的 (camera, seq)。
pub(super) fn remeasure_by_rule(
    segments: &[PathSegment],
    boundary_guard: u32,
    cameras: &[String],
    frames: &[(usize, u64, bool)],
    mut measure: impl FnMut(usize, u64, u32) -> Result<FrameMetrics, String>,
) -> Vec<FrameMetrics> {
    let arrived: Vec<FrameMetrics> = frames
        .iter()
        .map(|&(camera, seq, arrived)| {
            burst::frame_without_metrics(
                camera,
                seq,
                if arrived {
                    FrameState::NotInspected
                } else {
                    FrameState::Missing
                },
                None,
            )
        })
        .collect();
    let groups = frame_groups(&arrived, cameras.len());
    let total = groups.len() as u64;
    let outcome = Proportional.segment(&SegInput {
        groups: &groups,
        segments,
    });
    let regions = resolve_regions(segments, &outcome, total, MIN_CONFIDENCE, boundary_guard);
    let choice = frame_corridors(segments, cameras, &regions, &outcome, total);
    frames
        .iter()
        .zip(arrived)
        .map(|(&(camera, seq, present), skipped)| {
            let segment = choice
                .get(camera)
                .and_then(|c| c.get((seq as usize).wrapping_sub(1)))
                .copied()
                .flatten();
            match (present, segment) {
                (false, _) | (true, None) => skipped,
                (true, Some(segment)) => match measure(camera, seq, segment) {
                    Ok(metrics) => FrameMetrics {
                        camera,
                        seq,
                        ..metrics
                    },
                    Err(error) => FrameMetrics {
                        corridor: Some(segment),
                        ..burst::frame_without_metrics(camera, seq, FrameState::Error, Some(error))
                    },
                },
            }
        })
        .collect()
}

/// 历史帧记录按生产规则重测（见 `remeasure_by_rule`）。
pub(super) fn remeasure(
    recipe: &Recipe,
    frames: &[FrameRecord],
    mut measure: impl FnMut(&FrameRecord, u32) -> Result<FrameMetrics, String>,
) -> Vec<FrameMetrics> {
    let keys: Vec<(usize, u64, bool)> = frames
        .iter()
        .map(|f| (f.camera, f.seq, f.status != FrameState::Missing))
        .collect();
    remeasure_by_rule(
        &recipe.path_segments,
        recipe.boundary_guard,
        &recipe.plan_cameras(),
        &keys,
        |camera, seq, segment| {
            let frame = frames
                .iter()
                .find(|f| f.camera == camera && f.seq == seq)
                .ok_or("没有这一帧的记录")?;
            measure(frame, segment)
        },
    )
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
pub(super) fn frame_image(
    app: &AppHandle,
    detail: &PartDetail,
    camera: usize,
    seq: u64,
) -> Result<FrameImage, String> {
    let burst = detail
        .burst
        .as_ref()
        .ok_or("这条历史是旧拍照点记录，请按拍照点读原图")?;
    let frame = burst
        .frames
        .iter()
        .find(|f| f.camera == camera && f.seq == seq)
        .ok_or("没有这一帧的记录")?;
    if frame.status == FrameState::Missing {
        return Err("这台相机本帧组缺帧，没有原图".into());
    }
    let camera_id = burst
        .cameras
        .get(camera)
        .ok_or("相机下标超出本件计划相机")?;
    if detail.summary.source == RecordSource::OfflineCandidate {
        return burst_samples(app, detail)?.load(camera, seq);
    }
    let cycle_id = detail.summary.cycle_id.as_deref().ok_or("历史记录没有 cycleId，不能确定对应原图")?;
    recorded::load_frame_verified(app.state::<CycleHost>().recorders.root(detail.summary.source), cycle_id, camera_id, frame)
}

/// 段配方历史件一帧的录制原图（决策记录 5.4，camera 为计划相机下标）；未录制、录制队满丢弃或缺帧时报错。
#[tauri::command]
pub fn workspace_record_frame_image(app: AppHandle, history_id: i64, camera: usize, seq: u64) -> Result<tauri::ipc::Response, String> {
    let detail = app.state::<Store>().detail(history_id)?;
    Ok(preview_response(&frame_image(&app, &detail, camera, seq)?))
}

/// 历史件一帧按对照重测的叠图（历史详情，只读）：用对照保存的候选配方与这一帧在对照里所用的走廊段，从录制原图重测，
/// 给出走廊、搜索带与测到的胶边。示教帧尺寸取对照保存时记下的（`ComparisonBurst.sizes`），不依赖候选现在的示教，
/// 候选换了示教件或已删除也能看。按原发布包重现的对照用那个发布包重测（与历史原始记录的叠图同一入口）。
/// 这一帧在对照里没测（缺帧、本段不检测）时只给指标、不画线。
#[tauri::command]
pub async fn workspace_record_frame_view(
    app: AppHandle,
    history_id: i64,
    comparison: String,
    camera: usize,
    seq: u64,
) -> Result<burst::CalibrationFrameView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let detail = store.detail(history_id)?;
        let recipe_id = detail
            .summary
            .recipe_id
            .as_deref()
            .ok_or("历史记录没有配方")?;
        let host = app.state::<WorkspaceHost>();
        let found = host
            .comparisons
            .list(&host.root, &store, recipe_id, history_id)?
            .into_iter()
            .find(|c| c.id == comparison)
            .ok_or("找不到这次对照结果")?;
        let burst = found.burst.as_ref().ok_or("这次对照没有逐帧结果")?;
        let metrics = burst
            .frames
            .iter()
            .find(|m| m.camera == camera && m.seq == seq)
            .ok_or("这次对照没有这一帧的指标：重新触发测试后再看")?;
        let Some(segment) = metrics.corridor.filter(|_| {
            !matches!(
                metrics.state,
                FrameState::Missing | FrameState::NotInspected | FrameState::Unmeasured
            )
        }) else {
            return Ok(burst::CalibrationFrameView {
                segment: None,
                corridor: Vec::new(),
                search_half_px: None,
                edges: Vec::new(),
                part_edges: Vec::new(),
                metrics: metrics.clone(),
            });
        };
        if found.source == "original" {
            let bundle_id = found.bundle_id.as_deref().ok_or("这次重现没有记下发布包")?;
            let prepared = history_prepared(&app, &found.candidate_recipe, bundle_id)?;
            let image = frame_image(&app, &detail, camera, seq)?;
            let measured = prepared.measure_frame(camera, segment, seq, &image, &format!("record-view-{history_id}-c{camera}-{seq}"))?;
            let trace = crate::measure::FrameTrace { edges: measured.edges, part_edges: measured.part_edges, line: measured.line };
            return Ok(trace_view(&prepared.recipe, segment, trace, metrics));
        }
        if burst.sizes.is_empty() {
            return Err("这次对照没有保存示教帧尺寸（之前保存的对照或候选当时未示教完）：重新触发测试后再看叠图".into());
        }
        let sizes = burst.sizes.clone();
        let recipe = &found.candidate_recipe;
        let camera_id = recipe
            .plan_cameras()
            .get(camera)
            .cloned()
            .ok_or("相机下标超出候选计划相机")?;
        let graphs = vision::CorridorSet::build(recipe, &sizes)?;
        let image = frame_image(&app, &detail, camera, seq)?;
        burst::frame_view(
            &app,
            recipe,
            &graphs,
            (camera, &camera_id),
            segment,
            seq,
            &image,
            &format!("record-view-{history_id}-c{camera}-{seq}"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 候选能量哪些（相机, 段）：建得成走廊测量图、且没有定向配置问题的组合；其余连同原因列出（D-SB-57）。
pub(super) fn candidate_coverage(
    w: &Workspace,
    recipe: &Recipe,
) -> (vision::CorridorSet, Vec<vision::UnavailableCorridor>) {
    let (graphs, mut unavailable) =
        vision::CorridorSet::build_available(recipe, &super::burst::taught_sizes(w));
    let cameras = recipe.plan_cameras();
    for issue in crate::heading::issues(&recipe.path_segments, &recipe.inspect) {
        let Some(idx) = cameras.iter().position(|c| *c == issue.camera) else {
            continue;
        };
        for segment in recipe.path_segments.iter().filter(|s| {
            s.heading.is_some()
                && issue.segment.is_none_or(|i| i == s.index)
                && crate::plan::segment_cameras(s, &cameras).contains(&issue.camera)
        }) {
            if graphs.get(idx, segment.index).is_some() {
                unavailable.push(vision::UnavailableCorridor {
                    camera_idx: idx,
                    camera: issue.camera.clone(),
                    segment: segment.index,
                    reason: format!(
                        "第 {} 段相机 {}：{}",
                        segment.index, issue.camera, issue.message
                    ),
                    untaught: false,
                });
            }
        }
    }
    unavailable.sort_by_key(|u| (u.segment, u.camera_idx));
    (graphs, unavailable)
}

/// 候选里能量的（相机, 段）组合数。
fn usable_pairs(
    recipe: &Recipe,
    graphs: &vision::CorridorSet,
    unavailable: &[vision::UnavailableCorridor],
) -> usize {
    let cameras = recipe.plan_cameras();
    recipe
        .path_segments
        .iter()
        .flat_map(|s| {
            crate::plan::segment_cameras(s, &cameras)
                .into_iter()
                .filter_map(|c| cameras.iter().position(|id| *id == c))
                .map(move |idx| (idx, s.index))
        })
        .filter(|&(idx, seg)| {
            graphs.get(idx, seg).is_some()
                && !unavailable
                    .iter()
                    .any(|u| (u.camera_idx, u.segment) == (idx, seg))
        })
        .count()
}

/// 没有可用走廊、复测不测的段的说明（按段列相机与原因）。
fn coverage_note(unavailable: &[vision::UnavailableCorridor]) -> Option<String> {
    (!unavailable.is_empty()).then(|| {
        format!(
            "这些段没有可用走廊，复测时不测（记未测、只算检测不完整），结论只覆盖其余段：{}",
            unavailable
                .iter()
                .map(|u| u.reason.as_str())
                .collect::<Vec<_>>()
                .join("；")
        )
    })
}

/// 历史复测里没测的一段一台相机（TS `UnmeasuredSegment`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmeasuredSegment {
    pub segment: u32,
    pub camera: String,
    pub reason: String,
}

/// 用候选当前的走廊量帧：每段每台检测相机一张走廊测量图，按各相机冻结的示教帧尺寸建（与加入标定同一做法）。
/// `partial` 建的测量器只量能量的（相机, 段），其余记在 `unavailable` 里，量到时记未测。
pub(super) struct CandidateMeasurer {
    engine: Arc<vision::Engine>,
    graphs: vision::CorridorSet,
    run: String,
    unavailable: BTreeMap<(usize, u32), String>,
}

impl CandidateMeasurer {
    /// 历史件从原图复测用（D-SB-57）：不要求每一段都示教完，至少要有一段能量；返回没测的段。
    pub(super) fn partial(
        app: &AppHandle,
        w: &Workspace,
        recipe: &Recipe,
    ) -> Result<(Self, Vec<UnmeasuredSegment>), String> {
        let (graphs, unavailable) = candidate_coverage(w, recipe);
        if usable_pairs(recipe, &graphs, &unavailable) == 0 {
            return Err(format!(
                "候选还没有任何一段可用的走廊，不能从原图复测：{}",
                unavailable
                    .iter()
                    .map(|u| u.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            ));
        }
        let engine = app
            .state::<vision::VisionHost>()
            .engine()
            .ok_or("lyFlow 核心库未加载")?;
        let skipped = unavailable
            .iter()
            .map(|u| UnmeasuredSegment {
                segment: u.segment,
                camera: u.camera.clone(),
                reason: u.reason.clone(),
            })
            .collect();
        Ok((
            Self {
                engine,
                graphs: graphs.with_channels(&super::burst::teaching_channels(w)),
                run: vision::unique_run_id(&format!("candidate-{}", recipe.revision_id)),
                unavailable: unavailable
                    .into_iter()
                    .map(|u| ((u.camera_idx, u.segment), u.reason))
                    .collect(),
            },
            skipped,
        ))
    }

    /// 这一（相机, 段）不量时的原因。
    pub(super) fn skipped(&self, camera: usize, segment: u32) -> Option<&str> {
        self.unavailable.get(&(camera, segment)).map(String::as_str)
    }

    pub(super) fn new(app: &AppHandle, w: &Workspace, recipe: &Recipe) -> Result<Self, String> {
        recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
        let graphs = vision::CorridorSet::build(recipe, &super::burst::teaching_sizes(w)?)?
            .with_channels(&super::burst::teaching_channels(w));
        let engine = app
            .state::<vision::VisionHost>()
            .engine()
            .ok_or("lyFlow 核心库未加载")?;
        Ok(Self {
            engine,
            graphs,
            run: vision::unique_run_id(&format!("candidate-{}", recipe.revision_id)),
            unavailable: BTreeMap::new(),
        })
    }

    pub(super) fn measure(
        &self,
        camera: usize,
        segment: u32,
        seq: u64,
        image: &FrameImage,
    ) -> Result<FrameMetrics, String> {
        let graph = self
            .graphs
            .get(camera, segment)
            .ok_or("候选没有这台相机在这一段的走廊测量图")?;
        vision::measure_frame(
            &self.engine,
            graph,
            image,
            seq,
            &format!("{}-c{camera}-{seq}", self.run),
        )
        .map(|m| m.metrics)
    }
}

/// `workspace_compare` 的段配方分支：`raw` 从原图用候选走廊复测整件；否则用记录里存的每帧指标按候选规则重判
/// （要求每帧指标的测量定义与计划身份不变，见 `store::same_measurement_layout`）。
/// 一种复测能不能做（TS `CompareOption`）：不能做时 reason 写明原因。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOption {
    pub available: bool,
    pub reason: Option<String>,
    /// 能做但只覆盖一部分时的说明（如有段没有可用走廊、不测）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl CompareOption {
    fn of(blocker: Option<String>) -> Self {
        Self {
            available: blocker.is_none(),
            reason: blocker,
            note: None,
        }
    }

    fn checked(result: Result<Option<String>, String>) -> Self {
        match result {
            Ok(note) => Self {
                available: true,
                reason: None,
                note,
            },
            Err(reason) => Self::of(Some(reason)),
        }
    }
}

/// 对照用的候选：记录所属配方的候选编号、修订与版本。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareCandidate {
    pub id: String,
    pub revision: u64,
    pub version: u32,
}

/// 一条历史记录能做哪几种复测（TS `CompareOptions`）：以记录为准，按记录的配方找候选，与工作台当前选的配方无关；
/// 原因与真正执行对照时的检查同一处给出。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOptions {
    pub candidate: Option<CompareCandidate>,
    pub original: CompareOption,
    pub rules: CompareOption,
    pub raw: CompareOption,
}

fn recording_reason(detail: &PartDetail) -> String {
    use crate::store::RecordingState::*;
    match detail.recording.state {
        Off => "录制关闭，本件未保存原图",
        NotRetained => "按录制策略本件未保存原图",
        Pending => "原图仍在落盘",
        Incomplete => "原图不完整",
        Failed => "原图录制失败",
        Complete => "原图已按保留策略清理或无法核验",
    }
    .into()
}

/// 按规则重判的前置条件：复用记录里存的区域与每帧指标，测量布局须相同、原记录须有区域。
fn rules_blocker(burst: &PartDetailBurst, original: &Recipe, recipe: &Recipe) -> Option<String> {
    if !original.is_burst() || !recipe.is_burst() {
        return Some("段配方记录只能与段配方候选对照".into());
    }
    if !crate::store::same_burst_plan(original, recipe) {
        return Some("段结构（段数、类型、预期帧数或检测相机）与记录时不同，存的每帧指标是按旧段测的，不能复用；请从原图复测".into());
    }
    if !crate::store::same_measurement_layout(original, recipe) {
        return Some("存的每帧指标与候选的走廊、当量或检测参数不同，不能复用；请从原图复测".into());
    }
    if burst.judgement.verdict == Verdict::ErrInspect
        || burst.frames.is_empty()
        || burst.judgement.regions.is_empty()
    {
        return Some("原记录没有区域判定（结论为 ERR、没有逐帧指标或只录图未判定），没有可重判的区域；请从原图复测".into());
    }
    None
}

/// 从原图复测的前置条件：计划相机相同、原图齐（候选调试记录读样本集，不看录制）、候选至少有一段能量（D-SB-57）。
/// 能做时返回没有可用走廊、不测的段的说明。
fn raw_blocker(
    detail: &PartDetail,
    burst: &PartDetailBurst,
    original: &Recipe,
    recipe: &Recipe,
    w: &Workspace,
) -> Result<Option<String>, String> {
    if !original.is_burst() || !recipe.is_burst() {
        return Err("段配方记录只能与段配方候选对照".into());
    }
    if original.plan_cameras() != recipe.plan_cameras() {
        return Err("历史原图的计划相机与候选不同；请重新采集样本".into());
    }
    if detail.summary.source != RecordSource::OfflineCandidate && !raw_complete(detail, burst) {
        return Err(format!(
            "原图未完整保留，无法完成整件复测：{}",
            recording_reason(detail)
        ));
    }
    let (graphs, unavailable) = candidate_coverage(w, recipe);
    if usable_pairs(recipe, &graphs, &unavailable) == 0 {
        return Err("候选还没有任何一段可用的走廊；在配方工作台示教走廊后才能从原图复测".into());
    }
    Ok(coverage_note(&unavailable))
}

/// 按原发布包重现的前置条件。
fn original_blocker(detail: &PartDetail, burst: &PartDetailBurst) -> Option<String> {
    if detail.summary.source == RecordSource::OfflineCandidate {
        return Some("候选离线调试记录没有发布包".into());
    }
    if detail.summary.bundle_id.is_none() {
        return Some(
            "本件没有不可变发布包（维护采集件或未匹配到可用配方），不能按原版本重现".into(),
        );
    }
    if !raw_complete(detail, burst) {
        return Some(format!(
            "原图未完整保留，不能按原版本重现：{}",
            recording_reason(detail)
        ));
    }
    None
}

/// 记录能做哪几种复测。`original` 是记录的配方快照，`w` 是记录所属配方的候选（没有为 None）。
pub(super) fn compare_options(
    detail: &PartDetail,
    original: Option<&Recipe>,
    w: Option<&Workspace>,
) -> CompareOptions {
    let unavailable = |reason: &str| CompareOption::of(Some(reason.into()));
    let candidate = w.map(|w| CompareCandidate {
        id: w.doc.id.clone(),
        revision: w.revision,
        version: w.doc.version,
    });
    let Some(burst) = detail.burst.as_ref() else {
        let reason = "这条历史是旧拍照点记录，不能按相机帧复测";
        return CompareOptions {
            candidate,
            original: unavailable(reason),
            rules: unavailable(reason),
            raw: unavailable(reason),
        };
    };
    let original_option = CompareOption::of(original_blocker(detail, burst));
    let (rules, raw) = match (original, w) {
        (None, _) => (
            unavailable("历史配方快照缺失"),
            unavailable("历史配方快照缺失"),
        ),
        (_, None) => {
            let reason =
                "这个配方在本机没有候选（如从发布包导入），不能按候选对照；可按原发布包重现";
            (unavailable(reason), unavailable(reason))
        }
        (Some(original), Some(w)) => match w.doc.build() {
            Err(error) => {
                let reason = format!("候选配置不完整：{error}");
                (unavailable(&reason), unavailable(&reason))
            }
            Ok(recipe) => (
                CompareOption::of(rules_blocker(burst, original, &recipe)),
                CompareOption::checked(raw_blocker(detail, burst, original, &recipe, w)),
            ),
        },
    };
    CompareOptions {
        candidate,
        original: original_option,
        rules,
        raw,
    }
}

pub(super) fn compare(
    app: &AppHandle,
    w: &Workspace,
    recipe: &Recipe,
    detail: &PartDetail,
    original: &Recipe,
    raw: bool,
) -> Result<(judge::Judgement, ComparisonBurst), String> {
    let burst = detail
        .burst
        .as_ref()
        .ok_or("这条历史是旧拍照点记录，不能用段配方候选对照")?;
    if raw {
        raw_blocker(detail, burst, original, recipe, w)?;
    } else if let Some(reason) = rules_blocker(burst, original, recipe) {
        return Err(reason);
    }
    let cameras = recipe.plan_cameras();
    let mut unmeasured = Vec::new();
    let (judgement, metrics) = if raw {
        let (measurer, skipped) = CandidateMeasurer::partial(app, w, recipe)?;
        unmeasured = skipped;
        let metrics = remeasure(recipe, &burst.frames, |frame, segment| {
            if let Some(reason) = measurer.skipped(frame.camera, segment) {
                return Ok(FrameMetrics {
                    corridor: Some(segment),
                    ..burst::frame_without_metrics(
                        frame.camera,
                        frame.seq,
                        FrameState::Unmeasured,
                        Some(reason.to_owned()),
                    )
                });
            }
            measurer.measure(
                frame.camera,
                segment,
                frame.seq,
                &frame_image(app, detail, frame.camera, frame.seq)?,
            )
        });
        let judgement = judge_frames(
            &recipe.path_segments,
            recipe.boundary_guard,
            recipe.boundary_refine,
            recipe.expected_total(),
            &cameras,
            &metrics,
            &burst.judgement.alarms,
        );
        (judgement, metrics)
    } else {
        let regions: Vec<Region> = burst.judgement.regions.iter().map(|r| r.region.clone()).collect();
        let metrics = stored_metrics(&burst.frames);
        let judgement = burst_judge::judge(
            &recipe.path_segments,
            recipe.expected_total(),
            &regions,
            &frame_groups(&metrics, cameras.len()),
            &metrics,
            &cameras,
            &burst.judgement.alarms,
        );
        (judgement, metrics)
    };
    // 叠图按这些尺寸重建测量图；还没示教帧的相机没有走廊，尺寸用不到
    let sizes = super::burst::taught_sizes(w);
    let conclusion = summary(&judgement);
    let mut compared = ComparisonBurst::of(recipe, judgement, metrics, sizes);
    compared.unmeasured = unmeasured;
    Ok((conclusion, compared))
}

/// `workspace_compare_original` 的段配方分支：按原发布包逐帧重现（见 `reproduce::measure_burst`）。
pub(super) fn reproduce_original(
    app: &AppHandle,
    detail: &PartDetail,
    prepared: &crate::production::Prepared,
    cycle_id: &str,
) -> Result<(judge::Judgement, ComparisonBurst), String> {
    let burst = detail
        .burst
        .as_ref()
        .ok_or("这条历史是旧拍照点记录，不能按相机帧重现")?;
    if let Some(reason) = original_blocker(detail, burst) {
        return Err(reason);
    }
    let (judgement, metrics) = reproduce::measure_burst(
        prepared,
        &burst.frames,
        &burst.judgement.alarms,
        cycle_id,
        |frame| frame_image(app, detail, frame.camera, frame.seq),
    )?;
    Ok((
        summary(&judgement),
        ComparisonBurst::of(&prepared.recipe, judgement, metrics, Vec::new()),
    ))
}

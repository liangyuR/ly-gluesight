//! 段配方的在途件（规格 §0/§2/§3，D-SB-7、D-SB-10、D-SB-13、D-SB-45）：三台计划相机按 SDK 帧号配组（`Grouper`），
//! 每帧到达时按累计 N_exp 预测所属段，只由预测段的检测相机用该段走廊送测，其余相机这一帧记“本段不检测”；
//! 预测边界附近的原图留到判定。partEnd 后按收尾规则（已接受的帧都测完，且完整收尾窗口到期）按比例分段得出区域，
//! 对判定需要、但到达时没用对应走廊测过的帧补测一次，补测完再用 `burst_judge` 判定。缺帧、帧数越界、丢帧只告警
//! 不改 OK/NG；三台相机整件没有一帧测成判 ERR 91；会话变化判 ERR 98。纯逻辑，不碰 AppHandle：送测、录制与落库由
//! `Machine` 负责。

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::burst_judge::{self, BurstJudgement, FrameMetrics, FrameState};
use crate::frame::{CounterSource, Frame, FrameImage};
use crate::frame_group::{loss_alarms, Grouper, Offer, MAX_SEQ};
use crate::judge::{fault, Judgement};
use crate::measure::{FrameMeasured, MeasureError};
use crate::plan::{wider_checks, PathSegment, SegmentChecks};
use crate::recipe::Recipe;
use crate::segmentation::{
    frame_corridors, resolve_regions, segment_part, Region, SegInput, SegOutcome,
};
use crate::shot_router::{ArmCam, Floor, FrameBaseline, Ledger, Ledgers};
use crate::store::{BurstRecord, FrameRecord};

use super::{FrameCellStatus, FrameCellView, FrameGroupView, Part, PartViewBurst};

/// 分段边界的最低置信度：比例分段器在帧数容差内给 1、之外给 0（D-SB-40）。
const MIN_CONFIDENCE: f32 = 0.5;

/// 生产的 seq 上限在 ΣN_exp + Σtol 之上留的余量：超出即帧号异常，按 OutOfRange 丢弃并告警（防异常帧号生成海量帧组）。
const SEQ_MARGIN: u64 = 64;

/// 同一相机同一类异常每件只告警一次。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Warning {
    Duplicate,
    OutOfRange,
    Manual,
}

/// 一台相机一帧（已被 Grouper 接受）。
#[derive(Clone, Debug)]
struct BurstFrame {
    frame_counter: u64,
    /// 来源是 Chunk 触发计数或模拟计数时才有意义
    trigger_counter: Option<u64>,
    /// 送测时刻；测完（或判为没测）后为空
    measuring_since: Option<Instant>,
    /// 测完后的指标（含没测成的状态与原因）
    metrics: Option<FrameMetrics>,
    /// 送测用的走廊（段号）：到达时按预测段，收尾补测后改为补测用的段；这台相机不检测预测段时为空、不送测
    corridor: Option<u32>,
}

/// 收尾补测的一帧：计划相机下标、seq、用第几段走廊、原图（模拟测量时为空）。
pub(super) type PostJob = (usize, u64, u32, Option<Arc<FrameImage>>);

/// 段配方一件的帧归属与测量状态。相机下标与计划相机顺序（`Recipe.inspect`）一致。
pub(super) struct BurstPart {
    cams: Vec<FrameBaseline>,
    cameras: Vec<String>,
    grouper: Grouper,
    frames: BTreeMap<(usize, u64), BurstFrame>,
    /// 已送测、还没有结果的帧数
    pending: usize,
    trusted: Vec<CounterSource>,
    /// 每帧单独结论用的允差：未分段区域覆盖全部段，取各段中较宽一档（与 burst_judge 的区域允差相同）
    checks: SegmentChecks,
    /// 判定后存下，落库与结果显示用
    pub(super) judgement: Option<BurstJudgement>,
    warned: BTreeSet<(usize, Warning)>,
    /// 维护采集：只录图不送测，收尾只等收尾窗口
    pub(super) collect: bool,
    segments: Vec<PathSegment>,
    boundary_guard: u32,
    boundary_refine: bool,
    /// 预测边界附近的帧组：这些帧（全部计划相机）的原图留到判定，供收尾补测
    bands: Vec<RangeInclusive<u64>>,
    /// 留着的原图；模拟测量没有原图，记 None 也算留着
    retained: BTreeMap<(usize, u64), Option<Arc<FrameImage>>>,
    /// 收尾补测已经排过（每件只排一次）
    post_started: bool,
}

/// 预测边界附近要留原图的帧组（D-SB-45）：第 k 个预测边界 C_k 两侧各 `boundary_guard` + m_k 帧，
/// m_k = ⌈Σtol × C_k / ΣN_exp⌉ + 1 是总帧组数在 ΣN_exp ± Σtol 内时比例边界偏离预测边界的最大帧数。单段或未逐段标定时为空。
fn retention_bands(segments: &[PathSegment], guard: u32) -> Vec<RangeInclusive<u64>> {
    let Some(counts) = segments
        .iter()
        .map(|s| s.expected_frames.map(u64::from))
        .collect::<Option<Vec<u64>>>()
    else {
        return Vec::new();
    };
    let total: u64 = counts.iter().sum();
    if counts.len() < 2 || total == 0 {
        return Vec::new();
    }
    let tolerance = u64::from(crate::plan::tolerance_total(segments));
    let mut cumulative = 0u64;
    counts[..counts.len() - 1]
        .iter()
        .map(|n| {
            cumulative += n;
            let reach = u64::from(guard) + (tolerance * cumulative).div_ceil(total) + 1;
            cumulative.saturating_sub(reach)..=cumulative + reach + 1
        })
        .collect()
}

/// 一帧到达后的去向。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Arrival {
    /// 本件计划相机 `camera` 的第 `seq` 帧，已登记为测量中
    Accepted { camera: usize, seq: u64 },
    /// 同一相机同一 seq 又来一帧：保留先到的，丢弃这一帧
    Duplicate { camera: usize, seq: u64 },
    /// 上一件的迟到帧，或布防前那次打开的帧：不计入本件
    Stale { camera: usize },
    /// 帧号比基线大得离谱（计数跳变）：不计入本件
    OutOfRange { camera: usize, seq: u64 },
    /// 会话变化或计数来源不可信：整件判 ERR 98（原因已记进 `Part::fault`）
    Fault(String),
    /// 不是本件的计划相机
    Unknown,
}

impl BurstPart {
    pub(super) fn group_count(&self) -> u64 {
        self.grouper.group_count()
    }

    /// 布防：cams 是 `shot_router::arm_frames` 按计划相机顺序给出的帧计数基线。seq 上限 = ΣN_exp + Σtol + 余量。
    pub(super) fn arm(recipe: &Recipe, cams: Vec<FrameBaseline>, trusted: Vec<CounterSource>) -> Self {
        let limit = recipe.expected_total().map_or(MAX_SEQ, |n| u64::from(n) + u64::from(recipe.tolerance_total()) + SEQ_MARGIN);
        let checks = wider_checks(&recipe.path_segments.iter().map(|s| &s.checks).collect::<Vec<_>>());
        Self {
            grouper: Grouper::with_limit(
                cams.iter()
                    .map(|c| c.baseline.unwrap_or_default())
                    .collect(),
                limit,
            ),
            cameras: recipe.plan_cameras(),
            cams,
            frames: BTreeMap::new(),
            pending: 0,
            trusted,
            checks,
            judgement: None,
            warned: BTreeSet::new(),
            collect: false,
            bands: retention_bands(&recipe.path_segments, recipe.boundary_guard),
            segments: recipe.path_segments.clone(),
            boundary_guard: recipe.boundary_guard,
            boundary_refine: recipe.boundary_refine,
            retained: BTreeMap::new(),
            post_started: false,
        }
    }

    /// 到达时这一帧送测用的走廊（段号）；不送测（这台相机不检测预测段、维护采集）为 None。
    pub(super) fn arrival_corridor(&self, camera: usize, seq: u64) -> Option<u32> {
        self.frames
            .get(&(camera, seq))
            .filter(|f| f.measuring_since.is_some())
            .and_then(|f| f.corridor)
    }

    /// 预测边界附近的帧留下原图，供收尾补测。
    pub(super) fn retain_image(&mut self, camera: usize, seq: u64, image: Option<Arc<FrameImage>>) {
        if !self.collect && self.bands.iter().any(|band| band.contains(&seq)) {
            self.retained.insert((camera, seq), image);
        }
    }

    /// 本件的分段结果与区域（比例分段，D-SB-40；配方开了方向细化时按已测帧的胶条方向细化，D-SB-47）。
    fn regions(&self) -> (SegOutcome, Vec<Region>) {
        let groups = self.grouper.groups();
        let metrics: Vec<FrameMetrics> = if self.boundary_refine {
            self.frames
                .values()
                .filter_map(|frame| frame.metrics.clone())
                .collect()
        } else {
            Vec::new()
        };
        let outcome = segment_part(
            &SegInput {
                groups: &groups,
                segments: &self.segments,
            },
            &metrics,
            self.boundary_refine,
        );
        let regions = resolve_regions(
            &self.segments,
            &outcome,
            self.grouper.group_count(),
            MIN_CONFIDENCE,
            self.boundary_guard,
        );
        (outcome, regions)
    }

    /// 判定需要、但还没用对应走廊送测过的（帧, 相机）与该用的段号（D-SB-45）。用对应走廊测过但没测成（超时、失败）的不重测。
    fn post_targets(&self) -> Vec<((usize, u64), u32)> {
        let (outcome, regions) = self.regions();
        let choice = frame_corridors(
            &self.segments,
            &self.cameras,
            &regions,
            &outcome,
            self.grouper.group_count(),
        );
        self.frames
            .iter()
            .filter_map(|(&(camera, seq), frame)| {
                let segment = choice
                    .get(camera)?
                    .get((seq as usize).checked_sub(1)?)
                    .copied()
                    .flatten()?;
                let measured = frame.corridor.is_some_and(|arrival| {
                    arrival == segment || self.same_heading_graph(camera, arrival, segment)
                });
                (!measured).then_some(((camera, seq), segment))
            })
            .collect()
    }

    /// 两段在这台相机上都开了逐帧定向、找边参数也相同（D-SB-47）：测量不依赖帧归哪一段（扇区已含相邻段），
    /// 用到达时那段的定向图测过就不必补测。
    fn same_heading_graph(&self, camera: usize, a: u32, b: u32) -> bool {
        let key = |index: u32| {
            let segment = self.segments.iter().find(|s| s.index == index)?;
            segment.heading.as_ref()?;
            let edge = segment
                .offset_ref
                .as_ref()
                .filter(|o| self.cameras.get(camera) == Some(&o.camera))
                .and_then(|o| o.edge.clone());
            Some(edge)
        };
        key(a).is_some() && key(a) == key(b)
    }

    /// 这台相机（相机组序号）在本件的帧计数基线；不是计划相机、或还在等本件首帧定基线时为空。
    pub(super) fn baseline(&self, cam: u8) -> Option<u64> {
        self.cams
            .iter()
            .find(|c| c.cam == cam)
            .and_then(|c| c.baseline)
    }

    pub(super) fn camera_name(&self, camera: usize) -> &str {
        self.cameras.get(camera).map_or("?", String::as_str)
    }

    /// 第一次出现时返回 true：同一相机同一类异常每件只告警一次。
    pub(super) fn first_warning(&mut self, camera: usize, warning: Warning) -> bool {
        self.warned.insert((camera, warning))
    }

    /// 本件收尾：每台相机下一件的帧计数下限 = max(本件基线, 本件见到的最高帧计数)，PLC 报的触发数与计划数都不参与（D-SB-7）；
    /// 本件一帧没收到、基线仍未定的相机不记下限。
    pub(super) fn close(&self, ledgers: &mut Ledgers) {
        for (i, cam) in self.cams.iter().enumerate() {
            let Some(baseline) = cam.baseline else {
                continue;
            };
            ledgers.set_frame_floor(&Floor {
                cam: cam.cam,
                camera: cam.camera.clone(),
                session: cam.session,
                baseline: baseline.max(self.grouper.max_counter(i)),
            });
        }
    }

    /// 布防后相机重连过（会话变化）时返回原因。
    pub(super) fn check_sessions(&self, cameras: &[ArmCam]) -> Result<(), String> {
        for armed in &self.cams {
            let now = cameras.iter().find(|camera| camera.cam == armed.cam && camera.camera == armed.camera)
                .ok_or_else(|| format!("相机 {} 已不在本件相机组中", armed.camera))?;
            if now.session != armed.session {
                return Err(format!("相机 {} 检测中已重连（会话 {} → {}）", armed.camera, armed.session, now.session));
            }
        }
        Ok(())
    }

    /// 回调侧计数账：会话一致、计数来源都可信（帧可能被帧通道满丢掉，节拍看不到）。
    pub(super) fn check_observed(&self, cam: u8, observed: &Ledger) -> Result<(), (u16, String)> {
        let Some(armed) = self.cams.iter().find(|armed| armed.cam == cam) else { return Ok(()) };
        if observed.session != armed.session {
            return Err((fault::DEVICE_LOST, format!("相机 {} 检测中会话不一致（{} → {}）", armed.camera, armed.session, observed.session)));
        }
        if let Some(source) = observed.sources.iter().copied().chain(observed.source).find(|source| !self.trusted.contains(source)) {
            return Err((fault::DEVICE_LOST, format!("相机 {} 本会话曾出现不可信的{}，需要重新打开相机", armed.camera, source.label())));
        }
        Ok(())
    }
}

/// 没测成的一帧的指标：状态与原因，数值为空。
fn unmeasured(camera: usize, seq: u64, state: FrameState, error: String) -> FrameMetrics {
    FrameMetrics {
        camera,
        seq,
        state,
        error: Some(error),
        width_min: None,
        width_max: None,
        width_mean: None,
        widths: Vec::new(),
        spacing_mm: 0.0,
        gap_max_mm: 0.0,
        corridor: None,
        present_ratio: 0.0,
        x_mm: None,
        heading_deg: None,
        heading_fallback: false,
        edge_mm: None,
    }
}

/// 测量结果写进记录前的核对：帧身份与送测的帧一致、数值都是有限数，否则记为测量失败（不让一帧坏输出拖垮整件落库）。
fn checked(metrics: FrameMetrics, camera: usize, seq: u64) -> FrameMetrics {
    let finite = |value: Option<f64>| value.is_none_or(f64::is_finite);
    let valid = (metrics.camera, metrics.seq) == (camera, seq)
        && finite(metrics.width_min)
        && finite(metrics.width_max)
        && finite(metrics.width_mean)
        && finite(metrics.x_mm)
        && finite(metrics.heading_deg)
        && finite(metrics.edge_mm)
        && metrics.spacing_mm.is_finite()
        && metrics.gap_max_mm.is_finite()
        && metrics.present_ratio.is_finite()
        && metrics.widths.iter().flatten().all(|w| w.is_finite());
    if valid { metrics } else { unmeasured(camera, seq, FrameState::Error, "测量输出与送测帧不一致或含无效数值".into()) }
}

/// 结论摘要（旧 `Judgement` 的形状）：verdict / plc_code / fault_code / reason 与区域判定相同，segments、gaps 为空。
pub(super) fn summary(judgement: &BurstJudgement) -> Judgement {
    Judgement {
        verdict: judgement.verdict,
        plc_code: judgement.plc_code,
        fault_code: judgement.fault_code,
        reason: judgement.reason.clone(),
        segments: Vec::new(),
        gaps: Vec::new(),
    }
}

/// 没有区域判定时（拒绝布防、在途中断）按结论摘要给出段配方的判定，结论字段与摘要逐一相同。
fn from_summary(judgement: &Judgement) -> BurstJudgement {
    BurstJudgement {
        verdict: judgement.verdict,
        plc_code: judgement.plc_code,
        fault_code: judgement.fault_code,
        reason: judgement.reason.clone(),
        ..BurstJudgement::error(judgement.fault_code, judgement.reason.clone())
    }
}

fn same_conclusion(burst: &BurstJudgement, judgement: &Judgement) -> bool {
    (burst.verdict, burst.plc_code, burst.fault_code, burst.reason.as_str()) == (judgement.verdict, judgement.plc_code, judgement.fault_code, judgement.reason.as_str())
}

/// 没有帧的记录（拒绝布防、进程中断补记录）：预期帧组数与容差取配方，各相机帧数为 0，判定与摘要相同。
pub(super) fn empty_record(recipe: &Recipe, judgement: &Judgement) -> BurstRecord {
    BurstRecord {
        expected_total: recipe.expected_total(),
        tolerance_total: recipe.tolerance_total(),
        device_triggers: None,
        camera_frames: vec![0; recipe.plan_cameras().len()],
        judgement: from_summary(judgement),
        frames: Vec::new(),
    }
}

impl Part {
    fn burst_deadline(&self, now: Instant) -> bool {
        self.end_at.is_some_and(|end| now.saturating_duration_since(end) >= self.drain_timeout)
    }

    /// 段配方收一帧（帧号配组，D-SB-10）。收尾截止后到的帧返回 None。
    pub(super) fn receive_burst(&mut self, frame: &Frame) -> Option<Arrival> {
        let now = Instant::now();
        self.expire_burst(now, self.proc_timeout);
        if self.burst_deadline(now) { return None; }
        let burst = &mut self.burst;
        let Some(camera) = burst.cams.iter().position(|c| c.cam == frame.cam) else { return Some(Arrival::Unknown) };
        let armed = &burst.cams[camera];
        let reason = if frame.session < armed.session {
            return Some(Arrival::Stale { camera });
        } else if frame.session > armed.session {
            format!("相机 {} 检测中重新打开过（会话 {} → {}），之后的帧号对不上本件", armed.camera, armed.session, frame.session)
        } else if !burst.trusted.contains(&frame.counter) {
            format!("相机 {} 的帧计数来源变为{}，本件帧号不可信", armed.camera, frame.counter.label())
        } else {
            let baseline = match armed.baseline {
                Some(baseline) => baseline,
                None => {
                    let baseline = frame.frame_counter.saturating_sub(1);
                    burst.cams[camera].baseline = Some(baseline);
                    burst.grouper.set_baseline(camera, baseline);
                    baseline
                }
            };
            return Some(match burst.grouper.offer(camera, frame.frame_counter) {
                Offer::Accepted { seq } => {
                    let trigger_counter = matches!(
                        frame.counter,
                        CounterSource::ChunkTrigger | CounterSource::Synthetic
                    )
                    .then_some(frame.trigger_counter);
                    let corridor =
                        crate::plan::arrival_corridor(&burst.segments, &burst.cameras, camera, seq);
                    let measuring = !burst.collect && corridor.is_some();
                    let metrics = (!burst.collect && corridor.is_none()).then(|| {
                        unmeasured(
                            camera,
                            seq,
                            FrameState::NotInspected,
                            "这一帧的预测段不由这台相机检测".into(),
                        )
                    });
                    burst.frames.insert(
                        (camera, seq),
                        BurstFrame {
                            frame_counter: frame.frame_counter,
                            trigger_counter,
                            measuring_since: measuring.then_some(now),
                            metrics,
                            corridor,
                        },
                    );
                    if measuring {
                        burst.pending += 1;
                    }
                    Arrival::Accepted { camera, seq }
                }
                Offer::Duplicate => Arrival::Duplicate { camera, seq: frame.frame_counter - baseline },
                Offer::Stale => Arrival::Stale { camera },
                Offer::OutOfRange { seq } => Arrival::OutOfRange { camera, seq },
            });
        };
        self.fault.get_or_insert((fault::DEVICE_LOST, reason.clone()));
        Some(Arrival::Fault(reason))
    }

    /// 段配方的一帧测量结果。不属于本件（上一件迟到的结果）、或这一帧已不在测量中（超时后才到）时丢弃，返回 None。
    /// 没测成（队满、超时、失败）记为 Unmeasured / Error，只算检测不完整，不判 ERR（D-SB-13）。
    pub(super) fn apply_burst(&mut self, measured: FrameMeasured) -> Option<FrameMetrics> {
        self.expire_burst(Instant::now(), self.proc_timeout);
        if measured.cycle_id != self.cycle_id { return None; }
        let burst = &mut self.burst;
        let (camera, seq) = (measured.camera_idx, measured.seq);
        let frame = burst.frames.get_mut(&(camera, seq)).filter(|f| f.measuring_since.is_some())?;
        let metrics = checked(measured.metrics(), camera, seq);
        frame.metrics = Some(metrics.clone());
        frame.measuring_since = None;
        burst.pending = burst.pending.saturating_sub(1);
        Some(metrics)
    }

    /// 测量超过 T_proc 的帧记为没测（Unmeasured），返回这些帧。
    pub(super) fn expire_burst(&mut self, now: Instant, proc: Duration) -> Vec<(usize, u64)> {
        let burst = &mut self.burst;
        let mut expired = Vec::new();
        for (&(camera, seq), frame) in burst.frames.iter_mut() {
            let Some(since) = frame.measuring_since else { continue };
            if now.saturating_duration_since(since) >= proc {
                let why = MeasureError::Timeout.message();
                frame.metrics = Some(unmeasured(camera, seq, FrameState::Unmeasured, why));
                frame.measuring_since = None;
                expired.push((camera, seq));
            }
        }
        burst.pending = burst.pending.saturating_sub(expired.len());
        expired
    }

    /// partEnd 后等待完整收尾窗口，完成所有已接受帧的测量；有要补测的帧时还要等收尾补测完（维护采集、整件故障不补测）。
    pub(super) fn burst_complete(&self, now: Instant) -> bool {
        let burst = &self.burst;
        if self.end_at.is_none() || burst.pending > 0 || !self.burst_deadline(now) {
            return false;
        }
        burst.collect
            || burst.post_started
            || self.fault.is_some()
            || burst.post_targets().is_empty()
    }

    /// 收尾补测（D-SB-45）：收尾窗口到期、到达时送测的帧都测完后排一次。按比例分段得出区域，找出判定需要、但到达时
    /// 没用对应走廊测过的（帧, 相机），用留着的原图补测（返回给 `Machine` 送测）；原图没留住的记没测（检测不完整）。
    /// 已有故障（整件 ERR）时不补测。留着的原图随之释放。
    pub(super) fn start_post_measure(&mut self, now: Instant) -> Vec<PostJob> {
        if self.end_at.is_none() || !self.burst_deadline(now) {
            return Vec::new();
        }
        let faulted = self.fault.is_some();
        let burst = &mut self.burst;
        if burst.collect || burst.post_started || burst.pending > 0 {
            return Vec::new();
        }
        burst.post_started = true;
        let retained = std::mem::take(&mut burst.retained);
        if faulted {
            return Vec::new();
        }
        let mut jobs = Vec::new();
        for ((camera, seq), segment) in burst.post_targets() {
            let Some(frame) = burst.frames.get_mut(&(camera, seq)) else {
                continue;
            };
            frame.corridor = Some(segment);
            match retained.get(&(camera, seq)) {
                Some(image) => {
                    frame.measuring_since = Some(now);
                    frame.metrics = None;
                    jobs.push((camera, seq, segment, image.clone()));
                }
                None => {
                    frame.metrics = Some(unmeasured(
                        camera,
                        seq,
                        FrameState::Unmeasured,
                        format!("收尾补测要用第 {segment} 段走廊，但这一帧原图没有留住"),
                    ));
                }
            }
        }
        burst.pending += jobs.len();
        jobs
    }

    /// 段配方判定：会话变化、运动超时等故障优先（ERR）；否则比例分段 → 区域（含过渡区）→ 丢帧告警（各相机帧数彼此比较）
    /// → burst_judge。三台相机整件没有一帧测成时 burst_judge 判 ERR 91。
    pub(super) fn burst_judgement(&self) -> BurstJudgement {
        let burst = &self.burst;
        let groups = burst.grouper.groups();
        let loss = loss_alarms(&burst.grouper.camera_counts());
        if let Some((code, reason)) = &self.fault {
            let group_count = u32::try_from(burst.grouper.group_count()).unwrap_or(u32::MAX);
            return BurstJudgement {
                group_count,
                alarms: loss,
                ..BurstJudgement::error(*code, reason.clone())
            };
        }
        let metrics: Vec<FrameMetrics> = burst.frames.values().filter_map(|frame| frame.metrics.clone()).collect();
        let segments = &burst.segments;
        let (_, regions) = burst.regions();
        burst_judge::judge(
            segments,
            self.recipe.expected_total(),
            &regions,
            &groups,
            &metrics,
            &burst.cameras,
            &loss,
        )
    }

    /// 落库用的段配方记录：1..=帧组数内每台相机每一帧一条，缺帧也有。判定取本件的区域判定；结论摘要与它不同
    /// （在途中断记 ERR 98 等）时按摘要给出。
    pub(super) fn burst_record(&self, judgement: &Judgement) -> BurstRecord {
        let burst = &self.burst;
        let group_count = burst.grouper.group_count();
        let decided = burst.judgement.clone().filter(|decided| same_conclusion(decided, judgement)).unwrap_or_else(|| BurstJudgement {
            group_count: u32::try_from(group_count).unwrap_or(u32::MAX),
            alarms: loss_alarms(&burst.grouper.camera_counts()),
            ..from_summary(judgement)
        });
        let region = |seq: u64| decided.regions.iter().position(|r| (r.region.first_seq..=r.region.last_seq).contains(&seq));
        let frames = (1..=group_count)
            .flat_map(|seq| (0..burst.cams.len()).map(move |camera| (camera, seq)))
            .map(|(camera, seq)| match burst.frames.get(&(camera, seq)) {
                Some(frame) => {
                    let metrics = frame.metrics.clone().unwrap_or_else(|| unmeasured(camera, seq, FrameState::Unmeasured, "本件结束时测量仍未完成".into()));
                    FrameRecord { camera, seq, status: metrics.state, frame_counter: Some(frame.frame_counter), trigger_counter: frame.trigger_counter,
                        region: region(seq), metrics: Some(metrics), raw_file: None }
                }
                None => FrameRecord { camera, seq, status: FrameState::Missing, frame_counter: None, trigger_counter: None, region: region(seq), metrics: None, raw_file: None },
            })
            .collect();
        BurstRecord {
            expected_total: self.recipe.expected_total(),
            tolerance_total: self.recipe.tolerance_total(),
            device_triggers: self.device_triggers.map(u32::from),
            camera_frames: burst.grouper.camera_counts(),
            judgement: decided,
            frames,
        }
    }

    /// 在线显示：帧组按 seq，帧格按计划相机顺序。还没到的帧在判定前显示等待、判定或收尾截止后显示缺帧。
    pub(super) fn burst_view(&self) -> PartViewBurst {
        let burst = &self.burst;
        let settled = burst.judgement.is_some() || self.burst_deadline(Instant::now());
        let groups = burst.grouper.groups().into_iter().map(|group| {
            let cells: Vec<FrameCellView> = (0..burst.cams.len()).map(|camera| match burst.frames.get(&(camera, group.seq)) {
                Some(frame) => {
                    let (status, verdict) = match &frame.metrics {
                        Some(m) => (FrameCellStatus::from(m.state), (m.state == FrameState::Measured).then(|| burst_judge::frame_verdict(m, &burst.checks))),
                        None => (FrameCellStatus::Measuring, None),
                    };
                    FrameCellView { status, verdict, frame_counter: Some(frame.frame_counter) }
                }
                None => FrameCellView { status: if settled { FrameCellStatus::Missing } else { FrameCellStatus::Waiting }, verdict: None, frame_counter: None },
            }).collect();
            let incomplete = group.incomplete || cells.iter().any(|cell| !matches!(cell.status, FrameCellStatus::Measured | FrameCellStatus::Measuring | FrameCellStatus::Waiting));
            FrameGroupView { seq: group.seq, incomplete, cells }
        }).collect();
        PartViewBurst {
            cameras: burst.cameras.clone(),
            expected_total: self.recipe.expected_total(),
            tolerance_total: self.recipe.tolerance_total(),
            device_triggers: self.device_triggers.map(u32::from),
            group_count: burst.grouper.group_count(),
            camera_frames: burst.grouper.camera_counts(),
            measured_frames: burst.frames.values().filter(|frame| frame.metrics.is_some()).count(),
            pending_measure: burst.pending,
            groups,
        }
    }
}

//! 段配方的发布前验证、验证样本与发布（D3）。
//!
//! 只服务段配方（`RecipeDoc.segments` 非空）。旧拍照点配方的验证（组选图、胶路示教中线、示教与标定、完整采集拍照点数、
//! 特征定位统计）、验证样本（`workflow::capture_sample`、导入单帧图片）与发布（`freeze_bundle` → schema 3）仍在 workspace.rs
//! 与 workflow.rs，行为不变。
//!
//! 验证项（决策记录第九节第 13 条）：
//! - 设备与采集：计划相机解析、三台单视角独立设备（`check_burst_cameras`）、就绪；
//! - 走廊示教：恰好 3 台计划相机，每台都选了示教帧、走廊完整落在帧内、有像素当量，且示教帧采集后设备配置没变；
//! - 段标定：整件 ΣN_exp（expected_total）已由多轮标定得出即通过（决策补充“多段配方的发布与运行”），逐段列出 N_exp 或待定原因；
//! - 标准件 x_ref：只在有启用胶偏检查的直线段时列出；单段 AbsoluteX 要有标准件得出的 x_ref，多段（分段算法未就绪）
//!   与 ToEdge（测量量未实现）运行时不判胶偏，列出原因、不阻断；
//! - 独立正常样本：样本轮次与走廊示教、多轮标定轮次不同，相机配置与段结构一致；用当前候选的走廊逐帧重测、
//!   burst_judge 得出结论，帧组数超出 ΣN_exp ± Σtol 只告警；
//! - 生产版本：候选基于当前生产版本。

use std::collections::BTreeSet;

use super::*;
use super::burst::{corridor_saved, measure_round, offset_reference, CameraTeaching};
use super::burst_history::{judge_frames, remeasure, CandidateMeasurer};
use crate::burst_judge::{BurstJudgement, FrameMetrics};
use crate::frame_group::{loss_alarms, LossAlarm};
use crate::plan::{OffsetMeasure, PathSegment, SegmentKind, PLAN_CAMERAS};
use crate::recipe_capture::{CaptureRoundV2, CaptureState};

/// 样本库里一组样本最多保留的数量（与旧拍照点配方相同）。
const SAMPLE_BANK_LIMIT: usize = 100;

fn kind_label(kind: SegmentKind) -> &'static str {
    match kind { SegmentKind::Straight => "直线", SegmentKind::Corner => "弯道" }
}

/// 验证样本的段结构身份：产品代码、有序计划相机、各段 (序号, 类型)。N_exp / ΣN_exp 不在内——标定前采的样本标定后仍可用；
/// 发布时的计划版本另按 D-SB-19 分配。
fn sample_tag(product_code: u16, cameras: &[String], segments: &[PathSegment]) -> Value {
    json!([product_code, cameras, segments.iter().map(|s| (s.index, s.kind)).collect::<Vec<_>>()])
}

fn doc_sample_tag(doc: &RecipeDoc) -> Value {
    sample_tag(doc.product_code, &doc.plan_cameras(), &doc.segments)
}

/// 段配方验证项的输入：设备检查与样本结论由调用方（访问设备、读图、测量）给出，其余只读候选。
pub(super) struct BurstInputs<'a> {
    /// 计划相机解析、三台单视角独立设备、就绪（`cycle::usable_cams` + `check_ready_at`）
    pub device: Result<(), String>,
    /// 当前设备的相机标签（`camera_state`，与示教帧冻结时同一口径），按相机编号
    pub cameras: &'a BTreeMap<String, Value>,
    pub samples: &'a [SampleResult],
    /// 候选基于当前生产版本
    pub base_matches: bool,
}

/// 段配方的全部验证项，按页面展示顺序。
pub(super) fn burst_checks(w: &Workspace, inputs: BurstInputs) -> Vec<Check> {
    let mut checks = vec![Check {
        name: "设备与采集".into(),
        passed: inputs.device.is_ok(),
        detail: inputs.device.err().unwrap_or_else(|| "三台计划相机已就绪".into()),
    }];
    checks.push(corridor_check(w, inputs.cameras));
    checks.push(segment_check(w));
    checks.extend(x_ref_check(&w.doc));
    let normals: Vec<_> = inputs.samples.iter().filter(|r| matches!(r.expected, Verdict::Ok | Verdict::OkWithExcursion)).collect();
    checks.push(Check {
        name: "独立正常样本".into(),
        passed: !normals.is_empty() && normals.iter().all(|r| r.passed),
        detail: "至少一套独立正常整圈样本（采集轮次独立于走廊示教与多轮标定），所选正常样本均须符合预期；帧组数超出预期只告警；缺陷样本验证结果单独保留".into(),
    });
    checks.push(Check {
        name: "生产版本".into(),
        passed: inputs.base_matches,
        detail: if inputs.base_matches { "候选基于当前生产版本".into() } else { "生产配方已被其他操作更新，请重新建立候选".into() },
    });
    checks
}

/// 走廊示教（规格 §1 Plan.cameras = 3、§4 检测走廊）：每台计划相机都有示教帧、走廊完整落在帧内、有像素当量，
/// 示教帧采集后这台相机的设备配置没变（`current` 是当前设备的相机标签）。
fn corridor_check(w: &Workspace, current: &BTreeMap<String, Value>) -> Check {
    let doc = &w.doc;
    let mut problems = Vec::new();
    if doc.inspect.len() != PLAN_CAMERAS {
        problems.push(format!("段配方需要恰好 {PLAN_CAMERAS} 台计划相机共用一个触发，当前 {} 台", doc.inspect.len()));
    }
    for inspect in &doc.inspect {
        let camera = &inspect.camera;
        let Some(teaching) = w.camera_teaching.iter().find(|t| &t.camera == camera) else {
            problems.push(format!("相机 {camera}：待选示教帧"));
            continue;
        };
        if !corridor_saved(inspect, teaching.frame.size) {
            problems.push(format!("相机 {camera}：走廊未示教或超出示教帧"));
        } else if inspect.mm_per_px.is_none() {
            problems.push(format!("相机 {camera}：缺像素当量"));
        } else if current.get(camera) != Some(&teaching.frame.camera_tag) {
            problems.push(format!("相机 {camera}：示教帧采集后设备参数或样本集已改变，请重新采集并选示教帧"));
        }
    }
    Check {
        name: "走廊示教".into(),
        passed: problems.is_empty(),
        detail: if problems.is_empty() { format!("{} 台计划相机都已示教走廊与像素当量", doc.inspect.len()) } else { problems.join("；") },
    }
}

/// 当前设备的相机标签（`camera_state`，与示教帧冻结时同一口径：样本集相机带样本集编号），按相机编号。
pub(super) fn current_camera_tags(app: &AppHandle) -> BTreeMap<String, Value> {
    let sample_set = crate::offline::sample_set(app).map(|set| set.manifest().id.clone());
    app.state::<CycleHost>().camera.configs().iter().map(|c| (c.id.clone(), camera_state(c, sample_set.as_deref()))).collect()
}

/// 段配方候选试运行（`workspace_debug_run`）用的各计划相机示教帧尺寸（计划相机顺序，交给 `Candidate::new_burst`）：
/// 每台都选了示教帧，且示教帧采集后这台相机的设备标签（`current`）没变——与走廊示教验证项同一口径。开跑前
/// `recheck_debug_candidate` 再核对一次。
pub(super) fn debug_sizes(w: &Workspace, current: &BTreeMap<String, Value>) -> Result<Vec<[u32; 2]>, String> {
    w.doc.inspect.iter().map(|inspect| {
        let camera = &inspect.camera;
        let teaching = w.camera_teaching.iter().find(|t| &t.camera == camera)
            .ok_or_else(|| format!("相机 {camera} 还没有选示教帧：先完成走廊示教"))?;
        if current.get(camera) != Some(&teaching.frame.camera_tag) {
            return Err(format!("相机 {camera}：示教帧采集后设备参数或样本集已改变，请重新采集并选示教帧"));
        }
        Ok(teaching.frame.size)
    }).collect()
}

/// 段标定（规格 §1 未标定不能发布；决策补充“多段配方的发布与运行”）：整件 ΣN_exp 已由多轮标定得出即通过，
/// 各段 N_exp 可以为空（待分段算法）；逐段列出 N_exp 或待定原因。
fn segment_check(w: &Workspace) -> Check {
    let doc = &w.doc;
    let total = doc.expected_total();
    let tolerance_total = crate::plan::tolerance_total(&doc.segments);
    let normal = w.calibration.rounds.iter().filter(|r| !r.standard).count() as u32;
    let head = match total {
        Some(total) => format!("整件预期 {total} ± {tolerance_total} 个帧组（PLC shotCount = {total}）"),
        None if normal < doc.calibration_rounds => format!("整件预期帧组数待多轮标定：还差 {} 轮（至少 {} 轮，不含标准件轮次）", doc.calibration_rounds - normal, doc.calibration_rounds),
        None => format!("整件预期帧组数待多轮标定：已有 {normal} 轮，请应用标定"),
    };
    let segments: Vec<String> = doc.segments.iter().map(|s| {
        let kind = kind_label(s.kind);
        match (s.expected_frames, total) {
            (Some(n), _) => format!("第 {} 段（{kind}）N_exp {n} ± {}", s.index, s.tolerance),
            (None, Some(_)) => format!("第 {} 段（{kind}）待分段算法，容差 ±{}", s.index, s.tolerance),
            (None, None) => format!("第 {} 段（{kind}）待标定", s.index),
        }
    }).collect();
    Check { name: "段标定".into(), passed: total.is_some(), detail: format!("{head}；{}", segments.join("；")) }
}

/// 标准件 x_ref（规格 §0/§1、D-SB-18）：只在有启用胶偏检查的直线段时列出。单段 AbsoluteX 要有标准件多轮得出的 x_ref；
/// 多段计划（分段算法未就绪，运行时整条胶路未分段、不判胶偏）与 ToEdge（测量量未实现，结果标胶偏未判）列出原因、不阻断。
fn x_ref_check(doc: &RecipeDoc) -> Option<Check> {
    let cameras = doc.plan_cameras();
    let single = doc.segments.len() == 1;
    let items: Vec<(bool, String)> = doc.segments.iter()
        .filter(|s| s.kind == SegmentKind::Straight && s.checks.offset.as_ref().is_some_and(|o| o.enabled))
        .filter_map(|s| offset_reference(s, &cameras).map(|reference| (s.index, reference)))
        .map(|(index, reference)| match (reference.x_ref.zip(reference.spread), reference.measure) {
            (Some((x_ref, spread)), _) => (true, format!("第 {index} 段 x_ref {x_ref:.3} ± {spread:.3} mm（相机 {}）", reference.camera)),
            (None, _) if !single => (true, format!("第 {index} 段：分段算法未就绪，运行时整条胶路按未分段处理、不判胶偏，暂不需要 x_ref")),
            (None, OffsetMeasure::ToEdge) => (true, format!("第 {index} 段：测量量“到边缘距离”未实现，结果标胶偏未判")),
            (None, OffsetMeasure::AbsoluteX) => (false, format!("第 {index} 段：待标准件轮次得出 x_ref（相机 {}）", reference.camera)),
        })
        .collect();
    (!items.is_empty()).then(|| Check {
        name: "标准件 x_ref".into(),
        passed: items.iter().all(|(passed, _)| *passed),
        detail: items.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("；"),
    })
}

/// 段配方验证样本的采集轮次（规格 §0 独立验证、D-SB-35）：属于本候选、已完整完成；没用于走廊示教、没加入多轮标定；
/// 相机集合等于计划相机，各相机采集时的设备配置与当前设备相同（`current` 按相机编号，`state_value` 口径）。
pub(super) fn check_sample_round(w: &Workspace, round: &CaptureRoundV2, current: &BTreeMap<String, Value>) -> Result<(), String> {
    let id = &round.round_id;
    if round.recipe_id != w.doc.id || round.state != CaptureState::Complete || round.group_count == 0 {
        return Err(format!("采集轮次 {id} 不属于当前候选、尚未完整完成或没有帧组，不能作为验证样本"));
    }
    if w.camera_teaching.iter().any(|t| &t.round == id) {
        return Err(format!("采集轮次 {id} 已用于走廊示教：示教与验证样本必须来自不同的采集轮次，请重新采集"));
    }
    if w.calibration.rounds.iter().any(|r| &r.round_id == id) {
        return Err(format!("采集轮次 {id} 已加入多轮标定：标定与验证样本必须来自不同的采集轮次，请重新采集"));
    }
    let cameras = w.doc.plan_cameras();
    let ids: BTreeSet<&str> = round.cameras.iter().map(|c| c.camera_id.as_str()).collect();
    if round.cameras.len() != cameras.len() || ids != cameras.iter().map(String::as_str).collect() {
        return Err(format!("采集轮次 {id} 的相机 {:?} 与计划相机 {cameras:?} 不同", round.camera_ids()));
    }
    for camera in &round.cameras {
        if current.get(&camera.camera_id) != Some(&camera.camera_config) {
            return Err(format!("采集后相机 {} 的参数已改变，请重新采集验证样本", camera.camera_id));
        }
    }
    Ok(())
}

/// 丢帧告警按计划相机下标重算（轮次里的告警按本轮相机顺序，与计划相机顺序可能不同）。
fn round_loss(cameras: &[String], round: &CaptureRoundV2) -> Vec<LossAlarm> {
    let counts: Vec<u32> = cameras.iter()
        .map(|c| round.cameras.iter().position(|r| &r.camera_id == c).and_then(|j| round.camera_frames.get(j).copied()).unwrap_or(0))
        .collect();
    loss_alarms(&counts, round.device_triggers.map(u32::from))
}

/// 用当前候选判一轮验证样本：每帧指标（`measure_round` 的结果，按计划相机）→ 未分段区域 → 丢帧告警 → burst_judge。
pub(super) fn judge_round(doc: &RecipeDoc, round: &CaptureRoundV2, metrics: &[FrameMetrics]) -> BurstJudgement {
    let cameras = doc.plan_cameras();
    judge_frames(&doc.segments, doc.expected_total(), &cameras, metrics, &round_loss(&cameras, round))
}

/// 段配方样本的结论：按 burst_judge 的整件结论对照人工期望（规则同 `SampleResult::judged`）。帧组数超出 ΣN_exp ± Σtol
/// 与检测不完整只在原因里告警，不改是否符合（规格 §2、§0）。
pub(super) fn sample_result(sample: &Sample, name: String, sn: String, judgement: &BurstJudgement, expected_total: Option<u32>, tolerance_total: u32) -> SampleResult {
    let mut result = SampleResult::judged(sample, name, sn, burst_history::summary(judgement));
    let mut warnings = Vec::new();
    if judgement.flags.frame_count_anomaly {
        let expected = expected_total.map_or("预期".to_string(), |n| format!("预期 {n} ± {tolerance_total}"));
        warnings.push(format!("帧组数 {} 超出{expected}（只告警，不影响结论）", judgement.group_count));
    }
    if judgement.flags.incomplete {
        warnings.push("检测不完整：有帧组缺帧或有帧没测成（只告警，不影响结论）".to_string());
    }
    if !warnings.is_empty() {
        result.reason = format!("{}；{}", result.reason, warnings.join("；"));
    }
    result
}

fn failed_sample(sample: &Sample, reason: String) -> SampleResult {
    SampleResult {
        history_id: sample.history_id,
        sample_id: sample.sample_id.clone(),
        name: sample.sample_id.clone().unwrap_or_else(|| format!("记录 #{}", sample.history_id.unwrap_or(0))),
        sn: String::new(),
        expected: sample.expected,
        actual: None,
        passed: false,
        reason,
    }
}

/// 一件验证样本：独立采集轮次（样本库）或生产历史记录（原图齐全），都用当前候选的走廊逐帧重测后判定。
fn validate_sample(app: &AppHandle, w: &Workspace, recipe: &Recipe, measurer: Result<&CandidateMeasurer, &String>, sample: &Sample,
    current: &BTreeMap<String, Value>) -> Result<SampleResult, String> {
    let tolerance_total = recipe.tolerance_total();
    if let Some(sample_id) = &sample.sample_id {
        let bank = w.sample_bank.iter().find(|b| &b.id == sample_id).ok_or("样本组不存在")?;
        let round_id = bank.capture_id.as_deref().ok_or("段配方的验证样本须来自独立采集轮次，导入的单帧图片不能用于段配方")?;
        if bank.geometry_tag != doc_sample_tag(&w.doc) {
            return Err("样本来自另一版段序列、计划相机或产品代码，请重新采集".into());
        }
        let cycle = app.state::<CycleHost>();
        let capture = cycle.camera.recipe_capture();
        let round = capture.get_burst(round_id)?;
        check_sample_round(w, &round, current)?;
        let measurer = measurer.map_err(|e| e.clone())?;
        let metrics = measure_round(&w.doc.plan_cameras(), &round, |camera, camera_id, seq| {
            let image = crate::image_file::load(&capture.burst_frame_path(round_id, camera_id, seq)?)?;
            measurer.measure(camera, seq, &image)
        });
        let judgement = judge_round(&w.doc, &round, &metrics);
        return Ok(sample_result(sample, bank.name.clone(), String::new(), &judgement, recipe.expected_total(), tolerance_total));
    }
    let history_id = sample.history_id.ok_or("样本来源未指定")?;
    let store = app.state::<Store>();
    let detail = store.detail(history_id)?;
    if detail.summary.source == store::RecordSource::OfflineCandidate { return Err("候选调试记录不能作为独立发布验证样本".into()); }
    let burst = detail.burst.as_ref().ok_or("旧拍照点记录不能作为段配方的验证样本")?;
    let original = detail.summary.recipe_revision.as_deref().map(|h| store.recipe_snapshot(h)).transpose()?.flatten().ok_or("原始配方快照缺失")?;
    if !original.is_burst() || sample_tag(original.product_code, &original.plan_cameras(), &original.path_segments) != doc_sample_tag(&w.doc) {
        return Err("历史样本的段序列、计划相机或产品代码与候选不同".into());
    }
    if !burst_history::raw_complete(&detail, burst) { return Err("历史样本的原图不完整，不能整件复测".into()); }
    let measurer = measurer.map_err(|e| e.clone())?;
    let metrics = remeasure(&burst.frames, |frame| {
        measurer.measure(frame.camera, frame.seq, &burst_history::frame_image(app, &detail, frame.camera, frame.seq)?)
    });
    let judgement = judge_frames(&recipe.path_segments, recipe.expected_total(), &recipe.plan_cameras(), &metrics, &burst.judgement.alarms);
    Ok(sample_result(sample, format!("SN {}", detail.summary.sn), detail.summary.sn.clone(), &judgement, recipe.expected_total(), tolerance_total))
}

/// 段配方的发布前验证（`workspace_validate` 对段配方调用；旧拍照点配方仍走 `super::validation`）。
pub(super) fn validation(app: &AppHandle, w: &Workspace) -> Result<Validation, String> {
    let recipe = w.doc.build()?;
    let initial_environment = environment(app, &recipe)?;
    let cycle = app.state::<CycleHost>();
    let device = cycle::usable_cams(app, &recipe).and_then(|cams| cycle.camera.check_ready_at(&cams));
    let configs = cycle.camera.configs();
    let tags = current_camera_tags(app);
    let current: BTreeMap<String, Value> = configs.iter().map(|c| (c.id.clone(), state_value(c))).collect();
    let measurer = CandidateMeasurer::new(app, w, &recipe);
    let samples: Vec<SampleResult> = w.samples.iter()
        .map(|sample| validate_sample(app, w, &recipe, measurer.as_ref(), sample, &current).unwrap_or_else(|reason| failed_sample(sample, reason)))
        .collect();
    let base = cycle.recipe(&recipe.id).map(|r| r.revision_id.clone());
    let checks = burst_checks(w, BurstInputs { device, cameras: &tags, samples: &samples, base_matches: base == w.base_revision });
    if environment(app, &recipe)? != initial_environment { return Err("验证期间设备、标定或引擎发生变化，请重新验证".into()); }
    Ok(Validation {
        revision: w.revision,
        passed: checks.iter().all(|c| c.passed),
        checked_at: ly_plc::now_ms(),
        checks,
        samples,
        environment_tag: initial_environment,
        fixture: Vec::new(),
    })
}

/// 把一轮新采集记进样本库并选为验证样本（同一轮不重复加入由调用方先查）。
pub(super) fn bank_round(w: &mut Workspace, round: &CaptureRoundV2, current: &BTreeMap<String, Value>, expected: Verdict) -> Result<(), String> {
    check_sample_round(w, round, current)?;
    if w.sample_bank.len() >= SAMPLE_BANK_LIMIT { return Err(format!("最多保留 {SAMPLE_BANK_LIMIT} 组验证样本")); }
    let configs: Vec<Option<&Value>> = w.doc.plan_cameras().iter()
        .map(|c| round.cameras.iter().find(|r| &r.camera_id == c).map(|r| &r.camera_config)).collect();
    w.sample_bank.push(BankSample {
        id: round.round_id.clone(),
        name: format!("独立采集 {}", round.created_at),
        geometry_tag: doc_sample_tag(&w.doc),
        expected,
        created_at: round.created_at,
        capture_id: Some(round.round_id.clone()),
        camera_state: state_value(&configs),
    });
    w.samples.push(Sample { history_id: None, sample_id: Some(round.round_id.clone()), expected });
    w.changed();
    Ok(())
}

/// 段配方的“保存为独立验证样本”（`workspace_capture_sample` 对段配方调用）：样本是新采集轮次（CaptureRoundV2），
/// 帧留在采集轮次目录里，验证时按 (相机, seq) 读。
pub(super) fn capture_sample(app: AppHandle, id: String, revision: u64, round_id: String, expected: Verdict) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let Some(_gate) = workflow::wait_idle_plc_gate(&cycle, "加入验证样本")? else { return Err("请等待当前工件和 PLC 事务结束后加入验证样本".into()) };
    let round = cycle.camera.recipe_capture().get_burst(&round_id)?;
    let current: BTreeMap<String, Value> = cycle.camera.configs().iter().map(|c| (c.id.clone(), state_value(c))).collect();
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.expect(revision)?;
    if w.sample_bank.iter().any(|b| b.id == round_id) { return view(&app, w); }
    bank_round(&mut w, &round, &current, expected)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    view(&app, w)
}

/// 段配方发布前冻结（schema 4，D-SB-30）：每台计划相机的示教帧、标定（当量等于示教当量）与走廊，测量图按配方与各相机
/// 示教帧尺寸生成（`production::burst_graphs`，生产加载时重算比对）。
pub(super) fn freeze_bundle(app: &AppHandle, host: &WorkspaceHost, doc: &RecipeDoc, teaching: &[CameraTeaching]) -> Result<crate::release::ReleaseBundle, String> {
    let recipe = doc.build()?;
    recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
    let engine = app.state::<vision::VisionHost>().engine().ok_or("发布前必须加载兼容的图像核心库")?;
    let mut sizes = Vec::with_capacity(recipe.inspect.len());
    let mut cameras = Vec::with_capacity(recipe.inspect.len());
    for inspect in &recipe.inspect {
        let taught = teaching.iter().find(|t| t.camera == inspect.camera).ok_or_else(|| format!("相机 {} 没有示教帧，不能发布", inspect.camera))?;
        let external = vision::camera_calib_path(app, inspect)?;
        let source = if external.exists() {
            Some(serde_json::from_str::<Value>(&crate::fsio::read_text(&external).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?)
        } else { None };
        let reference = vision::camera_calib_key(inspect)?;
        let calibration = json!({"schemaVersion": 1, "camera": inspect.camera, "reference": reference, "mmPerPx": inspect.mm_per_px, "source": source});
        sizes.push(taught.frame.size);
        cameras.push(crate::release::CameraInput {
            camera: inspect.camera.clone(),
            image: crate::release::ResourceSource::File(image_file(host, &recipe.id, &taught.frame.id)?),
            image_id: taught.frame.id.clone(),
            calibration: crate::release::ResourceSource::Bytes(serde_json::to_vec(&calibration).map_err(|e| e.to_string())?),
        });
    }
    crate::release::publish_burst(&releases_root(app)?, crate::release::BurstPublishInput {
        recipe: doc.clone(),
        versions: crate::release::Versions { engine: engine.identity.clone(), graph: crate::production::GRAPH_VERSION_BURST.into() },
        validation_site: if crate::offline::enabled(app) { ValidationSite::Offline } else { ValidationSite::Real },
        graph: crate::release::ResourceSource::Bytes(serde_json::to_vec(&crate::production::burst_graphs(&recipe, &sizes)?).map_err(|e| e.to_string())?),
        cameras,
    })
}

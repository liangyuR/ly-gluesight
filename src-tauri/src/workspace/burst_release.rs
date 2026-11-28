//! 段配方的发布前检查与发布（D3）。
//!
//! 只服务段配方（`RecipeDoc.segments` 非空）。
//!
//! 验证项（决策记录第九节第 13 条）：
//! - 相机配置：计划相机解析、三台单视角独立设备（`check_burst_cameras`）；相机是否在线就绪不在发布时核对，接件时再查（D-SB-46）；
//! - 走廊示教：恰好 3 台计划相机，每台都选了示教帧、走廊完整落在帧内、有像素当量，且示教帧采集后设备配置没变；
//! - 段帧数：各段 N_exp 取自示教件已确认的分段（RB-3），逐段列出 N_exp 或待定原因；
//! - 胶偏参考 x_ref：只在有启用胶偏检查的直线段时列出参考值或不判的原因，不阻断（运行时只有按比例分段可信的多段配方
//!   判胶偏，单段配方整条胶路按未分段处理）。
//!
//! 结果验证不在这里：在历史页用任意配方重算历史记录。

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::LazyLock;

use super::burst::{offset_reference, segment_corridor_saved, untaught_pairs, CameraTeaching};
use super::*;
use crate::plan::{OffsetMeasure, PathSegment, SegmentKind, PLAN_CAMERAS};

fn kind_label(kind: SegmentKind) -> &'static str {
    match kind {
        SegmentKind::Straight => "直线",
        SegmentKind::Corner => "弯道",
    }
}

/// 段配方的基础验证项。`device` 不含在线就绪。走廊示教合格后才追加建图和试测结果。
pub(super) fn burst_checks(
    w: &Workspace,
    device: Result<(), String>,
    measurements: impl FnOnce() -> Vec<Check>,
) -> Vec<Check> {
    let mut checks = vec![Check {
        advisory: false,
        name: "相机配置".into(),
        teaching_targets: Vec::new(),
        passed: device.is_ok(),
        detail: device
            .err()
            .unwrap_or_else(|| "三台计划相机已配置；是否在线就绪在接件时核对".into()),
    }];
    // RB-8：没有示教件之前候选不能发布
    checks.push(Check {
        advisory: false,
        name: "示教件".into(),
        teaching_targets: Vec::new(),
        passed: w.teaching_piece.is_some(),
        detail: w.teaching_piece.as_ref().map_or_else(
            || "还没有示教件：先选示教件".into(),
            |p| format!("示教件 SN {}", p.sn),
        ),
    });
    let corridor = corridor_check(w);
    let corridors_ready = corridor.passed;
    checks.push(corridor);
    checks.push(segment_check(w));
    checks.extend(super::pass_align::alignment_check(w));
    checks.extend(x_ref_check(&w.doc));
    if corridors_ready {
        checks.extend(measurements());
    }
    checks
}

/// 走廊示教（规格 §1 Plan.cameras = 3、§4 检测走廊、D-SB-45）：每段每台检测相机都有示教帧、走廊完整落在帧内，
/// 每台相机有像素当量。示教后改了相机参数不拦发布。
fn corridor_check(w: &Workspace) -> Check {
    let doc = &w.doc;
    let mut problems = Vec::new();
    let mut teaching_targets = BTreeSet::new();
    if doc.inspect.len() != PLAN_CAMERAS {
        problems.push(format!(
            "段配方需要恰好 {PLAN_CAMERAS} 台计划相机共用一个触发，当前 {} 台",
            doc.inspect.len()
        ));
    }
    for (camera, segment) in untaught_pairs(w) {
        problems.push(format!(
            "第 {segment} 段相机 {camera}：走廊未示教或超出示教帧"
        ));
        teaching_targets.insert(TeachingTarget {
            segment: Some(segment),
            camera,
        });
    }
    for inspect in doc.inspect.iter().filter(|i| i.mm_per_px.is_none()) {
        problems.push(format!("相机 {}：缺像素当量", inspect.camera));
        teaching_targets.insert(TeachingTarget {
            segment: None,
            camera: inspect.camera.clone(),
        });
    }
    for issue in crate::heading::issues(&doc.segments, &doc.inspect) {
        problems.push(issue.message);
        teaching_targets.insert(TeachingTarget {
            segment: issue.segment,
            camera: issue.camera,
        });
    }
    let pairs: usize = doc
        .segments
        .iter()
        .map(|s| crate::plan::segment_cameras(s, &doc.plan_cameras()).len())
        .sum();
    Check {
        advisory: false,
        name: "走廊示教".into(),
        teaching_targets: teaching_targets.into_iter().collect(),
        passed: problems.is_empty(),
        detail: if problems.is_empty() {
            format!(
                "{pairs} 条段走廊都已示教，{} 台相机都有像素当量",
                doc.inspect.len()
            )
        } else {
            problems.join("；")
        },
    }
}

/// 段帧数（规格 §1 没有 N_exp 不能发布）：各段 N_exp 取示教件已确认分段的帧组数（RB-3；D-SB-45：生产按累计 N_exp 预测
/// 每帧所属段来选走廊）。逐段列出 N_exp 或待定原因。
fn segment_check(w: &Workspace) -> Check {
    let doc = &w.doc;
    let total = doc.expected_total();
    let head = match total {
        Some(total) => format!("整件预期 {total} 个帧组（PLC shotCount = {total}）"),
        None => "整件预期帧组数待示教件分段：先在分段步骤确认示教件边界".into(),
    };
    let multi = doc.segments.len() > 1;
    let last = doc.segments.len().saturating_sub(1);
    // 按示教件的笔结构（D-SB-62）：去掉过渡区、段与每笔首尾跳过的帧后一帧不剩的段，生产整段不检
    let empty: BTreeSet<u32> = match (total, w.teaching_piece.as_ref()) {
        (Some(total), Some(piece)) if !piece.passes.is_empty() => {
            crate::segmentation::proportional_coverage(
                &doc.segments,
                &doc.plan_cameras(),
                u64::from(total),
                doc.boundary_guard,
                &[],
                super::pieces::piece_pass_rules(w),
            )
            .zero_coverage_segments
            .into_iter()
            .collect()
        }
        _ => BTreeSet::new(),
    };
    let segments: Vec<String> = doc
        .segments
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let kind = if s.skip { "不检测" } else { kind_label(s.kind) };
            let offset_min = offset_frames_min(s, k > 0, k < last, doc.boundary_guard);
            match (s.expected_frames, total) {
                (Some(n), _) if !s.skip && empty.contains(&s.index) => format!(
                    "第 {} 段（{kind}）N_exp {n}：去掉过渡区与跳过帧后示教件上没有可判的帧，生产整段不检",
                    s.index
                ),
                (Some(n), _) if multi && !s.skip && s.kind == SegmentKind::Straight && n < offset_min => format!(
                    "第 {} 段（{kind}）N_exp {n}：短于 {offset_min} 帧，去掉过渡区、跳过帧与首尾各一帧后没有帧判胶偏，建议并入相邻弯道段",
                    s.index
                ),
                (Some(n), _) => format!("第 {} 段（{kind}）N_exp {n}", s.index),
                (None, _) => format!("第 {} 段（{kind}）待示教件分段", s.index),
            }
        })
        .chain(doc.segments.iter().flat_map(|s| {
            super::burst::heading_hints(doc, s, &w.calibration.rounds)
                .into_iter()
                .map(move |hint| format!("第 {} 段提示（不阻断）：{hint}", s.index))
        }))
        .collect();
    Check {
        advisory: false,
        name: "段帧数".into(),
        teaching_targets: Vec::new(),
        passed: total.is_some()
            && (!multi || doc.segments.iter().all(|s| s.expected_frames.is_some())),
        detail: format!("{head}；{}", segments.join("；")),
    }
}

/// 直线段至少要多少帧，运行时才剩帧判胶偏（D-SB-40）：每一头去掉过渡区与本段跳过帧中较多的那一截（两者从同一端数起、
/// 互相重叠，`resolve_regions`），胶偏判定再去掉区域首尾各一帧（`burst_judge::region_offset`），至少留 1 帧。
fn offset_frames_min(s: &PathSegment, before: bool, after: bool, guard: u32) -> u32 {
    let head = s.skip_head.max(if before { guard } else { 0 });
    let tail = s.skip_tail.max(if after { guard } else { 0 });
    head + tail + 3
}

/// 胶偏参考 x_ref（规格 §0/§1、D-SB-18；示教件兼作标准件，RB-3）：只在有启用胶偏检查的直线段时列出，只作说明、不阻断发布——
/// 运行时单段配方整条胶路按未分段处理、不判胶偏，多段配方暂不计算 x_ref（该段标胶偏未判），ToEdge 填了图纸胶边距即按它判。
fn x_ref_check(doc: &RecipeDoc) -> Option<Check> {
    let cameras = doc.plan_cameras();
    let single = doc.segments.len() == 1;
    let segmented = doc.segments.iter().all(|s| s.expected_frames.is_some());
    let items: Vec<String> = doc.segments.iter()
        .filter(|s| !s.skip && s.kind == SegmentKind::Straight && s.checks.offset.as_ref().is_some_and(|o| o.enabled))
        .filter_map(|s| offset_reference(s, &cameras).map(|reference| (s.index, reference)))
        .map(|(index, reference)| match (reference.x_ref.zip(reference.spread), reference.measure) {
            (_, OffsetMeasure::ToEdge) if reference.edge.as_ref().is_some_and(|e| e.nominal_mm.is_some()) => format!(
                "第 {index} 段：按图纸胶边距 {:.2} mm 判（相机 {}）",
                reference.edge.as_ref().and_then(|e| e.nominal_mm).unwrap_or_default(),
                reference.camera
            ),
            (Some((x_ref, spread)), _) => format!("第 {index} 段 x_ref {x_ref:.3} ± {spread:.3} mm（相机 {}）", reference.camera),
            (None, OffsetMeasure::ToEdge) if reference.edge.is_none() => {
                format!("第 {index} 段：没有配置找边参数，“到边缘距离”测不了，结果标胶偏未判")
            }
            (None, _) if single => format!("第 {index} 段：单段配方运行时整条胶路按未分段处理、不判胶偏，不需要 x_ref"),
            (None, _) if segmented => format!("第 {index} 段：多段配方暂不计算 x_ref，运行时按比例分段，该段标胶偏未判（没有参考值）"),
            (None, _) => format!("第 {index} 段：示教件分段未确认，运行时整条胶路按未分段处理、不判胶偏，暂不需要 x_ref"),
        })
        .collect();
    (!items.is_empty()).then(|| Check {
        advisory: false,
        name: "胶偏参考 x_ref".into(),
        teaching_targets: Vec::new(),
        passed: true,
        detail: items.join("；"),
    })
}

/// 一（段, 相机）用候选走廊在自己的示教帧上试测的结果：测成为 Ok，没测成为原因。
#[derive(Clone)]
pub(super) struct TeachTrial {
    pub segment: u32,
    pub camera: String,
    pub outcome: Result<(), String>,
    pub incomplete: bool,
}

/// “示教帧试测”（D-SB-59）：与生产预热同一口径。部分没测成只提醒——发布后这些段不测、帧记未测，结论只覆盖其余段；
/// 一段都没测成时拦发布（来件只能录原图、回检测错误）。没有可试测的组合（走廊没示教完由“走廊示教”拦）时不出这一项。
pub(super) fn teach_trial_check(trials: &[TeachTrial]) -> Option<Check> {
    if trials.is_empty() {
        return None;
    }
    let failed: Vec<String> = trials
        .iter()
        .filter_map(|t| {
            t.outcome
                .as_ref()
                .err()
                .map(|e| format!("第 {} 段相机 {}：{e}", t.segment, t.camera))
        })
        .collect();
    let all = failed.len() == trials.len();
    let incomplete: Vec<String> = trials
        .iter()
        .filter(|t| t.incomplete)
        .map(|t| format!("第 {} 段相机 {}", t.segment, t.camera))
        .collect();
    Some(Check {
        advisory: (!failed.is_empty() || !incomplete.is_empty()) && !all,
        name: "示教帧试测".into(),
        teaching_targets: Vec::new(),
        passed: failed.is_empty() && incomplete.is_empty(),
        detail: if failed.is_empty() && !incomplete.is_empty() {
            format!(
                "存在可用测量，但部分测站未完整量测，请复核截断/未知证据：{}",
                incomplete.join("；")
            )
        } else if failed.is_empty() {
            format!("{} 组（段, 相机）在各自示教帧上都测成", trials.len())
        } else if all {
            format!(
                "每一段在示教帧上都没测成，发布后来件只能录原图、回检测错误：{}",
                failed.join("；")
            )
        } else {
            format!(
                "{} / {} 组在示教帧上没测成；可以发布，但生产时这些段不测（帧记未测，结论只覆盖其余段），建议在示教页重新取色或调整走廊后再发布：{}",
                failed.len(),
                trials.len(),
                failed.join("；")
            )
        },
    })
}

pub(super) fn graph_and_trial_checks<'a>(
    w: &'a Workspace,
    recipe: &Recipe,
    measure: impl FnOnce(vision::CorridorSet, Vec<Vec<&'a CameraTeaching>>) -> Vec<Check>,
) -> Vec<Check> {
    let mut teaching_targets = BTreeSet::new();
    let result = release_teaching_frames(recipe, &w.camera_teaching).and_then(|frames| {
        let sizes: Vec<_> = frames
            .iter()
            .map(|taught| taught.first().map_or([0, 0], |t| t.frame.size))
            .collect();
        let (graphs, unavailable) = vision::CorridorSet::build_available(recipe, &sizes);
        if unavailable.is_empty() {
            return Ok((graphs, frames));
        }
        let problems: Vec<_> = unavailable
            .into_iter()
            .map(|issue| {
                let context = format!("第 {} 段相机 {}：", issue.segment, issue.camera);
                teaching_targets.insert(TeachingTarget {
                    segment: Some(issue.segment),
                    camera: issue.camera,
                });
                if issue.reason.starts_with(&context) {
                    issue.reason
                } else {
                    context + &issue.reason
                }
            })
            .collect();
        Err(problems.join("；"))
    });
    let (passed, detail, trial) = match result {
        Ok((graphs, frames)) => (
            true,
            format!("{} 组（段, 相机）测量图已构建", graphs.iter().count()),
            measure(graphs, frames),
        ),
        Err(error) => (false, error, Vec::new()),
    };
    let mut checks = vec![Check {
        name: "测量图".into(),
        teaching_targets: teaching_targets.into_iter().collect(),
        passed,
        detail,
        advisory: false,
    }];
    checks.extend(trial);
    checks
}

#[derive(Clone)]
pub(super) struct CachedTeachingFrame {
    fingerprint: [u8; 32],
    trial: Option<TeachTrial>,
}

/// 每次读取实际文件。候选、核心库和文件内容都相同时，复用该帧的解码校验与试测结果。
fn measurement_checks(
    app: &AppHandle,
    w: &Workspace,
    recipe: &Recipe,
    engine: Option<&vision::Engine>,
) -> Vec<Check> {
    type Key = (u64, u64, Option<String>);
    type Frames = HashMap<(usize, u32), CachedTeachingFrame>;
    static CACHE: LazyLock<Mutex<HashMap<String, (Key, Frames)>>> = LazyLock::new(Default::default);
    let key: Key = (
        w.instance_id,
        w.revision,
        engine.map(|engine| engine.identity.clone()),
    );
    let mut cached = CACHE
        .lock()
        .unwrap()
        .get(&w.doc.id)
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    graph_and_trial_checks(w, recipe, |graphs, teaching| {
        let graphs = graphs.with_channels(&super::burst::teaching_channels(w));
        let host = app.state::<WorkspaceHost>();
        let run = engine
            .map(|_| vision::unique_run_id(&format!("publish-trial-{}-{}", recipe.id, w.revision)));
        let checks = teaching_image_checks(
            recipe,
            &teaching,
            &mut cached,
            |frame| {
                let path = image_file(&host, &recipe.id, &frame.frame.id)?;
                crate::release::read_resource_file(&path)
            },
            |(index, segment), frame, mut image| {
                let (engine, run) = (engine?, run.as_ref()?);
                let graph = graphs.get(index, segment)?;
                image.capture = frame.frame.mix_capture.clone();
                let mut incomplete = false;
                let outcome = vision::measure_frame(
                    engine,
                    graph,
                    &image,
                    0,
                    &format!("{run}-c{index}-s{segment}"),
                )
                .and_then(|m| {
                    incomplete = m.metrics.measurement_incomplete();
                    if m.metrics.has_usable_evidence() {
                        Ok(())
                    } else {
                        Err(m.metrics.error.unwrap_or_else(|| "没有可用测量证据".into()))
                    }
                });
                Some(TeachTrial {
                    segment,
                    camera: frame.camera.clone(),
                    outcome,
                    incomplete,
                })
            },
        );
        CACHE
            .lock()
            .unwrap()
            .insert(w.doc.id.clone(), (key, cached));
        checks
    })
}

pub(super) fn teaching_image_checks(
    recipe: &Recipe,
    teaching: &[Vec<&CameraTeaching>],
    cached: &mut HashMap<(usize, u32), CachedTeachingFrame>,
    mut read: impl FnMut(&CameraTeaching) -> Result<Vec<u8>, String>,
    mut measure: impl FnMut((usize, u32), &CameraTeaching, FrameImage) -> Option<TeachTrial>,
) -> Vec<Check> {
    let mut trials = Vec::new();
    let mut problems = Vec::new();
    let mut teaching_targets = Vec::new();
    let mut frame_count = 0;
    for (index, (inspect, frames)) in recipe.inspect.iter().zip(teaching).enumerate() {
        let size = frames.first().map(|frame| frame.frame.size);
        for frame in frames {
            let segment = frame.segment.expect("已选定段示教帧");
            let key = (index, segment);
            frame_count += 1;
            let checked = (|| {
                if !crate::recipe::valid_image_id(&frame.frame.id) {
                    return Err("示教图编号无效".to_owned());
                }
                if Some(frame.frame.size) != size {
                    return Err("同一相机各段的冻结尺寸不同".to_owned());
                }
                let bytes = read(frame)?;
                let fingerprint = Sha256::digest(&bytes).into();
                if let Some(previous) = cached
                    .get(&key)
                    .filter(|entry| entry.fingerprint == fingerprint)
                {
                    return Ok(previous.trial.clone());
                }
                let (image, _) =
                    crate::release::decode_teaching_image(&bytes, inspect, segment, size)?;
                let trial = measure(key, frame, image);
                cached.insert(
                    key,
                    CachedTeachingFrame {
                        fingerprint,
                        trial: trial.clone(),
                    },
                );
                Ok(trial)
            })();
            match checked {
                Ok(trial) => trials.extend(trial),
                Err(error) => {
                    cached.remove(&key);
                    problems.push(format!("第 {segment} 段相机 {}：{error}", inspect.camera));
                    teaching_targets.push(TeachingTarget {
                        segment: Some(segment),
                        camera: inspect.camera.clone(),
                    });
                }
            }
        }
    }
    let passed = problems.is_empty();
    let mut checks = vec![Check {
        name: "示教图像".into(),
        passed,
        advisory: false,
        teaching_targets,
        detail: if passed {
            format!("{frame_count} 张示教图可读取，尺寸与冻结记录一致")
        } else {
            problems.join("；")
        },
    }];
    if passed {
        checks.extend(teach_trial_check(&trials));
    }
    checks
}

/// 拦发布的第一项：没满足且不是只提醒的项（D-SB-59）。`workspace_publish` 按它决定能否发布。
pub(super) fn publish_blocker(checks: &[Check]) -> Option<&Check> {
    checks.iter().find(|c| !c.passed && !c.advisory)
}

/// 发布前检查（D-SB-46，发布时当场核对，不要求相机在线）：相机配置、走廊示教、段标定、x_ref，
/// 以及示教帧试测（D-SB-59，部分没测成只提醒）。结果随待生效版本存档（`Release.validation`）。
pub(super) fn publish_checks(app: &AppHandle, w: &Workspace) -> Result<Validation, String> {
    let recipe = w.doc.build()?;
    let (environment_tag, engine) = environment(app, &recipe)?;
    let device = cycle::usable_cams(app, &recipe).map(|_| ());
    let mut checks = burst_checks(w, device, || {
        measurement_checks(app, w, &recipe, engine.as_deref())
    });
    checks.extend(super::mix_teaching::publish_checks(
        w,
        &app.state::<CycleHost>().camera.configs(),
    ));
    checks.push(Check {
        name: "图像核心库".into(),
        teaching_targets: Vec::new(),
        passed: engine.is_some(),
        detail: if engine.is_some() {
            "已加载兼容的图像核心库".into()
        } else {
            "发布前必须加载兼容的图像核心库".into()
        },
        advisory: false,
    });
    Ok(Validation {
        revision: w.revision,
        passed: publish_blocker(&checks).is_none(),
        checked_at: ly_plc::now_ms(),
        checks,
        environment_tag,
    })
}

/// 发布前检查的结果（验证与发布页展示，不改候选）。示教帧试测要跑图像引擎，不占主线程。
#[tauri::command(async)]
pub fn workspace_publish_checks(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<Vec<Check>, String> {
    let w = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        let w = items.get(&id).ok_or("候选配置不存在")?;
        w.expect(revision)?;
        w.clone()
    };
    if !w.doc.is_burst() {
        return Err("旧拍照点配方已停用，请创建段配方".into());
    }
    publish_checks(&app, &w).map(|v| v.checks)
}

fn release_teaching_frames<'a>(
    recipe: &Recipe,
    teaching: &'a [CameraTeaching],
) -> Result<Vec<Vec<&'a CameraTeaching>>, String> {
    let cameras = recipe.plan_cameras();
    recipe
        .inspect
        .iter()
        .map(|inspect| {
            recipe
                .path_segments
                .iter()
                .filter(|segment| {
                    crate::plan::segment_cameras(segment, &cameras).contains(&inspect.camera)
                })
                .map(|s| {
                    teaching
                        .iter()
                        .find(|t| t.camera == inspect.camera && t.segment == Some(s.index))
                        .filter(|_| segment_corridor_saved(inspect, s.index, teaching))
                        .ok_or_else(|| {
                            format!(
                                "第 {} 段相机 {} 没有示教帧或走廊，不能发布",
                                s.index, inspect.camera
                            )
                        })
                })
                .collect()
        })
        .collect()
}

/// 段配方发布前冻结（schema 4，D-SB-30）：每台计划相机的示教帧、标定（当量等于示教当量）与走廊，测量图按配方与各相机
/// 示教帧尺寸生成（`production::burst_graphs`，生产加载时重算比对）。
pub(super) fn freeze_bundle(
    app: &AppHandle,
    host: &WorkspaceHost,
    doc: &RecipeDoc,
    teaching: &[CameraTeaching],
) -> Result<crate::release::ReleaseBundle, String> {
    let recipe = doc.build()?;
    recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
    let engine = app
        .state::<vision::VisionHost>()
        .engine()
        .ok_or("发布前必须加载兼容的图像核心库")?;
    let mut sizes = Vec::with_capacity(recipe.inspect.len());
    let mut cameras = Vec::with_capacity(recipe.inspect.len());
    for (inspect, taught) in recipe
        .inspect
        .iter()
        .zip(release_teaching_frames(&recipe, teaching)?)
    {
        let external = vision::camera_calib_path(app, inspect)?;
        let source = if external.exists() {
            Some(
                serde_json::from_str::<Value>(
                    &crate::fsio::read_text(&external).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let reference = vision::station_key(&inspect.camera)?.reference();
        let mut calibration = json!({"schemaVersion": 1, "camera": inspect.camera, "reference": reference, "mmPerPx": inspect.mm_per_px, "source": source, "acquisition": crate::production::acquisition_tag(&app.state::<CycleHost>().camera.configs().into_iter().find(|c| c.id == inspect.camera).ok_or("发布相机不存在")?)});
        for t in &taught {
            if let Some(segment) = t.segment.filter(|&s| {
                inspect
                    .segment_corridor(s)
                    .is_some_and(|c| c.mix_check.is_some())
            }) {
                if let Some(source) = &t.frame.mix_capture {
                    calibration["mixTeachSources"][segment.to_string()] = json!(source);
                }
            }
        }
        // 不检测任何一段的相机没有示教帧，尺寸记 [0, 0]
        sizes.push(taught.first().map_or([0, 0], |t| t.frame.size));
        cameras.push(crate::release::CameraInput {
            camera: inspect.camera.clone(),
            calibration: crate::release::ResourceSource::Bytes(
                serde_json::to_vec(&calibration).map_err(|e| e.to_string())?,
            ),
            corridors: taught
                .iter()
                .map(|t| {
                    Ok(crate::release::CorridorInput {
                        segment: t.segment.unwrap_or_default(),
                        image: crate::release::ResourceSource::File(image_file(
                            host,
                            &recipe.id,
                            &t.frame.id,
                        )?),
                        image_id: t.frame.id.clone(),
                        expected_size: Some(t.frame.size),
                    })
                })
                .collect::<Result<_, String>>()?,
        });
    }
    crate::release::publish_burst(
        &releases_root(app)?,
        crate::release::BurstPublishInput {
            recipe: doc.clone(),
            versions: crate::release::Versions {
                engine: engine.identity.clone(),
                graph: crate::production::GRAPH_VERSION_BURST.into(),
            },
            validation_site: ValidationSite::Real,
            graph: crate::release::ResourceSource::Bytes(
                serde_json::to_vec(&crate::production::burst_graphs(&recipe, &sizes)?)
                    .map_err(|e| e.to_string())?,
            ),
            cameras,
        },
    )
}

//! 比例分段离线评估（D-SB-40）。不是测试：结论取决于现场数据，没有固定的回归可防，所以不放在 #[test] 里。
//!
//! 读一个项目里某个候选的 workspace.json：段列表（类型、容差、boundary_guard）与采集页已确认的手动分段（真值）。
//! 对每个已确认轮次做留一评估：用其余已确认轮次按多轮标定的规则（`calibrate_counts`，均值四舍五入）得出各段 N_exp，
//! 再用生产的比例分段器（`Proportional`）按这一轮的帧组数推算边界，与它的手动边界比较；同时列出“按原帧号”
//! （累计 N_exp 不缩放）的误差作对照。有分段模板时再按图像把模板边界对齐到各已确认轮次（D-SB-44 对齐复核的建议），
//! 与手动边界比较。读项目 recipe-capture 下的原图，不需要 lyFlow。
//!
//! 图像对齐部分同时评估 D-SB-51 的自动确认（容许帧数取配方 boundary_guard）：列出每个边界的对齐误差、清晰度与
//! 需复核原因；统计可信边界的数量与比例、可信边界中误差超过 boundary_guard 的个数（验收要求为 0）、全部边界可信
//! 可整轮自动确认的轮次数；按误差是否超过 boundary_guard 分两组列出清晰度分布（min / 中位 / max），并给出在
//! 可信边界误差全不超过 boundary_guard 的前提下可信比例最高的 `ALIGN_SHARPNESS`。至少要 2 个已确认轮次，
//! 其中一个是分段模板轮。
//!
//! ```powershell
//! cargo run --manifest-path src-tauri/Cargo.toml --example segment_eval -- C:\ProgramData\ly\GlueSight-dev\projects\seg-new14 NEW-14
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use gluesight_lib::probe_api::{
    align_cuts, calibrate_counts, downsample, load_image, FrameGroup, Proportional, RecipeDoc,
    ReviewReason, SegInput, SegOutcome, SegmentKind, Segmenter, Sequence, ALIGN_FACTOR,
    ALIGN_SHARPNESS,
};
use serde_json::Value;

struct Truth {
    round: String,
    total: u64,
    cuts: Vec<u64>,
}

fn segment_counts(truth: &Truth) -> Vec<u32> {
    let mut previous = 0;
    truth
        .cuts
        .iter()
        .chain(std::iter::once(&truth.total))
        .map(|&cut| {
            let count = cut - previous;
            previous = cut;
            count as u32
        })
        .collect()
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [project, candidate] = args.as_slice() else {
        return Err("用法：segment_eval <项目目录> <候选编号>".into());
    };
    let path = PathBuf::from(project)
        .join("workspaces")
        .join(candidate)
        .join("workspace.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
    let workspace: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let doc: RecipeDoc = serde_json::from_value(workspace["doc"].clone())
        .map_err(|e| format!("配方文档无法解析：{e}"))?;
    let segments = doc.segments.clone();
    if segments.len() < 2 {
        return Err(format!(
            "候选 {candidate} 只有 {} 段：先在段列表里按胶路划好直线段 / 弯道段",
            segments.len()
        ));
    }

    let rounds = workspace["captureReview"]["rounds"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut truths: Vec<Truth> = rounds
        .iter()
        .filter(|(_, review)| review["status"] == "confirmed")
        .filter_map(|(round, review)| {
            let cuts: Vec<u64> = review["cuts"]
                .as_array()?
                .iter()
                .filter_map(Value::as_u64)
                .collect();
            let total = review["groupCount"].as_u64()?;
            (cuts.len() + 1 == segments.len()).then(|| Truth {
                round: round.clone(),
                total,
                cuts,
            })
        })
        .collect();
    truths.sort_by(|a, b| a.round.cmp(&b.round));
    if truths.len() < 2 {
        return Err(format!(
            "已确认手动分段的轮次只有 {} 轮：留一评估至少要 2 轮，请先在采集页确认分段",
            truths.len()
        ));
    }

    let guard = u64::from(doc.boundary_guard);
    let tolerance: u64 = segments.iter().map(|s| u64::from(s.tolerance)).sum();
    let kinds: Vec<&str> = segments
        .iter()
        .map(|s| {
            if s.kind == SegmentKind::Straight {
                "直线"
            } else {
                "弯道"
            }
        })
        .collect();
    println!(
        "候选 {candidate}：{} 段（{}），Σtol {tolerance}，boundary_guard {guard}，已确认轮次 {}",
        segments.len(),
        kinds.join(" / "),
        truths.len()
    );
    println!(
        "{:<26} {:>4} {:>4} {:>6}  {:<28} {:<28} {:<20} 原帧号误差",
        "轮次", "帧组", "ΣN", "容差内", "手动边界", "比例边界", "比例误差"
    );

    let mut proportional_errors: BTreeMap<u64, usize> = BTreeMap::new();
    let mut fixed_errors: BTreeMap<u64, usize> = BTreeMap::new();
    let mut outside = 0;
    for (held, truth) in truths.iter().enumerate() {
        let mut calibrated = segments.clone();
        for (k, segment) in calibrated.iter_mut().enumerate() {
            let counts: Vec<u32> = truths
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != held)
                .map(|(_, t)| segment_counts(t)[k])
                .collect();
            segment.expected_frames = Some(
                calibrate_counts(&counts, 0, segment.tolerance)
                    .map_err(|e| format!("{e:?}"))?
                    .n_exp,
            );
        }
        let expected: u64 = calibrated
            .iter()
            .filter_map(|s| s.expected_frames)
            .map(u64::from)
            .sum();
        let groups: Vec<FrameGroup> = (1..=truth.total)
            .map(|seq| FrameGroup {
                seq,
                present: vec![true; 3],
                incomplete: false,
            })
            .collect();
        let found: Vec<(u64, f32)> = match Proportional.segment(&SegInput {
            groups: &groups,
            segments: &calibrated,
        }) {
            SegOutcome::Boundaries(boundaries) => boundaries
                .iter()
                .map(|b| (b.frame_seq, b.confidence))
                .collect(),
            SegOutcome::Unsegmented => Vec::new(),
        };
        let trusted = found.iter().all(|&(_, confidence)| confidence >= 0.5);
        if !trusted {
            outside += 1;
        }
        let mut cumulative = 0;
        let fixed: Vec<u64> = calibrated[..calibrated.len() - 1]
            .iter()
            .map(|s| {
                cumulative += u64::from(s.expected_frames.unwrap_or(0));
                cumulative
            })
            .collect();
        let errors: Vec<u64> = found
            .iter()
            .zip(&truth.cuts)
            .map(|(&(seq, _), &cut)| seq.abs_diff(cut))
            .collect();
        let fixed_err: Vec<u64> = fixed
            .iter()
            .zip(&truth.cuts)
            .map(|(&seq, &cut)| seq.abs_diff(cut))
            .collect();
        if trusted {
            errors
                .iter()
                .for_each(|&e| *proportional_errors.entry(e).or_default() += 1);
        }
        fixed_err
            .iter()
            .for_each(|&e| *fixed_errors.entry(e).or_default() += 1);
        println!(
            "{:<26} {:>4} {:>4} {:>6}  {:<28} {:<28} {:<20} {:?}",
            truth.round,
            truth.total,
            expected,
            if trusted { "是" } else { "否" },
            format!("{:?}", truth.cuts),
            format!(
                "{:?}",
                found.iter().map(|&(seq, _)| seq).collect::<Vec<_>>()
            ),
            format!("{errors:?}"),
            fixed_err
        );
    }

    let summarize = |name: &str, errors: &BTreeMap<u64, usize>| {
        let total: usize = errors.values().sum();
        let within = |limit: u64| {
            errors
                .iter()
                .filter(|(&e, _)| e <= limit)
                .map(|(_, &n)| n)
                .sum::<usize>()
        };
        let max = errors.keys().max().copied().unwrap_or(0);
        println!(
            "{name}：{total} 个边界，误差 0 帧 {}，≤1 帧 {}，≤guard({guard}) 帧 {}，最大 {max} 帧；分布 {errors:?}",
            within(0),
            within(1),
            within(guard)
        );
    };
    summarize("比例边界（只计容差内的轮次）", &proportional_errors);
    summarize("按原帧号（对照）", &fixed_errors);
    println!(
        "帧组数超出 ΣN_exp ± Σtol、整件按“分段不确定”处理的轮次：{outside} / {}",
        truths.len()
    );

    let template = &workspace["captureReview"]["template"];
    let (Some(template_round), Some(template_cuts)) =
        (template["roundId"].as_str(), template["cuts"].as_array())
    else {
        println!("没有分段模板：跳过图像对齐评估");
        return Ok(());
    };
    let template_cuts: Vec<u64> = template_cuts.iter().filter_map(Value::as_u64).collect();
    let cameras = doc.plan_cameras();
    let capture = PathBuf::from(project).join("recipe-capture");
    let sequences = |round: &str, groups: u64| -> Vec<Sequence> {
        cameras
            .iter()
            .map(|camera| {
                (1..=groups)
                    .map(|seq| {
                        load_image(
                            &capture
                                .join(round)
                                .join(camera)
                                .join(format!("{seq:06}.pgm")),
                        )
                        .ok()
                        .map(|image| downsample(&image, ALIGN_FACTOR))
                    })
                    .collect()
            })
            .collect()
    };
    let Some(template_truth) = truths.iter().find(|t| t.round == template_round) else {
        println!("模板轮 {template_round} 不在已确认轮次中：跳过图像对齐评估");
        return Ok(());
    };
    let missing = |images: &[Sequence]| images.iter().flatten().filter(|f| f.is_none()).count();
    let template_images = sequences(template_round, template_truth.total);
    let mut aligned_errors: BTreeMap<u64, usize> = BTreeMap::new();
    let mut checked: Vec<Vec<Checked>> = Vec::new();
    println!(
        "图像对齐（模板 {template_round}，边界 {template_cuts:?}，缺图 {} 帧）：",
        missing(&template_images)
    );
    for truth in truths.iter().filter(|t| t.round != template_round) {
        let images = sequences(&truth.round, truth.total);
        let aligned = align_cuts(&template_images, &images, &template_cuts, guard);
        let round: Vec<Checked> = aligned
            .iter()
            .zip(&truth.cuts)
            .map(|(a, &cut)| Checked {
                error: a.cut.abs_diff(cut),
                sharpness: a.sharpness,
                reason: a.reason,
            })
            .collect();
        round
            .iter()
            .for_each(|c| *aligned_errors.entry(c.error).or_default() += 1);
        println!(
            "  {:<26} 缺图 {} 手动 {:?} 对齐 {:?} 误差 {:?} 清晰度 [{}] {}",
            truth.round,
            missing(&images),
            truth.cuts,
            aligned.iter().map(|a| a.cut).collect::<Vec<_>>(),
            round.iter().map(|c| c.error).collect::<Vec<_>>(),
            round
                .iter()
                .map(|c| format!("{:.2}", c.sharpness))
                .collect::<Vec<_>>()
                .join(", "),
            if round.is_empty() {
                "对齐失败".to_string()
            } else {
                round
                    .iter()
                    .map(|c| reason_label(c.reason))
                    .collect::<Vec<_>>()
                    .join(" / ")
            }
        );
        checked.push(round);
    }
    summarize("图像对齐（相对模板轮）", &aligned_errors);
    report_auto_confirm(&checked, guard);
    Ok(())
}

/// 一个边界的对齐结果与手动真值的比较。
struct Checked {
    error: u64,
    sharpness: f64,
    reason: Option<ReviewReason>,
}

impl Checked {
    /// 不看清晰度时是否可信：没被挪动、与比例换算相差不超过 boundary_guard。
    fn candidate(&self) -> bool {
        matches!(self.reason, None | Some(ReviewReason::Unclear))
    }
}

fn reason_label(reason: Option<ReviewReason>) -> &'static str {
    match reason {
        None => "可信",
        Some(ReviewReason::Moved) => "挪动",
        Some(ReviewReason::Deviates) => "偏离比例",
        Some(ReviewReason::Unclear) => "不清晰",
    }
}

fn spread(values: &[f64]) -> String {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    match (sorted.first(), sorted.last()) {
        (Some(min), Some(max)) => format!(
            "{} 个，min {min:.2} / 中位 {:.2} / max {max:.2}",
            sorted.len(),
            sorted[sorted.len() / 2]
        ),
        _ => "0 个".into(),
    }
}

/// D-SB-51 自动确认统计：`trusted` 判定一个边界是否可信，返回（可信数，其中误差超过 guard 的数，全部边界可信的轮次数）。
fn tally(
    checked: &[Vec<Checked>],
    guard: u64,
    trusted: impl Fn(&Checked) -> bool,
) -> (usize, usize, usize) {
    let all = checked.iter().flatten();
    let count = all.clone().filter(|c| trusted(c)).count();
    let wrong = all.filter(|c| trusted(c) && c.error > guard).count();
    let rounds = checked
        .iter()
        .filter(|round| !round.is_empty() && round.iter().all(&trusted))
        .count();
    (count, wrong, rounds)
}

fn report_auto_confirm(checked: &[Vec<Checked>], guard: u64) {
    let all: Vec<&Checked> = checked.iter().flatten().collect();
    let total = all.len();
    let percent = |n: usize| {
        if total == 0 {
            0.0
        } else {
            n as f64 * 100.0 / total as f64
        }
    };
    let (trusted, wrong, rounds) = tally(checked, guard, |c| c.reason.is_none());
    let reasons = [
        ReviewReason::Moved,
        ReviewReason::Deviates,
        ReviewReason::Unclear,
    ]
    .map(|r| {
        format!(
            "{} {}",
            reason_label(Some(r)),
            all.iter().filter(|c| c.reason == Some(r)).count()
        )
    });
    println!(
        "D-SB-51 自动确认（ALIGN_SHARPNESS {ALIGN_SHARPNESS}，容许 boundary_guard {guard}）：{total} 个边界，可信 {trusted}（{:.0}%），\
         可信中误差 > guard {wrong}（验收要求 0）；需复核 {}（{}）",
        percent(trusted),
        total - trusted,
        reasons.join("，")
    );
    println!(
        "全部边界可信、可整轮自动确认的轮次：{rounds} / {}",
        checked.len()
    );

    let sharpness = |close: bool| -> Vec<f64> {
        all.iter()
            .filter(|c| (c.error <= guard) == close)
            .map(|c| c.sharpness)
            .collect()
    };
    println!(
        "清晰度分布：误差 ≤ guard {}；误差 > guard {}",
        spread(&sharpness(true)),
        spread(&sharpness(false))
    );
    let candidates: Vec<&Checked> = all.iter().copied().filter(|c| c.candidate()).collect();
    let worst = candidates
        .iter()
        .filter(|c| c.error > guard)
        .map(|c| c.sharpness)
        .reduce(f64::max);
    let lowest = candidates.iter().map(|c| c.sharpness).reduce(f64::min);
    println!(
        "未挪动且不偏离比例的边界（只差清晰度一条）：{}",
        spread(&candidates.iter().map(|c| c.sharpness).collect::<Vec<_>>())
    );
    let (label, (count, wrong, rounds)) = match (worst, lowest) {
        (Some(worst), _) => (
            format!("> {worst:.2}（其中误差 > guard 的边界清晰度最高为 {worst:.2}）"),
            tally(checked, guard, |c| c.candidate() && c.sharpness > worst),
        ),
        (None, Some(lowest)) => (
            format!("≤ {lowest:.2} 均可（这些边界误差都 ≤ guard，阈值下限只受样本量限制）"),
            tally(checked, guard, |c| c.candidate() && c.sharpness >= lowest),
        ),
        (None, None) => {
            println!("没有只差清晰度一条的边界：无法校准 ALIGN_SHARPNESS");
            return;
        }
    };
    println!(
        "建议 ALIGN_SHARPNESS {label}：可信 {count}（{:.0}%），误差 > guard {wrong}，整轮自动确认 {rounds} / {}",
        percent(count),
        checked.len()
    );
}

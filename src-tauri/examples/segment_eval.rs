//! 比例分段离线评估（D-SB-40）。不是测试：结论取决于现场数据，没有固定的回归可防，所以不放在 #[test] 里。
//!
//! 读一个项目里某个候选的 workspace.json：段列表（类型、容差、boundary_guard）与采集页已确认的手动分段（真值）。
//! 对每个已确认轮次做留一评估：用其余已确认轮次按多轮标定的规则（`calibrate_counts`，均值四舍五入）得出各段 N_exp，
//! 再用生产的比例分段器（`Proportional`）按这一轮的帧组数推算边界，与它的手动边界比较；同时列出“按原帧号”
//! （累计 N_exp 不缩放）的误差作对照。有分段模板时再按图像把模板边界对齐到各已确认轮次（D-SB-44 对齐复核的建议），
//! 与手动边界比较。读项目 recipe-capture 下的原图，不需要 lyFlow。
//!
//! ```powershell
//! cargo run --manifest-path src-tauri/Cargo.toml --example segment_eval -- C:\ProgramData\ly\GlueSight-dev\projects\seg-new14 NEW-14
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use gluesight_lib::probe_api::{
    align_cuts, calibrate_counts, downsample, load_image, FrameGroup, Proportional, RecipeDoc,
    SegInput, SegOutcome, SegmentKind, Segmenter, Sequence, ALIGN_FACTOR,
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
    let template_images = sequences(template_round, template_truth.total);
    let mut aligned_errors: BTreeMap<u64, usize> = BTreeMap::new();
    println!("图像对齐（模板 {template_round}，边界 {template_cuts:?}）：");
    for truth in truths.iter().filter(|t| t.round != template_round) {
        let aligned = align_cuts(
            &template_images,
            &sequences(&truth.round, truth.total),
            &template_cuts,
        );
        let errors: Vec<u64> = aligned
            .iter()
            .zip(&truth.cuts)
            .map(|(a, &cut)| a.cut.abs_diff(cut))
            .collect();
        errors
            .iter()
            .for_each(|&e| *aligned_errors.entry(e).or_default() += 1);
        println!(
            "  {:<26} 手动 {:?} 对齐 {:?} 误差 {:?}",
            truth.round,
            truth.cuts,
            aligned.iter().map(|a| a.cut).collect::<Vec<_>>(),
            errors
        );
    }
    summarize("图像对齐（相对模板轮）", &aligned_errors);
    Ok(())
}

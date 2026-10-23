//! 测试用的段配方记录：三台相机按 (camera, seq) 的帧记录，判定由 burst_judge 按配方算出（不手写结论）。

use super::{BurstRecord, FrameRecord};
use crate::burst_judge::{self, FrameMetrics, FrameState};
use crate::frame_group::{loss_alarms, FrameGroup};
use crate::judge::Judgement;
use crate::recipe::Recipe;
use crate::segmentation::{resolve_regions, Region, SegOutcome};

/// 段配方样例配方（cam1–cam3、一个直线段 N_exp = 4）。
pub(crate) fn recipe() -> Recipe {
    crate::recipe::samples_burst().build().unwrap()
}

/// 合格的一帧：10 站胶宽都是 `width`（站距 1 mm），无断口，胶中心 x = 0。
pub(crate) fn measured(camera: usize, seq: u64, width: f64) -> FrameMetrics {
    FrameMetrics {
        camera,
        seq,
        state: FrameState::Measured,
        error: None,
        width_min: Some(width),
        width_max: Some(width),
        width_mean: Some(width),
        widths: vec![Some(width as f32); 10],
        spacing_mm: 1.0,
        gap_max_mm: 0.0,
        present_ratio: 1.0,
        x_mm: Some(0.0),
    }
}

/// 三台相机 seq 1..=groups 的帧记录，胶宽 4.0；`missing` 里的 (camera, seq) 是缺帧（没有指标）。
pub(crate) fn frames(groups: u64, missing: &[(usize, u64)]) -> Vec<FrameRecord> {
    (1..=groups)
        .flat_map(|seq| (0..3).map(move |camera| (camera, seq)))
        .map(|(camera, seq)| {
            let lost = missing.contains(&(camera, seq));
            FrameRecord {
                camera,
                seq,
                status: if lost { FrameState::Missing } else { FrameState::Measured },
                frame_counter: (!lost).then_some(u64::MAX - 100 + seq),
                trigger_counter: (!lost).then_some(seq),
                region: None,
                metrics: (!lost).then(|| measured(camera, seq, 4.0)),
                raw_file: None,
            }
        })
        .collect()
}

/// 按配方判一件：区域缺省为覆盖全部段的未分段区域；各相机帧数、丢帧告警、帧所属区域按帧记录算出。
/// 返回入库用的结论摘要（verdict / 结果码 / 原因与区域判定相同）和段配方记录。
pub(crate) fn record(recipe: &Recipe, mut frames: Vec<FrameRecord>, regions: Option<Vec<Region>>, device_triggers: Option<u32>) -> (Judgement, BurstRecord) {
    let cameras = recipe.plan_cameras();
    let total = frames.iter().map(|frame| frame.seq).max().unwrap_or(0);
    let groups: Vec<FrameGroup> = (1..=total)
        .map(|seq| {
            let present: Vec<bool> = (0..cameras.len())
                .map(|camera| frames.iter().any(|f| (f.camera, f.seq) == (camera, seq) && f.status != FrameState::Missing))
                .collect();
            FrameGroup { seq, incomplete: present.contains(&false), present }
        })
        .collect();
    let regions = regions.unwrap_or_else(|| resolve_regions(&recipe.path_segments, &SegOutcome::Unsegmented, total, 0.5));
    let camera_frames: Vec<u32> = (0..cameras.len())
        .map(|camera| frames.iter().filter(|f| f.camera == camera && f.status != FrameState::Missing).count() as u32)
        .collect();
    let metrics: Vec<FrameMetrics> = frames.iter().filter_map(|frame| frame.metrics.clone()).collect();
    let alarms = loss_alarms(&camera_frames);
    let judgement = burst_judge::judge(&recipe.path_segments, recipe.expected_total(), &regions, &groups, &metrics, &cameras, &alarms);
    for frame in &mut frames {
        frame.region = regions.iter().position(|region| (region.first_seq..=region.last_seq).contains(&frame.seq));
    }
    let summary = Judgement {
        verdict: judgement.verdict,
        plc_code: judgement.plc_code,
        fault_code: judgement.fault_code,
        reason: judgement.reason.clone(),
        segments: Vec::new(),
        gaps: Vec::new(),
    };
    let record = BurstRecord {
        expected_total: recipe.expected_total(),
        tolerance_total: recipe.tolerance_total(),
        device_triggers,
        camera_frames,
        judgement,
        frames,
    };
    (summary, record)
}

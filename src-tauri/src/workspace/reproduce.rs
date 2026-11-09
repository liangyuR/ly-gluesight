use crate::burst_judge::{BurstJudgement, FrameMetrics};
use crate::frame_group::LossAlarm;
use crate::production::Prepared;
use crate::store::FrameRecord;

/// 段配方按原发布包重现（D3）：按 (计划相机下标, seq) 读每帧录制原图、用发布包里这台相机的走廊重测（缺帧仍记缺帧、不读图；
/// 读图或测量失败记测量失败，只算检测不完整），再按原配方的段与 ΣN_exp 判定；丢帧告警沿用原记录。测完再核一次发布包没被改动。
/// 返回判定与重测的每帧指标。
pub(super) fn measure_burst_with_operation(
    prepared: &Prepared,
    frames: &[FrameRecord],
    loss: &[LossAlarm],
    passes: &[u32],
    cycle_id: &str,
    mut load: impl FnMut(&FrameRecord) -> Result<crate::frame::FrameImage, String>,
    operation: Option<&crate::operation::Operation>,
) -> Result<(BurstJudgement, Vec<FrameMetrics>), String> {
    let recipe = &prepared.recipe;
    let run = format!("reproduce-{cycle_id}-{}", ly_plc::now_ms());
    let metrics = super::burst_history::remeasure_with_operation(
        recipe,
        frames,
        passes,
        |frame, segment| {
            let image = load(frame)?;
            prepared
                .measure_frame(
                    frame.camera,
                    segment,
                    frame.seq,
                    &image,
                    &format!("{run}-c{}-{}", frame.camera, frame.seq),
                )
                .map(|m| m.metrics)
        },
        operation,
    )?;
    if let Some(operation) = operation {
        operation.stage("summarizing")?;
    }
    prepared.verify()?;
    let judgement = super::burst_history::judge_frames(
        &recipe.path_segments,
        recipe.boundary_guard,
        recipe.expected_total(),
        &recipe.plan_cameras(),
        &metrics,
        loss,
        recipe.pass_rules(passes),
    );
    Ok((judgement, metrics))
}

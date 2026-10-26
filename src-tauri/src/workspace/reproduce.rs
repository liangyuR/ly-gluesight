use crate::burst_judge::BurstJudgement;
use crate::frame_group::LossAlarm;
use crate::production::Prepared;
use crate::store::FrameRecord;

/// 段配方按原发布包重现（D3）：按 (计划相机下标, seq) 读每帧录制原图、用发布包里这台相机的走廊重测（缺帧仍记缺帧、不读图；
/// 读图或测量失败记测量失败，只算检测不完整），再按原配方的段与 ΣN_exp 判定；丢帧告警沿用原记录。测完再核一次发布包没被改动。
pub(super) fn measure_burst(prepared: &Prepared, frames: &[FrameRecord], loss: &[LossAlarm], cycle_id: &str,
    mut load: impl FnMut(&FrameRecord) -> Result<crate::frame::FrameImage, String>) -> Result<BurstJudgement, String> {
    let recipe = &prepared.recipe;
    let run = format!("reproduce-{cycle_id}-{}", ly_plc::now_ms());
    let metrics = super::burst_history::remeasure(frames, |frame| {
        let image = load(frame)?;
        prepared.measure_frame(frame.camera, frame.seq, &image, &format!("{run}-c{}-{}", frame.camera, frame.seq)).map(|m| m.metrics)
    });
    prepared.verify()?;
    Ok(super::burst_history::judge_frames(&recipe.path_segments, recipe.expected_total(), &recipe.plan_cameras(), &metrics, loss))
}

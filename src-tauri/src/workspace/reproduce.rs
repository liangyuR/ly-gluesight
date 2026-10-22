use serde_json::{json, Value};

use crate::burst_judge::BurstJudgement;
use crate::frame_group::LossAlarm;
use crate::judge::{self, Judgement, PointState};
use crate::production::Prepared;
use crate::store::FrameRecord;

/// 段配方按原发布包重现（D3）：按 (计划相机下标, seq) 读每帧录制原图、用发布包里这台相机的走廊重测（缺帧仍记缺帧、不读图；
/// 读图或测量失败记测量失败，只算检测不完整），再按原配方的段与 ΣN_exp 判定；丢帧告警沿用原记录。测完再核一次发布包没被改动。
pub(super) fn measure_burst(prepared: &Prepared, frames: &[FrameRecord], loss: &[LossAlarm], cycle_id: &str,
    mut load: impl FnMut(&FrameRecord) -> Result<crate::frame::FrameImage, String>) -> Result<BurstJudgement, String> {
    let recipe = &prepared.recipe;
    if !recipe.is_burst() { return Err("原发布包是旧拍照点配方，不能按相机帧重现".into()); }
    let run = format!("reproduce-{cycle_id}-{}", ly_plc::now_ms());
    let metrics = super::burst_history::remeasure(frames, |frame| {
        let image = load(frame)?;
        prepared.measure_frame(frame.camera, frame.seq, &image, &format!("{run}-c{}-{}", frame.camera, frame.seq)).map(|m| m.metrics)
    });
    prepared.verify()?;
    Ok(super::burst_history::judge_frames(&recipe.path_segments, recipe.expected_total(), &recipe.plan_cameras(), &metrics, loss))
}

pub(super) fn measure(prepared: &Prepared, count: usize, sn: &str, cycle_id: &str,
    mut load: impl FnMut(usize, u8) -> Result<crate::frame::FrameImage, String>) -> Result<(Judgement, Vec<Value>), String> {
    let recipe = &prepared.recipe;
    if count != recipe.shot_count() { return Err("原图数量与原发布计划不一致".into()); }
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut measurements = Vec::new();
    let mut failures = Vec::new();
    for (k, shot) in recipe.shots.iter().enumerate() {
        if !shot.measured() { continue; }
        let images = shot.enabled_views().into_iter().map(|view| load(k, view).map(|image| (view, std::sync::Arc::new(image))))
            .collect::<Result<Vec<_>, _>>()?;
        match prepared.measure_views(k, &images, &format!("reproduce-{cycle_id}-{k}-{}", ly_plc::now_ms())) {
            Ok(multi) => {
                let reference_failed = multi.reference_failed();
                let reading = multi.reading;
                for (i, &j) in reading.idx.iter().enumerate() {
                    table[j as usize] = crate::measure::point_state(reading.st[i], reading.d[i], reading.w[i]);
                }
                measurements.push(json!({"cycleId":cycle_id,"bundleId":prepared.bundle.id,
                    "shotId":shot.id,"camera":shot.camera,"sn":sn,"k":k,"cam":0,"referenceFailed":reference_failed,
                    "score":reading.coverage,"ms":reading.ms,"error":null,"idx":reading.idx,"views":multi.views,
                    "d":reading.d,"w":reading.w,"st":reading.st,"px":reading.px}));
            }
            Err(error) => {
                failures.push(format!("拍照点 {}：{error}", shot.id));
                for j in recipe.owned_points(k) { table[j] = PointState::Invalid; }
                measurements.push(json!({"cycleId":cycle_id,"bundleId":prepared.bundle.id,
                    "shotId":shot.id,"camera":shot.camera,"sn":sn,"k":k,"cam":0,"referenceFailed":[],
                    "score":0,"ms":0,"error":error,"idx":[],"d":[],"w":[],"st":[],"px":[]}));
            }
        }
    }
    prepared.verify()?;
    let judgement = if failures.is_empty() { judge::judge(recipe, &table) }
        else { Judgement::error(judge::fault::PROCESS_TIMEOUT, failures.join("；")) };
    Ok((judgement, measurements))
}

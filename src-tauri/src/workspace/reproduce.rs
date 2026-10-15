use serde_json::{json, Value};

use crate::judge::{self, Judgement, PointState};
use crate::production::Prepared;

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
                let reading = multi.reading;
                for (i, &j) in reading.idx.iter().enumerate() {
                    table[j as usize] = if reading.st[i] == crate::measure::ST_OK {
                        PointState::Measured { d: reading.d[i], w: reading.w[i] }
                    } else if reading.st[i] == crate::measure::ST_GAP { PointState::Gap } else { PointState::Invalid };
                }
                measurements.push(json!({"cycleId":cycle_id,"bundleId":prepared.bundle.id,
                    "shotId":shot.id,"camera":shot.camera,"sn":sn,"k":k,"cam":0,"located":true,
                    "score":reading.coverage,"ms":reading.ms,"error":null,"idx":reading.idx,"views":multi.views,
                    "d":reading.d,"w":reading.w,"st":reading.st,"px":reading.px}));
            }
            Err(error) => {
                failures.push(format!("拍照点 {}：{error}", shot.id));
                for j in recipe.owned_points(k) { table[j] = PointState::Invalid; }
                measurements.push(json!({"cycleId":cycle_id,"bundleId":prepared.bundle.id,
                    "shotId":shot.id,"camera":shot.camera,"sn":sn,"k":k,"cam":0,"located":false,
                    "score":0,"ms":0,"error":error,"idx":[],"d":[],"w":[],"st":[],"px":[]}));
            }
        }
    }
    prepared.verify()?;
    let judgement = if failures.is_empty() { judge::judge(recipe, &table) }
        else { Judgement::error(judge::fault::PROCESS_TIMEOUT, failures.join("；")) };
    Ok((judgement, measurements))
}

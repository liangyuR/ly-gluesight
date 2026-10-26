use std::sync::Arc;
use serde::Serialize;
use crate::{frame::FrameImage, recipe::Recipe, vision::{self, CorridorGraph, Engine, FrameMeasurement}};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugInfo { pub recipe_id: String, pub revision: u64 }

pub struct Candidate {
    pub info: DebugInfo,
    pub recipe: Arc<Recipe>,
    engine: Arc<Engine>,
    /// 按计划相机下标的走廊测量图
    corridors: Vec<CorridorGraph>,
}

impl Candidate {
    /// 段配方的候选试运行（`workspace_debug_run` 对段配方调用）：按各相机示教帧尺寸（计划相机顺序）建走廊测量图；前置 `Recipe::ready_burst`。
    pub fn new_burst(recipe: Recipe, revision: u64, engine: Arc<Engine>, sizes: &[[u32; 2]]) -> Result<Self, String> {
        recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
        if sizes.len() != recipe.inspect.len() { return Err("候选示教帧尺寸与检测相机数量不一致".into()); }
        let corridors = recipe.inspect.iter().zip(sizes).map(|(inspect, &size)| vision::plan_corridor(inspect, &recipe, size)).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { info: DebugInfo { recipe_id: recipe.id.clone(), revision }, recipe: Arc::new(recipe), engine, corridors })
    }

    /// 段配方一台相机一帧的候选测量（每帧独立，结果只量不判）。
    pub fn measure_frame(&self, job: &crate::measure::FrameJob, image: &FrameImage) -> Result<FrameMeasurement, String> {
        if job.bundle_id.is_some() || !self.matches(&job.recipe) { return Err("候选调试测量与冻结修订不一致".into()); }
        let corridor = self.corridors.get(job.camera_idx).filter(|c| c.camera == job.camera).ok_or("候选没有这台相机的走廊测量图")?;
        vision::measure_frame(&self.engine, corridor, image, job.seq, &format!("debug-{}-c{}-{}", job.cycle_id, job.camera_idx, job.seq))
    }

    pub fn matches(&self, recipe: &Recipe) -> bool {
        self.recipe.id == recipe.id && self.recipe.revision_id == recipe.revision_id
    }
}

pub fn check_start(offline: bool, samples: bool, busy: bool, capturing: bool, running: bool) -> Result<(), String> {
    if !offline || !samples { return Err("候选试运行仅支持离线样本集模式".into()); }
    if busy || capturing || running { return Err("当前有在途工件、采集或离线运行，请结束后再试运行".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn real_or_occupied_environment_rejects_candidate_debug() {
        for args in [(false, true, false, false, false), (true, false, false, false, false),
            (true, true, true, false, false), (true, true, false, true, false), (true, true, false, false, true)] {
            assert!(super::check_start(args.0, args.1, args.2, args.3, args.4).is_err());
        }
        assert!(super::check_start(true, true, false, false, false).is_ok());
    }
}

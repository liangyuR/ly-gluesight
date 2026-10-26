use crate::{
    frame::FrameImage,
    recipe::Recipe,
    vision::{self, CorridorSet, Engine, FrameMeasurement},
};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugInfo { pub recipe_id: String, pub revision: u64 }

pub struct Candidate {
    pub info: DebugInfo,
    pub recipe: Arc<Recipe>,
    pub overview: crate::workspace::Overview,
    engine: Arc<Engine>,
    graphs: HashMap<(usize, u8), Value>,
    sizes: HashMap<(usize, u8), [u32; 2]>,
    /// 用特征定位的视图的模板目录（工作台按冻结示教原图生成，按内容寻址、不会改写）
    bases: HashMap<(usize, u8), String>,
    /// 段配方：按（计划相机下标, 段号）的走廊测量图（旧拍照点配方为空，前面三项只服务旧配方）
    corridors: CorridorSet,
}

impl Candidate {
    /// 段配方的候选试运行（`workspace_debug_run` 对段配方调用）：按各相机示教帧尺寸（计划相机顺序）建走廊测量图；前置 `Recipe::ready_burst`。
    pub fn new_burst(recipe: Recipe, revision: u64, engine: Arc<Engine>, sizes: &[[u32; 2]], overview: crate::workspace::Overview) -> Result<Self, String> {
        recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
        if sizes.len() != recipe.inspect.len() { return Err("候选示教帧尺寸与检测相机数量不一致".into()); }
        let corridors = CorridorSet::build(&recipe, sizes)?;
        Ok(Self { info: DebugInfo { recipe_id: recipe.id.clone(), revision }, recipe: Arc::new(recipe), engine,
            graphs: HashMap::new(), sizes: HashMap::new(), bases: HashMap::new(), corridors, overview })
    }

    /// 段配方一台相机一帧的候选测量（每帧独立，结果只量不判）。
    pub fn measure_frame(
        &self,
        job: &crate::measure::FrameJob,
        image: &FrameImage,
    ) -> Result<FrameMeasurement, String> {
        if job.bundle_id.is_some() || !self.matches(&job.recipe) {
            return Err("候选调试测量与冻结修订不一致".into());
        }
        let corridor = self
            .corridors
            .get(job.camera_idx, job.segment)
            .filter(|c| c.camera == job.camera)
            .ok_or("候选没有这台相机在这一段的走廊测量图")?;
        vision::measure_frame(
            &self.engine,
            corridor,
            image,
            job.seq,
            &format!("debug-{}-c{}-{}", job.cycle_id, job.camera_idx, job.seq),
        )
    }

    pub fn new(recipe: Recipe, revision: u64, engine: Arc<Engine>, sizes: HashMap<(usize, u8), [u32; 2]>, bases: HashMap<(usize, u8), String>, overview: crate::workspace::Overview) -> Result<Self, String> {
        recipe.ready()?;
        vision::require_locate(&recipe, &engine.locate)?;
        let mut graphs = HashMap::new();
        for (k, shot) in recipe.shots.iter().enumerate() {
            if !shot.measured() { continue; }
            for view in shot.enabled_views() {
                if !sizes.contains_key(&(k, view)) { return Err("候选示教图缺少尺寸".into()); }
                let projected = recipe.for_view(k, view)?;
                if projected.shots[k].fixture.is_some() && !bases.contains_key(&(k, view)) { return Err("候选示教图缺少定位模板".into()); }
                graphs.insert((k, view), vision::build_taught_graph(&projected, k)?);
            }
        }
        Ok(Self {
            info: DebugInfo {
                recipe_id: recipe.id.clone(),
                revision,
            },
            recipe: Arc::new(recipe),
            engine,
            graphs,
            sizes,
            bases,
            corridors: CorridorSet::default(),
            overview,
        })
    }

    pub fn matches(&self, recipe: &Recipe) -> bool {
        self.recipe.id == recipe.id && self.recipe.revision_id == recipe.revision_id
    }

    pub fn measure(&self, job: &crate::measure::Job, _image: &FrameImage) -> Result<crate::measure::Measured, String> {
        if job.bundle_id.is_some() || !self.matches(&job.recipe) { return Err("候选调试测量与冻结修订不一致".into()); }
        let run_id = format!("debug-{}-{}", job.cycle_id, job.k);
        let images = &job.images;
        if images.is_empty() { return Err("候选调试缺少原图".into()); }
        Ok(vision::measure_views_with(&self.recipe, job.k, &images, |view, image| {
            if self.sizes.get(&(job.k, view)).copied() != Some([image.width, image.height]) { return Err("调试图像尺寸与示教不一致".into()); }
            let graph = self.graphs.get(&(job.k, view)).ok_or("候选测量图缺失")?;
            let base = self.bases.get(&(job.k, view)).map_or("", String::as_str);
            vision::measure_shot_with_graph(&self.engine, &self.recipe.for_view(job.k, view)?, job.k, image, &format!("{run_id}-v{view}"), base, graph)
        })?.into_measured(job))
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

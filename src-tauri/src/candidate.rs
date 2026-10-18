use std::collections::HashMap;
use std::sync::Arc;
use serde::Serialize;
use serde_json::Value;
use crate::{frame::FrameImage, recipe::Recipe, vision::{self, Engine}};

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
}

impl Candidate {
    pub fn new(recipe: Recipe, revision: u64, engine: Arc<Engine>, sizes: HashMap<(usize, u8), [u32; 2]>, overview: crate::workspace::Overview) -> Result<Self, String> {
        recipe.ready()?;
        let mut graphs = HashMap::new();
        for (k, shot) in recipe.shots.iter().enumerate() {
            if !shot.measured() { continue; }
            for view in shot.enabled_views() {
                if !sizes.contains_key(&(k, view)) { return Err("候选示教图缺少尺寸".into()); }
                graphs.insert((k, view), vision::build_taught_graph(&recipe.for_view(k, view)?, k)?);
            }
        }
        Ok(Self { info: DebugInfo { recipe_id: recipe.id.clone(), revision }, recipe: Arc::new(recipe), engine, graphs, sizes, overview })
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
            vision::measure_shot_with_graph(&self.engine, &self.recipe.for_view(job.k, view)?, job.k, image, &format!("{run_id}-v{view}"), "", graph)
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

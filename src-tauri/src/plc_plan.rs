use serde::{Deserialize, Serialize};

use crate::recipe::{InspectMode, Recipe};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanShot {
    pub shot_id: String,
    pub camera_id: String,
    pub center: [f32; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlcPlan {
    pub protocol_version: u16,
    pub recipe_id: String,
    pub plan_version: u32,
    pub plan_hash: u32,
    pub shot_count: u16,
    pub camera_slots: [String; 3],
    pub camera_shots: [u16; 3],
    pub shots: Vec<PlanShot>,
}

impl PlcPlan {
    pub fn compile(recipe_id: String, version: u32, camera_slots: [String; 3], shots: Vec<PlanShot>) -> Result<Self, String> {
        if shots.is_empty() || shots.len() > u16::MAX as usize {
            return Err("S7 一期拍照计划必须包含 1–65535 个拍照点".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut cameras = std::collections::HashSet::new();
        for id in camera_slots.iter().filter(|id| !id.is_empty()) {
            if !cameras.insert(id) { return Err("PLC 相机槽不能引用同一相机".into()); }
        }
        let mut counts = [0u16; 3];
        for shot in &shots {
            if shot.shot_id.is_empty() || !ids.insert(&shot.shot_id) || !shot.center.iter().all(|v| v.is_finite()) {
                return Err("拍照点 ID 必须非空且唯一，坐标必须有效".into());
            }
            let slot = camera_slots.iter().position(|id| !id.is_empty() && id == &shot.camera_id)
                .ok_or_else(|| format!("拍照点 {} 的相机 {} 不在 PLC 三个相机槽中", shot.shot_id, shot.camera_id))?;
            counts[slot] += 1;
        }
        let bytes = serde_json::to_vec(&(1u16, &recipe_id, version, &camera_slots, &shots)).map_err(|e| e.to_string())?;
        let hash = bytes.into_iter().fold(2166136261u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(16777619));
        Ok(Self { protocol_version: 1, recipe_id, plan_version: version, plan_hash: hash,
            shot_count: shots.len() as u16, camera_slots, camera_shots: counts, shots })
    }

    pub fn from_recipe(recipe: &Recipe, camera_slots: [String; 3]) -> Result<Self, String> {
        if recipe.mode != InspectMode::FlyShot {
            return Err("一期 S7 握手仅支持飞拍配方".into());
        }
        let shots = recipe.shots.iter().enumerate().map(|(index, center)| PlanShot {
            shot_id: format!("P{}", index + 1), camera_id: recipe.camera.clone(), center: *center,
        }).collect();
        Self::compile(recipe.id.clone(), recipe.version, camera_slots, shots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots() -> Vec<PlanShot> {
        ["cam1", "cam2", "cam3", "cam1"].into_iter().enumerate().map(|(index, camera)| PlanShot {
            shot_id: format!("P{}", index + 1), camera_id: camera.into(), center: [index as f32, 10.0],
        }).collect()
    }

    #[test]
    fn plan_binds_order_and_camera_counts_without_requiring_camera_arrival_order() {
        let slots = ["cam1".into(), "cam2".into(), "cam3".into()];
        let plan = PlcPlan::compile("part".into(), 1, slots.clone(), shots()).unwrap();
        assert_eq!(plan.camera_shots, [2, 1, 1]);
        assert_eq!(plan.shot_count, 4);
        let mut reordered = shots();
        reordered.swap(0, 1);
        assert_ne!(plan.plan_hash, PlcPlan::compile("part".into(), 1, slots.clone(), reordered).unwrap().plan_hash);
        assert_ne!(plan.plan_hash, PlcPlan::compile("part".into(), 2, slots, shots()).unwrap().plan_hash);
    }

    #[test]
    fn invalid_bindings_and_duplicate_shots_cannot_publish_a_plan() {
        let slots = ["cam1".into(), "cam2".into(), "cam3".into()];
        let mut duplicate = shots();
        duplicate[1].shot_id = duplicate[0].shot_id.clone();
        assert!(PlcPlan::compile("p".into(), 1, slots.clone(), duplicate).is_err());
        let mut unknown = shots();
        unknown[0].camera_id = "cam4".into();
        assert!(PlcPlan::compile("p".into(), 1, slots, unknown).is_err());
    }

    #[test]
    fn production_recipe_export_preserves_its_real_camera_and_shot_order() {
        let recipe = crate::recipe::builtin().into_iter().find(|r| r.mode == InspectMode::FlyShot).unwrap();
        let mut recipe = (*recipe).clone();
        recipe.camera = "cam2".into();
        let plan = PlcPlan::from_recipe(&recipe, ["cam1".into(), "cam2".into(), "cam3".into()]).unwrap();
        assert_eq!(plan.camera_shots, [0, recipe.shots.len() as u16, 0]);
        assert_eq!(plan.shots.iter().map(|s| s.center).collect::<Vec<_>>(), recipe.shots);
        assert!(plan.shots.iter().all(|s| s.camera_id == "cam2"));
        assert!(PlcPlan::from_recipe(&recipe, ["cam1".into(), String::new(), String::new()]).is_err());
        recipe.mode = InspectMode::Follow;
        assert!(PlcPlan::from_recipe(&recipe, ["cam1".into(), "cam2".into(), "cam3".into()]).is_err());
    }
}

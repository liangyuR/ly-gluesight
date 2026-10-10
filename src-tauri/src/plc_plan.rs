use serde::{Deserialize, Serialize};

use crate::recipe::Recipe;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanShot {
    pub shot_id: String,
    pub pose_id: String,
    pub camera_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlcPlan {
    pub protocol_version: u16,
    pub recipe_id: String,
    pub plan_version: u32,
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
            if shot.shot_id.is_empty() || !ids.insert(&shot.shot_id) || shot.pose_id.trim().is_empty() {
                return Err("拍照点 ID 必须非空且唯一，Pose 标识不能为空".into());
            }
            let slot = camera_slots.iter().position(|id| !id.is_empty() && id == &shot.camera_id)
                .ok_or_else(|| format!("拍照点 {} 的相机 {} 不在 PLC 三个相机槽中", shot.shot_id, shot.camera_id))?;
            counts[slot] += 1;
        }
        Ok(Self { protocol_version: 1, recipe_id, plan_version: version,
            shot_count: shots.len() as u16, camera_slots, camera_shots: counts, shots })
    }

    pub fn from_recipe(recipe: &Recipe, camera_slots: [String; 3]) -> Result<Self, String> {
        let shots = recipe.shots.iter().map(|s| PlanShot {
            shot_id: s.id.clone(), pose_id: s.pose_id.clone(), camera_id: s.camera.clone(),
        }).collect();
        Self::compile(recipe.id.clone(), recipe.version, camera_slots, shots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_views_still_use_one_device_and_four_triggers() {
        let mut doc = crate::recipe::samples().remove(1);
        for (shot, view) in doc.shots.iter_mut().zip([1, 2, 3, 1]) { shot.view = view; }
        let recipe = doc.build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe, ["cam1".into(), String::new(), String::new()]).unwrap();
        assert_eq!(plan.camera_shots, [4, 0, 0]);
        assert_eq!(plan.shot_count, 4);
    }

    fn shots() -> Vec<PlanShot> {
        ["cam1", "cam2", "cam3", "cam1"].into_iter().enumerate().map(|(index, camera)| PlanShot {
            shot_id: format!("P{}", index + 1), pose_id: format!("A{}", index + 1), camera_id: camera.into(),
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
        assert_ne!(plan.shots, PlcPlan::compile("part".into(), 1, slots.clone(), reordered).unwrap().shots);
        assert_ne!(plan.plan_version, PlcPlan::compile("part".into(), 2, slots.clone(), shots()).unwrap().plan_version);
        let mut pose = shots();
        pose[2].pose_id = "B3".into();
        assert_ne!(plan.shots, PlcPlan::compile("part".into(), 1, slots, pose).unwrap().shots);
    }

    #[test]
    fn invalid_bindings_and_duplicate_shots_cannot_publish_a_plan() {
        let slots = ["cam1".into(), "cam2".into(), "cam3".into()];
        let mut duplicate = shots();
        duplicate[1].shot_id = duplicate[0].shot_id.clone();
        assert!(PlcPlan::compile("p".into(), 1, slots.clone(), duplicate).is_err());
        let mut unknown = shots();
        unknown[0].camera_id = "cam4".into();
        assert!(PlcPlan::compile("p".into(), 1, slots.clone(), unknown).is_err());
        let mut no_pose = shots();
        no_pose[0].pose_id = " ".into();
        assert!(PlcPlan::compile("p".into(), 1, slots, no_pose).is_err());
    }

    #[test]
    fn production_recipe_export_preserves_each_shot_camera_and_order() {
        let mut doc = crate::recipe::samples().remove(1);
        for (shot, camera) in doc.shots.iter_mut().zip(["cam1", "cam2", "cam3", "cam1"]) {
            shot.camera = camera.into();
        }
        doc.shots[1].pose_id = "P1".into();
        let recipe = doc.build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe, ["cam1".into(), "cam2".into(), "cam3".into()]).unwrap();
        assert_eq!(plan.camera_shots, [2, 1, 1]);
        assert_eq!(plan.shots.iter().map(|s| (s.shot_id.as_str(), s.pose_id.as_str(), s.camera_id.as_str())).collect::<Vec<_>>(),
            [("P1", "P1", "cam1"), ("P2", "P1", "cam2"), ("P3", "P3", "cam3"), ("P4", "P4", "cam1")]);
        assert!(PlcPlan::from_recipe(&recipe, ["cam1".into(), "cam2".into(), String::new()]).is_err());
    }

    #[test]
    fn documented_example_counts() {
        let shots = (1..=2).map(|i| PlanShot { shot_id: format!("P{i}"), pose_id: format!("P{i}"), camera_id: "cam1".into() }).collect();
        let plan = PlcPlan::compile("DEMO".into(), 1, ["cam1".into(), String::new(), String::new()], shots).unwrap();
        assert_eq!((plan.plan_version, plan.camera_shots), (1, [2, 0, 0]));
    }
}

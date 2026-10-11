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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlcPlan {
    pub protocol_version: u16,
    pub recipe_id: String,
    pub plan_version: u32,
    pub shot_count: u16,
    pub device_id: String,
    pub device_shots: u16,
    pub shots: Vec<PlanShot>,
}

impl PlcPlan {
    pub fn compile(recipe_id: String, version: u32, device_id: String, shots: Vec<PlanShot>) -> Result<Self, String> {
        if shots.is_empty() || shots.len() > u16::MAX as usize {
            return Err("S7 一期拍照计划必须包含 1–65535 个拍照点".into());
        }
        let mut ids = std::collections::HashSet::new();
        if device_id.trim().is_empty() {
            return Err("PLC 拍照计划必须指定唯一相机设备 ID".into());
        }
        for shot in &shots {
            if shot.shot_id.is_empty() || !ids.insert(&shot.shot_id) || shot.pose_id.trim().is_empty() {
                return Err("拍照点 ID 必须非空且唯一，Pose 标识不能为空".into());
            }
            if shot.camera_id != device_id {
                return Err(format!("拍照点 {} 的设备 {} 与唯一相机设备 {} 不一致", shot.shot_id, shot.camera_id, device_id));
            }
        }
        Ok(Self { protocol_version: crate::handshake::PROTOCOL_VERSION, recipe_id, plan_version: version,
            shot_count: shots.len() as u16, device_id, device_shots: shots.len() as u16, shots })
    }

    pub fn from_recipe(recipe: &Recipe, device_id: String) -> Result<Self, String> {
        if recipe.plan_version == 0 {
            return Err(format!("配方 {} 还没有分配 PLC 计划版本：请保存或发布一次后再布防", recipe.id));
        }
        let shots = recipe.shots.iter().map(|s| PlanShot {
            shot_id: s.id.clone(), pose_id: s.pose_id.clone(), camera_id: s.camera.clone(),
        }).collect();
        Self::compile(recipe.id.clone(), recipe.plan_version, device_id, shots)
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
        let plan = PlcPlan::from_recipe(&recipe, "cam1".into()).unwrap();
        assert_eq!(plan.device_shots, 4);
        assert_eq!(plan.shot_count, 4);
    }

    #[test]
    fn plan_version_is_the_assigned_plan_version_not_the_recipe_revision() {
        let mut doc = crate::recipe::samples().remove(1);
        doc.version = 9;
        doc.plan_version = 41;
        let plan = PlcPlan::from_recipe(&doc.build().unwrap(), "cam1".into()).unwrap();
        assert_eq!(plan.plan_version, 41);
        doc.plan_version = 0;
        assert!(PlcPlan::from_recipe(&doc.build().unwrap(), "cam1".into()).unwrap_err().contains("计划版本"));
    }

    fn shots() -> Vec<PlanShot> {
        ["cam1"; 4].into_iter().enumerate().map(|(index, camera)| PlanShot {
            shot_id: format!("P{}", index + 1), pose_id: format!("A{}", index + 1), camera_id: camera.into(),
        }).collect()
    }

    #[test]
    fn plan_binds_single_device_counts_and_shot_order() {
        let device = "cam1".to_owned();
        let plan = PlcPlan::compile("part".into(), 1, device.clone(), shots()).unwrap();
        assert_eq!(plan.device_shots, 4);
        assert_eq!(plan.shot_count, 4);
        let mut reordered = shots();
        reordered.swap(0, 1);
        assert_ne!(plan.shots, PlcPlan::compile("part".into(), 1, device.clone(), reordered).unwrap().shots);
        assert_ne!(plan.plan_version, PlcPlan::compile("part".into(), 2, device.clone(), shots()).unwrap().plan_version);
        let mut pose = shots();
        pose[2].pose_id = "B3".into();
        assert_ne!(plan.shots, PlcPlan::compile("part".into(), 1, device, pose).unwrap().shots);
    }

    #[test]
    fn invalid_bindings_and_duplicate_shots_cannot_publish_a_plan() {
        let device = "cam1".to_owned();
        let mut duplicate = shots();
        duplicate[1].shot_id = duplicate[0].shot_id.clone();
        assert!(PlcPlan::compile("p".into(), 1, device.clone(), duplicate).is_err());
        let mut unknown = shots();
        unknown[0].camera_id = "cam4".into();
        assert!(PlcPlan::compile("p".into(), 1, device.clone(), unknown).is_err());
        let mut no_pose = shots();
        no_pose[0].pose_id = " ".into();
        assert!(PlcPlan::compile("p".into(), 1, device, no_pose).is_err());
    }

    #[test]
    fn production_recipe_export_rejects_another_device() {
        let mut doc = crate::recipe::samples().remove(1);
        doc.shots[1].camera = "cam2".into();
        let recipe = doc.build().unwrap();
        assert!(PlcPlan::from_recipe(&recipe, "cam1".into()).unwrap_err().contains("唯一相机设备"));
    }

    #[test]
    fn empty_device_and_legacy_multidevice_plan_are_rejected() {
        assert!(PlcPlan::compile("part".into(), 1, " ".into(), shots()).is_err());
        let plan = PlcPlan::compile("part".into(), 1, "cam1".into(), shots()).unwrap();
        let value = serde_json::to_value(&plan).unwrap();
        assert_eq!(value["deviceId"], "cam1");
        assert_eq!(value["deviceShots"], 4);
        assert!(value.get("cameraSlots").is_none());
        assert!(value.get("cameraShots").is_none());
        let mut legacy = value;
        let object = legacy.as_object_mut().unwrap();
        object.remove("deviceId");
        object.remove("deviceShots");
        object.insert("cameraSlots".into(), serde_json::json!(["cam1", "cam2", "cam3"]));
        object.insert("cameraShots".into(), serde_json::json!([2, 1, 1]));
        assert!(serde_json::from_value::<PlcPlan>(legacy).is_err());
    }


}

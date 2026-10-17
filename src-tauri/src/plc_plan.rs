use serde::{Deserialize, Serialize};

use crate::recipe::Recipe;

/// 一次 PLC 触发：同一触发线上的这些相机各出一帧。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanShot {
    pub shot_id: String,
    pub pose_id: String,
    pub camera_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlcPlan {
    pub protocol_version: u16,
    pub recipe_id: String,
    pub plan_version: u32,
    pub shot_count: u16,
    /// 接在 PLC 触发线上的相机；每次触发同时触发全部
    pub device_ids: Vec<String>,
    /// 每台相机的计划触发数
    pub device_shots: u16,
    pub shots: Vec<PlanShot>,
}

impl PlcPlan {
    pub fn compile(recipe_id: String, version: u32, device_ids: Vec<String>, shots: Vec<PlanShot>) -> Result<Self, String> {
        if shots.is_empty() || shots.len() > u16::MAX as usize {
            return Err("S7 一期拍照计划必须包含 1–65535 个拍照点".into());
        }
        if device_ids.is_empty() || device_ids.iter().enumerate().any(|(i, id)| id.trim().is_empty() || device_ids[..i].contains(id)) {
            return Err("PLC 拍照计划必须指定不重复、非空的相机设备 ID".into());
        }
        let mut ids = std::collections::HashSet::new();
        for shot in &shots {
            if shot.shot_id.is_empty() || !ids.insert(&shot.shot_id) || shot.pose_id.trim().is_empty() {
                return Err("拍照点 ID 必须非空且唯一，Pose 标识不能为空".into());
            }
            if shot.camera_ids != device_ids {
                return Err(format!("拍照点 {} 触发的相机 {:?} 与触发线上的相机 {:?} 不一致", shot.shot_id, shot.camera_ids, device_ids));
            }
        }
        Ok(Self { protocol_version: crate::handshake::PROTOCOL_VERSION, recipe_id, plan_version: version,
            shot_count: shots.len() as u16, device_ids, device_shots: shots.len() as u16, shots })
    }

    pub fn from_recipe(recipe: &Recipe, device_id: String) -> Result<Self, String> {
        if recipe.plan_version == 0 {
            return Err(format!("配方 {} 还没有分配 PLC 计划版本：请保存或发布一次后再布防", recipe.id));
        }
        let shots = recipe.shots.iter().map(|s| PlanShot {
            shot_id: s.id.clone(), pose_id: s.pose_id.clone(), camera_ids: vec![s.camera.clone()],
        }).collect();
        Self::compile(recipe.id.clone(), recipe.plan_version, vec![device_id], shots)
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
        assert_eq!(plan.shots.iter().map(|shot| shot.shot_id.as_str()).collect::<Vec<_>>(),
            recipe.shots.iter().map(|shot| shot.id.as_str()).collect::<Vec<_>>());
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
        (0..4).map(|index| PlanShot {
            shot_id: format!("P{}", index + 1), pose_id: format!("A{}", index + 1), camera_ids: vec!["cam1".into()],
        }).collect()
    }

    #[test]
    fn invalid_bindings_and_duplicate_shots_cannot_publish_a_plan() {
        let device = vec!["cam1".to_owned()];
        let mut duplicate = shots();
        duplicate[1].shot_id = duplicate[0].shot_id.clone();
        assert!(PlcPlan::compile("p".into(), 1, device.clone(), duplicate).is_err());
        let mut unknown = shots();
        unknown[0].camera_ids = vec!["cam4".into()];
        assert!(PlcPlan::compile("p".into(), 1, device.clone(), unknown).is_err());
        let mut no_pose = shots();
        no_pose[0].pose_id = " ".into();
        assert!(PlcPlan::compile("p".into(), 1, device, no_pose).is_err());
    }

    #[test]
    fn empty_device_and_legacy_multidevice_plan_are_rejected() {
        assert!(PlcPlan::compile("part".into(), 1, vec![" ".into()], shots()).is_err());
        let plan = PlcPlan::compile("part".into(), 1, vec!["cam1".into()], shots()).unwrap();
        let mut legacy = serde_json::to_value(&plan).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.remove("deviceIds");
        object.insert("deviceId".into(), serde_json::json!("cam1"));
        assert!(serde_json::from_value::<PlcPlan>(legacy.clone()).is_err());
        let object = legacy.as_object_mut().unwrap();
        object.remove("deviceId");
        object.remove("deviceShots");
        object.insert("cameraSlots".into(), serde_json::json!(["cam1", "cam2", "cam3"]));
        object.insert("cameraShots".into(), serde_json::json!([2, 1, 1]));
        assert!(serde_json::from_value::<PlcPlan>(legacy).is_err());
    }

    #[test]
    fn every_trigger_fires_all_cameras_on_the_line_so_device_shots_counts_triggers() {
        let cameras: Vec<String> = ["cam1", "cam2", "cam3"].map(String::from).to_vec();
        let triggers = |n: usize| (0..n).map(|k| PlanShot { shot_id: format!("S{:03}", k + 1), pose_id: format!("S{:03}", k + 1), camera_ids: cameras.clone() }).collect::<Vec<_>>();
        let plan = PlcPlan::compile("p".into(), 1, cameras.clone(), triggers(20)).unwrap();
        assert_eq!((plan.shot_count, plan.device_shots), (20, 20));
        let mut partial = triggers(20);
        partial[3].camera_ids.pop();
        assert!(PlcPlan::compile("p".into(), 1, cameras.clone(), partial).is_err());
        let mut twice = cameras.clone();
        twice[2] = "cam1".into();
        assert!(PlcPlan::compile("p".into(), 1, twice, triggers(1)).is_err());
    }
}

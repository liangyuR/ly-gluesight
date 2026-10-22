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
        if version > i32::MAX as u32 {
            return Err(format!("计划版本 {version} 超出 PLC DInt 上限 2147483647，PLC 无法核对"));
        }
        if shots.is_empty() || shots.len() > i16::MAX as usize {
            return Err("S7 一期拍照计划必须包含 1–32767 个拍照点".into());
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

    pub fn from_recipe(recipe: &Recipe) -> Result<Self, String> {
        if recipe.plan_version == 0 {
            return Err(format!("配方 {} 还没有分配 PLC 计划版本：请保存或发布一次后再布防", recipe.id));
        }
        let shots = trigger_groups(recipe)?.into_iter().map(|group| {
            let first = &recipe.shots[group[0]];
            PlanShot { shot_id: first.id.clone(), pose_id: first.pose_id.clone(), camera_ids: group.iter().map(|&k| recipe.shots[k].camera.clone()).collect() }
        }).collect();
        Self::compile(recipe.id.clone(), recipe.plan_version, recipe.cameras(), shots)
    }
}

/// PLC 开始请求要填的参数，供工程师配置 PLC 程序；按与接件核对相同的触发分组编出。
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlcParams {
    pub product_code: u16,
    /// 候选还没分配新号时沿用候选文件里的值，界面不展示
    pub plan_version: u32,
    /// 触发次数，deviceShots 与之相同
    pub shot_count: u16,
    pub shots: Vec<ParamShot>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParamShot {
    pub shot_id: String,
    pub pose_id: String,
}

impl PlcParams {
    /// 不要求已分配计划版本：候选发布前也要能算出 PLC 需要改什么。
    pub fn from_recipe(recipe: &Recipe) -> Result<Self, String> {
        if !(1..=i16::MAX as u16).contains(&recipe.product_code) {
            return Err(format!("产品代码 {} 超出 PLC Int 范围 1–32767", recipe.product_code));
        }
        let groups = trigger_groups(recipe)?;
        if groups.is_empty() || groups.len() > i16::MAX as usize {
            return Err("S7 一期拍照计划必须包含 1–32767 次触发".into());
        }
        let shots = groups.iter().map(|group| {
            let first = &recipe.shots[group[0]];
            ParamShot { shot_id: first.id.clone(), pose_id: first.pose_id.clone() }
        }).collect();
        Ok(Self { product_code: recipe.product_code, plan_version: recipe.plan_version, shot_count: groups.len() as u16, shots })
    }
}

/// 配方拍照点按 PLC 触发分组（拍照点下标）：相邻、同 Pose 的拍照点属于同一次触发，同一台相机再次出现时算下一次触发。
/// PLC 每次触发同时触发配方用到的全部相机，所以每组都必须按配方相机顺序各有一个拍照点。
pub fn trigger_groups(recipe: &Recipe) -> Result<Vec<Vec<usize>>, String> {
    let cameras = recipe.cameras();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (k, shot) in recipe.shots.iter().enumerate() {
        match groups.last_mut() {
            Some(group) if recipe.shots[group[0]].pose_id == shot.pose_id && !group.iter().any(|&i| recipe.shots[i].camera == shot.camera) => group.push(k),
            _ => groups.push(vec![k]),
        }
    }
    if let Some(group) = groups.iter().find(|group| group.iter().map(|&k| &recipe.shots[k].camera).ne(cameras.iter())) {
        let first = &recipe.shots[group[0]];
        return Err(format!("拍照点 {}（Pose {}）这次触发拍到的相机与配方相机 {} 不一致：PLC 每次触发同时触发全部相机，同一 Pose 须按相机顺序各有一个拍照点",
            first.id, first.pose_id, cameras.join("、")));
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_views_still_use_one_device_and_four_triggers() {
        let mut doc = crate::recipe::samples().remove(1);
        for ((shot, view), pose) in doc.shots.iter_mut().zip([1, 2, 3, 1]).zip(["P1", "P1", "P2", "P2"]) { shot.view = view; shot.pose_id = pose.into(); }
        let recipe = doc.build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe).unwrap();
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
        let plan = PlcPlan::from_recipe(&doc.build().unwrap()).unwrap();
        assert_eq!(plan.plan_version, 41);
        doc.plan_version = 0;
        assert!(PlcPlan::from_recipe(&doc.build().unwrap()).unwrap_err().contains("计划版本"));
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

    /// 两个 Pose × 两台相机：P1 拍 cam1、cam2，P2 拍 cam1、cam2。
    fn two_camera_doc() -> crate::recipe::RecipeDoc {
        let mut doc = crate::recipe::samples().remove(1);
        for (shot, (pose, camera)) in doc.shots.iter_mut().zip([("P1", "cam1"), ("P1", "cam2"), ("P2", "cam1"), ("P2", "cam2")]) {
            shot.pose_id = pose.into();
            shot.camera = camera.into();
        }
        doc.sync_imaging();
        doc
    }

    #[test]
    fn multi_camera_recipe_is_one_trigger_per_pose_firing_every_camera() {
        let recipe = two_camera_doc().build().unwrap();
        assert_eq!(trigger_groups(&recipe).unwrap(), [vec![0, 1], vec![2, 3]]);
        let plan = PlcPlan::from_recipe(&recipe).unwrap();
        assert_eq!((plan.shot_count, plan.device_shots), (2, 2));
        assert_eq!(plan.device_ids, ["cam1", "cam2"]);
        assert!(plan.shots.iter().all(|shot| shot.camera_ids == ["cam1", "cam2"]));
    }

    #[test]
    fn candidate_without_plan_version_still_gets_params_counted_by_trigger() {
        let mut doc = two_camera_doc();
        doc.plan_version = 0;
        let params = PlcParams::from_recipe(&doc.build().unwrap()).unwrap();
        assert_eq!(params.shot_count, 2);
        assert_eq!(params.shots.iter().map(|shot| (shot.shot_id.as_str(), shot.pose_id.as_str())).collect::<Vec<_>>(),
            [(doc.shots[0].id.as_str(), "P1"), (doc.shots[2].id.as_str(), "P2")]);
    }

    #[test]
    fn a_pose_missing_a_camera_or_in_another_camera_order_cannot_form_a_plan() {
        let mut missing = two_camera_doc();
        missing.shots[3].pose_id = "P3".into();
        let mut swapped = two_camera_doc();
        swapped.shots.swap(2, 3);
        for doc in [missing, swapped] {
            let recipe = doc.build().unwrap();
            assert!(trigger_groups(&recipe).is_err());
            assert!(PlcPlan::from_recipe(&recipe).is_err());
        }
    }
}

use serde::{Deserialize, Serialize};

use crate::plan::{self, ExportSegment, PlanExport};
use crate::recipe::Recipe;

/// 一次 PLC 触发：同一触发线上的这些相机各出一帧。旧拍照点模型。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanShot {
    pub shot_id: String,
    pub pose_id: String,
    pub camera_ids: Vec<String>,
}

/// PLC 计划：生产接件与 S7 会话按它核对开始请求。旧拍照点配方按触发列出 `shots`；
/// 段配方 `shots` 为空，`shot_count = device_shots = ΣN_exp`，另带段序列与整件容差（规格 §1/§5）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlcPlan {
    pub protocol_version: u16,
    pub recipe_id: String,
    pub plan_version: u32,
    /// 旧配方：触发次数；段配方：ΣN_exp
    pub shot_count: u16,
    /// 接在 PLC 触发线上的相机；每次触发同时触发全部
    pub device_ids: Vec<String>,
    /// 每台相机的计划触发数；段配方同为 ΣN_exp
    pub device_shots: u16,
    /// 旧拍照点模型的触发列表；段配方为空
    pub shots: Vec<PlanShot>,
    /// 段配方的整件帧数容差 Σtolerance；旧配方为 0
    #[serde(default)]
    pub tolerance_total: u32,
    /// 段配方的段序列；为空即旧拍照点计划
    #[serde(default)]
    pub segments: Vec<ExportSegment>,
}

impl PlcPlan {
    /// 段计划：段序列非空。
    pub fn is_burst(&self) -> bool {
        !self.segments.is_empty()
    }

    /// 段配方的示教采集计划：既没有拍照点也没有段，计数为 0。只能用于已绑定采集轮次的 S7 事务，
    /// 会话核对时不比较 shotCount / deviceShots（规格 §5：示教轮次不核对，PLC 可填估算值，见 PlcSession::validate_plan）。
    pub fn is_teaching(&self) -> bool {
        self.shots.is_empty() && self.segments.is_empty()
    }

    /// 段配方的示教采集计划：本轮相机共用一个触发；PLC 写来的计数只由采集轮次记录（CaptureRoundV2.plcShotCount）。
    pub fn teaching(recipe_id: String, version: u32, device_ids: Vec<String>) -> Result<Self, String> {
        if version > i32::MAX as u32 {
            return Err(format!("计划版本 {version} 超出 PLC DInt 上限 2147483647，PLC 无法核对"));
        }
        if device_ids.is_empty() || device_ids.iter().enumerate().any(|(i, id)| id.trim().is_empty() || device_ids[..i].contains(id)) {
            return Err("示教采集必须指定不重复、非空的相机设备 ID".into());
        }
        Ok(Self { protocol_version: crate::handshake::PROTOCOL_VERSION, recipe_id, plan_version: version,
            shot_count: 0, device_ids, device_shots: 0, shots: Vec::new(), tolerance_total: 0, segments: Vec::new() })
    }

    /// 段计划（规格 §5）：PLC 的 shotCount 与 deviceShots 都等于 ΣN_exp（1–32767）；三台不重复的相机共用一个触发；
    /// 整件容差取各段容差之和。
    pub fn compile_burst(recipe_id: String, version: u32, device_ids: Vec<String>, segments: Vec<ExportSegment>, expected_total: u32) -> Result<Self, String> {
        if version > i32::MAX as u32 {
            return Err(format!("计划版本 {version} 超出 PLC DInt 上限 2147483647，PLC 无法核对"));
        }
        if segments.is_empty() {
            return Err("段计划至少需要一段".into());
        }
        if !(1..=plan::MAX_EXPECTED_TOTAL).contains(&expected_total) {
            return Err(format!("段计划的预期帧组数 ΣN_exp = {expected_total} 超出 PLC shotCount 范围 1–{}", plan::MAX_EXPECTED_TOTAL));
        }
        if device_ids.len() != plan::PLAN_CAMERAS || device_ids.iter().enumerate().any(|(i, id)| id.trim().is_empty() || device_ids[..i].contains(id)) {
            return Err(format!("段计划需要 {} 台不重复、非空的相机共用一个触发：{device_ids:?}", plan::PLAN_CAMERAS));
        }
        let tolerance_total = segments.iter().fold(0u32, |sum, segment| sum.saturating_add(segment.tolerance));
        Ok(Self { protocol_version: crate::handshake::PROTOCOL_VERSION, recipe_id, plan_version: version,
            shot_count: expected_total as u16, device_ids, device_shots: expected_total as u16, shots: Vec::new(), tolerance_total, segments })
    }

    /// 按计划自身的字段重编一次：内容与计数自洽时与原计划相等（段计划走 compile_burst，示教采集计划走 teaching，
    /// 旧计划走 compile 并核对拍照点）。
    pub fn recompiled(&self) -> Result<Self, String> {
        if self.is_burst() {
            Self::compile_burst(self.recipe_id.clone(), self.plan_version, self.device_ids.clone(), self.segments.clone(), u32::from(self.shot_count))
        } else if self.is_teaching() {
            Self::teaching(self.recipe_id.clone(), self.plan_version, self.device_ids.clone())
        } else {
            Self::compile(self.recipe_id.clone(), self.plan_version, self.device_ids.clone(), self.shots.clone())
        }
    }

    /// 旧拍照点计划。
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
            shot_count: shots.len() as u16, device_ids, device_shots: shots.len() as u16, shots, tolerance_total: 0, segments: Vec::new() })
    }

    /// 段配方编段计划（ΣN_exp 未标定时不能编），旧拍照点配方按触发分组编。
    pub fn from_recipe(recipe: &Recipe) -> Result<Self, String> {
        if recipe.plan_version == 0 {
            return Err(format!("配方 {} 还没有分配 PLC 计划版本：请保存或发布一次后再布防", recipe.id));
        }
        if recipe.is_burst() {
            let export = PlanExport::from_recipe(recipe)?;
            let Some(total) = export.expected_total else {
                let pending: Vec<String> = export.segments.iter().filter(|s| s.expected_frames.is_none()).map(|s| s.index.to_string()).collect();
                return Err(format!("配方 {} 的段 {} 待标定，且没有整件标定：标定出预期帧数前编不出 PLC 计划", recipe.id, pending.join("、")));
            };
            return Self::compile_burst(recipe.id.clone(), recipe.plan_version, export.cameras, export.segments, total);
        }
        let shots = trigger_groups(recipe)?.into_iter().map(|group| {
            let first = &recipe.shots[group[0]];
            PlanShot { shot_id: first.id.clone(), pose_id: first.pose_id.clone(), camera_ids: group.iter().map(|&k| recipe.shots[k].camera.clone()).collect() }
        }).collect();
        Self::compile(recipe.id.clone(), recipe.plan_version, recipe.cameras(), shots)
    }
}

impl PlanExport {
    /// 段配方的计划导出（规格 §1，命令 recipe_plan_export）：不要求已分配计划号，也不要求已标定（未标定时 expectedTotal 为 null）。
    /// 旧拍照点配方没有段计划，返回错误（它的 PLC 参数见 [`PlcParams`]）。
    pub fn from_recipe(recipe: &Recipe) -> Result<Self, String> {
        if !recipe.is_burst() {
            return Err(format!("配方 {} 是旧拍照点配方，没有段计划可导出", recipe.id));
        }
        Ok(plan::export(&recipe.id, recipe.product_code, recipe.plan_version, &recipe.path_segments, &recipe.plan_cameras(), recipe.part_calib.as_ref()))
    }
}

/// PLC 开始请求要填的参数，供工程师配置 PLC 程序；按与接件核对相同的触发分组编出。
/// 旧拍照点模型；段配方的导出见 [`PlanExport::from_recipe`]（D-SB-25）。
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
/// 旧拍照点模型：段配方没有拍照点，不经过这里（D-SB-1）。
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

    /// 规格 §1/§5：段配方的 shotCount = deviceShots = ΣN_exp，计划相机按 inspect 顺序共用一个触发；
    /// 有段待标定且没有整件标定时编不出计划，有整件标定时取整件帧组数（决策补充第二节）。
    #[test]
    fn plc_plan_counts_are_sum_of_expected_frames_and_uncalibrated_plan_cannot_compile() {
        let mut doc = crate::recipe::samples_burst();
        doc.segments.push(plan::new_segment(2, plan::SegmentKind::Corner, &doc.segment_defaults));
        doc.segments[1].expected_frames = Some(3);
        doc.inspect.swap(0, 2);
        let compiled = PlcPlan::from_recipe(&doc.build().unwrap()).unwrap();
        assert_eq!((compiled.shot_count, compiled.device_shots, compiled.tolerance_total), (4 + 3, 4 + 3, 1 + 1));
        assert_eq!(compiled.device_ids, ["cam3", "cam2", "cam1"]);
        doc.segments[1].expected_frames = None;
        assert!(PlcPlan::from_recipe(&doc.build().unwrap()).unwrap_err().contains("待标定"));
        doc.part_calib = Some(plan::PartCalib { expected_total: 9, calib: plan::SegmentCalib { rounds: 10, min: 8, max: 10, mean: 9.2 } });
        let compiled = PlcPlan::from_recipe(&doc.build().unwrap()).unwrap();
        assert_eq!((compiled.shot_count, compiled.device_shots), (9, 9));
    }

}

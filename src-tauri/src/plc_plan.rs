use serde::{Deserialize, Serialize};

use crate::plan::{self, ExportSegment, PlanExport};
use crate::recipe::Recipe;

/// PLC 计划：生产接件与 S7 会话按它核对开始请求。段计划 `shot_count = device_shots = ΣN_exp`，
/// 另带段序列与整件容差（规格 §1/§5）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlcPlan {
    pub protocol_version: u16,
    pub recipe_id: String,
    pub plan_version: u32,
    /// ΣN_exp；示教采集计划为 0
    pub shot_count: u16,
    /// 接在 PLC 触发线上的相机；每次触发同时触发全部
    pub device_ids: Vec<String>,
    /// 每台相机的计划触发数，同为 ΣN_exp
    pub device_shots: u16,
    /// 整件帧数容差 Σtolerance
    #[serde(default)]
    pub tolerance_total: u32,
    /// 段序列；为空即示教采集计划
    #[serde(default)]
    pub segments: Vec<ExportSegment>,
}

impl PlcPlan {
    /// 段计划：段序列非空。
    pub fn is_burst(&self) -> bool {
        !self.segments.is_empty()
    }

    /// 段配方的示教采集计划：没有段，计数为 0。只能用于已绑定采集轮次的 S7 事务，
    /// 会话核对时不比较 shotCount / deviceShots（规格 §5：示教轮次不核对，PLC 可填估算值，见 PlcSession::validate_plan）。
    pub fn is_teaching(&self) -> bool {
        self.segments.is_empty()
    }

    /// 段配方的示教采集计划：本轮相机共用一个触发；PLC 写来的计数只由采集轮次记录（CaptureRoundV2.plcShotCount）。
    pub fn teaching(recipe_id: String, version: u32, device_ids: Vec<String>) -> Result<Self, String> {
        if version > i32::MAX as u32 {
            return Err(format!("计划版本 {version} 超出 PLC DInt 上限 2147483647，PLC 无法核对"));
        }
        if device_ids.is_empty() || device_ids.iter().enumerate().any(|(i, id)| id.trim().is_empty() || device_ids[..i].contains(id)) {
            return Err("示教采集必须指定不重复、非空的相机设备 ID".into());
        }
        Ok(Self {
            protocol_version: crate::handshake::PROTOCOL_VERSION,
            recipe_id,
            plan_version: version,
            shot_count: 0,
            device_ids,
            device_shots: 0,
            tolerance_total: 0,
            segments: Vec::new(),
        })
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
        let tolerance_total = segments
            .iter()
            .fold(0u32, |sum, segment| sum.saturating_add(segment.tolerance));
        Ok(Self {
            protocol_version: crate::handshake::PROTOCOL_VERSION,
            recipe_id,
            plan_version: version,
            shot_count: expected_total as u16,
            device_ids,
            device_shots: expected_total as u16,
            tolerance_total,
            segments,
        })
    }

    /// 按计划自身的字段重编一次：内容与计数自洽时与原计划相等（段计划走 compile_burst，示教采集计划走 teaching）。
    pub fn recompiled(&self) -> Result<Self, String> {
        if self.is_burst() {
            Self::compile_burst(self.recipe_id.clone(), self.plan_version, self.device_ids.clone(), self.segments.clone(), u32::from(self.shot_count))
        } else {
            Self::teaching(
                self.recipe_id.clone(),
                self.plan_version,
                self.device_ids.clone(),
            )
        }
    }

    /// 编段计划（ΣN_exp 未标定时不能编）。
    pub fn from_recipe(recipe: &Recipe) -> Result<Self, String> {
        if recipe.plan_version == 0 {
            return Err(format!("配方 {} 还没有分配 PLC 计划版本：请保存或发布一次后再布防", recipe.id));
        }
        let export = PlanExport::from_recipe(recipe)?;
        let Some(total) = export.expected_total else {
            let pending: Vec<String> = export
                .segments
                .iter()
                .filter(|s| s.expected_frames.is_none())
                .map(|s| s.index.to_string())
                .collect();
            return Err(format!(
                "配方 {} 的段 {} 待标定，且没有整件标定：标定出预期帧数前编不出 PLC 计划",
                recipe.id,
                pending.join("、")
            ));
        };
        Self::compile_burst(
            recipe.id.clone(),
            recipe.plan_version,
            export.cameras,
            export.segments,
            total,
        )
    }
}

impl PlanExport {
    /// 段配方的计划导出（规格 §1，命令 recipe_plan_export）：不要求已分配计划号，也不要求已标定（未标定时 expectedTotal 为 null）。
    pub fn from_recipe(recipe: &Recipe) -> Result<Self, String> {
        Ok(plan::export(&recipe.id, recipe.product_code, recipe.plan_version, &recipe.path_segments, &recipe.plan_cameras(), recipe.part_calib.as_ref()))
    }
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

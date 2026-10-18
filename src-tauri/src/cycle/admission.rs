use crate::handshake::Request;
use crate::judge::fault;
use crate::plc_plan::PlcPlan;
use crate::recipe::Recipe;

/// PLC 开始请求对照选中配方：先核对产品代码，再核对触发数量，计划版本与内容留给 S7 会话按计划核对。
/// 人工选型号时 PLC 下发了别的产品，报配方不可用而不是计划不一致。
pub(super) fn admit(request: &Request, recipe: &Recipe, plan: &PlcPlan) -> Result<(), (u16, String)> {
    if request.product_code != recipe.product_code {
        return Err((fault::NO_RECIPE, format!("PLC 产品代码 {} 与所选配方 {}（产品代码 {}）不一致",
            request.product_code, recipe.id, recipe.product_code)));
    }
    if request.shot_count != plan.shot_count || request.device_shots != plan.device_shots {
        return Err((fault::SHOT_COUNT_MISMATCH, format!("PLC 下发触发次数 {}，配方 {} 为 {}（{} 个拍照点）",
            request.shot_count, recipe.id, plan.shot_count, recipe.shot_count())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_code_mismatch_is_reported_before_count_and_count_before_plan_version() {
        let recipe = crate::recipe::samples().remove(1).build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe).unwrap();
        let request = |product_code: u16, shots: u16, plan_version: u32| Request {
            protocol_version: crate::handshake::PROTOCOL_VERSION, request_seq: 1, sn: "SN-1".into(),
            product_code, shot_count: shots, plan_version, device_shots: shots,
        };
        let other = recipe.product_code + 1;
        for (request, expected) in [
            (request(other, plan.shot_count + 1, plan.plan_version + 1), Some(fault::NO_RECIPE)),
            (request(other, plan.shot_count, plan.plan_version), Some(fault::NO_RECIPE)),
            (request(recipe.product_code, plan.shot_count + 1, plan.plan_version + 1), Some(fault::SHOT_COUNT_MISMATCH)),
            (request(recipe.product_code, plan.shot_count, plan.plan_version + 1), None),
            (request(recipe.product_code, plan.shot_count, plan.plan_version), None),
        ] {
            assert_eq!(admit(&request, &recipe, &plan).err().map(|(code, _)| code), expected, "{request:?}");
        }
    }
}

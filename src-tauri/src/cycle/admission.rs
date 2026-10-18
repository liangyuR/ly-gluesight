use crate::handshake::Request;
use crate::judge::fault;
use crate::plc_plan::PlcPlan;
use crate::recipe::Recipe;

/// PLC 开始请求对照选中配方：先核对产品代码，再看配方能否编出拍照计划，然后核对触发数量，计划版本与内容留给 S7 会话按计划核对。
/// 人工选型号时 PLC 下发了别的产品，报配方不可用，不让计划编译失败或计划不一致抢先。
pub(super) fn admit(request: &Request, recipe: &Recipe, plan: Result<PlcPlan, String>) -> Result<PlcPlan, (u16, String)> {
    if request.product_code != recipe.product_code {
        return Err((fault::NO_RECIPE, format!("PLC 产品代码 {} 与所选配方 {}（产品代码 {}）不一致",
            request.product_code, recipe.id, recipe.product_code)));
    }
    let plan = plan.map_err(|reason| (fault::PLAN_MISMATCH, reason))?;
    if request.shot_count != plan.shot_count || request.device_shots != plan.device_shots {
        return Err((fault::SHOT_COUNT_MISMATCH, format!("PLC 下发 shotCount={} deviceShots={}，配方 {} 需 shotCount={} deviceShots={}（{} 个拍照点）",
            request.shot_count, request.device_shots, recipe.id, plan.shot_count, plan.device_shots, recipe.shot_count())));
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_code_checked_before_plan_compile_and_trigger_counts_before_plan_version() {
        let recipe = crate::recipe::samples().remove(1).build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe).unwrap();
        let (code, n, version) = (recipe.product_code, plan.shot_count, plan.plan_version);
        let request = |product_code: u16, shot_count: u16, device_shots: u16, plan_version: u32| Request {
            protocol_version: crate::handshake::PROTOCOL_VERSION, request_seq: 1, sn: "SN-1".into(),
            product_code, shot_count, plan_version, device_shots,
        };
        let broken = || Err::<PlcPlan, String>("配方用到的相机不在设备配置里".into());
        for (request, compiled, expected) in [
            (request(code + 1, n + 1, n + 1, version + 1), Ok(plan.clone()), Some(fault::NO_RECIPE)),
            (request(code + 1, n, n, version), broken(), Some(fault::NO_RECIPE)),
            (request(code, n, n, version), broken(), Some(fault::PLAN_MISMATCH)),
            (request(code, n + 1, n + 1, version + 1), Ok(plan.clone()), Some(fault::SHOT_COUNT_MISMATCH)),
            (request(code, n, n + 1, version), Ok(plan.clone()), Some(fault::SHOT_COUNT_MISMATCH)),
            (request(code, n, n, version + 1), Ok(plan.clone()), None),
            (request(code, n, n, version), Ok(plan.clone()), None),
        ] {
            assert_eq!(admit(&request, &recipe, compiled).err().map(|(code, _)| code), expected, "{request:?}");
        }
    }
}

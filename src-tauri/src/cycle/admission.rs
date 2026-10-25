use crate::handshake::Request;
use crate::judge::fault;
use crate::plc_plan::PlcPlan;
use crate::recipe::Recipe;

/// PLC 开始请求对照选中配方：先核对型号，再看配方能否编出拍照计划。协议 7 的请求不带拍照数量与计划，不再比对。
/// 人工选型号时 PLC 下发了别的型号，报配方不可用，不让计划编译失败抢先。
pub(super) fn admit(request: &Request, recipe: &Recipe, plan: Result<PlcPlan, String>) -> Result<PlcPlan, (u16, String)> {
    if request.product_code != recipe.product_code {
        return Err((fault::NO_RECIPE, format!("PLC 型号 {} 与所选配方 {}（型号 {}）不一致",
            request.product_code, recipe.id, recipe.product_code)));
    }
    plan.map_err(|reason| (fault::PLAN_MISMATCH, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生产请求先核型号（95），再看配方能否编出计划（100）：型号不符时不让计划编译失败抢先。
    #[test]
    fn admission_checks_product_before_plan() {
        let recipe = crate::recipe::samples_burst().build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe).unwrap();
        let code = recipe.product_code;
        let request = |product_code: u16| Request {
            protocol_version: crate::handshake::PROTOCOL_VERSION, request_seq: 1, sn: "SN-1".into(), product_code,
        };
        let broken = || Err::<PlcPlan, String>("配方用到的相机不在设备配置里".into());
        for (request, compiled, expected) in [
            (request(code + 1), Ok(plan.clone()), Some(fault::NO_RECIPE)),
            (request(code + 1), broken(), Some(fault::NO_RECIPE)),
            (request(code), broken(), Some(fault::PLAN_MISMATCH)),
            (request(code), Ok(plan.clone()), None),
        ] {
            assert_eq!(admit(&request, &recipe, compiled).err().map(|(code, _)| code), expected, "{request:?}");
        }
    }
}

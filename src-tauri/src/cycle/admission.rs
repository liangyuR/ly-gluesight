use crate::handshake::Request;
use crate::judge::fault;
use crate::plc_plan::PlcPlan;
use crate::recipe::Recipe;

/// PLC 开始请求对照选中配方：先核对型号，再看配方能否编出拍照计划。协议 7 的请求不带拍照数量与计划，不再比对。
/// 人工选型号时 PLC 下发了别的型号，报配方不可用，不让计划编译失败抢先。
pub(super) fn admit(request: &Request, recipe: &Recipe, plan: Result<PlcPlan, String>) -> Result<PlcPlan, (u16, String)> {
    if request.product_code != recipe.product_code {
        return Err((
            fault::NO_RECIPE,
            format!(
                "PLC 型号 {} 与所选配方 {}（型号 {}）不一致",
                request.product_code, recipe.id, recipe.product_code
            ),
        ));
    }
    plan.map_err(|reason| (fault::PLAN_MISMATCH, reason))
}

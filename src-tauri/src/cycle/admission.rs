use crate::handshake::Request;
use crate::judge::fault;
use crate::plc_plan::PlcPlan;
use crate::recipe::Recipe;

/// PLC 开始请求对照选中配方：先核对产品代码，再看配方能否编出拍照计划，然后核对触发数量，计划版本与内容留给 S7 会话按计划核对。
/// 人工选型号时 PLC 下发了别的产品，报配方不可用，不让计划编译失败或计划不一致抢先。
/// 段配方的 shotCount 与 deviceShots 都必须等于 ΣN_exp（D-SB-2）；旧配方等于触发次数。
pub(super) fn admit(request: &Request, recipe: &Recipe, plan: Result<PlcPlan, String>) -> Result<PlcPlan, (u16, String)> {
    if request.product_code != recipe.product_code {
        return Err((fault::NO_RECIPE, format!("PLC 产品代码 {} 与所选配方 {}（产品代码 {}）不一致",
            request.product_code, recipe.id, recipe.product_code)));
    }
    let plan = plan.map_err(|reason| (fault::PLAN_MISMATCH, reason))?;
    if request.shot_count != plan.shot_count || request.device_shots != plan.device_shots {
        let basis = if plan.is_burst() { "各段预期帧数之和 ΣN_exp".to_owned() } else { format!("{} 个拍照点", recipe.shot_count()) };
        return Err((fault::SHOT_COUNT_MISMATCH, format!("PLC 下发 shotCount={} deviceShots={}，配方 {} 需 shotCount={} deviceShots={}（{basis}）",
            request.shot_count, request.device_shots, recipe.id, plan.shot_count, plan.device_shots)));
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规格 §5：生产请求先核产品代码（95），再看配方能否编出计划（100），再核 shotCount 与 deviceShots 都等于 ΣN_exp（94）；
    /// 计划版本留给 S7 会话核对。
    #[test]
    fn admission_checks_product_then_plan_then_counts_against_sum() {
        let mut doc = crate::recipe::samples_burst();
        doc.segments.push(crate::plan::new_segment(2, crate::plan::SegmentKind::Corner, &doc.segment_defaults));
        doc.segments[1].expected_frames = Some(3);
        let recipe = doc.build().unwrap();
        let plan = PlcPlan::from_recipe(&recipe).unwrap();
        let (code, n, version) = (recipe.product_code, 4 + 3, plan.plan_version);
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

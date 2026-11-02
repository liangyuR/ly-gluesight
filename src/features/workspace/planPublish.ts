import type { WorkspacePlanView } from "../plan/types";
import { PLAN_CAMERAS } from "../plan/vis";

/**
 * 段配方不能发布的原因（界面层由 DTO 得出）；空串 = 计划本身不挡发布。
 * 发布门槛：有整件 ΣN_exp（plan.calibrated）；多段配方还要各段都有 N_exp（D-SB-45，生产按累计 N_exp 预测每帧所属段），N_exp 取自示教件分段；
 * 三台相机不是 3 台时不能发布（D-SB-24）。走廊示教、设备等其余前置由后端发布前检查给出。
 */
export function planPublishBlocker(plan: Pick<WorkspacePlanView, "cameras" | "segments" | "calibrated">): string {
  if (plan.cameras.length !== PLAN_CAMERAS) return `段配方需要 ${PLAN_CAMERAS} 台相机共用一个触发，当前 ${plan.cameras.length} 台，不能发布`;
  const pending = plan.segments.filter(segment => segment.expectedFrames == null).map(segment => `段 ${segment.index}`);
  if (plan.calibrated && (plan.segments.length <= 1 || !pending.length)) return "";
  return pending.length ? `${pending.join("、")} 没有预期帧数：先在分段步骤确认示教件边界，才能发布` : "尚无段：先在段列表添加至少一段，再分段";
}

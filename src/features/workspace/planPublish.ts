import type { WorkspacePlanView } from "../plan/types";
import { PLAN_CAMERAS } from "../plan/vis";

/**
 * 段配方不能发布的原因（界面层由 DTO 得出）；空串 = 计划本身不挡发布。
 * 发布门槛：整件 ΣN_exp 已标定（plan.calibrated）；多段配方还要各段 N_exp 都已标定（D-SB-45，生产按累计 N_exp 预测每帧所属段）；
 * 三台相机不是 3 台时不能发布（D-SB-24）。走廊示教、设备等其余前置由后端发布前检查给出。
 */
export function planPublishBlocker(plan: Pick<WorkspacePlanView, "cameras" | "segments" | "calibrated">): string {
  if (plan.cameras.length !== PLAN_CAMERAS) return `段配方需要 ${PLAN_CAMERAS} 台相机共用一个触发，当前 ${plan.cameras.length} 台，不能发布`;
  const pending = plan.segments.filter(segment => segment.expectedFrames == null).map(segment => `段 ${segment.index}`);
  if (plan.calibrated && (plan.segments.length <= 1 || !pending.length)) return "";
  return pending.length ? `${pending.join("、")} 待标定：先完成多轮标定得出预期帧数，才能发布` : "尚无段：先在段列表添加至少一段，再标定";
}

/** 整件已标定但仍有段 N_exp 为 null 时的说明（待逐段标定，运行时按整件核对帧数）；没有这种段返回空串。 */
export function pendingSegmentsNote(plan: Pick<WorkspacePlanView, "segments" | "calibrated">): string {
  if (!plan.calibrated) return "";
  const pending = plan.segments.filter(segment => segment.expectedFrames == null).map(segment => `段 ${segment.index}`);
  return pending.length ? `${pending.join("、")} 待逐段标定，运行时按整件帧数核对` : "";
}

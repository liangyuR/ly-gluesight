import type { WorkspacePlanView } from "../plan/types";
import { PLAN_CAMERAS } from "../plan/vis";

/**
 * 段配方不能发布的原因（界面层由 DTO 得出）；空串 = 计划本身不挡发布。
 * 发布门槛是整件 ΣN_exp 已标定（plan.calibrated），各段 N_exp 为 null 只是待分段算法，不挡发布（决策补充第二节）；
 * 三台相机不是 3 台时不能发布（D-SB-24）。走廊示教等其余前置由后端进度步骤给出。
 */
export function planPublishBlocker(plan: Pick<WorkspacePlanView, "cameras" | "segments" | "calibrated">): string {
  if (plan.cameras.length !== PLAN_CAMERAS) return `段配方需要 ${PLAN_CAMERAS} 台相机共用一个触发，当前 ${plan.cameras.length} 台，不能发布`;
  if (plan.calibrated) return "";
  const pending = plan.segments.filter(segment => segment.expectedFrames == null).map(segment => `段 ${segment.index}`);
  return pending.length ? `${pending.join("、")} 待标定：先完成多轮标定得出预期帧数，才能发布` : "尚无段：先在段列表添加至少一段，再标定";
}

/** 整件已标定但仍有段 N_exp 为 null 时的说明（待分段算法，运行时按整件核对帧数）；没有这种段返回空串。 */
export function pendingSegmentsNote(plan: Pick<WorkspacePlanView, "segments" | "calibrated">): string {
  if (!plan.calibrated) return "";
  const pending = plan.segments.filter(segment => segment.expectedFrames == null).map(segment => `段 ${segment.index}`);
  return pending.length ? `${pending.join("、")} 待分段算法，运行时按整件帧数核对` : "";
}

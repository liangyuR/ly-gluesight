import type { ProgressStepV2 } from "../plan/types";
import type { CameraTeachingTask } from "./cameraTasks";
import type { ProgressStep } from "./types";
import type { TeachingTask } from "./teachingTasks";

/** 段配方：下一步该做什么。整圈采集没完成先去采集；否则按相机顺序找第一台没示教完的相机，都完成时为 null。 */
export function cameraTeachingAction(tasks: CameraTeachingTask[], capture: ProgressStepV2 | undefined) {
  if (capture && capture.status !== "done") return { label: "前往整圈采集", href: "/recipe/capture", capture: true };
  const next = tasks.find(task => task.status !== "done");
  return next ? { label: `示教 ${next.camera} 的走廊`, href: next.href, capture: false } : null;
}

/** 旧拍照点模型：按 Pose 分组的示教任务，保留给旧配方的进度总览。 */
export function teachingAction(tasks: TeachingTask[], capture: ProgressStep | undefined, step: ProgressStep | undefined) {
  const recapture = tasks.some(task => task.phase === "recapture") || capture?.status === "blocked";
  if (capture?.status !== "done" || recapture || step?.blockers.some(b => b.target.page === "capture")) return { label: recapture ? "前往重新采集" : "前往整圈采集", href: "/recipe/capture", capture: true };
  const next = tasks.find(task => task.phase !== "done");
  return next ? { label: "继续示教", href: next.href, capture: false } : null;
}

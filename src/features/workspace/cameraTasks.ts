/**
 * 段配方的走廊示教任务：计划里的每台相机示教一条走廊（按直线段几何），三台共用一个触发。
 */
import type { CameraTeachingView, WorkspacePlanView } from "../plan/types";
import { PLAN_CAMERAS } from "../plan/vis";
import type { FrozenImage, StepStatus, WorkspaceView } from "./types";

/** selectFrame 还没选示教帧；teach 已选帧、走廊未保存；scale 走廊已保存但缺像素当量；done 走廊与当量都有。 */
export type CameraTeachPhase = "selectFrame" | "teach" | "scale" | "done";

export interface CameraTeachingTask {
  camera: string;
  status: CameraTeachPhase;
  /** 已选的示教帧 */
  frame?: FrozenImage;
  label: string;
  /** 这台相机的走廊示教页 */
  href: string;
}

const phaseLabel: Record<CameraTeachPhase, string> = { selectFrame: "待选帧", teach: "待示教走廊", scale: "缺像素当量", done: "已示教" };

/** 走廊示教页（ProgressTargetV2 的 corridor{camera} 指向这里）。 */
export function corridorPath(camera: string): string {
  return `/recipe/teach?camera=${encodeURIComponent(camera)}`;
}

function phaseOf(teaching: CameraTeachingView | undefined): CameraTeachPhase {
  if (teaching?.saved || teaching?.covered) return teaching.mmPerPx != null ? "done" : "scale";
  return teaching?.frame ? "teach" : "selectFrame";
}

/** 按计划相机顺序列出每台相机的走廊示教任务；后端没给示教状态的相机按“待选帧”。 */
export function cameraTeachingTasks(plan: Pick<WorkspacePlanView, "cameras" | "cameraTeaching">): CameraTeachingTask[] {
  return plan.cameras.map(camera => {
    const teaching = plan.cameraTeaching.find(t => t.camera === camera);
    const status = phaseOf(teaching);
    const label = status === "done" && !teaching?.saved ? "按段已示教" : phaseLabel[status];
    return { camera, status, ...(teaching?.frame ? { frame: teaching.frame } : {}), label, href: corridorPath(camera) };
  });
}

/**
 * 走廊示教步骤的状态；没有段计划时为 null。
 * 计划相机不是 3 台时阻塞；后端记为完成但仍有相机未示教完时按任务实际状态显示“进行中”。
 */
export function cameraTeachStatus(view: Pick<WorkspaceView, "plan">): StepStatus | null {
  const plan = view.plan;
  if (!plan) return null;
  if (plan.cameras.length !== PLAN_CAMERAS) return "blocked";
  const tasks = cameraTeachingTasks(plan);
  const derived: StepStatus = tasks.every(task => task.status === "done") ? "done" : tasks.every(task => task.status === "selectFrame") ? "notStarted" : "inProgress";
  const step = plan.progress.steps.find(s => s.id === "corridor");
  if (!step) return derived;
  return step.status === "done" && derived !== "done" ? "inProgress" : step.status;
}

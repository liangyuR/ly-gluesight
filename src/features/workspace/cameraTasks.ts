/**
 * 段配方的走廊示教任务（D-SB-45）：走廊只按“段 × 该段的检测相机”示教，每一对一条走廊、一张示教帧；
 * 相机只留参数（像素当量、检测参数）。缺任何一对或相机缺像素当量，走廊示教就没完成。
 */
import type { SegmentKind, WorkspacePlanView } from "../plan/types";
import { PLAN_CAMERAS } from "../plan/vis";
import type { FrozenImage, StepStatus, WorkspaceView } from "./types";
import { segmentCameras } from "./workbenchDraft";

/** selectFrame 还没选示教帧；teach 已选帧、走廊未保存；done 走廊已保存。 */
export type CorridorPhase = "selectFrame" | "teach" | "done";

export interface CorridorTask {
  camera: string;
  segment: number;
  kind: SegmentKind;
  status: CorridorPhase;
  frame?: FrozenImage;
  label: string;
  /** 这一段这台相机的走廊示教页 */
  href: string;
}

export interface CameraParamsTask {
  camera: string;
  mmPerPx: number | null;
  /** 后端判定：有像素当量且它检测的每一段都有已保存的走廊 */
  ready: boolean;
  label: string;
  href: string;
}

const phaseLabel: Record<CorridorPhase, string> = { selectFrame: "待选帧", teach: "待画走廊", done: "已示教" };

type TeachingPlan = Pick<WorkspacePlanView, "cameras" | "segments" | "cameraTeaching">;

/** 走廊示教页（ProgressTargetV2 的 corridor{camera, segment} 指向这里）；不带段时定位到相机参数。 */
export function corridorPath(camera: string, segment?: number | null): string {
  return `/recipe/teach?camera=${encodeURIComponent(camera)}` + (segment != null ? `&segment=${segment}` : "");
}

/** 按计划相机顺序列出每台相机的参数状态；后端没给示教状态的相机按缺像素当量。 */
export function cameraParamsTasks(plan: TeachingPlan): CameraParamsTask[] {
  return plan.cameras.map(camera => {
    const teaching = plan.cameraTeaching.find(t => t.camera === camera);
    const mmPerPx = teaching?.mmPerPx ?? null;
    return { camera, mmPerPx, ready: !!teaching?.ready, label: mmPerPx == null ? "缺像素当量" : "已设", href: corridorPath(camera) };
  });
}

/** 按相机、再按段的顺序列出每一对（段, 检测相机）的走廊任务。 */
export function corridorTasks(plan: TeachingPlan): CorridorTask[] {
  return plan.cameras.flatMap(camera => {
    const views = plan.cameraTeaching.find(t => t.camera === camera)?.segments ?? [];
    return plan.segments.filter(segment => segmentCameras(segment, plan.cameras).includes(camera)).map(segment => {
      const view = views.find(v => v.segment === segment.index);
      const status: CorridorPhase = view?.saved ? "done" : view ? "teach" : "selectFrame";
      return { camera, segment: segment.index, kind: segment.kind, status, ...(view ? { frame: view.frame } : {}), label: phaseLabel[status], href: corridorPath(camera, segment.index) };
    });
  });
}

/** 下一个要做的示教：第一条未完成的走廊，其次第一台缺像素当量的相机；都完成为 null。 */
export function nextTeachingHref(plan: TeachingPlan): string | null {
  return corridorTasks(plan).find(task => task.status !== "done")?.href ?? cameraParamsTasks(plan).find(item => item.mmPerPx == null)?.href ?? null;
}

/**
 * 走廊示教步骤的状态；没有段计划时为 null。
 * 计划相机不是 3 台时阻塞；后端记为完成但仍有相机未就绪时按实际状态显示“进行中”。
 */
export function cameraTeachStatus(view: Pick<WorkspaceView, "plan">): StepStatus | null {
  const plan = view.plan;
  if (!plan) return null;
  if (plan.cameras.length !== PLAN_CAMERAS) return "blocked";
  const derived: StepStatus = cameraParamsTasks(plan).every(item => item.ready) ? "done" : corridorTasks(plan).every(task => task.status === "selectFrame") ? "notStarted" : "inProgress";
  const step = plan.progress.steps.find(s => s.id === "corridor");
  if (!step) return derived;
  return step.status === "done" && derived !== "done" ? "inProgress" : step.status;
}

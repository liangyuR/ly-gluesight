import type { TeachingTask } from "./teachingTasks";
import type { Progress, StepId, StepStatus } from "./types";

export type StageStatus = StepStatus | "waiting";
export interface Stage { path: string; label: string; hint: string; ids: StepId[]; sub: StepId[]; optional: boolean; needsCapture: boolean }

/** 工作台步骤：步骤条与进度总览共用，编号与顺序一致。 */
export const stages: Stage[] = [
  { path: "/recipe/capture", label: "整圈采集", hint: "按拍照计划采集整圈原图", ids: ["capture"], sub: [], optional: false, needsCapture: false },
  { path: "/recipe/geometry", label: "默认参数", hint: "默认检测与判定参数，单帧示教时可按图覆盖", ids: ["defaults"], sub: [], optional: true, needsCapture: false },
  { path: "/recipe/teach", label: "单帧示教", hint: "每组选一张检测图并示教胶路中线", ids: ["teach"], sub: ["calibration"], optional: false, needsCapture: true },
  { path: "/recipe/overview", label: "工件总览", hint: "在工件图上核对各拍照点位置", ids: ["overview"], sub: [], optional: true, needsCapture: true },
  { path: "/recipe/validation", label: "验证与发布", hint: "独立样本验证通过后发布生效", ids: ["validation", "publish"], sub: [], optional: false, needsCapture: true },
];

export const stageStatusText: Record<StageStatus, string> = { notStarted: "未开始", inProgress: "进行中", done: "完成", blocked: "阻塞", optional: "可选", waiting: "等待采集" };
export const stageStatusTone: Record<StageStatus, "info" | "ok" | "warn" | "neutral"> = { notStarted: "neutral", inProgress: "info", done: "ok", blocked: "warn", optional: "neutral", waiting: "neutral" };

export const stageOf = (id: StepId) => stages.find(stage => stage.ids.includes(id) || stage.sub.includes(id));

/** 后端记为完成但仍有未完成的示教组时，按示教组实际状态显示。 */
export function teachStatus(status: StepStatus, tasks: TeachingTask[]): StepStatus {
  const current = tasks.find(task => task.phase !== "done");
  return status === "done" && current ? current.exception ? "blocked" : "inProgress" : status;
}

/** 整圈采集未完成时，依赖采集的步骤一律等待，不沿用后端的旧完成状态。 */
export function stageStatus(progress: Progress, stage: Stage, tasks: TeachingTask[]): StageStatus | null {
  const capture = progress.steps.find(step => step.id === "capture");
  if (stage.needsCapture && capture && capture.status !== "done") return "waiting";
  const statuses = progress.steps.filter(step => stage.ids.includes(step.id)).map(step => step.id === "teach" ? teachStatus(step.status, tasks) : step.status);
  if (!statuses.length) return null;
  return (["blocked", "inProgress", "notStarted", "optional"] as StepStatus[]).find(status => statuses.includes(status)) ?? "done";
}

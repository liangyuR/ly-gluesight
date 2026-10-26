import type { StepIdV2 } from "../plan/types";
import { cameraTeachStatus } from "./cameraTasks";
import type { StepStatus, WorkspaceView } from "./types";

export type StageStatus = StepStatus | "waiting";
export const stageStatusText: Record<StageStatus, string> = { notStarted: "未开始", inProgress: "进行中", done: "完成", blocked: "阻塞", optional: "可选", waiting: "等待采集" };
export const stageStatusTone: Record<StageStatus, "info" | "ok" | "warn" | "neutral"> = { notStarted: "neutral", inProgress: "info", done: "ok", blocked: "warn", optional: "neutral", waiting: "neutral" };

export interface StageV2 { path: string; label: string; hint: string; ids: StepIdV2[]; optional: boolean; needsCapture: boolean }

/**
 * 工作台步骤（段配方）：采集 → 段列表 → 走廊示教 → 多轮标定 → 验证与发布（StepIdV2）。
 * 原“默认参数”页已拆分：测量默认参数在走廊示教，标定轮数在多轮标定，边界过渡帧数在段列表，名称与型号在工件总览。
 */
export const stagesV2: StageV2[] = [
  { path: "/recipe/capture", label: "整圈采集", hint: "三台相机同时采集整圈，按帧号配组，每轮记各相机帧数", ids: ["capture"], optional: false, needsCapture: false },
  { path: "/recipe/segments", label: "段列表", hint: "沿胶路划分直线段与弯道段，设各段检测项与帧数容差", ids: ["segments"], optional: false, needsCapture: false },
  { path: "/recipe/teach", label: "走廊示教", hint: "每台相机示教一条检测走廊，所有帧共用", ids: ["corridor"], optional: false, needsCapture: true },
  { path: "/recipe/calibration", label: "多轮标定", hint: "多轮采集取均值得出预期帧数，确认帧数容差", ids: ["calibration"], optional: false, needsCapture: true },
  { path: "/recipe/validation", label: "验证与发布", hint: "独立样本验证通过后发布生效", ids: ["validation", "publish"], optional: false, needsCapture: true },
];

export const stageOfV2 = (id: StepIdV2) => stagesV2.find(stage => stage.ids.includes(id));

/**
 * 段配方一个步骤的状态；没有段计划时为 null。
 * 采集未完成时依赖采集的步骤等待采集；走廊示教按每台相机的实际任务状态（cameraTeachStatus），不照搬后端的完成。
 */
export function stageStatusV2(view: Pick<WorkspaceView, "plan">, stage: StageV2): StageStatus | null {
  const plan = view.plan;
  if (!plan) return null;
  const capture = plan.progress.steps.find(step => step.id === "capture");
  if (stage.needsCapture && capture && capture.status !== "done") return "waiting";
  const statuses = stage.ids.flatMap(id => {
    const status = id === "corridor" ? cameraTeachStatus(view) : plan.progress.steps.find(step => step.id === id)?.status;
    return status ? [status] : [];
  });
  if (!statuses.length) return null;
  return (["blocked", "inProgress", "notStarted", "optional"] as StepStatus[]).find(status => statuses.includes(status)) ?? "done";
}

import type { Verdict } from "../cycle/types";
import type { ProgressTargetV2, ProgressV2, StepIdV2 } from "../plan/types";
import { corridorPath } from "./cameraTasks";
import type { StepStatus } from "./types";

export const stepStatusText: Record<StepStatus, string> = { notStarted: "未开始", inProgress: "进行中", done: "完成", blocked: "阻塞" };
export const stepStatusTone: Record<StepStatus, "info" | "ok" | "warn" | "neutral"> = { notStarted: "neutral", inProgress: "info", done: "ok", blocked: "warn" };

/** 段配方（StepIdV2）进度目标的路由。 */
export function targetPathV2(target: ProgressTargetV2) {
  switch (target.page) {
    case "capture": return "/recipe/capture";
    case "segments": return "/recipe/segments";
    case "corridor": return corridorPath(target.camera, target.segment);
    case "calibration": return "/recipe/calibration";
    case "validation": return "/recipe/validation";
    case "camera": return "/camera";
  }
}

/** 段配方的进度步骤；后端没输出该步骤时为 undefined。 */
export function stepOfV2(progress: ProgressV2, id: StepIdV2) {
  return progress.steps.find(step => step.id === id);
}

/** 人工期望只区分“合格”与具体异常类型；合格样本的实际 OK 与 OK·局部超差都算符合。 */
export const expectedChoices: [Verdict, string][] = [
  ["OK", "合格"], ["NG_POSITION", "NG · 位置超差"], ["NG_WIDTH", "NG · 胶宽超差"],
  ["NG_ABSOLUTE", "NG · 超绝对限"], ["NG_GAP", "NG · 断胶"], ["ERR_INSPECT", "ERR · 未测成"],
];
export const expectedValue = (v: Verdict): Verdict => v === "OK_WITH_EXCURSION" ? "OK" : v;
export const expectedLabel = (v: Verdict) => expectedChoices.find(([value]) => value === expectedValue(v))![1];

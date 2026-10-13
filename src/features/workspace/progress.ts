import type { Verdict } from "../cycle/types";
import type { CalibrationState, Progress, ProgressTarget, ShotState, StepId, StepStatus, ViewProgress } from "./types";

export const stepStatusText: Record<StepStatus, string> = { notStarted: "未开始", inProgress: "进行中", done: "完成", blocked: "阻塞", optional: "可选" };
export const stepStatusTone: Record<StepStatus, "info" | "ok" | "warn" | "neutral"> = { notStarted: "neutral", inProgress: "info", done: "ok", blocked: "warn", optional: "neutral" };
export const shotStateText: Record<ShotState, string> = {
  recapture: "需重新采集", unconfigured: "未配置", draft: "编辑草稿", pendingTrial: "待试测", trialFailed: "试测异常",
  trialPassed: "试测通过", taught: "已示教", skipped: "不检",
};

export function calibrationText(c: CalibrationState) {
  switch (c.state) {
    case "unverified": return "标定未验证";
    case "stale": return "当量或设备已变，需重新验证";
    case "verified": return `人工标定验证通过 · 误差 ${c.errorMm.toFixed(3)} mm`;
    case "failed": return `人工标定验证未通过 · 误差 ${c.errorMm.toFixed(3)} mm`;
    case "sampleSet": return `样本集声明当量 ${c.mmPerPx} mm/px（样本集 ${c.setId}）`;
  }
}

export function viewStateText(v: Pick<ViewProgress, "state" | "trialStale">) {
  return v.trialStale ? "试测已过期" : shotStateText[v.state];
}

export function targetPath(target: ProgressTarget) {
  switch (target.page) {
    case "capture": return "/recipe/capture";
    case "teach": return `/recipe/teach?frame=${target.k}`;
    case "validation": return "/recipe/validation";
    case "camera": return "/camera";
  }
}

export function stepOf(progress: Progress, id: StepId) {
  return progress.steps.find(step => step.id === id)!;
}

/** 人工期望只区分“合格”与具体异常类型；合格样本的实际 OK 与 OK·局部超差都算符合。 */
export const expectedChoices: [Verdict, string][] = [
  ["OK", "合格"], ["NG_POSITION", "NG · 位置超差"], ["NG_WIDTH", "NG · 胶宽超差"],
  ["NG_ABSOLUTE", "NG · 超绝对限"], ["NG_GAP", "NG · 断胶"], ["ERR_INSPECT", "ERR · 未测成"],
];
export const expectedValue = (v: Verdict): Verdict => v === "OK_WITH_EXCURSION" ? "OK" : v;
export const expectedLabel = (v: Verdict) => expectedChoices.find(([value]) => value === expectedValue(v))![1];

import type { ShotSpec } from "../cycle/types";
import type { Progress } from "./types";
import { teachingGroups } from "./teachingGroups";

export type TeachingPhase = "recapture" | "failed" | "conflict" | "unknown" | "select" | "teach" | "trial" | "save" | "done";
const labels: Record<TeachingPhase, string> = { recapture: "需重采", failed: "试测异常", conflict: "需重新选图", unknown: "状态待确认", select: "待选图", teach: "待示教", trial: "待试测", save: "待保存", done: "已完成" };
const order: Record<TeachingPhase, number> = { recapture: 0, failed: 1, conflict: 2, unknown: 3, select: 4, teach: 5, trial: 6, save: 7, done: 8 };

export function teachingTasks(shots: ShotSpec[], progress: Progress) {
  return teachingGroups(shots, progress).map((group, index) => {
    const { choice } = group;
    const view = choice ? progress.shots[choice.k]?.views.find(v => v.view === choice.view) : undefined;
    const recapture = group.members.find(k => progress.shots[k]?.state === "recapture");
    let phase: TeachingPhase;
    if (recapture !== undefined || view?.state === "recapture") phase = "recapture";
    else if (group.selected.length > 1) phase = "conflict";
    else if (!choice) phase = "select";
    else if (!view || !view.enabled || view.state === "skipped") phase = "unknown";
    else if (view.trialStale || view.state === "pendingTrial") phase = "trial";
    else if (view.state === "trialFailed") phase = "failed";
    else if (view.state === "taught") phase = "done";
    else if (view.state === "trialPassed") phase = "save";
    else if (view.state === "unconfigured" || view.state === "draft") phase = "teach";
    else phase = "unknown";
    const exception = ["recapture", "failed", "conflict", "unknown"].includes(phase);
    const tone = phase === "done" ? "ok" as const : exception ? "warn" as const : "neutral" as const;
    const href = phase === "recapture" ? "/recipe/capture" : `/recipe/teach?frame=${choice?.k ?? group.members[0]}`;
    const action = phase === "recapture" ? "前往重采" : phase === "select" || phase === "conflict" ? "选择检测图" : phase === "done" || phase === "unknown" ? "查看" : "继续示教";
    const message = phase === "recapture" ? `${shots[recapture ?? choice!.k].camera} 需整圈重新采集` : phase === "conflict" ? "本组选择了多张检测图，请保留一张。" : phase === "unknown" ? "当前检测图的进度尚未确认，请查看后重试。" : view?.hint;
    return { ...group, view, phase, label: labels[phase], exception, tone, href, action, message, index };
  }).sort((a, b) => order[a.phase] - order[b.phase] || a.index - b.index);
}

export type TeachingTask = ReturnType<typeof teachingTasks>[number];

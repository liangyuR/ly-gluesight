import type { ShotSpec } from "../cycle/types";
import type { Progress, ShotState } from "./types";
import { selectedViews } from "./teach";
import { viewStateText } from "./progress";

export function teachingGroups(shots: ShotSpec[], progress: Progress) {
  const groups = new Map<string, number[]>();
  shots.forEach((shot, k) => groups.set(shot.poseId, [...(groups.get(shot.poseId) ?? []), k]));
  return [...groups].map(([poseId, members]) => {
    const selected = members.flatMap(k => shots[k].skip ? [] : selectedViews(shots[k]).map(view => ({ k, view })));
    const choice = selected.length === 1 ? selected[0] : null;
    const state = choice ? progress.shots[choice.k]?.views.find(v => v.view === choice.view) : undefined;
    const complete = !!state && state.state === "taught" && !state.trialStale;
    return { poseId, members, selected, choice, complete, status: selected.length > 1 ? "需重新选图" : !choice ? "待选检测图" : state ? viewStateText(state) : "待示教" };
  });
}

const shortStateText: Record<ShotState, string> = {
  recapture: "需重采", unconfigured: "未配置", draft: "草稿", pendingTrial: "待试测", trialFailed: "试测异常",
  trialPassed: "试测通过", taught: "已示教", skipped: "不检",
};

/** 单帧示教左栏的两三字状态；其它页面仍用 teachingGroups 的长文案。 */
export function groupShortStatus(group: ReturnType<typeof teachingGroups>[number], progress: Progress) {
  if (group.selected.length > 1) return "需重选图";
  if (!group.choice) return "待选图";
  const state = progress.shots[group.choice.k]?.views.find(v => v.view === group.choice!.view);
  if (!state) return "待示教";
  return state.trialStale ? "已过期" : shortStateText[state.state];
}

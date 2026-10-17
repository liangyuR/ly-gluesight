import type { ShotSpec } from "../cycle/types";
import type { Progress } from "./types";
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

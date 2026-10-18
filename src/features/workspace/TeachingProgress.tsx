import type { ProgressStep } from "./types";
import type { TeachingTask } from "./teachingTasks";

export function teachingAction(tasks: TeachingTask[], capture: ProgressStep | undefined, step: ProgressStep | undefined) {
  const recapture = tasks.some(task => task.phase === "recapture") || capture?.status === "blocked";
  if (capture?.status !== "done" || recapture || step?.blockers.some(b => b.target.page === "capture")) return { label: recapture ? "前往重新采集" : "前往整圈采集", href: "/recipe/capture", capture: true };
  const next = tasks.find(task => task.phase !== "done");
  return next ? { label: "继续示教", href: next.href, capture: false } : null;
}

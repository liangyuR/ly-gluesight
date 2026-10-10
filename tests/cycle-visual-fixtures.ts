import type { FrameView, Measured, PartView, Recipe, ResultView } from "../src/features/cycle/types";
import { workspaceView } from "./fixtures";

export function cycleFrame(status: FrameView["status"] = "done", k = 0): FrameView {
  return { status, cam: 0, camera: "CAM-1", shotId: `P${k + 1}`, view: 1, session: 1, ordinal: k + 1, error: null, arrivedMs: 20, frameCounter: k + 101, triggerCounter: k + 201, counterJump: false, score: .9, points: 2, gapPoints: 0, ms: 8 };
}
export function cyclePart(sn = 1, cycleId = `cycle-${sn}`): PartView {
  return { cycleId, bundleHash: "bundle-A", sn, recipeId: "A", recipeHash: "hash-A", n: 2, received: 2, triggers: 2, queue: 0,
    filled: 4, total: 4, frames: [cycleFrame(), cycleFrame("done", 1)], measuredFrames: 2 };
}
export function cycleMeasurement(sn = 1, cycleId = `cycle-${sn}`): Measured {
  return { cycleId, shotId: "P1", camera: "CAM-1", bundleHash: "bundle-A", sn, k: 0, cam: 0, located: true, score: .9, ms: 8, error: null,
    idx: [0, 1, 2, 3], d: [3, 4.5, 3, 3], w: [2, 2.5, null, 3], st: [0, 0, 0, 0], px: [[10, 10], [20, 20], [30, 30], [40, 40]] };
}
export function cycleResult(sn = 1, cycleId = `cycle-${sn}`): ResultView {
  return { cycleId, sn, recipeId: "A", ts: 1, drainMs: 24, verdict: "NG_POSITION", plcCode: 2, faultCode: 0,
    reason: "测试样本偏移超差", gaps: [], segments: [
      { verdict: "NG_POSITION", min: 3, max: 4.5, excursionLen: 1, wMin: null, wMax: null, wExcursionLen: 0 },
      { verdict: "OK", min: 3, max: 3, excursionLen: 0, wMin: null, wMax: null, wExcursionLen: 0 },
    ] };
}
/** 带胶宽限值的布局（胶宽曲线用）。 */
export function widthLayout(): Recipe {
  const layout = workspaceView().layout;
  for (const g of layout.segments) g.width = { nominal: 2, tolUpper: .5, tolLower: .5, absMin: 1, absMax: 4, maxExcursionLen: 1 };
  return layout;
}

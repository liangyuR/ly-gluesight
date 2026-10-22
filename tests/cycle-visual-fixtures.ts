import type { FrameView, Measured, PartView, ResultView } from "../src/features/cycle/types";
import type { FrameGroupView } from "../src/features/plan/types";
import { burstCameraFrames, burstCameras, burstGroups, burstJudgement, burstMissing } from "./fixtures";

/** 段配方在途件（PartView.burst）：12 个帧组都已测完，CAM-2 缺 seq 5；旧字段按决策记录 5.1 取值。 */
export function burstPartView(sn = "1", cycleId = `cycle-${sn}`): PartView {
  const cameraFrames = burstCameraFrames();
  const received = cameraFrames.reduce((sum, n) => sum + n, 0);
  const groups: FrameGroupView[] = Array.from({ length: burstGroups }, (_, i) => {
    const seq = i + 1;
    const incomplete = seq === burstMissing.seq;
    return { seq, incomplete, cells: burstCameras.map((_, c) => incomplete && c === burstMissing.camera
      ? { status: "missing", verdict: null, frameCounter: null }
      : { status: "measured", verdict: "OK", frameCounter: 1000 * (c + 1) + seq }) };
  });
  return { cycleId, bundleId: "bundle-S", sn, recipeId: "S", recipeRevision: "revisionId-S", n: 12, received, triggers: 12, queue: 0,
    filled: 0, total: 0, frames: [], measuredFrames: received,
    burst: { cameras: burstCameras, expectedTotal: 12, toleranceTotal: 3, deviceTriggers: 12, groupCount: burstGroups, cameraFrames, measuredFrames: received, pendingMeasure: 0, groups } };
}
/** 段配方一件的结果（ResultView.burst）：flatten 的旧字段与 burst 相同，segments、gaps 为空。 */
export function burstResult(sn = "1", cycleId = `cycle-${sn}`): ResultView {
  const burst = burstJudgement();
  return { cycleId, sn, recipeId: "S", ts: 1, drainMs: 24, verdict: burst.verdict, plcCode: burst.plcCode, faultCode: burst.faultCode, reason: burst.reason, segments: [], gaps: [], burst };
}

export function cycleFrame(status: FrameView["status"] = "done", k = 0): FrameView {
  return { status, cam: 0, camera: "CAM-1", shotId: `P${k + 1}`, view: 1, session: 1, ordinal: k + 1, error: null, arrivedMs: 20, frameCounter: k + 101, triggerCounter: k + 201, counterJump: false, score: .9, points: 2, gapPoints: 0, ms: 8, referenceFailed: [] };
}
export function cyclePart(sn = "1", cycleId = `cycle-${sn}`): PartView {
  return { cycleId, bundleId: "bundle-A", sn, recipeId: "A", recipeRevision: "revisionId-A", n: 2, received: 2, triggers: 2, queue: 0,
    filled: 4, total: 4, frames: [cycleFrame(), cycleFrame("done", 1)], measuredFrames: 2, burst: null };
}
export function cycleMeasurement(sn = "1", cycleId = `cycle-${sn}`): Measured {
  return { cycleId, shotId: "P1", camera: "CAM-1", bundleId: "bundle-A", sn, k: 0, cam: 0, referenceFailed: [], score: .9, ms: 8, queueMs: null, engineMs: null, coreMs: null, error: null,
    idx: [0, 1, 2, 3], d: [3, 4.5, 3, 3], w: [2, 2.5, null, 3], st: [0, 0, 0, 0], px: [[10, 10], [20, 20], [30, 30], [40, 40]] };
}
export function cycleResult(sn = "1", cycleId = `cycle-${sn}`): ResultView {
  return { cycleId, sn, recipeId: "A", ts: 1, drainMs: 24, verdict: "NG_POSITION", plcCode: 2, faultCode: 0, burst: null,
    reason: "测试样本偏移超差", gaps: [], segments: [
      { verdict: "NG_POSITION", min: 3, max: 4.5, excursionLen: 1, wMin: null, wMax: null, wExcursionLen: 0, referenceInvalid: false },
      { verdict: "OK", min: 3, max: 3, excursionLen: 0, wMin: null, wMax: null, wExcursionLen: 0, referenceInvalid: false },
    ] };
}

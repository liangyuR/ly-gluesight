import type { PartView, ResultView } from "../src/features/cycle/types";
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

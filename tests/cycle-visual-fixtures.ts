import type { CameraConfig, CameraStatus, FollowCalib } from "../src/features/camera/types";
import type { FrameView, Measured, PartView, Recipe, ResultView } from "../src/features/cycle/types";
import { workspaceView } from "./fixtures";

export function cycleFrame(status: FrameView["status"] = "done"): FrameView {
  return { status, cam: 0, s: null, arrivedMs: 20, frameCounter: 1, triggerCounter: 1, counterJump: false, score: .9, points: 2, gapPoints: 0, ms: 8 };
}
export function cyclePart(sn = 1): PartView {
  return { sn, recipeId: "A", recipeHash: "hash-A", mode: "flyShot", n: 2, received: 2, triggers: 2, queue: 0,
    filled: 4, total: 4, frames: [cycleFrame(), cycleFrame()], measuredFrames: 2, nozzleS: null, endS: null, activeCam: null };
}
export function cycleMeasurement(sn = 1): Measured {
  return { sn, k: 0, cam: 0, s: null, located: true, score: .9, ms: 8, error: null,
    idx: [0, 1, 2, 3], d: [3, 4.5, 3, 3], w: [2, 2.5, null, 3], st: [0, 0, 0, 0], px: [[10, 10], [20, 20], [30, 30], [40, 40]] };
}
export function cycleResult(sn = 1): ResultView {
  return { sn, recipeId: "A", ts: 1, drainMs: 24, verdict: "NG_POSITION", plcCode: 2, faultCode: 0,
    reason: "测试样本偏移超差", gaps: [], segments: [{ verdict: "NG_POSITION", min: 3, max: 4.5, excursionLen: 1, wMin: null, wMax: null, wExcursionLen: 0 }] };
}
export function followLayout(): Recipe {
  const layout = workspaceView().layout;
  layout.mode = "follow";
  layout.follow = { cameras: ["CAM-1", "CAM-2"], timing: { kind: "timed", speedMmS: 20, delayMs: 0 },
    nearMm: 1, farMm: 3, stepMm: 1, overrunMm: 0, searchMm: 4, beadWidth: 2, polarity: "any", minContrast: 20, autoSync: true, startZoneMm: 0 };
  layout.segments[0].width = { nominal: 2, tolUpper: .5, tolLower: .5, absMin: 1, absMax: 4, maxExcursionLen: 1 };
  return layout;
}
export function followCalib(nozzle: [number, number] = [50, 30]): FollowCalib {
  return { nozzle, angleDeg: 0, mirror: false, mmPerPx: 1, maskPx: 8, imageSize: [100, 60] };
}
export function cycleCameraConfig(id = "CAM-1"): CameraConfig {
  return { id, name: id, source: "replay", serial: "", acquisition: "freeRun", fps: 25, triggerSource: "Software", triggerActivation: "RisingEdge",
    triggerDelayUs: 0, debouncerUs: 0, exposureUs: 100, gainDb: 0, strobe: false, chunk: true, replayDir: "D:/test-records", replayChannel: 0, follow: followCalib() };
}
export function cycleCameraStatus(id = "CAM-1", cam = 0): CameraStatus {
  return { cam, id, name: id, source: "replay", acquisition: "freeRun", ready: true, message: "离线回放相机",
    device: null, sdkVersion: null, frames: 4, fps: 25, maxFps: null, lostPackets: 0, droppedFrames: 0, warnings: [] };
}

import { describe, expect, it } from "vitest";
import { computeVis, currentFrame, runs } from "../src/features/cycle/vis";
import type { Measured, PartView, FrameView, Judgement } from "../src/features/cycle/types";
import { workspaceView } from "./fixtures";

const frame = (status: FrameView["status"]): FrameView => ({ status, cam: 0, arrivedMs: null, frameCounter: null,
  triggerCounter: null, counterJump: false, score: null, points: 0, gapPoints: 0, ms: null });
const part = (): PartView => ({ sn: 1, recipeId: "A", recipeHash: "hash-A", n: 2, received: 2,
  triggers: 2, queue: 0, filled: 4, total: 4, frames: [frame("done"), frame("done")], measuredFrames: 2 });
const measured = (st = [0, 0, 0, 0]): Measured => ({ sn: 1, k: 0, cam: 0, located: true, score: .9, ms: 1,
  error: null, idx: [0, 1, 2, 3], d: [3, 4.5, 3, 3], w: [null, null, null, null], st, px: [] });

describe("测量结果的 UI 状态", () => {
  it.each(["wrong-id", "wrong-hash", "no-part"])("%s 不将测量套用到其他工件快照", mismatch => {
    const p = part(); if (mismatch === "wrong-id") p.recipeId = "B"; if (mismatch === "wrong-hash") p.recipeHash = "other-version";
    expect(computeVis(workspaceView().layout, mismatch === "no-part" ? null : p, [measured()], null)).toEqual(["none", "none", "none", "none"]);
  });

  it("合格、超差、断胶和测不成分别显示", () => {
    expect(computeVis(workspaceView().layout, part(), [measured([0, 0, 1, 2])], null)).toEqual(["ok", "exc", "gap", "inv"]);
  });

  it("重复测量失败不会覆盖已测成的点", () => {
    expect(computeVis(workspaceView().layout, part(), [measured(), measured([2, 2, 2, 2])], null)).toEqual(["ok", "exc", "ok", "ok"]);
  });

  it("缺帧只标记尚未测量的归属点", () => {
    const p = part(); p.frames[1].status = "missing";
    const m = measured(); m.idx = [0, 2]; m.d = [3, 3]; m.st = [0, 0];
    expect(computeVis(workspaceView().layout, p, [m], null)).toEqual(["ok", "none", "ok", "miss"]);
  });

  it.each(["NG_POSITION", "NG_WIDTH", "NG_ABSOLUTE"] as const)("最终 %s 将相关超差标为 NG", verdict => {
    const result: Judgement = { verdict, plcCode: 2, faultCode: 0, reason: "超限", gaps: [],
      segments: [{ verdict, min: 3, max: 4.5, excursionLen: 4, wMin: null, wMax: null, wExcursionLen: 0 }] };
    expect(computeVis(workspaceView().layout, part(), [measured()], result)).toEqual(["ok", "ng", "ok", "ok"]);
  });

  it("当前帧跳过尚未到达和缺失帧，颜色连续段保持边界", () => {
    const p = part(); p.frames = [frame("done"), frame("missing"), frame("waiting")];
    expect(currentFrame(p)).toBe(0); expect(currentFrame(null)).toBe(-1);
    expect(runs(["ok", "ok", "gap", "ok"])).toEqual([{ state: "ok", from: 0, to: 1 }, { state: "gap", from: 2, to: 2 }, { state: "ok", from: 3, to: 3 }]);
  });
});

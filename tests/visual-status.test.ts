import { describe, expect, it } from "vitest";
import { computeVis } from "../src/features/cycle/vis";
import type { Measured, PartView, FrameView } from "../src/features/cycle/types";
import { workspaceView } from "./fixtures";

const frame = (status: FrameView["status"]): FrameView => ({ status, cam: 0, camera: "CAM-1", shotId: "P1", view: 1, session: null, ordinal: null, error: null, arrivedMs: null, frameCounter: null,
  triggerCounter: null, counterJump: false, score: null, points: 0, gapPoints: 0, ms: null, referenceFailed: [] });
const part = (): PartView => ({ cycleId: "cycle-1", bundleId: null, sn: "1", recipeId: "A", recipeRevision: "revisionId-A", n: 2, received: 2,
  triggers: 2, queue: 0, filled: 4, total: 4, frames: [frame("done"), frame("done")], measuredFrames: 2, burst: null });
const measured = (st = [0, 0, 0, 0]): Measured => ({ cycleId: "cycle-1", shotId: "P1", camera: "CAM-1", bundleId: null, sn: "1", k: 0, cam: 0, referenceFailed: [], score: .9, ms: 1, queueMs: null, engineMs: null, coreMs: null,
  error: null, idx: [0, 1, 2, 3], d: [3, 4.5, 3, 3], w: [null, null, null, null], st, px: [] });

describe("测量结果的 UI 状态", () => {
  it("配方修订不匹配时不将测量套用到当前工件", () => {
    const p = part(); p.recipeRevision = "other-version";
    expect(computeVis(workspaceView().layout, p, [measured()], null)).toEqual(["none", "none", "none", "none"]);
  });

  it.each([
    ["合格、超差、断胶和测不成", [0, 0, 1, 2], [3, 4.5, null, null], ["ok", "exc", "gap", "inv"]],
    // 无基准站有胶但位置没判，不能画成合格；超出检测范围不参与判定，不能画成断胶或未测成
    ["有胶无基准与超出检测范围", [4, 4, 5, 5], [null, null, null, null], ["noRef", "noRef", "out", "out"]],
  ])("%s分别显示", (_, st, d, vis) => {
    const m = measured(st); m.d = d;
    expect(computeVis(workspaceView().layout, part(), [m], null)).toEqual(vis);
  });

  it("缺帧只标记尚未测量的归属点", () => {
    const p = part(); p.frames[1].status = "missing";
    const m = measured(); m.idx = [0, 2]; m.d = [3, 3]; m.st = [0, 0];
    expect(computeVis(workspaceView().layout, p, [m], null)).toEqual(["ok", "none", "ok", "miss"]);
  });
});

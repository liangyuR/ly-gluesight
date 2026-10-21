import { describe, expect, it } from "vitest";
import type { Judgement, Measured, Recipe } from "../src/features/cycle/types";
import { shotDiffs } from "../src/features/history/replay";
import { partDetail, workspaceView } from "./fixtures";

const layout = () => workspaceView().layout;
const judgement = (verdicts: Judgement["verdict"][]): Judgement => ({ verdict: verdicts.includes("NG_GAP") ? "NG_GAP" : "OK", plcCode: 1, faultCode: 0, reason: "",
  gaps: [], segments: verdicts.map(verdict => ({ verdict, min: 3, max: 3, excursionLen: 0, wMin: null, wMax: null, wExcursionLen: 0, referenceInvalid: false })) });
const measured = (k: number, d: number[], st: number[]): Measured => ({ cycleId: "cycle-1", shotId: `P${k + 1}`, camera: "CAM-1", bundleId: "bundle-A", sn: "SN-101", k, cam: 0,
  referenceFailed: [], score: 1, ms: 5, queueMs: null, engineMs: null, coreMs: null, error: null, idx: [2 * k, 2 * k + 1], d, w: [null, null], st, px: [] });

function original() {
  const detail = partDetail();
  detail.judgement = judgement(["OK", "OK"]);
  return detail;
}

describe("原图重跑逐拍照点比对", () => {
  it.each([
    ["重跑与原测量逐点一致", [3, 3], [0, 0], "OK", []],
    ["偏移差在 0.01 mm 容差内", [3.004, 3], [0, 0], "OK", []],
    ["偏移差超过 0.01 mm", [3.02, 3], [0, 0], "OK", [1]],
    ["分段结论改变而逐点相同", [3, 3], [0, 0], "NG_POSITION", [1]],
  ] as const)("%s → 只有实际变化的拍照点标为差异", (_, d, st, verdict, differing) => {
    const replay = { candidateRecipe: layout(), judgement: judgement(["OK", verdict]), measurements: [measured(0, [3, 3], [0, 0]), measured(1, [...d], [...st])] };
    expect(shotDiffs(layout(), original(), replay)!.filter(s => s.differs).map(s => s.k)).toEqual(differing);
  });

  it("规则重判没有重新测量 → 只按分段结论变化标差异", () => {
    const diffs = shotDiffs(layout(), original(), { candidateRecipe: layout(), judgement: judgement(["NG_POSITION", "OK"]), measurements: [] })!;
    expect(diffs.map(s => [s.remeasured, s.differs])).toEqual([[false, true], [false, false]]);
  });

  it("重跑时拍照点测量失败 → 该点全部记为状态变化", () => {
    const failed = { ...measured(0, [], []), error: "原图解码失败", idx: [] };
    const diffs = shotDiffs(layout(), original(), { candidateRecipe: layout(), judgement: judgement(["OK", "OK"]), measurements: [failed, measured(1, [3, 3], [0, 0])] })!;
    expect(diffs[0]).toMatchObject({ differs: true, changed: [0, 1], replay: { error: "原图解码失败", inv: 2 } });
    expect(diffs[1].differs).toBe(false);
  });

  it("重跑时拍照点定位失败 → 有胶站记为无基准的状态变化并注明原因，不当成测量失败", () => {
    const lost = { ...measured(0, [], [4, 4]), d: [null, null], referenceFailed: [{ view: 1, reason: "定位失败" }] };
    const diffs = shotDiffs(layout(), original(), { candidateRecipe: layout(), judgement: judgement(["OK", "OK"]), measurements: [lost, measured(1, [3, 3], [0, 0])] })!;
    expect(diffs[0]).toMatchObject({ differs: true, changed: [0, 1], maxDd: null, replay: { error: "定位失败", noRef: 2, inv: 0 } });
  });

  it("多视角拍照点只有非首视角分段结论改变 → 该拍照点标为差异", () => {
    const multiView = (): Recipe => {
      const base = layout(), [p1, p2] = base.segments;
      return { ...base, segments: [p1, { ...p1, view: 2, first: 2 }, { ...p2, first: 4 }],
        points: { x: [10, 20, 10, 20, 30, 40], y: Array(6).fill(10), seg: [0, 0, 1, 1, 2, 2], k: [0, 0, 0, 0, 1, 1] } };
    };
    const detail = { ...original(), judgement: judgement(["OK", "OK", "OK"]) };
    const diffs = shotDiffs(multiView(), detail, { candidateRecipe: multiView(), judgement: judgement(["OK", "NG_POSITION", "OK"]), measurements: [] })!;
    expect(diffs.filter(s => s.differs).map(s => s.k)).toEqual([0]);
  });

  it.each([
    ["测点归属改变", (r: Recipe): Recipe => ({ ...r, points: { x: [10, 20, 30], y: [10, 10, 10], seg: [0, 0, 1], k: [0, 0, 1] } })],
    ["重新示教中线平移而测点数不变", (r: Recipe): Recipe => ({ ...r, points: { ...r.points, x: r.points.x.map(x => x + 5) } })],
  ])("候选%s → 不做逐点对应", (_, change) => {
    expect(shotDiffs(layout(), original(), { candidateRecipe: change(layout()), judgement: judgement(["OK", "OK"]), measurements: [] })).toBeNull();
  });
});

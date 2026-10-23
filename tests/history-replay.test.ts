import { describe, expect, it } from "vitest";
import type { Judgement, Measured, PoseRecord, Recipe } from "../src/features/cycle/types";
import { burstSideOf, poseDiffers, regionDiffs, shotDiffs, type BurstSide } from "../src/features/history/replay";
import type { RegionKind, RegionResult } from "../src/features/plan/types";
import { burstJudgement, burstPartDetail, partDetail, workspaceView } from "./fixtures";

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

  it("原结果与重跑同为无基准站、只有胶宽变了 → 记为胶宽差异", () => {
    const detail = original();
    detail.points = { d: [null, null, 3, 3], st: [4, 4, 0, 0], w: [2, 2, null, null] };
    const lost = { ...measured(0, [], [4, 4]), d: [null, null], w: [2.5, 2], referenceFailed: [{ view: 1, reason: "定位失败" }] };
    const diffs = shotDiffs(layout(), detail, { candidateRecipe: layout(), judgement: judgement(["OK", "OK"]), measurements: [lost, measured(1, [3, 3], [0, 0])] })!;
    expect(diffs[0]).toMatchObject({ differs: true, changed: [], maxDd: null, maxDw: 0.5 });
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

describe("复测位姿与原记录对照", () => {
  // 回归：offset 相对各自示教框中心，从原图复测用的工作台候选把框右移了 60 px 时，曾直接相减 offset。
  // 原记录框 [100, 100, 21, 21]（tc [110, 110]）、5°、offset [5, 0]；候选框 [160, 100, 21, 21]（tc [170, 110]）。
  // 同一个变换在候选框下的 offset = o + (R − I)(tc′ − tc) = [5 + 60(cos 5° − 1), 60 sin 5°] = [4.77168, 5.22934]（手算）。
  const pose = (offset: [number, number]): PoseRecord => ({ accepted: true, reason: null, score: .95, peakRatio: .4, angle: 5, offset, endShiftMm: null, excludedMm: null, endLimitMm: 9.6 });
  it.each([
    ["同一个锁定（图像上同一变换）、offset 因框不同而不同", [4.77168, 5.22934], false],
    ["offset 数值相同、图像上相差约 5.2 px", [5, 0], true],
  ] as [string, [number, number], boolean][])("候选改了框，%s → 按各自的框在图像上比", (_, rerunOffset, differs) => {
    expect(poseDiffers(pose([5, 0]), [100, 100, 21, 21], pose(rerunOffset), [160, 100, 21, 21])).toBe(differs);
  });
});

describe("段配方按区域比对", () => {
  // 原结果：直线段 1（帧组 1–6，带胶偏统计）与弯道段 2（帧组 7–12）两个区域，其余由默认判定带出
  const region = (kind: RegionKind, firstSeq: number, lastSeq: number, over: Partial<RegionResult> = {}): RegionResult => ({
    ...burstJudgement().regions[0], region: { kind, firstSeq, lastSeq }, frameCount: lastSeq - firstSeq + 1, expected: null, countStatus: null, offsetUnjudged: null, incompleteGroups: [], ...over,
  });
  const regions = (straight: Partial<RegionResult> = {}, corner: Partial<RegionResult> = {}) => [
    region({ type: "straight", segment: 1 }, 1, 6, { offset: { meanShift: .3, maxDev: .6 }, ...straight }),
    region({ type: "corner", segment: 2 }, 7, 12, corner),
  ];
  const side = (over: Partial<BurstSide> = {}): BurstSide => ({ ...burstSideOf(burstPartDetail().burst!), judgement: burstJudgement({ regions: regions() }), ...over });
  const changed = (replay: BurstSide) => regionDiffs(side(), replay)!.filter(d => d.differs).map(d => d.key);

  it.each([
    ["与原结果逐项一致", regions(), []],
    ["弯道区域结论改变", regions({}, { verdict: "NG_WIDTH" }), ["corner:2-2"]],
    ["直线区域胶偏均值差超过 0.01 mm", regions({ offset: { meanShift: .32, maxDev: .6 } }), ["straight:1-1"]],
    ["直线区域胶偏差在 0.01 mm 容差内", regions({ offset: { meanShift: .304, maxDev: .6 } }), []],
    ["弯道区域不完整组变化", regions({}, { incompleteGroups: [9] }), ["corner:2-2"]],
  ] as [string, RegionResult[], string[]][])("重判结果%s → 只有实际变化的区域标为差异", (_, replayRegions, differing) => {
    expect(changed(side({ judgement: burstJudgement({ regions: replayRegions }) }))).toEqual(differing);
  });

  const segments = () => side().segments;
  it.each([
    ["段类型变了", { segments: segments().map(s => s.index === 2 ? { ...s, kind: "straight" as const } : s) }],
    ["某段预期帧数变了", { segments: segments().map(s => s.index === 1 ? { ...s, expectedFrames: 5 } : s) }],
    ["相机顺序变了", { cameras: [...side().cameras].reverse() }],
    ["整件预期总数变了", { expectedTotal: 13 }],
  ] as [string, Partial<BurstSide>][])("计划身份%s → 区域无法对应，不做逐区域比对", (_, over) => {
    expect(regionDiffs(side(), side(over))).toBeNull();
  });

  it("只改限值、容差、胶偏参考与检测区间 → 计划身份不变，仍可逐区域比对", () => {
    const tuned = segments().map(s => ({ ...s, tolerance: s.tolerance + 2, inspectRange: { aMm: 8, bMm: 100 }, checks: { ...s.checks, width: { ...s.checks.width, nominalMm: 5 } },
      offsetRef: s.offsetRef && { ...s.offsetRef, xRef: .2, spread: .1 } }));
    expect(regionDiffs(side(), side({ segments: tuned }))!.some(d => d.differs)).toBe(false);
  });
});

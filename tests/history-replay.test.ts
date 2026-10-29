import { describe, expect, it } from "vitest";
import { burstSideOf, regionDiffs, type BurstSide } from "../src/features/history/replay";
import type { RegionKind, RegionResult } from "../src/features/plan/types";
import { burstJudgement, burstPartDetail } from "./fixtures";

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
    ["弯道区域结论改变", regions({}, { verdict: "NG_WIDTH" }), ["corner:2-2"]],
    ["直线区域胶偏差在 0.01 mm 容差内", regions({ offset: { meanShift: .304, maxDev: .6 } }), []],
  ] as [string, RegionResult[], string[]][])("重判结果%s → 只有实际变化的区域标为差异", (_, replayRegions, differing) => {
    expect(changed(side({ judgement: burstJudgement({ regions: replayRegions }) }))).toEqual(differing);
  });

  const segments = () => side().segments;
  it.each([
    ["段类型变了", { segments: segments().map(s => s.index === 2 ? { ...s, kind: "straight" as const } : s) }],
    ["相机顺序变了", { cameras: [...side().cameras].reverse() }],
  ] as [string, Partial<BurstSide>][])("计划身份%s → 区域无法对应，不做逐区域比对", (_, over) => {
    expect(regionDiffs(side(), side(over))).toBeNull();
  });

  it("只改限值、容差、胶偏参考与检测区间 → 计划身份不变，仍可逐区域比对", () => {
    const tuned = segments().map(s => ({ ...s, tolerance: s.tolerance + 2, inspectRange: { aMm: 8, bMm: 100 }, checks: { ...s.checks, width: { ...s.checks.width, nominalMm: 5 } },
      offsetRef: s.offsetRef && { ...s.offsetRef, xRef: .2, spread: .1 } }));
    expect(regionDiffs(side(), side({ segments: tuned }))!.some(d => d.differs)).toBe(false);
  });
});

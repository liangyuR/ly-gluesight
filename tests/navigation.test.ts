import { describe, expect, it } from "vitest";
import { hitFrame, mapIssues } from "../src/features/workspace/navigation/geometry";
import type { NavigationMapData } from "../src/features/workspace/navigation/types";

const map = (overrides: Partial<NavigationMapData> = {}): NavigationMapData => ({
  format: 1, image: { id: "image", width: 200, height: 100, mime: "image/png" }, points: [[0, 0], [0.1, 0], [1, 0], [1, 1]],
  closed: false, captureStart: 0, captureEnd: 1, cuts: [], confirmed: true, structureRevision: 0, ...overrides,
});

describe("帧组与胶路位置", () => {
  it("交叉胶路同距离命中 → 不任意跳转，可选择段消歧", () => {
    const value = map({ image: { id: "cross", width: 100, height: 100, mime: "image/png" }, points: [[0, 0], [1, 1], [0, 1], [1, 0]], cuts: [0.5] });
    const ranges: [number, number][] = [[1, 10], [11, 20]];
    expect(hitFrame(value, ranges, [50, 50], 5)).toEqual({ kind: "ambiguous" });
    expect(hitFrame(value, ranges, [50, 50], 5, 0)).toMatchObject({ kind: "hit", segment: 0 });
    expect(hitFrame(value, ranges, [50, 50], 5, 1)).toMatchObject({ kind: "hit", segment: 1 });
    expect(hitFrame({ ...value, cuts: [] }, [[1, 20]], [50, 50], 5, 0)).toEqual({ kind: "ambiguous" });
  });

  it.each([
    { value: map({ cuts: [0.5, 0.4] }), count: 3 },
    { value: map({ points: [[0, 0], [0.5, 0], [0.5, 0], [1, 0]] }), count: 1 },
  ])("空间分段倒序或相邻顶点重复 → 不能确认", ({ value, count }) => {
    expect(mapIssues(value, count).length).toBeGreaterThan(0);
  });
});

import { describe, expect, it } from "vitest";
import { fractionFrame, frameFraction, hitFrame, mapIssues, pathSlice, pointOnPath } from "../src/features/workspace/navigation/geometry";
import type { NavigationMapData } from "../src/features/workspace/navigation/types";

const map = (overrides: Partial<NavigationMapData> = {}): NavigationMapData => ({
  format: 1, image: { id: "image", width: 200, height: 100, mime: "image/png" }, points: [[0, 0], [0.1, 0], [1, 0], [1, 1]],
  closed: false, captureStart: 0, captureEnd: 1, cuts: [], confirmed: true, structureRevision: 0, ...overrides,
});

describe("帧组与胶路位置", () => {
  it.each([
    { seq: 2, ranges: [[1, 3]] as [number, number][], expected: [150, 0] },
    { seq: 7, ranges: [[7, 7]] as [number, number][], expected: [150, 0] },
  ])("非均匀顶点、非方形图片或单帧段 seq=$seq → 按像素弧长落在帧中心并可反算", ({ seq, ranges, expected }) => {
    const value = map();
    const fraction = frameFraction(value, ranges, seq)!.fraction;
    const point = pointOnPath(value, fraction)!;
    expect(point[0]).toBeCloseTo(expected[0]); expect(point[1]).toBeCloseTo(expected[1]);
    expect(fractionFrame(fraction, [0, 1], ranges[0])).toBe(seq);
  });

  it.each([
    { label: "采集起点外5像素", click: [35, 0], expected: { kind: "miss" } },
  ])("局部采集且点击$label → 保留原始序号，只允许采集区及其法向容差命中", ({ click, expected }) => {
    const value = map({ points: [[0, 0], [1, 0]], captureStart: 0.2, captureEnd: 0.8, cuts: [0.5] });
    const ranges: [number, number][] = [[1, 4], [5, 8]];
    expect(frameFraction(value, ranges, 3)!.fraction).toBeCloseTo(0.3875);
    expect(hitFrame(value, ranges, [77.5, 0], 1)).toMatchObject({ kind: "hit", seq: 3, segment: 0 });
    expect(frameFraction(value, ranges, 8)!.fraction).toBeCloseTo(0.7625);
    expect(hitFrame(value, ranges, [click[0], click[1]], 12)).toMatchObject(expected);
    expect(hitFrame(value, ranges, [100, 0], 1, 0)).toMatchObject({ kind: "hit", seq: 4 });
    expect(hitFrame(value, ranges, [100, 0], 1, 1)).toMatchObject({ kind: "hit", seq: 5 });
  });

  it.each([
    { label: "采集起点外5像素", click: [5, 100], expected: { kind: "miss" } },
  ])("闭合胶路跨起点采集且点击$label → 展开定位连续，未采集区不吸附端点", ({ click, expected }) => {
    const value = map({ image: { id: "square", width: 100, height: 100, mime: "image/png" }, points: [[0, 0], [1, 0], [1, 1], [0, 1]], closed: true, captureStart: 0.75, captureEnd: 1.25, cuts: [1] });
    expect(mapIssues(value, 2)).toEqual([]);
    expect(pointOnPath(value, frameFraction(value, [[1, 2], [3, 4]], 2)!.fraction)).toEqual([0, 25]);
    expect(pointOnPath(value, frameFraction(value, [[1, 2], [3, 4]], 3)!.fraction)).toEqual([25, 0]);
    expect(pathSlice(value, 0.75, 1.25)).toEqual([[0, 100], [0, 0], [0, 0], [100, 0]]);
    expect(hitFrame(value, [[1, 2], [3, 4]], [0, 0], 1)).toEqual({ kind: "ambiguous" });
    expect(hitFrame(value, [[1, 2], [3, 4]], [0, 0], 1, 1)).toMatchObject({ kind: "hit", seq: 3 });
    expect(hitFrame(value, [[1, 2], [3, 4]], [click[0], click[1]], 12)).toMatchObject(expected);
  });

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
  ])("空间分段倒序、相邻重复点、零长度或范围非法 → 不能确认", ({ value, count }) => {
    expect(mapIssues(value, count).length).toBeGreaterThan(0);
  });
});

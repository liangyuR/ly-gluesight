import { describe, expect, it } from "vitest";
import type { PathSegment } from "../src/features/plan/types";
import { addSegment, changeKind, moveSegment, removeSegment, segmentIssues } from "../src/features/workspace/segmentEdit";
import { kindDefaults, pathSegment } from "./fixtures";

const stats = { rounds: 10, min: 4, max: 6, mean: 5 };
/** 三段都已标定：直线（带 x_ref）、弯道、直线。 */
const calibrated = (): PathSegment[] => [
  pathSegment(1, "straight", { lengthMm: 120, expectedFrames: 5, calib: stats, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
  pathSegment(2, "corner", { lengthMm: 30, expectedFrames: 4, calib: stats }),
  pathSegment(3, "straight", { lengthMm: 80, expectedFrames: 3, calib: stats }),
];

describe("段列表编辑规则", () => {
  it.each([
    ["添加弯道段", (segments: PathSegment[]) => addSegment(segments, "corner", kindDefaults())],
    ["删除第 2 段", (segments: PathSegment[]) => removeSegment(segments, 1)],
    ["第 3 段上移", (segments: PathSegment[]) => moveSegment(segments, 2, -1)],
    ["第 1 段改成弯道", (segments: PathSegment[]) => changeKind(segments, 0, "corner", kindDefaults())],
  ])("%s → 全部 N_exp、标定统计与 x_ref 清空，段序号保持从 1 连续", (_, edit) => {
    const next = edit(calibrated());
    expect(next.map(segment => segment.index)).toEqual(next.map((_, i) => i + 1));
    expect(next.every(segment => segment.expectedFrames === null && segment.calib === null)).toBe(true);
    expect(next.every(segment => segment.offsetRef === null || (segment.offsetRef.xRef === null && segment.offsetRef.spread === null))).toBe(true);
  });

  it("直线段改成弯道 → 按弯道默认重建检测项：不带胶偏检测、胶偏参考与均匀性，长度和检测区间保留", () => {
    const straight = pathSegment(1, "straight", { lengthMm: 120, inspectRange: { aMm: 5, bMm: 115 },
      offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } });
    straight.checks.width.maxRangeMm = 1;
    const [corner] = changeKind([straight], 0, "corner", kindDefaults());
    expect(corner).toMatchObject({ kind: "corner", lengthMm: 120, inspectRange: { aMm: 5, bMm: 115 }, offsetRef: null });
    expect(corner.checks.offset).toBeNull();
    expect(corner.checks.width.maxRangeMm).toBeNull();
  });

  it("弯道段改成直线 → 带上直线段的胶偏检测，N_exp 待标定", () => {
    const [straight] = changeKind([pathSegment(1, "corner", { expectedFrames: 4 })], 0, "straight", kindDefaults());
    expect(straight).toMatchObject({ kind: "straight", expectedFrames: null });
    expect(straight.checks.offset).toMatchObject({ enabled: true });
  });
});

describe("段列表保存前检查", () => {
  it.each<[string, Partial<PathSegment>, string]>([
    ["帧数容差清空（NaN 序列化成 null，后端读不了）", { tolerance: NaN }, "帧数容差"],
    ["帧数容差带小数", { tolerance: 1.5 }, "帧数容差"],
    ["检测区间起点不小于终点", { inspectRange: { aMm: 10, bMm: 10 } }, "检测区间"],
    ["检测区间起点为负", { inspectRange: { aMm: -1, bMm: 10 } }, "检测区间"],
    ["检测区间终点清空", { inspectRange: { aMm: 0, bMm: NaN } }, "检测区间"],
    ["长度不大于 0", { lengthMm: 0 }, "长度"],
  ])("%s → 不发送，指出该段的问题", (_, change, field) => {
    const issues = segmentIssues([pathSegment(1, "straight", change)]);
    expect(issues).toHaveLength(1);
    expect(issues[0]).toContain("段 1");
    expect(issues[0]).toContain(field);
  });

  it("检测项里有未填写的数值 → 不发送", () => {
    const segment = pathSegment(1, "straight");
    segment.checks.width.nominalMm = NaN;
    expect(segmentIssues([segment])).toHaveLength(1);
  });

  it("参数都合法（含未设长度与检测区间） → 没有问题", () => {
    expect(segmentIssues([pathSegment(1, "straight"), pathSegment(2, "corner", { lengthMm: 30, inspectRange: { aMm: 0, bMm: 30 } })])).toEqual([]);
  });
});

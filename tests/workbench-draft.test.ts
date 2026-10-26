import { describe, expect, it } from "vitest";
import type { PathSegment } from "../src/features/plan/types";
import { segmentsForSave, spans, toggleCut, valid, type WorkbenchDraft } from "../src/features/workspace/workbenchDraft";
import { kindDefaults, pathSegment } from "./fixtures";

const stats = { rounds: 10, min: 4, max: 6, mean: 5 };
/** 三段都已标定：直线（带 x_ref、长度、容差 2）、弯道、直线。 */
const calibrated = (): PathSegment[] => [
  pathSegment(1, "straight", { lengthMm: 120, tolerance: 2, expectedFrames: 5, calib: stats, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
  pathSegment(2, "corner", { lengthMm: 30, expectedFrames: 4, calib: stats }),
  pathSegment(3, "straight", { lengthMm: 80, expectedFrames: 3, calib: stats }),
];

describe("分段标定工作台的边界编辑", () => {
  const draft: WorkbenchDraft = { groupCount: 12, cuts: [4, 8], kinds: ["straight", "corner", "straight"] };
  it.each<[string, number, number[], WorkbenchDraft["kinds"]]>([
    ["在弯道段中间（帧组 6 之后）插入 → 弯道一分为二", 6, [4, 6, 8], ["straight", "corner", "corner", "straight"]],
    ["删除帧组 4 之后的边界 → 与后一段合并，保留前一段的直线", 4, [8], ["straight", "straight"]],
    ["边界只能在两个帧组之间：帧组 12 之后不插入", 12, [4, 8], ["straight", "corner", "straight"]],
  ])("%s", (_, after, cuts, kinds) => {
    const next = toggleCut(draft, after);
    expect([next.cuts, next.kinds]).toEqual([cuts, kinds]);
    expect(valid(next)).toBe(true);
    expect(spans(next).reduce((sum, [first, last]) => sum + last - first + 1, 0)).toBe(12);
  });
});

describe("工作台保存的段列表", () => {
  it("段的类型序列不变 → 原样保存，各段参数、N_exp、标定与 x_ref 都保留", () => {
    const current = calibrated();
    expect(segmentsForSave(current, ["straight", "corner", "straight"], kindDefaults())).toBe(current);
  });

  it("段的类型序列变了 → 序号从 1 连续，N_exp、标定与 x_ref 清空；同位置同类型的段沿用长度与容差，其余取默认", () => {
    const next = segmentsForSave(calibrated(), ["straight", "straight"], kindDefaults());
    expect(next.map(s => s.index)).toEqual([1, 2]);
    expect(next.every(s => s.expectedFrames === null && s.calib === null && (s.offsetRef?.xRef ?? null) === null)).toBe(true);
    expect([next[0].lengthMm, next[0].tolerance]).toEqual([120, 2]);
    expect(next[1].lengthMm).toBeNull();
  });
});

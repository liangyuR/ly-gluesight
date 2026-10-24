import { describe, expect, it } from "vitest";
import type { PathSegment } from "../src/features/plan/types";
import { addSegment, changeKind, moveSegment, removeSegment } from "../src/features/workspace/segmentEdit";
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
});

import { describe, expect, it } from "vitest";
import type { PathSegment } from "../src/features/plan/types";
import { segmentsForSave } from "../src/features/workspace/workbenchDraft";
import { applyToKind } from "../src/features/workspace/segmentEdit";

import { kindDefaults, pathSegment } from "./fixtures";

const CAMS = ["CAM-1", "CAM-2", "CAM-3"];
const all = (n: number) => Array.from({ length: n }, () => [...CAMS]);

const stats = { rounds: 10, min: 4, max: 6, mean: 5 };
/** 三段都已标定：直线（带 x_ref、长度、容差 2）、弯道、直线。 */
const calibrated = (): PathSegment[] => [
  pathSegment(1, "straight", { lengthMm: 120, tolerance: 2, expectedFrames: 5, calib: stats, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
  pathSegment(2, "corner", { lengthMm: 30, expectedFrames: 4, calib: stats }),
  pathSegment(3, "straight", { lengthMm: 80, expectedFrames: 3, calib: stats }),
];

describe("工作台保存的段列表", () => {

  it("段的类型序列变了 → 序号从 1 连续，N_exp、标定与 x_ref 清空；同位置同类型的段沿用长度与容差，新段沿用同类型已有段的检测项与容差", () => {
    const next = segmentsForSave(calibrated(), { kinds: ["straight", "straight"], cameras: all(2) }, kindDefaults(), CAMS);
    expect(next.map(s => s.index)).toEqual([1, 2]);
    expect(next.every(s => s.expectedFrames === null && s.calib === null && (s.offsetRef?.xRef ?? null) === null)).toBe(true);
    // 第 2 段由弯道改成直线：新直线段沿用已有直线段（第 1 段）的容差 2，而不是默认 1
    expect([next[0].lengthMm, next[0].tolerance]).toEqual([120, 2]);
    expect(next[1].lengthMm).toBeNull();
    expect(next[1].tolerance).toBe(2);
  });

  // 套用按钮在走廊示教页，那里看不到帧数容差：顺带改掉容差或逐帧定向，操作员察觉不到
  it("检测项套用到同类段 → 其余同类型段只改检测项，帧数容差、逐帧定向、相机、长度与标定不变", () => {
    const segments = calibrated();
    segments[0].checks = { ...segments[0].checks, width: { ...segments[0].checks.width, nominalMm: 5 } };
    segments[0].heading = { toleranceDeg: 8 };
    segments[2].cameras = ["CAM-3"];
    const next = applyToKind(segments, 0);
    expect([next[2].checks.width.nominalMm, next[2].tolerance, next[2].heading]).toEqual([5, 1, undefined]);
    expect([next[2].cameras, next[2].lengthMm, next[2].expectedFrames]).toEqual([["CAM-3"], 80, 3]);
    expect(next[1]).toBe(segments[1]);
  });

  it("只改检测相机 → N_exp 与标定保留；选全存为空（= 全部相机）；胶偏参考相机不在本段时改为本段第一台并清空 x_ref", () => {
    const next = segmentsForSave(calibrated(), { kinds: ["straight", "corner", "straight"], cameras: [["CAM-2"], [...CAMS], ["CAM-1", "CAM-3"]] }, kindDefaults(), CAMS);
    expect(next.map(s => s.cameras)).toEqual([["CAM-2"], [], ["CAM-1", "CAM-3"]]);
    expect(next.map(s => s.expectedFrames)).toEqual([5, 4, 3]);
    expect(next[0].offsetRef).toMatchObject({ camera: "CAM-2", xRef: null, spread: null });
  });
});

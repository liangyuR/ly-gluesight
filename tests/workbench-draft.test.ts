import { describe, expect, it } from "vitest";
import type { PathSegment } from "../src/features/plan/types";
import { passCells, segmentsForSave, toggleCamera, toggleKind, valid, type WorkbenchDraft } from "../src/features/workspace/workbenchDraft";
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

  // 段参数由后端按示教件帧范围对应旧段迁移（D-SB-52）：这里按位置沿用会把相邻段的长度带到新段上
  it("段的类型序列变了 → 序号从 1 连续，N_exp、标定与 x_ref 清空；不按位置沿用长度，检测项与容差取同类型已有段", () => {
    const next = segmentsForSave(calibrated(), { kinds: ["straight", "straight"], cameras: all(2) }, kindDefaults(), CAMS);
    expect(next.map(s => s.index)).toEqual([1, 2]);
    expect(next.every(s => s.expectedFrames === null && s.calib === null && (s.offsetRef?.xRef ?? null) === null)).toBe(true);
    expect(next.map(s => [s.lengthMm, s.tolerance])).toEqual([[null, 2], [null, 2]]);
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

  // D-SB-53：不检测是第三态，段类型与检测相机要保留（取消后恢复）；全部不检测时不能保存
  it("点段名按 直线 → 弯道 → 不检测 → 直线 循环；不检测时相机不能改，保存带 skip 并保留相机；全部不检测为无效草稿", () => {
    const draft: WorkbenchDraft = { groupCount: 9, cuts: [3, 6], kinds: ["straight", "corner", "straight"], cameras: [["CAM-1"], ["CAM-2"], ["CAM-3"]], skips: [false, false, false] };
    const state = (d: WorkbenchDraft) => `${d.kinds[0]}${d.skips[0] ? "-skip" : ""}`;
    const once = toggleKind(draft, 0), twice = toggleKind(once, 0), thrice = toggleKind(twice, 0);
    expect([once, twice, thrice].map(state)).toEqual(["corner", "corner-skip", "straight"]);
    expect(thrice.cameras[0]).toEqual(["CAM-1"]);
    expect(toggleCamera(twice, 0, "CAM-2", CAMS)).toBe(twice);
    const saved = segmentsForSave(calibrated(), { ...twice, kinds: ["corner", "corner", "straight"] }, kindDefaults(), CAMS);
    expect([saved[0].skip, saved[0].cameras, saved[1].skip]).toEqual([true, ["CAM-1"], undefined]);
    expect(valid({ ...draft, skips: [true, true, true] })).toBe(false);
  });

  it("只改检测相机 → N_exp 与标定保留；选全存为空（= 全部相机）；胶偏参考相机不在本段时改为本段第一台并清空 x_ref", () => {
    const next = segmentsForSave(calibrated(), { kinds: ["straight", "corner", "straight"], cameras: [["CAM-2"], [...CAMS], ["CAM-1", "CAM-3"]] }, kindDefaults(), CAMS);
    expect(next.map(s => s.cameras)).toEqual([["CAM-2"], [], ["CAM-1", "CAM-3"]]);
    expect(next.map(s => s.expectedFrames)).toEqual([5, 4, 3]);
    expect(next[0].offsetRef).toMatchObject({ camera: "CAM-2", xRef: null, spread: null });
  });
});

describe("工作台的笔结构", () => {
  // 防的回归：时间轴标出的每笔首尾跳过与后端 passes::skipped 不一致（整笔不超过首尾之和时整笔不检），操作员按错误的跳过范围放边界
  it("每笔开头 2、结尾 1 → 各笔首尾标跳过，3 帧的笔整笔不检，笔间最后一帧标笔界；笔结构与帧组数对不上 → 不标", () => {
    const cells = passCells([5, 3, 4], 12, { head: 2, tail: 1 })!;
    expect(cells.map(c => c.skip ?? "-")).toEqual(["head", "head", "-", "-", "tail", "short", "short", "short", "head", "head", "-", "tail"]);
    expect(cells.flatMap((c, k) => c.end ? [k + 1] : [])).toEqual([5, 8]);
    expect(passCells([5, 3], 12, { head: 2, tail: 1 })).toBeNull();
  });
});

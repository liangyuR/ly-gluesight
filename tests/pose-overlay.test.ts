import { describe, expect, it } from "vitest";
import { partFrames, poseTransform } from "../src/features/cycle/pose";
import type { PoseRecord } from "../src/features/cycle/types";
import { cyclePart } from "./cycle-visual-fixtures";
import { workspaceView } from "./fixtures";

const accepted = (angle: number, offset: [number, number]): PoseRecord => ({
  accepted: true, reason: null, score: .95, peakRatio: .4, angle, offset, endShiftMm: [0, 0], excludedMm: [1, 1], endLimitMm: 9.6,
});

describe("叠图位姿变换与 LyFlow Pose2D 约定一致", () => {
  // 期望值逐个手算：T(p) = R(θ)(p − tc) + tc + offset，y 向下、θ > 0 为屏幕顺时针；第 2、3 组与 vision/fixture.rs 测试同一组数
  // （tc [380, 290] 由框 [332, 242, 97, 97] 的 (x + (w − 1)/2, y + (h − 1)/2) 得出，center [431, 262] 即 offset [51, −28]）
  it.each([
    { name: "绕示教框中心转 90° → 屏幕上顺时针（+x 转到 +y）", rect: [0, 0, 1, 1], angle: 90, offset: [0, 0], p: [10, 0], expected: [0, 10] },
    { name: "示教框中心 → 落到 center，转角不改变转心的位置", rect: [332, 242, 97, 97], angle: 7.5, offset: [51, -28], p: [380, 290], expected: [431, 262] },
    { name: "转心右侧 10 px → center + 10·(cos 7.5°, sin 7.5°)", rect: [332, 242, 97, 97], angle: 7.5, offset: [51, -28], p: [390, 290], expected: [431 + 10 * Math.cos(7.5 * Math.PI / 180), 262 + 10 * Math.sin(7.5 * Math.PI / 180)] },
  ] as { name: string; rect: [number, number, number, number]; angle: number; offset: [number, number]; p: [number, number]; expected: [number, number] }[])("$name", ({ rect, angle, offset, p, expected }) => {
    const q = poseTransform(rect, accepted(angle, offset))!(p);
    expect(q[0]).toBeCloseTo(expected[0], 9);
    expect(q[1]).toBeCloseTo(expected[1], 9);
  });
});

describe("位姿只能和测出它的配方快照配对", () => {
  it.each([
    ["同一配方、不同修订", { recipeRevision: "revisionId-B" }],
    ["不同配方", { recipeId: "B" }],
  ])("件与布局%s → 不给帧，叠图与缩略图不按它的位姿搬", (_, change) => {
    const layout = workspaceView().layout;
    expect(partFrames(layout, cyclePart())).not.toBeNull();
    expect(partFrames(layout, { ...cyclePart(), ...change })).toBeNull();
  });
});

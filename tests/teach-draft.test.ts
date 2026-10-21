import { describe, expect, it } from "vitest";
import type { ShotSpec } from "../src/features/cycle/types";
import { shotTeach } from "../src/features/workspace/teach";

describe("示教草稿", () => {
  // 防的回归：草稿从已保存的拍照点重建时丢了 fixture，保存草稿或参数时把已示教的特征定位发成 null 清掉
  it("拍照点带特征定位 → 草稿保留同样的 fixture", () => {
    const fixture = { rect: [100, 40, 32, 32] as [number, number, number, number], source: "capture-test-v1", searchRadiusPx: 20, angleRangeDeg: 0, minScore: 0.6, maxPeakRatio: 1, maxEndShiftMm: 3 };
    const shot: Pick<ShotSpec, "path" | "mmPerPx" | "detect" | "limits" | "fixture"> = { path: [[10, 10], [210, 10]], mmPerPx: 0.1, fixture };
    expect(shotTeach(shot).fixture).toEqual(fixture);
  });
});

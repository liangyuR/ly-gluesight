import { describe, expect, it } from "vitest";
import type { ShotSpec } from "../src/features/cycle/types";
import { sameTeach, shotTeach } from "../src/features/workspace/teach";

describe("示教草稿", () => {
  // 防的回归：草稿从已保存的拍照点重建时丢了 fixture，保存草稿或参数时把已示教的特征定位发成 null 清掉
  it("拍照点带特征定位 → 草稿保留同样的 fixture", () => {
    const fixture = { rect: [100, 40, 32, 32] as [number, number, number, number], source: "capture-test-v1", searchRadiusPx: 20, angleRangeDeg: 0, minScore: 0.6, maxPeakRatio: 1, maxEndShiftMm: 3 };
    const shot: Pick<ShotSpec, "path" | "mmPerPx" | "detect" | "limits" | "fixture"> = { path: [[10, 10], [210, 10]], mmPerPx: 0.1, fixture };
    expect(shotTeach(shot).fixture).toEqual(fixture);
  });

  // 防的回归（应用内端到端发现）：新画的框草稿是 { 门限…, source: "", rect }，后端保存后字段顺序不同、source 改绑到示教图，
  // 草稿与已保存的永远不等，试测后仍算“有未试测的修改”，示教保存不了
  it("新框经后端保存（来源改绑、字段顺序不同）→ 与草稿相同；框不同 → 不同", () => {
    const path: [number, number][] = [[10, 10], [210, 10]];
    const draft = { path, mmPerPx: 0.1, fixture: { searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1, source: "", rect: [100, 40, 32, 32] as [number, number, number, number] } };
    const saved = shotTeach({ path, mmPerPx: 0.1, fixture: { rect: [100, 40, 32, 32], source: "capture-test-v1", searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1 } });
    expect(sameTeach(draft, saved)).toBe(true);
    expect(sameTeach({ ...draft, fixture: { ...draft.fixture, rect: [101, 40, 32, 32] } }, saved)).toBe(false);
  });
});

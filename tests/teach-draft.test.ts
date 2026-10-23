import { describe, expect, it } from "vitest";
import type { ShotSpec } from "../src/features/cycle/types";
import type { CameraTeachDraft } from "../src/features/plan/types";
import { corridorDraft, corridorError, sameCorridor, sameTeach, shotTeach, withCorridor } from "../src/features/workspace/teach";
import { burstRecipe } from "./fixtures";

describe("示教草稿（旧拍照点配方）", () => {
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

const draft = (over: Partial<CameraTeachDraft> = {}): CameraTeachDraft => ({ corridor: [[10, 30], [90, 30]], mmPerPx: 0.1, detect: null, ...over });

describe("走廊示教草稿", () => {
  // 防的回归：后端以 f32 存走廊与当量，取回的值与草稿的 f64 永远不等，草稿被判成一直有未保存的修改，自动保存反复触发、切不了相机
  it("草稿与已保存的走廊只差 f32 精度 → 视为没改；点位、当量或检测参数不同 → 视为已改", () => {
    const saved = corridorDraft({ corridor: [[10.1, 30.3], [90.7, 30.3]], mmPerPx: 0.0875, detect: null });
    const f64 = draft({ corridor: [[10.1 + 1e-9, 30.3], [90.7, 30.3 - 1e-9]], mmPerPx: 0.0875 + 1e-12 });
    expect(sameCorridor(f64, saved)).toBe(true);
    expect(sameCorridor({ ...f64, corridor: [[10.1, 30.3], [91, 30.3]] }, saved)).toBe(false);
    expect(sameCorridor({ ...f64, mmPerPx: 0.09 }, saved)).toBe(false);
    expect(sameCorridor({ ...f64, detect: { searchMm: 4, polarity: "dark", widthRange: [1, 6] } }, saved)).toBe(false);
  });

  // 防的回归：自动保存把草稿写到别的相机的 inspect 项里，或在配方没有这台相机时凭空造出一项
  it("把一台相机的草稿并入配方 → 只改这台相机的走廊、当量与检测参数，其余相机与配方字段原样；配方没有这台相机则原样返回", () => {
    const doc = burstRecipe();
    const next = withCorridor(doc, "CAM-2", draft({ corridor: [[1, 2], [3, 4]], mmPerPx: 0.2, detect: { searchMm: 5, polarity: "light", widthRange: [1, 5] } }));
    expect(next.inspect![1]).toMatchObject({ camera: "CAM-2", corridor: [[1, 2], [3, 4]], mmPerPx: 0.2, detect: { searchMm: 5, polarity: "light", widthRange: [1, 5] } });
    expect(next.inspect![0]).toEqual(doc.inspect![0]);
    expect(next.inspect![2]).toEqual(doc.inspect![2]);
    expect({ ...next, inspect: doc.inspect }).toEqual(doc);
    expect(withCorridor(doc, "CAM-9", draft())).toBe(doc);
  });

  // 防的回归：只有一个点、当量缺失或自定义检测参数自相矛盾的走廊被当作“已示教”保存，到验证和发布才发现测不了
  it.each<[string, CameraTeachDraft, string]>([
    ["走廊与当量都合法 → 可以保存", draft(), ""],
    ["只有一个点 → 不能保存", draft({ corridor: [[10, 30]] }), "至少两个点"],
    ["两个点重合（长度为零）→ 不能保存", draft({ corridor: [[10, 30], [10, 30]] }), "长度不能为零"],
    ["没有像素当量 → 不能保存", draft({ mmPerPx: null }), "像素当量"],
    ["像素当量超过 10 mm/px → 不能保存", draft({ mmPerPx: 12 }), "像素当量"],
    ["自定义胶宽范围下限不小于上限 → 不能保存", draft({ detect: { searchMm: 4, polarity: "dark", widthRange: [5, 5] } }), "胶宽范围"],
    ["自定义胶宽上限不小于搜索宽度 → 不能保存", draft({ detect: { searchMm: 2, polarity: "dark", widthRange: [1, 4] } }), "搜索宽度"],
  ])("%s", (_, input, expected) => {
    const error = corridorError(input);
    if (expected) expect(error).toContain(expected); else expect(error).toBe("");
  });
});

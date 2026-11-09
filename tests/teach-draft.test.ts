import { describe, expect, it } from "vitest";
import type { RecipeDoc } from "../src/features/cycle/types";
import type { CameraTeachDraft } from "../src/features/plan/types";
import { draftKey, measureMode, sameCorridor, savedDraft, withCorridor, withMeasure, withSectorDeg } from "../src/features/workspace/teach";
const draft = (over: Partial<CameraTeachDraft> = {}): CameraTeachDraft => ({ corridor: [[10, 30], [90, 30]], mmPerPx: 0.1, window: null, ...over });

describe("走廊示教草稿", () => {
  // 防的回归：后端以 f32 存走廊与当量，取回的值与草稿的 f64 永远不等，草稿被判成一直有未保存的修改，自动保存反复触发、切不了段
  it("草稿与已保存的段走廊只差 f32 精度 → 视为没改；点位或当量不同 → 视为已改", () => {
    const doc = { inspect: [{ camera: "cam1", mmPerPx: 0.0875, segments: [{ segment: 1, corridor: [[10.1, 30.3], [90.7, 30.3]] }] }] } as unknown as RecipeDoc;
    const saved = savedDraft(doc, draftKey("cam1", 1));
    const f64 = draft({ corridor: [[10.1 + 1e-9, 30.3], [90.7, 30.3 - 1e-9]], mmPerPx: 0.0875 + 1e-12 });
    expect(sameCorridor(f64, saved)).toBe(true);
    expect(sameCorridor({ ...f64, corridor: [[10.1, 30.3], [91, 30.3]] }, saved)).toBe(false);
    expect(sameCorridor({ ...f64, mmPerPx: 0.09 }, saved)).toBe(false);
  });

  // 防的回归：胶色模型按旧走廊取样，走廊改了还留着，这一段按失效的颜色测；或者只拖扇区、自动保存未改动的走廊时把模型丢了，操作员得反复取色
  it("走廊点改动 → 去掉这一段的胶色模型；只改扇区或走廊在 f32 内未变 → 原样保留", () => {
    const doc = { inspect: [{ camera: "cam1", mmPerPx: 0.1, segments: [
      { segment: 1, corridor: [[10.1, 30], [90, 30]], colorModel: "{\"glue\":{}}" },
      { segment: 2, corridor: [[0, 0], [50, 0]], colorModel: "{\"other\":1}" },
    ] }] } as unknown as RecipeDoc;
    const segment = (next: RecipeDoc, index: number) => next.inspect!.find(item => item.camera === "cam1")!.segments!.find(item => item.segment === index)!;
    const moved = withCorridor(doc, draftKey("cam1", 1), draft({ corridor: [[10.1, 30], [95, 30]] }));
    expect(segment(moved, 1)).not.toHaveProperty("colorModel");
    expect(segment(moved, 2).colorModel).toBe("{\"other\":1}");
    expect(segment(withCorridor(doc, draftKey("cam1", 1), draft({ corridor: [[10.1 + 1e-9, 30], [90, 30]] })), 1).colorModel).toBe("{\"glue\":{}}");
    expect(segment(withSectorDeg(doc, "cam1", 1, 45), 1)).toMatchObject({ sectorDeg: 45, colorModel: "{\"glue\":{}}" });
  });

  // 防的回归：灰度帧被当成可按胶色测；旧配方没取色的走廊被显示成按胶色（与生产实际按灰度测不符）；彩色帧上还没画走廊的段默认不是按胶色
  it.each([
    ["灰度帧即使选过按胶色", { segment: 1, corridor: [], measure: "color" as const }, false, "gray"],
    ["彩色帧、旧走廊没取色也没选", { segment: 1, corridor: [[0, 0], [9, 0]] as [number, number][] }, true, "gray"],
    ["彩色帧、有胶色模型", { segment: 1, corridor: [[0, 0], [9, 0]] as [number, number][], colorModel: "{}" }, true, "color"],
    ["彩色帧、还没画走廊", undefined, true, "color"],
    ["彩色帧、选了按灰度", { segment: 1, corridor: [], measure: "gray" as const }, true, "gray"],
  ])("%s → 按%s测量", (_, item, colorFrame, expected) => {
    expect(measureMode(item, colorFrame)).toBe(expected);
  });

  // 防的回归：还没画走廊时选的测量方式因空走廊项被清掉而丢失；改按灰度后留着的胶色模型让这一段继续按胶色测
  it("改按灰度 → 去掉胶色模型；还没画走廊的段也留下这条选择", () => {
    const doc = { inspect: [{ camera: "cam1", mmPerPx: 0.1, segments: [
      { segment: 1, corridor: [[10, 30], [90, 30]], colorModel: "{\"glue\":{}}" },
    ] }] } as unknown as RecipeDoc;
    const segments = (next: RecipeDoc) => next.inspect!.find(item => item.camera === "cam1")!.segments!;
    const gray = withMeasure(doc, "cam1", 1, "gray");
    expect(segments(gray)[0]).toEqual({ segment: 1, corridor: [[10, 30], [90, 30]], measure: "gray" });
    expect(segments(withMeasure(doc, "cam1", 2, "gray"))).toContainEqual({ segment: 2, corridor: [], measure: "gray" });
  });

});

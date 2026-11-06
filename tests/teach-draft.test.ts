import { describe, expect, it } from "vitest";
import type { RecipeDoc } from "../src/features/cycle/types";
import type { CameraTeachDraft } from "../src/features/plan/types";
import { draftKey, sameCorridor, savedDraft, withCorridor, withSectorDeg } from "../src/features/workspace/teach";
const draft = (over: Partial<CameraTeachDraft> = {}): CameraTeachDraft => ({ corridor: [[10, 30], [90, 30]], mmPerPx: 0.1, detect: null, window: null, ...over });

describe("走廊示教草稿", () => {
  // 防的回归：后端以 f32 存走廊与当量，取回的值与草稿的 f64 永远不等，草稿被判成一直有未保存的修改，自动保存反复触发、切不了段
  it("草稿与已保存的段走廊只差 f32 精度 → 视为没改；点位、当量或检测参数不同 → 视为已改", () => {
    const doc = { inspect: [{ camera: "cam1", mmPerPx: 0.0875, calib: null, detect: null, segments: [{ segment: 1, corridor: [[10.1, 30.3], [90.7, 30.3]] }] }] } as unknown as RecipeDoc;
    const saved = savedDraft(doc, draftKey("cam1", 1));
    const f64 = draft({ corridor: [[10.1 + 1e-9, 30.3], [90.7, 30.3 - 1e-9]], mmPerPx: 0.0875 + 1e-12 });
    expect(sameCorridor(f64, saved)).toBe(true);
    expect(sameCorridor({ ...f64, corridor: [[10.1, 30.3], [91, 30.3]] }, saved)).toBe(false);
    expect(sameCorridor({ ...f64, mmPerPx: 0.09 }, saved)).toBe(false);
    expect(sameCorridor({ ...f64, detect: { searchMm: 4, polarity: "dark", widthRange: [1, 6] } }, saved)).toBe(false);
  });

  // 防的回归：胶色模型按旧走廊取样，走廊改了还留着，这一段按失效的颜色测；或者只拖扇区、自动保存未改动的走廊时把模型丢了，操作员得反复取色
  it("走廊点改动 → 去掉这一段的胶色模型；只改扇区或走廊在 f32 内未变 → 原样保留", () => {
    const doc = { inspect: [{ camera: "cam1", mmPerPx: 0.1, calib: null, detect: null, segments: [
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

});

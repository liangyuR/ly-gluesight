import { describe, expect, it } from "vitest";
import type { RecipeDoc } from "../src/features/cycle/types";
import type { CameraTeachDraft } from "../src/features/plan/types";
import { confirmedSegmentRange, corridorDraft, draftKey, sameCorridor, savedDraft, withCorridor } from "../src/features/workspace/teach";
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

  // 防的回归（D-SB-42）：段走廊草稿按相机保存时覆盖了默认走廊或相机的像素当量、检测参数，或冲掉了别的段的走廊；清空的段走廊留在配方里
  it("段走廊草稿 → 只写这一段的走廊，默认走廊、相机参数与其他段不变；清空 → 这一段从配方去掉", () => {
    const detect = { searchMm: 4, polarity: "dark" as const, widthRange: [1, 6] as [number, number] };
    const doc = { inspect: [{ camera: "cam1", corridor: [[1, 1], [9, 1]], mmPerPx: 0.11, calib: null, detect, segments: [{ segment: 3, corridor: [[5, 5], [6, 6]] }] }] } as unknown as RecipeDoc;
    const key = draftKey("cam1", 2);
    const next = withCorridor(doc, key, { corridor: [[2, 2], [8, 8]], mmPerPx: 0.5, detect: null });
    const cam = next.inspect![0];
    expect([cam.corridor, cam.mmPerPx, cam.detect]).toEqual([[[1, 1], [9, 1]], 0.11, detect]);
    expect(cam.segments).toEqual([{ segment: 2, corridor: [[2, 2], [8, 8]] }, { segment: 3, corridor: [[5, 5], [6, 6]] }]);
    expect(savedDraft(next, key)).toEqual({ corridor: [[2, 2], [8, 8]], mmPerPx: 0.11, detect });
    expect(withCorridor(next, key, { corridor: [], mmPerPx: 0.11, detect }).inspect![0].segments).toEqual([{ segment: 3, corridor: [[5, 5], [6, 6]] }]);
  });

  // 防的回归（D-SB-42）：段示教帧的可选范围差一帧（边界帧组归到下一段），或在只存了边界草稿的轮次上按草稿划段
  it.each([
    ["已确认，中间段", "confirmed", 2, { start: 11, end: 20 }],
    ["已确认，首段", "confirmed", 1, { start: 1, end: 10 }],
    ["已确认，末段", "confirmed", 3, { start: 21, end: 30 }],
    ["只存了草稿", "draft", 2, null],
  ])("段帧组范围：%s", (_, status, segment, expected) => {
    const segments = [{ index: 1 }, { index: 2 }, { index: 3 }];
    expect(confirmedSegmentRange({ status, cuts: [10, 20], groupCount: 30 }, segments, segment)).toEqual(expected);
  });
});

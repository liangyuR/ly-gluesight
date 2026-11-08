import { describe, expect, it } from "vitest";
import { historyReturnContext, historyReturnSearch } from "../src/features/history/returnContext";

// 防的回归：从分段页返回历史件时复测配方、相机、帧组、对照或“显示原始记录”丢了，要重新找到问题帧再复测
describe("历史详情的返回地址", () => {
  it("离开时的复测上下文写进地址 → 读回同一份上下文", () => {
    const context = { candidate: "B", camera: 2, seq: 40, comparison: "compare-1", original: true };
    const search = new URLSearchParams(historyReturnSearch(context));
    expect(historyReturnContext(search, 3, [1, 20, 40, 41])).toEqual(context);
  });
});

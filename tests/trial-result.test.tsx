import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { TrialResult } from "../src/features/workspace/TrialResult";
import type { FrameState } from "../src/features/plan/types";
import { frameMetrics } from "./fixtures";

// 防的回归：缺胶站的胶宽是 null，被当成 0 画进表里，既不像缺胶，也会把胶宽均值拉偏；
// 以及偷偷把已停用的“相对示教中线偏移”列加回来（规格 §4：胶偏只按整段的 x 均值与最大偏差判）
it("一帧有缺胶站 → 该站标缺胶且胶宽显示未测而不是 0，摘要给出断胶与 x，不再有偏移列", () => {
  render(<TrialResult metrics={frameMetrics(0, 3, { widths: [3.9, null, 4.1], widthMin: 3.9, widthMax: 4.1, widthMean: 4, gapMaxMm: 1, presentRatio: 2 / 3, xMm: -0.125 })} />);
  fireEvent.click(screen.getByText(/逐站胶宽 · 有胶 2 \/ 3 站/));
  expect(screen.getByText(/最长断胶 1\.0 mm · x -0\.125 mm/)).toBeVisible();
  const missing = within(screen.getByRole("row", { name: "2 缺胶 未测" }));
  expect(missing.queryByText("0.000")).not.toBeInTheDocument();
  expect(screen.getByRole("row", { name: "3 有胶 4.100" })).toBeVisible();
  expect(screen.queryByRole("columnheader", { name: /偏移/ })).not.toBeInTheDocument();
});

// 防的回归：缺帧、测量失败、出走廊的帧没有数值，却被画成一张空表或当 NG 对待（规格 §3：只算检测不完整）
it.each<[FrameState, string]>([
  ["missing", "缺帧"], ["error", "测量失败"], ["unmeasured", "未测"], ["outOfCorridor", "胶条走出走廊"],
])("帧状态为 %s → 不画逐站表，只说明本帧检测不完整、不判 NG", (state, text) => {
  render(<TrialResult metrics={frameMetrics(1, 5, { state, error: state === "error" ? "无有效边缘" : null, widths: [], widthMean: null, xMm: null })} />);
  expect(screen.getByRole("status")).toHaveTextContent(text);
  expect(screen.getByRole("status")).toHaveTextContent("不判 NG");
  expect(screen.queryByRole("table")).not.toBeInTheDocument();
});

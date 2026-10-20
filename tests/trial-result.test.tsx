import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { TrialResult, trialPoints } from "../src/features/workspace/TrialResult";

it("试测包含缺胶站且重新计算 → 空值保持未测，明细刷新为本次结果", () => {
  const measurement = {
    unit: "mm", ids: [10, 11], status: ["ok", "no_bead"], innerCenter: [-0.125, null],
    width: [2.375, null], px: [[20, 30], [40, 50]], coverage: 0.5, ms: 12, verdict: "NG",
  };
  const points = trialPoints(measurement)!;
  const { rerender } = render(<TrialResult points={points} />);
  fireEvent.click(screen.getByText("逐点试测结果 · 测得 1 / 2 · 未测 1"));
  const missing = within(screen.getByRole("row", { name: "11 缺胶 未测 未测" }));
  expect(missing.getAllByText("未测")).toHaveLength(2);
  expect(missing.queryByText("0.000")).not.toBeInTheDocument();
  expect(screen.getByText("-0.125")).toBeVisible();
  expect(points[1].px).toEqual([40, 50]);

  rerender(<TrialResult points={trialPoints({ ...measurement, status: ["ok", "ok"], innerCenter: [0.1, 0.2], width: [2.1, 2.2], coverage: 1, verdict: "OK" })!} />);
  expect(screen.getByText("逐点试测结果 · 测得 2 / 2 · 未测 0")).toBeVisible();
  expect(screen.queryByText("-0.125")).not.toBeInTheDocument();
  expect(screen.getByRole("row", { name: "11 测得 0.200 2.200" })).toBeVisible();
});

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { FixtureValidation } from "../src/features/workspace/FixtureValidation";
import type { FixtureViewReport } from "../src/features/workspace/types";

it("采用两端参数建议 → 先确认，再只按拍照点与图号交给后端，前端不传数值", async () => {
  // 防的回归（决定一）：建议值由前端回填或手改后写进配方，不经确认就改测量定义。页面到后端的接线由 pnpm test:e2e:fixture 覆盖
  const onApply = vi.fn().mockResolvedValue(true);
  const report: FixtureViewReport = { k: 1, view: 1, shotId: "P2", passed: true, samples: 12, checks: [], sigmaMm: .02, meanMm: .01, nominalMm: 0, toleranceMm: .5,
    searchUse: .2, rows: [], rotation: { enabled: false, leverMm: 40, leverThetaP95Mm: .01, thresholdMm: .1, exceeds: false, frames: 36, observableFrames: 36, allObservable: true, autoConditions: false },
    suggestion: { p99Mm: 1.2, maxEndShiftMm: 2.2, endMarginMm: 1.3, applicable: true, note: "两端 |Δ| 的 P99 = 1.20 mm" } };
  render(<FixtureValidation reports={[report]} current disabled={false} onApply={onApply}/>);
  const panel = screen.getByRole("region", { name: "拍照点 P2 图 1 特征定位统计" });
  await userEvent.click(within(panel).getByRole("button", { name: "采用建议…" }));
  expect(onApply).not.toHaveBeenCalled();
  const dialog = screen.getByRole("dialog", { name: "采用两端参数建议" });
  expect(dialog).toHaveTextContent("永远不判");
  await userEvent.click(within(dialog).getByRole("button", { name: "确认写入配方" }));
  expect(onApply).toHaveBeenCalledWith(1, 1);
});

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import { useSiteStatus } from "../src/features/cycle/api";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { historyApi } from "../src/features/history/api";
import type { FixtureViewReport } from "../src/features/workspace/types";
import { siteStatus, workspaceState } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ useSiteStatus: vi.fn() }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { applyFixtureSuggestion: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));

it("采用两端参数建议 → 先确认，再只按拍照点与图号调用后端，前端不传数值", async () => {
  // 防的回归（决定一）：建议值由前端回填或手改后写进配方，不经确认就改测量定义
  vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  const ws = workspaceState();
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.applyFixtureSuggestion).mockResolvedValue(ws.data!);
  const report: FixtureViewReport = { k: 1, view: 1, shotId: "P2", passed: true, samples: 12, checks: [], sigmaMm: .02, meanMm: .01, nominalMm: 0, toleranceMm: .5,
    searchUse: .2, rows: [], rotation: { enabled: false, leverMm: 40, leverThetaP95Mm: .01, thresholdMm: .1, exceeds: false, frames: 36, observableFrames: 36, allObservable: true, autoConditions: false },
    suggestion: { p99Mm: 1.2, maxEndShiftMm: 2.2, endMarginMm: 1.3, applicable: true, note: "两端 |Δ| 的 P99 = 1.20 mm" } };
  ws.data!.workspace.validation!.fixture = [report];
  render(<MemoryRouter><ValidationPage /></MemoryRouter>);
  const panel = screen.getByRole("region", { name: "拍照点 P2 图 1 特征定位统计" });
  await userEvent.click(within(panel).getByRole("button", { name: "采用建议…" }));
  expect(workspaceApi.applyFixtureSuggestion).not.toHaveBeenCalled();
  const dialog = screen.getByRole("dialog", { name: "采用两端参数建议" });
  expect(dialog).toHaveTextContent("永远不判");
  await userEvent.click(within(dialog).getByRole("button", { name: "确认写入配方" }));
  expect(workspaceApi.applyFixtureSuggestion).toHaveBeenCalledWith("A", 7, 1, 1);
});

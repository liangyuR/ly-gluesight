import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import RejudgeDialog from "../src/features/history/components/RejudgeDialog";
import { historyApi } from "../src/features/history/api";

vi.mock("../src/features/history/api", () => ({ historyApi: { rejudge: vi.fn() } }));

it("填写直线段、弯道段胶宽允差 → 批量重判请求带 straight / corner 覆盖，旧的位置与胶宽覆盖保持原样", async () => {
  vi.mocked(historyApi.rejudge).mockResolvedValue({ total: 0, skipped: 0, limitHit: false, matrix: [], changes: [], skipReasons: [] });
  render(<MemoryRouter><RejudgeDialog query={{ scope: "real" }} total={3} onClose={() => {}} /></MemoryRouter>);
  await userEvent.type(screen.getByLabelText("直线段胶宽上公差"), "0.5");
  await userEvent.type(screen.getByLabelText("弯道段胶宽连续超差允许长度"), "3");
  await userEvent.click(screen.getByRole("button", { name: /重判当前筛选/ }));
  expect(historyApi.rejudge).toHaveBeenCalledWith({
    query: { scope: "real" }, ids: [], useCurrentRecipe: false,
    overrides: { position: {}, width: {}, straight: { tolUpper: 0.5 }, corner: { maxExcursionLen: 3 } },
  });
});

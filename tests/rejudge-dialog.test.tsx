import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import RejudgeDialog from "../src/features/history/components/RejudgeDialog";
import { historyApi } from "../src/features/history/api";

vi.mock("../src/features/history/api", () => ({ historyApi: { rejudge: vi.fn() } }));

// 契约：后端 history::Overrides 的 straight / corner（KindOverride，camelCase）只作用于段配方记录，position / width 只作用于旧拍照点记录。
// 防的回归：某一行的输入写进了别的分组（直线段的值落到 width、弯道段的值落到 straight），或段配方两行又被置灰、不发给后端。
it("填写位置上公差、直线段上公差与弯道段连续超差允许长度 → 批量重判请求各自带在 position、straight、corner 里", async () => {
  vi.mocked(historyApi.rejudge).mockResolvedValue({ total: 0, skipped: 0, limitHit: false, matrix: [], changes: [], skipReasons: [] });
  render(<MemoryRouter><RejudgeDialog query={{ scope: "real" }} total={3} onClose={() => {}} /></MemoryRouter>);
  await userEvent.type(screen.getByLabelText("位置上公差"), "0.4");
  await userEvent.type(screen.getByLabelText("直线段胶宽上公差"), "0.5");
  await userEvent.type(screen.getByLabelText("弯道段胶宽连续超差允许长度"), "3");
  await userEvent.click(screen.getByRole("button", { name: /重判当前筛选/ }));
  expect(historyApi.rejudge).toHaveBeenCalledWith({
    query: { scope: "real" }, ids: [], useCurrentRecipe: false,
    overrides: { position: { tolUpper: 0.4 }, width: {}, straight: { tolUpper: 0.5 }, corner: { maxExcursionLen: 3 } },
  });
});

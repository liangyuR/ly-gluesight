import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RejudgeDialog from "../src/features/history/components/RejudgeDialog";
import { historyApi } from "../src/features/history/api";

vi.mock("../src/features/history/api", () => ({ historyApi: { rejudge: vi.fn() } }));

const show = () => render(<MemoryRouter><RejudgeDialog query={{ scope: "real" }} total={3} onClose={() => {}} /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(historyApi.rejudge).mockReset().mockResolvedValue({ total: 0, skipped: 0, limitHit: false, matrix: [], changes: [], skipReasons: [] });
});

// 后端 history::Overrides 还没有 straight / corner 字段（C5 才读取，登记见 docs/decisions/segment-burst.md 5.1）：
// 填了会被静默忽略，操作员却以为试算生效，所以这两行输入先置灰；C5 落地后改成可填并换掉这两条用例。
describe("直线段、弯道段胶宽覆盖（后端尚未读取）", () => {
  it("打开批量重判 → 直线段、弯道段两行输入全部置灰，旧的位置与胶宽两行可填", () => {
    show();
    for (const row of ["直线段", "弯道段"])
      for (const field of ["上公差", "下公差", "连续超差允许长度"]) expect(screen.getByLabelText(`${row}胶宽${field}`)).toBeDisabled();
    for (const row of ["位置", "胶宽"]) expect(screen.getByLabelText(`${row}上公差`)).toBeEnabled();
  });

  it("填了位置上公差 → 批量重判请求只带旧的位置与胶宽覆盖，不带 straight / corner", async () => {
    show();
    await userEvent.type(screen.getByLabelText("位置上公差"), "0.5");
    await userEvent.click(screen.getByRole("button", { name: /重判当前筛选/ }));
    expect(historyApi.rejudge).toHaveBeenCalledWith({
      query: { scope: "real" }, ids: [], useCurrentRecipe: false,
      overrides: { position: { tolUpper: 0.5 }, width: {} },
    });
  });
});

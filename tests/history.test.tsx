import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { historyApi } from "../src/features/history/api";
import type { HistoryPage as Page } from "../src/features/history/types";
import { deferred, partSummary, summary } from "./fixtures";

vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn(), exportCsv: vi.fn(), reveal: vi.fn(), rejudge: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ useRecipes: () => [summary()] }));
vi.mock("../src/features/plc/api", () => ({ subscribe: () => () => {} }));
const empty: Page = { total: 0, items: [], counts: { ok: 0, ng: 0, err: 0, excursion: 0 } };
const page: Page = { total: 60, items: [partSummary()], counts: { ok: 30, ng: 20, err: 5, excursion: 5 } };
const show = () => render(<MemoryRouter><HistoryPage /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(historyApi.query).mockResolvedValue(page);
  vi.mocked(historyApi.exportCsv).mockResolvedValue("D:/exports/result.csv");
  vi.mocked(historyApi.reveal).mockResolvedValue(undefined);
  vi.mocked(historyApi.rejudge).mockResolvedValue({ total: 60, skipped: 0, skipReasons: [], limitHit: false, matrix: [], changes: [] });
});

describe("历史筛选与分页", () => {

  it("导出使用筛选而不限制当前页，可打开输出目录", async () => {
    show(); await screen.findByText("58.3%");
    fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "101" } });
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(historyApi.exportCsv).toHaveBeenCalledWith(expect.objectContaining({ sn: "101" }));
    expect(vi.mocked(historyApi.exportCsv).mock.calls[0][0]).not.toHaveProperty("offset");
    await userEvent.click(await screen.findByRole("button", { name: "打开所在文件夹" }));
    expect(historyApi.reveal).toHaveBeenCalledWith("D:/exports/result.csv");
  });

  it("筛选改变后旧查询晚到不能覆盖新结果", async () => {
    const old = deferred<Page>();
    vi.mocked(historyApi.query).mockReturnValueOnce(old.promise).mockResolvedValue({ ...empty, total: 1, items: [{ ...partSummary(2), sn: 777 }] });
    show(); fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "777" } });
    await screen.findByText("777");
    await act(async () => old.resolve(page));
    expect(screen.getByText("777")).toBeVisible(); expect(screen.queryByText("101")).not.toBeInTheDocument();
  });

  it("旧查询失败不能污染新筛选结果", async () => {
    const old = deferred<Page>();
    vi.mocked(historyApi.query).mockReturnValueOnce(old.promise).mockResolvedValue(page);
    show(); fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "101" } });
    await screen.findByText("58.3%"); await act(async () => old.reject(new Error("旧查询失败")));
    expect(screen.queryByText("Error: 旧查询失败")).not.toBeInTheDocument(); expect(screen.getByText("101")).toBeVisible();
  });

  it("新筛选等待期间隐藏旧记录，不能沿用旧数量导出或重判", async () => {
    show(); await screen.findByText("58.3%");
    const request = deferred<Page>(); vi.mocked(historyApi.query).mockReturnValueOnce(request.promise);
    fireEvent.change(screen.getByRole("textbox", { name: "历史 SN" }), { target: { value: "404" } });
    expect(screen.queryByText("101")).toBeNull(); expect(screen.getByRole("status")).toHaveTextContent("正在加载");
    expect(screen.getByRole("button", { name: "批量重判" })).toBeDisabled(); expect(screen.getByRole("button", { name: "导出 CSV" })).toBeDisabled();
    await act(async () => request.resolve(empty)); expect(screen.getByText("没有符合条件的记录")).toBeVisible();
  });

  it("批量重判入口使用当前完整筛选，改变筛选时关闭旧批次", async () => {
    show(); await screen.findByText("58.3%");
    await userEvent.click(screen.getByRole("button", { name: "ERR" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "历史配方" }), "A");
    await userEvent.click(screen.getByRole("button", { name: "下一页" }));
    await userEvent.click(screen.getByRole("button", { name: "批量重判" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "重判当前筛选（60 件）" }));
    expect(historyApi.rejudge).toHaveBeenCalledWith(expect.objectContaining({ query: expect.objectContaining({ verdicts: ["ERR_INSPECT"], recipeId: "A" }), ids: [] }));
    expect(vi.mocked(historyApi.rejudge).mock.calls[0][0].query).not.toHaveProperty("offset");
    fireEvent.change(screen.getByRole("textbox", { name: "历史 SN" }), { target: { value: "new" } });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("导出等待防重复，失败后同一筛选可重试", async () => {
    show(); await screen.findByText("58.3%");
    const request = deferred<string>(); vi.mocked(historyApi.exportCsv).mockReturnValueOnce(request.promise);
    const button = screen.getByRole("button", { name: "导出 CSV" }); fireEvent.click(button); fireEvent.click(button);
    expect(historyApi.exportCsv).toHaveBeenCalledTimes(1); expect(screen.getByRole("button", { name: "导出中…" })).toBeDisabled();
    await act(async () => request.reject(new Error("磁盘已满")));
    expect(screen.getByText("Error: 磁盘已满")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(await screen.findByText("D:/exports/result.csv")).toBeVisible(); expect(screen.queryByText("Error: 磁盘已满")).toBeNull();
  });

});

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
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
  it("初始查询今天，并显示良率、分页边界", async () => {
    show(); await screen.findByText("58.3%");
    expect(historyApi.query).toHaveBeenCalledWith(expect.objectContaining({ from: expect.any(Number), offset: 0, limit: 50 }));
    expect(screen.getByRole("button", { name: "上一页" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "下一页" }));
    await waitFor(() => expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 50 })));
    expect(screen.getByRole("button", { name: "下一页" })).toBeDisabled();
    fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "101" } });
    await waitFor(() => expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ sn: "101", offset: 0 })));
  });

  it("筛选组展开成各个 NG 结论，并支持全部日期与指定配方", async () => {
    show(); await screen.findByText("58.3%");
    await userEvent.click(screen.getByRole("button", { name: "NG" }));
    await userEvent.click(screen.getByRole("button", { name: "全部" }));
    await userEvent.selectOptions(screen.getByRole("combobox"), "A");
    await waitFor(() => expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({
      from: null, recipeId: "A", verdicts: ["NG_POSITION", "NG_WIDTH", "NG_ABSOLUTE", "NG_GAP"],
    })));
  });

  it("导出使用筛选而不限制当前页，可打开输出目录", async () => {
    show(); await screen.findByText("58.3%");
    fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "101" } });
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(historyApi.exportCsv).toHaveBeenCalledWith(expect.objectContaining({ sn: "101" }));
    expect(vi.mocked(historyApi.exportCsv).mock.calls[0][0]).not.toHaveProperty("offset");
    await userEvent.click(await screen.findByRole("button", { name: "打开所在文件夹" }));
    expect(historyApi.reveal).toHaveBeenCalledWith("D:/exports/result.csv");
  });

  it("空结果禁用批量重判和导出", async () => {
    vi.mocked(historyApi.query).mockResolvedValue(empty); show();
    await screen.findByText("没有符合条件的记录");
    expect(screen.getByRole("button", { name: "导出 CSV" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量重判" })).toBeDisabled();
  });

  it("筛选改变后旧查询晚到不能覆盖新结果", async () => {
    const old = deferred<Page>();
    vi.mocked(historyApi.query).mockReturnValueOnce(old.promise).mockResolvedValue({ ...empty, total: 1, items: [{ ...partSummary(2), sn: 777 }] });
    show(); fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "777" } });
    await screen.findByText("777");
    await act(async () => old.resolve(page));
    expect(screen.getByText("777")).toBeVisible(); expect(screen.queryByText("101")).not.toBeInTheDocument();
  });

  it("查询错误能刷新恢复", async () => {
    vi.mocked(historyApi.query).mockRejectedValueOnce(new Error("数据库不可用")); show();
    expect(await screen.findByText("Error: 数据库不可用")).toBeVisible();
    await userEvent.click(screen.getByTitle("刷新"));
    await screen.findByText("58.3%"); expect(screen.queryByText("Error: 数据库不可用")).not.toBeInTheDocument();
  });

  it("旧查询失败不能污染新筛选结果", async () => {
    const old = deferred<Page>();
    vi.mocked(historyApi.query).mockReturnValueOnce(old.promise).mockResolvedValue(page);
    show(); fireEvent.change(screen.getByPlaceholderText("SN"), { target: { value: "101" } });
    await screen.findByText("58.3%"); await act(async () => old.reject(new Error("旧查询失败")));
    expect(screen.queryByText("Error: 旧查询失败")).not.toBeInTheDocument(); expect(screen.getByText("101")).toBeVisible();
  });

  it.each([["7 天", 7], ["30 天", 30]] as const)("日期 %s 从当地零点往前计算", async (label, days) => {
    await show(); await screen.findByText("58.3%");
    const midnight = new Date(); midnight.setHours(0, 0, 0, 0);
    await userEvent.click(screen.getByRole("button", { name: label }));
    await waitFor(() => expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ from: midnight.getTime() - days * 86_400_000, offset: 0 })));
    expect(screen.getByRole("button", { name: label })).toHaveAttribute("aria-pressed", "true");
  });

  it("所有结论可组合，再次点击取消筛选而不是查询空组", async () => {
    show(); await screen.findByText("58.3%");
    for (const name of ["OK", "局部超差", "NG", "ERR"]) await userEvent.click(screen.getByRole("button", { name }));
    expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ verdicts: ["OK", "OK_WITH_EXCURSION", "NG_POSITION", "NG_WIDTH", "NG_ABSOLUTE", "NG_GAP", "ERR_INSPECT"] }));
    for (const name of ["OK", "局部超差", "NG", "ERR"]) await userEvent.click(screen.getByRole("button", { name }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "历史配方" }), "A");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "历史配方" }), "");
    expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ verdicts: [], recipeId: null }));
  });

  it("上一页回到首屏，第二页改变筛选只发首屏查询", async () => {
    show(); await screen.findByText("58.3%");
    await userEvent.click(screen.getByRole("button", { name: "下一页" }));
    await userEvent.click(screen.getByRole("button", { name: "上一页" }));
    expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 0 }));
    await userEvent.click(screen.getByRole("button", { name: "下一页" }));
    vi.mocked(historyApi.query).mockClear();
    fireEvent.change(screen.getByRole("textbox", { name: "历史 SN" }), { target: { value: "999" } });
    await waitFor(() => expect(historyApi.query).toHaveBeenCalledTimes(1));
    expect(historyApi.query).toHaveBeenCalledWith(expect.objectContaining({ offset: 0, sn: "999" }));
    fireEvent.change(screen.getByRole("textbox", { name: "历史 SN" }), { target: { value: "" } });
    await waitFor(() => expect(historyApi.query).toHaveBeenLastCalledWith(expect.objectContaining({ sn: "" })));
  });

  it.each(["click", "Enter", " "])("记录行通过 %s 打开详情", async key => {
    function Location() { return <output aria-label="当前路径">{useLocation().pathname}</output>; }
    render(<MemoryRouter><HistoryPage /><Location /></MemoryRouter>); await screen.findByText("58.3%");
    const record = screen.getByRole("row", { name: "查看 SN 101 的记录" });
    expect(record).toHaveAttribute("tabindex", "0");
    if (key === "click") await userEvent.click(record);
    else { record.focus(); fireEvent.keyDown(record, { key }); }
    expect(screen.getByLabelText("当前路径")).toHaveTextContent("/history/1");
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

  it.each(["success", "error"])("旧筛选导出的 %s 不回写到新筛选", async kind => {
    show(); await screen.findByText("58.3%");
    const request = deferred<string>(); vi.mocked(historyApi.exportCsv).mockReturnValueOnce(request.promise);
    fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    fireEvent.change(screen.getByRole("textbox", { name: "历史 SN" }), { target: { value: "new" } });
    await screen.findByText("58.3%");
    await act(async () => kind === "success" ? request.resolve("D:/old.csv") : request.reject(new Error("旧导出失败")));
    expect(screen.queryByText("D:/old.csv")).toBeNull(); expect(screen.queryByText("Error: 旧导出失败")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(historyApi.exportCsv).toHaveBeenLastCalledWith(expect.objectContaining({ sn: "new" }));
  });

  it("打开导出目录失败显示原因，保留文件路径并允许重试", async () => {
    show(); await screen.findByText("58.3%"); await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    vi.mocked(historyApi.reveal).mockRejectedValueOnce(new Error("目录已移除"));
    await userEvent.click(await screen.findByRole("button", { name: "打开所在文件夹" }));
    expect(await screen.findByText("Error: 目录已移除")).toBeVisible(); expect(screen.getByText("D:/exports/result.csv")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "打开所在文件夹" }));
    expect(historyApi.reveal).toHaveBeenCalledTimes(2); expect(screen.queryByText("Error: 目录已移除")).toBeNull();
  });
});

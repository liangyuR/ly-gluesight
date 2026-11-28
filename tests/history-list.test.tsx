import { act, fireEvent, render as renderPage, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { expect, it, vi } from "vitest";
import { RejudgeTaskProvider } from "../src/features/history/rejudgeTask";
import type { HistoryPackagePreview, HistoryQuery, RejudgeResult } from "../src/features/history/types";
import type { OperationProgress } from "../src/features/workspace/operation";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import HistoryPage from "../src/pages/HistoryPage";
import { burstPartDetail, burstPartSummary, deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const render = (ui: ReactElement) => renderPage(ui, { wrapper: RejudgeTaskProvider });
const row = (id: number) => screen.findByRole("button", { name: `查看 SN SN-${100 + id} 的记录` });
const meta = (id: string, name: string) => ({ schema: 1, id, name, description: "", createdAt: 0, createdBy: "", copiedFrom: null });
const projects = [{ id: "line-a", current: true, error: null, meta: meta("line-a", "A 线") }, { id: "line-b", current: false, error: null, meta: meta("line-b", "B 线") }];
const page = (items: ReturnType<typeof burstPartSummary>[]) => ({ total: items.length, counts: { ok: items.length, excursion: 0, ng: 0, err: 0 }, items });
const queries = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "history_query").map(([, args]) => (args as { query: HistoryQuery }).query);
const rejudgeCalls = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "history_rejudge");
const emitHistory = (event: string, payload: number | number[]) => {
  const handler = vi.mocked(listen).mock.calls.filter(([name]) => name === event).at(-1)?.[1];
  act(() => handler?.({ event, id: 1, payload }));
};
const emitRejudgeProgress = (payload: OperationProgress) => {
  const event = "operation://progress";
  const handler = vi.mocked(listen).mock.calls.filter(([name]) => name === event).at(-1)![1];
  act(() => handler({ event, id: 1, payload }));
};

function mockTextFilteredHistory(rejudge?: () => Promise<RejudgeResult>, operation?: {
  cancel: (taskId: string) => Promise<boolean>;
  status: (taskId: string) => OperationProgress | null;
}) {
  const records = [burstPartSummary(1), { ...burstPartSummary(2), tags: ["待复核"] }];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      const { sn, tag } = (args as { query: HistoryQuery }).query;
      return page(records.filter(item => (!sn || item.sn.includes(sn)) && (!tag || item.tags?.includes(tag))));
    }
    if (rejudge) {
      if (command === "history_rejudge") return rejudge();
      if (command === "operation_status") return operation?.status((args as { taskId: string }).taskId) ?? null;
      if (command === "operation_cancel" && operation) return operation.cancel((args as { taskId: string }).taskId);
      if (command === "history_overview") return { ...burstPartDetail(), summary: records[0], burst: null };
      if (command === "history_neighbors") return { older: null, newer: null };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
}

const rejudgeResult: RejudgeResult = {
  total: 1, skipped: 0, limitHit: false,
  matrix: [{ from: "NG_GAP", to: "OK", count: 1 }],
  changes: [], skipReasons: [],
};

it("收起重判后离开页面并换筛选 → 重开同一任务，结果按原筛选导航", async () => {
  const pending = deferred<RejudgeResult>();
  mockTextFilteredHistory(() => pending.promise);
  const user = userEvent.setup();
  const dialog = () => within(screen.getByRole("dialog", { name: "批量重判" }));
  render(<MemoryRouter initialEntries={["/history"]}><Routes>
    <Route path="/history" element={<><Link to="/other">其他页面</Link><HistoryPage /></>} />
    <Route path="/other" element={<Link to="/history">返回历史页</Link>} />
    <Route path="/history/:id" element={<HistoryDetailPage />} />
  </Routes></MemoryRouter>);

  await row(1);
  await user.type(screen.getByLabelText("历史 SN"), "SN-101{Enter}");
  const open = screen.getByRole("button", { name: "批量重判" });
  await waitFor(() => expect(open).toBeEnabled());
  await user.click(open);
  await user.click(dialog().getByRole("button", { name: "重判当前筛选（1 件）" }));
  await waitFor(() => expect(rejudgeCalls()).toHaveLength(1));
  const { taskId } = rejudgeCalls()[0][1] as { taskId: string };
  expect(taskId).toEqual(expect.any(String));
  const originalRequest = { request: expect.objectContaining({ query: expect.objectContaining({ sn: "SN-101" }) }), taskId };
  expect(rejudgeCalls()).toEqual([["history_rejudge", originalRequest]]);
  emitRejudgeProgress({ taskId, stage: "rejudging", completed: 0, total: 1, failed: 0, skipped: 0, elapsedMs: 200 });

  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog", { name: "批量重判" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("link", { name: "其他页面" }));
  expect(screen.queryByLabelText("历史 SN")).not.toBeInTheDocument();
  await user.click(screen.getByRole("link", { name: "返回历史页" }));
  await row(1);
  await user.type(screen.getByLabelText("历史 SN"), "SN-102{Enter}");
  await waitFor(() => expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument());
  expect(await row(2)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "查看重判进度" }));

  expect(dialog().getByRole("status", { name: "批量重判进度" })).toHaveTextContent("正在重判");
  expect(dialog().getByText(/此任务使用原筛选范围/)).toBeVisible();
  expect(dialog().getByRole("button", { name: "重判中…" })).toBeDisabled();
  expect(rejudgeCalls()).toEqual([["history_rejudge", originalRequest]]);

  await act(async () => pending.resolve({
    ...rejudgeResult,
    changes: [{ id: 1, sn: "SN-101", ts: 0, from: "NG_GAP", to: "OK", reason: "允许长度调整", flags: null }],
  }));
  expect(dialog().getByRole("table", { name: "判定变化矩阵" })).toBeVisible();
  await user.click(dialog().getByRole("button", { name: "使用当前列表" }));
  expect(dialog().queryByRole("table", { name: "判定变化矩阵" })).not.toBeInTheDocument();
  expect(dialog().getByRole("button", { name: "重判当前筛选（1 件）" })).toBeEnabled();
  await user.keyboard("{Escape}");
  await user.click(screen.getByRole("button", { name: "查看上次重判" }));
  await user.click(dialog().getByText("SN SN-101"));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("history_neighbors", {
    id: 1, query: expect.objectContaining({ sn: "SN-101" }),
  }));
});

it("第二次重判开始即清除旧矩阵，失败后显示错误并可重试", async () => {
  const pending = deferred<RejudgeResult>();
  mockTextFilteredHistory(vi.fn<() => Promise<RejudgeResult>>()
    .mockResolvedValueOnce(rejudgeResult).mockReturnValueOnce(pending.promise).mockResolvedValueOnce(rejudgeResult));
  const user = userEvent.setup();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await user.click(screen.getByRole("button", { name: "批量重判" }));
  const dialog = within(screen.getByRole("dialog", { name: "批量重判" }));
  await user.click(dialog.getByRole("button", { name: /重判当前筛选/ }));
  expect(await dialog.findByRole("table", { name: "判定变化矩阵" })).toBeVisible();

  const retry = () => dialog.getByRole("button", { name: "再次重判" });
  await user.click(retry());
  expect(dialog.getByRole("button", { name: "重判中…" })).toBeDisabled();
  expect(dialog.queryByRole("table", { name: "判定变化矩阵" })).not.toBeInTheDocument();
  await act(async () => pending.reject(new Error("记录库忙")));
  expect(dialog.getByRole("alert")).toHaveTextContent("记录库忙");
  expect(dialog.queryByRole("table", { name: "判定变化矩阵" })).not.toBeInTheDocument();
  await user.click(retry());
  expect(await dialog.findByRole("table", { name: "判定变化矩阵" })).toBeVisible();
  expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
});

it("当前重判显示件数并收到取消回执 → 等待原请求结束，舍弃部分结果后可重试", async () => {
  const pending = deferred<RejudgeResult>();
  const retried = deferred<RejudgeResult>();
  const cancel = deferred<boolean>();
  let taskId = "";
  const progress = (id: string, stage: OperationProgress["stage"]): OperationProgress => ({
    taskId: id, stage, completed: 1, total: 2, failed: 0, skipped: 0, elapsedMs: 500,
  });
  mockTextFilteredHistory(vi.fn<() => Promise<RejudgeResult>>()
    .mockReturnValueOnce(pending.promise).mockReturnValueOnce(retried.promise), {
    cancel: () => cancel.promise,
    status: id => id === taskId ? progress(id, "cancelled") : null,
  });
  const user = userEvent.setup();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await user.click(screen.getByRole("button", { name: "批量重判" }));
  const dialog = within(screen.getByRole("dialog", { name: "批量重判" }));
  await user.click(dialog.getByRole("button", { name: "重判当前筛选（2 件）" }));
  await waitFor(() => expect(rejudgeCalls()).toHaveLength(1));
  taskId = (rejudgeCalls()[0][1] as { taskId: string }).taskId;
  expect(taskId).toEqual(expect.any(String));
  const event = "operation://progress";
  const oldListener = vi.mocked(listen).mock.calls.filter(([name]) => name === event).at(-1)![1];

  const status = () => dialog.getByRole("status", { name: "批量重判进度" });
  const cancelButton = () => dialog.getByRole("button", { name: "取消" });
  const matrix = () => dialog.queryByRole("table", { name: "判定变化矩阵" });
  expect(cancelButton()).toBeDisabled();
  emitRejudgeProgress(progress(taskId, "rejudging"));
  expect(status()).toHaveTextContent("正在重判");
  expect(status()).toHaveTextContent("已处理 1 / 2 件");
  expect(dialog.getByRole("progressbar", { name: "批量重判完成进度" })).toHaveAttribute("value", "50");
  expect(cancelButton()).toBeEnabled();

  await user.click(cancelButton());
  expect(invoke).toHaveBeenCalledWith("operation_cancel", { taskId });
  await act(async () => cancel.resolve(true));
  expect(status()).toHaveTextContent("正在取消");
  expect(cancelButton()).toBeDisabled();
  expect(dialog.getByRole("button", { name: "重判中…" })).toBeDisabled();
  expect(matrix()).not.toBeInTheDocument();
  await user.click(cancelButton());
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "operation_cancel"))
    .toEqual([["operation_cancel", { taskId }]]);
  expect(rejudgeCalls()).toHaveLength(1);

  await act(async () => pending.reject("OPERATION_CANCELLED: 任务已取消"));
  expect(invoke).toHaveBeenCalledWith("operation_status", { taskId });
  expect(status()).toHaveTextContent("已取消，本次没有结果");
  expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
  expect(matrix()).not.toBeInTheDocument();
  await user.click(dialog.getByRole("button", { name: "再次重判" }));
  await waitFor(() => expect(rejudgeCalls()).toHaveLength(2));
  const retryId = (rejudgeCalls()[1][1] as { taskId: string }).taskId;
  expect(retryId).toEqual(expect.any(String));
  expect(retryId).not.toBe(taskId);
  act(() => oldListener({ event, id: 1, payload: progress(taskId, "rejudging") }));
  expect(status()).toHaveTextContent("正在准备重判");
  emitRejudgeProgress(progress(retryId, "rejudging"));
  expect(status()).toHaveTextContent("已处理 1 / 2 件");
  await act(async () => retried.resolve(rejudgeResult));
  expect(await dialog.findByRole("table", { name: "判定变化矩阵" })).toBeVisible();
});

it("文本筛选仅应用最终输入，回车立即查询一次，等待期间禁用旧结果导出", async () => {
  mockTextFilteredHistory();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  const exportCsv = () => screen.getByRole("button", { name: "导出 CSV" });
  await waitFor(() => expect(exportCsv()).toBeEnabled());
  const start = queries().length;
  const subsequentQueries = () => queries().slice(start);
  const sn = screen.getByLabelText("历史 SN");
  const tag = screen.getByLabelText("历史 Tag");

  vi.useFakeTimers();
  try {
    fireEvent.change(sn, { target: { value: "SN-" } });
    expect(exportCsv()).toBeDisabled();
    await act(() => vi.advanceTimersByTimeAsync(150));
    fireEvent.change(sn, { target: { value: "SN-102" } });
    expect(sn).toHaveValue("SN-102");
    await act(() => vi.advanceTimersByTimeAsync(249));
    expect(subsequentQueries()).toEqual([]);

    await act(() => vi.advanceTimersByTimeAsync(1));
    const snQuery = expect.objectContaining({ sn: "SN-102", offset: 0 });
    expect(subsequentQueries()).toEqual([snQuery]);
    expect(screen.getByRole("button", { name: "查看 SN SN-102 的记录" })).toBeVisible();
    expect(exportCsv()).toBeEnabled();

    fireEvent.change(tag, { target: { value: "待复核" } });
    expect(exportCsv()).toBeDisabled();
    expect(subsequentQueries()).toEqual([snQuery]);
    await act(async () => { fireEvent.keyDown(tag, { key: "Enter", code: "Enter" }); });
    const tagQuery = expect.objectContaining({ sn: "SN-102", tag: "待复核", offset: 0 });
    expect(subsequentQueries()).toEqual([snQuery, tagQuery]);
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(subsequentQueries()).toEqual([snQuery, tagQuery]);
  } finally {
    vi.useRealTimers();
  }
});

it("中文 Tag 合成期间暂停或按确认键 → 只在合成结束后查询最终文字", async () => {
  mockTextFilteredHistory();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  const start = queries().length;
  const tag = screen.getByLabelText("历史 Tag");

  vi.useFakeTimers();
  try {
    fireEvent.compositionStart(tag);
    fireEvent.change(tag, { target: { value: "daifuhe" } });
    await act(() => vi.advanceTimersByTimeAsync(500));
    fireEvent.keyDown(tag, { key: "Enter", code: "Enter", isComposing: true });
    expect(queries().slice(start)).toEqual([]);

    fireEvent.change(tag, { target: { value: "待复核" } });
    fireEvent.compositionEnd(tag, { data: "待复核" });
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(queries().slice(start)).toEqual([expect.objectContaining({ tag: "待复核", offset: 0 })]);
    expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

it.each([
  { name: "历史页持续打开跨本机午夜", resumes: false },
  { name: "历史页隐藏期间跨本机午夜后恢复可见", resumes: true },
])("$name → 今天筛选切到新日期", async ({ resumes }) => {
  const midnight = new Date(2026, 10, 28).getTime();
  const previousMidnight = new Date(2026, 10, 27).getTime();
  const records = [
    { ...burstPartSummary(1), ts: previousMidnight },
    { ...burstPartSummary(2), ts: midnight },
  ];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      const { from } = (args as { query: HistoryQuery }).query;
      return page(records.filter(record => record.ts <= Date.now() && (from == null || record.ts >= from)));
    }
    throw new Error(`Unexpected command: ${command}`);
  });

  vi.useFakeTimers();
  const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  try {
    vi.setSystemTime(midnight - 100);
    await act(async () => { render(<MemoryRouter><HistoryPage /></MemoryRouter>); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "今天" })); });
    expect(queries().at(-1)?.from).toBe(previousMidnight);
    expect(screen.getByRole("button", { name: "查看 SN SN-101 的记录" })).toBeVisible();
    const before = queries().length;

    if (resumes) {
      hidden.mockReturnValue(true);
      await act(async () => { fireEvent(document, new Event("visibilitychange")); });
      vi.setSystemTime(midnight + 60_000);
      expect(queries()).toHaveLength(before);
      hidden.mockReturnValue(false);
      await act(async () => { fireEvent(document, new Event("visibilitychange")); });
    } else {
      await act(() => vi.advanceTimersByTimeAsync(99));
      expect(queries()).toHaveLength(before);
      await act(() => vi.advanceTimersByTimeAsync(1));
    }

    expect(queries().slice(before)).toEqual([expect.objectContaining({ from: midnight })]);
    expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看 SN SN-102 的记录" })).toBeVisible();
  } finally {
    hidden.mockRestore();
    vi.useRealTimers();
  }
});

it("查询在途时连续换筛选 → 只查最后条件，其失败时隐去旧记录", async () => {
  const oldRefresh = deferred<ReturnType<typeof page>>();
  const latestFilter = deferred<ReturnType<typeof page>>();
  let holdRefresh = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      if ((args as { query: HistoryQuery }).query.project === "line-a") return latestFilter.promise;
      return holdRefresh ? oldRefresh.promise : page([burstPartSummary(1)]);
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  const projectSelect = screen.getByLabelText("历史项目", { selector: "select" });
  await waitFor(() => expect(projectSelect).toHaveTextContent("B 线 · line-b"));
  const refresh = screen.getByRole("button", { name: "刷新" });
  await waitFor(() => expect(refresh).toBeEnabled());
  const beforeRefresh = queries().length;
  holdRefresh = true;
  await userEvent.click(refresh);
  await userEvent.selectOptions(projectSelect, "line-b");
  await userEvent.selectOptions(projectSelect, "当前项目（A 线）");
  expect(queries().slice(beforeRefresh).map(query => query.project)).toEqual([null]);

  await act(async () => oldRefresh.resolve(page([burstPartSummary(2)])));
  expect(queries().slice(beforeRefresh).map(query => query.project)).toEqual([null, "line-a"]);
  expect(screen.queryByRole("button", { name: "查看 SN SN-102 的记录" })).not.toBeInTheDocument();
  await act(async () => latestFilter.reject(new Error("记录库忙")));
  expect(await screen.findByRole("alert")).toHaveTextContent("记录库忙");
  expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument();
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 0 件");
});

it("首页查询期间连续入库 → 补查一次并显示最新记录", async () => {
  const firstRefresh = deferred<ReturnType<typeof page>>();
  const nextRefresh = deferred<ReturnType<typeof page>>();
  const responses: Promise<ReturnType<typeof page>>[] = [];
  let records = [burstPartSummary(1)];
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") return responses.shift() ?? page(records);
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await waitFor(() => expect(screen.getByRole("button", { name: "刷新" })).toBeEnabled());
  const beforeRefresh = queries().length;
  responses.push(firstRefresh.promise, nextRefresh.promise);
  records = [burstPartSummary(2), ...records];
  const firstPage = page(records);
  emitHistory("history://inserted", 2);
  for (const id of [3, 4, 5]) {
    records = [burstPartSummary(id), ...records];
    emitHistory("history://inserted", id);
  }
  expect(queries().slice(beforeRefresh)).toHaveLength(1);

  await act(async () => firstRefresh.resolve(firstPage));
  expect(await row(2)).toBeVisible();
  expect(queries().slice(beforeRefresh)).toHaveLength(2);
  await act(async () => nextRefresh.resolve(page(records)));
  expect(await row(5)).toBeVisible();
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 5 件");
  expect(queries().slice(beforeRefresh)).toHaveLength(2);
});

it("首页后台刷新时可勾选并翻页 → 换页及筛选期间只允许操作已加载的新结果", async () => {
  const homeRefresh = deferred<ReturnType<typeof page>>();
  const nextPage = deferred<ReturnType<typeof page>>();
  const filtered = deferred<ReturnType<typeof page>>();
  const records = Array.from({ length: 51 }, (_, index) => burstPartSummary(51 - index));
  let refreshing = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      const { offset, sn } = (args as { query: HistoryQuery }).query;
      if (sn) return filtered.promise;
      if (offset === 50) return nextPage.promise;
      return refreshing ? homeRefresh.promise : { ...page(records), items: records.slice(0, 50) };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  const user = userEvent.setup();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(51);
  const selection = screen.getByRole("checkbox", { name: /SN-151/ });
  const next = screen.getByRole("button", { name: "下一页" });
  const beforeRefresh = queries().length;
  refreshing = true;
  const updated = [burstPartSummary(52), ...records];
  emitHistory("history://inserted", 52);

  await user.click(selection);
  expect(selection).toBeChecked();
  expect(screen.getByRole("button", { name: "删除历史件（1）" })).toBeDisabled();
  await user.click(next);
  expect(queries().slice(beforeRefresh).map(query => query.offset)).toEqual([0]);
  expect(selection).toBeDisabled();
  expect(screen.getByRole("button", { name: "上一页" })).toBeDisabled();

  await act(async () => homeRefresh.resolve({ ...page(updated), items: updated.slice(0, 50) }));
  expect(queries().slice(beforeRefresh).map(query => query.offset)).toEqual([0, 50]);
  expect(screen.queryByRole("button", { name: "查看 SN SN-152 的记录" })).not.toBeInTheDocument();
  await act(async () => nextPage.resolve({ ...page(updated), items: updated.slice(50) }));
  expect(await row(1)).toBeVisible();
  expect(screen.queryByRole("button", { name: "查看 SN SN-151 的记录" })).not.toBeInTheDocument();
  const currentSelection = screen.getByRole("checkbox", { name: /SN-101/ });
  expect(currentSelection).toBeEnabled();
  expect(screen.getByRole("button", { name: "上一页" })).toBeEnabled();

  await user.type(screen.getByLabelText("历史 SN"), "SN-101{Enter}");
  expect(currentSelection).toBeDisabled();
  await act(async () => filtered.resolve(page([records.at(-1)!])));
  expect(screen.getByRole("checkbox", { name: /SN-101/ })).toBeEnabled();
});

it("后续页刷新期间先收到越界结果 → 最新查询仍有记录时保留页码", async () => {
  const early = deferred<ReturnType<typeof page>>();
  const slice = (total: number, ids: number[]) => ({ total, counts: { ok: total, excursion: 0, ng: 0, err: 0 }, items: ids.map(burstPartSummary) });
  let secondPage = slice(51, [1]);
  let hold = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      if ((args as { query: HistoryQuery }).query.offset === 0) return slice(51, [51]);
      if (!hold) return secondPage;
      hold = false;
      return early.promise;
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(51);
  await userEvent.click(screen.getByRole("button", { name: "下一页" }));
  await row(1);
  const before = queries().length;
  emitHistory("history://inserted", 52);
  expect(queries()).toHaveLength(before);

  hold = true;
  emitHistory("history://changed", [1]);
  secondPage = slice(51, [2]);
  emitHistory("history://changed", [2]);
  expect(queries().slice(before).map(query => query.offset)).toEqual([50]);

  await act(async () => early.resolve(slice(50, [])));
  expect(await row(2)).toBeVisible();
  expect(queries().slice(before).map(query => query.offset)).toEqual([50, 50]);
});

it.each([
  { changedDay: false, expected: "恢复仍在本页的勾选" },
  { changedDay: true, expected: "日期范围已变时清空旧勾选" },
])("从筛选后的第二页打开工件再返回 → $expected", async ({ changedDay }) => {
  let now = new Date(2026, 10, 28, 12).getTime();
  vi.spyOn(Date, "now").mockImplementation(() => now);
  const part = { ...burstPartSummary(1), projectId: "line-b", tags: ["待复核"] };
  let secondPage = [part, { ...burstPartSummary(2), projectId: "line-b", tags: ["待复核"] }];
  const returned = deferred<ReturnType<typeof page>>();
  const refreshed = deferred<ReturnType<typeof page>>();
  let returning = false;
  let returnQueries = 0;
  const filteredPage = { project: "line-b", tag: "待复核", offset: 50 };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      const { offset, project, tag } = (args as { query: HistoryQuery }).query;
      const matching = offset === 50 && project === filteredPage.project && tag === filteredPage.tag;
      if (matching && returning) return ++returnQueries === 1 ? returned.promise : refreshed.promise;
      return { ...page(matching ? secondPage : [burstPartSummary(51)]), total: 50 + secondPage.length };
    }
    if (command === "history_overview") return { ...burstPartDetail(), summary: part, burst: null };
    if (command === "history_neighbors") return { older: null, newer: null };
    throw new Error(`Unexpected command: ${command}`);
  });
  const user = userEvent.setup();
  render(<MemoryRouter initialEntries={["/history"]}><Routes>
    <Route path="/history" element={<HistoryPage />} />
    <Route path="/history/:id" element={<HistoryDetailPage />} />
  </Routes></MemoryRouter>);

  await row(51);
  const project = () => screen.getByLabelText("历史项目", { selector: "select" });
  const tag = () => screen.getByLabelText("历史 Tag");
  await waitFor(() => expect(project()).toHaveTextContent("B 线 · line-b"));
  await user.selectOptions(project(), "line-b");
  await user.type(tag(), "待复核");
  const next = screen.getByRole("button", { name: "下一页" });
  await waitFor(() => expect(next).toBeEnabled());
  await user.click(next);
  const opener = await row(1);
  await user.click(screen.getByRole("checkbox", { name: /SN-101/ }));
  await user.click(screen.getByRole("checkbox", { name: /SN-102/ }));
  expect(screen.getByRole("button", { name: "编辑 Tag（2）" })).toBeEnabled();
  await user.click(opener);

  const back = await screen.findByRole("button", { name: "返回历史列表" });
  const beforeReturn = queries().length;
  secondPage = [part];
  returning = true;
  if (changedDay) now = new Date(2026, 10, 29, 12).getTime();
  await user.click(back);
  await waitFor(() => expect(returnQueries).toBe(1));
  emitHistory("history://changed", [1]);
  const newPage = { ...page(secondPage), total: 51 };
  await act(async () => returned.resolve(newPage));
  const returnedRow = await row(1);
  expect(returnedRow).toBeVisible();
  await waitFor(() => expect(returnedRow).toHaveFocus());
  expect(project()).toHaveValue("line-b");
  expect(tag()).toHaveValue("待复核");
  expect(queries()[beforeReturn]).toMatchObject(filteredPage);
  expect(screen.getByRole<HTMLInputElement>("checkbox", { name: /SN-101/ }).checked).toBe(!changedDay);
  expect(screen.queryByRole("checkbox", { name: /SN-102/ })).not.toBeInTheDocument();
  const edit = () => screen.getByRole("button", { name: `编辑 Tag（${changedDay ? 0 : 1}）` });
  expect(edit()).toBeDisabled();
  await act(async () => refreshed.resolve(newPage));
  expect(edit()).toHaveProperty("disabled", changedDay);
});

// 防的回归：历史页默认只看当前项目（看不到其他项目的记录），或选“当前项目”后查询没带项目编号、仍混入其他项目的记录
it("默认查本机全部项目；选当前项目 → 查询带当前项目编号", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") return page([burstPartSummary(1)]);
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  expect(queries().at(-1)?.project ?? null).toBeNull();
  await userEvent.selectOptions(screen.getByLabelText("历史项目", { selector: "select" }), "当前项目（A 线）");
  await waitFor(() => expect(queries().at(-1)?.project).toBe("line-a"));
});

// 防的回归：勾选框点击被当成点行跳到详情；或换筛选后仍保留看不见的旧选择，导出包里混进当前列表外的件
it("勾选件后改筛选 → 不跳详情且选择被清空", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") return page([burstPartSummary(1), burstPartSummary(2)]);
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter initialEntries={["/history"]}><Routes>
    <Route path="/history" element={<HistoryPage />} />
    <Route path="/history/:id" element={<p>详情页</p>} />
  </Routes></MemoryRouter>);
  await row(1);
  await userEvent.click(screen.getByRole("checkbox", { name: /SN-101/ }));
  expect(screen.queryByText("详情页")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: /导出历史件（1）/ })).toBeEnabled();
  await userEvent.click(screen.getByRole("button", { name: "30 天" }));
  await waitFor(() => expect(screen.getByRole("button", { name: /导出历史件（0）/ })).toBeDisabled());
});

it("保存常用与自定义 Tag → 列表显示标签并按标签精确筛选", async () => {
  let tags: string[] = [];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_set_tags") { tags = (args as { tags: string[] }).tags; return tags; }
    if (command === "history_query") {
      const tag = (args as { query: { tag?: string } }).query.tag;
      return page([{ ...burstPartSummary(1), tags }, burstPartSummary(2)].filter(item => !tag || item.tags?.includes(tag)));
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await userEvent.click(screen.getByRole("checkbox", { name: /SN-101/ }));
  await userEvent.click(screen.getByRole("button", { name: "编辑 Tag（1）" }));
  const dialog = within(screen.getByRole("dialog", { name: "编辑 Tag" }));
  await userEvent.click(dialog.getByRole("button", { name: "故障件" }));
  await userEvent.type(dialog.getByLabelText("自定义 Tag"), "夜班复核{Enter}");
  await userEvent.click(dialog.getByRole("button", { name: "保存 Tag" }));
  const table = within(screen.getByRole("table", { name: "历史记录" }));
  expect(await table.findByRole("button", { name: "故障件" })).toBeVisible();
  await userEvent.click(table.getByRole("button", { name: "夜班复核" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "查看 SN SN-102 的记录" })).not.toBeInTheDocument());
  expect(screen.getByLabelText("历史 Tag")).toHaveValue("夜班复核");
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 1 件");
  expect(screen.getByRole("button", { name: "查看 SN SN-101 的记录" })).toBeVisible();
});

it.each([
  { change: "首页新件入库", nextPage: false },
  { change: "末页自动回退", nextPage: true },
])("批量 Tag 保存失败后$change → 重试保留原记录和标签", async ({ nextPage }) => {
  const total = nextPage ? 52 : 50;
  let records = Array.from({ length: total }, (_, index) => burstPartSummary(total - index));
  const setTags = vi.fn().mockRejectedValueOnce(new Error("暂时无法保存 Tag")).mockResolvedValue(["夜班复核"]);
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_set_tags") return setTags(args);
    if (command === "history_query") {
      const { offset = 0, limit = 50 } = (args as { query: HistoryQuery }).query;
      return { ...page(records), items: records.slice(offset, offset + limit) };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  const user = userEvent.setup();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(total);
  if (nextPage) await user.click(screen.getByRole("button", { name: "下一页" }));
  await row(1);
  await user.click(screen.getByRole("checkbox", { name: /SN-102/ }));
  await user.click(screen.getByRole("checkbox", { name: /SN-101/ }));
  await user.click(screen.getByRole("button", { name: "编辑 Tag（2）" }));
  const dialog = () => within(screen.getByRole("dialog", { name: "编辑 Tag" }));
  await user.type(dialog().getByLabelText("自定义 Tag"), "夜班复核{Enter}");
  await user.click(dialog().getByRole("button", { name: "保存 Tag" }));
  expect(await dialog().findByText("暂时无法保存 Tag")).toBeVisible();
  const request = { ids: [2, 1], tags: ["夜班复核"] };
  expect(setTags.mock.calls).toEqual([[request]]);

  if (nextPage) {
    records = records.slice(2);
    emitHistory("history://changed", [52, 51]);
  } else {
    records = [burstPartSummary(51), ...records];
    emitHistory("history://inserted", 51);
  }
  expect(await screen.findByText(nextPage ? "SN-150" : "SN-151", { selector: ".hist-sn .sn" })).toBeInTheDocument();
  expect(dialog().getByText(/为所选 2 件/)).toBeVisible();
  await user.click(dialog().getByRole("button", { name: "保存 Tag" }));
  expect(await screen.findByText("人工 Tag 已保存。")).toBeVisible();
  expect(setTags.mock.calls).toEqual([[request], [request]]);
});

it("确认删除已选历史件 → 记录与统计刷新，取消确认则保留记录", async () => {
  let deleted = false;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") return page(deleted ? [] : [{ ...burstPartSummary(1), cycleId: null }]);
    if (command === "history_delete") { deleted = true; return { deleted: 1, warnings: [] }; }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await userEvent.click(screen.getByRole("checkbox", { name: /SN-101/ }));
  expect(screen.getByRole("button", { name: "导出历史件（1）" })).toBeDisabled();
  await userEvent.click(screen.getByRole("button", { name: "删除历史件（1）" }));
  await userEvent.click(within(screen.getByRole("dialog", { name: "删除历史件" })).getByRole("button", { name: "取消" }));
  expect(await row(1)).toBeVisible();
  expect(deleted).toBe(false);
  await userEvent.click(screen.getByRole("button", { name: "删除历史件（1）" }));
  await userEvent.click(screen.getByRole("button", { name: "确认删除 1 件" }));
  expect(await screen.findByText("已删除 1 件历史记录。")).toBeVisible();
  await waitFor(() => expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument());
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 0 件");
  expect(screen.getByRole("button", { name: "删除历史件（0）" })).toBeDisabled();
});

it("历史包预览失效 → 原地重新读取，同一路径的新预览须再次确认", async () => {
  const original: HistoryPackagePreview = {
    path: "C:/history.zip", manifestSha256: "a".repeat(64), bytes: 2048,
    sourceProject: "line-a", sourceProjectName: "原项目", exportedAt: 1, appVersion: "1.0.0",
    parts: 1, withImages: 0, duplicates: 0, notes: [],
  };
  const updated = { ...original, manifestSha256: "b".repeat(64), sourceProjectName: "新项目", parts: 2 };
  const expired = "历史包内容已变化，请重新读取预览后再导入";
  const pick = vi.fn().mockResolvedValue(original);
  const preview = vi.fn().mockRejectedValueOnce("历史包暂时无法读取").mockResolvedValue(updated);
  const importPackage = vi.fn().mockRejectedValueOnce("历史库暂时忙").mockRejectedValueOnce(expired)
    .mockResolvedValue({ imported: 2, ids: [1, 2], skipped: [], notes: [] });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") return page([burstPartSummary(1)]);
    if (command === "operation_status") return null;
    if (command === "history_package_pick") return pick();
    if (command === "history_package_preview") return preview(args);
    if (command === "history_package_import") return importPackage(args);
    throw new Error(`Unexpected command: ${command}`);
  });

  const user = userEvent.setup();
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await user.click(screen.getByRole("button", { name: "导入历史件" }));
  await screen.findByRole("dialog", { name: "导入历史件" });
  const dialog = () => within(screen.getByRole("dialog", { name: "导入历史件" }));
  expect(dialog().getByText("原项目")).toBeVisible();
  await user.click(dialog().getByRole("button", { name: "导入" }));
  expect(await dialog().findByText("历史库暂时忙")).toBeVisible();

  await user.click(dialog().getByRole("button", { name: "导入" }));
  expect(await dialog().findByText(expired)).toBeVisible();
  expect(dialog().queryByRole("button", { name: "导入" })).not.toBeInTheDocument();
  await user.click(dialog().getByRole("button", { name: "重新读取预览" }));
  expect(await dialog().findByText("历史包暂时无法读取")).toBeVisible();
  expect(dialog().queryByRole("button", { name: "导入" })).not.toBeInTheDocument();
  await user.click(dialog().getByRole("button", { name: "重新读取预览" }));

  expect(await dialog().findByText("新项目")).toBeVisible();
  expect(dialog().getByText("2 件，其中 0 件带原图")).toBeVisible();
  expect(importPackage).toHaveBeenCalledTimes(2);
  await user.click(dialog().getByRole("button", { name: "导入" }));
  expect(await screen.findByRole("dialog", { name: "已导入历史件" })).toBeVisible();
  expect(pick).toHaveBeenCalledTimes(1);
  expect(preview.mock.calls).toEqual([[{ path: original.path }], [{ path: original.path }]]);
  const request = (manifestSha256: string) => ({ path: original.path, manifestSha256, taskId: expect.any(String) });
  expect(importPackage.mock.calls).toEqual([[request(original.manifestSha256)], [request(original.manifestSha256)], [request(updated.manifestSha256)]]);
});

it("按结果筛选与新件入库 → 每次只查询一次，列表和各结果计数同步更新", async () => {
  const okItems = [1, 3, 4].map(burstPartSummary);
  const ngItems = [{ ...burstPartSummary(2), verdict: "NG_GAP" as const }];
  const ngQuery = expect.objectContaining({ includeVerdictFacets: true, verdicts: expect.arrayContaining(["NG_GAP"]) });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      const { verdicts, includeVerdictFacets } = (args as { query: HistoryQuery }).query;
      const verdictFacets = { ok: okItems.length, excursion: 0, ng: ngItems.length, err: 0 };
      const items = [...okItems, ...ngItems].filter(item => !verdicts?.length || verdicts.includes(item.verdict));
      const counts = verdicts?.length ? { ...verdictFacets, ok: 0 } : verdictFacets;
      return { total: items.length, counts, items, verdictFacets: includeVerdictFacets ? verdictFacets : undefined };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(4);
  const filters = within(screen.getByRole("group", { name: "按结果筛选" }));
  const beforeFilter = queries().length;
  await userEvent.click(filters.getByRole("button", { name: /NG/ }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "查看 SN SN-104 的记录" })).not.toBeInTheDocument());
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 1 件");
  expect(filters.getByRole("button", { name: /OK/ })).toHaveTextContent("OK3");
  expect(filters.getByRole("button", { name: /NG/ })).toHaveTextContent("NG1");
  expect(queries().slice(beforeFilter)).toEqual([ngQuery]);

  const beforeInsert = queries().length;
  ngItems.push({ ...burstPartSummary(5), verdict: "NG_GAP" });
  emitHistory("history://inserted", 5);
  expect(await row(5)).toBeVisible();
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 2 件");
  expect(filters.getByRole("button", { name: /OK/ })).toHaveTextContent("OK3");
  expect(filters.getByRole("button", { name: /NG/ })).toHaveTextContent("NG2");
  expect(queries().slice(beforeInsert)).toEqual([ngQuery]);
});

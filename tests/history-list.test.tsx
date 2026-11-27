import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { expect, it, vi } from "vitest";
import type { HistoryQuery } from "../src/features/history/types";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import HistoryPage from "../src/pages/HistoryPage";
import { burstPartDetail, burstPartSummary, deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const row = (id: number) => screen.findByRole("button", { name: `查看 SN SN-${100 + id} 的记录` });
const meta = (id: string, name: string) => ({ schema: 1, id, name, description: "", createdAt: 0, createdBy: "", copiedFrom: null });
const projects = [{ id: "line-a", current: true, error: null, meta: meta("line-a", "A 线") }, { id: "line-b", current: false, error: null, meta: meta("line-b", "B 线") }];
const page = (items: ReturnType<typeof burstPartSummary>[]) => ({ total: items.length, counts: { ok: items.length, excursion: 0, ng: 0, err: 0 }, items });
const queries = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "history_query").map(([, args]) => (args as { query: HistoryQuery }).query);
const emitHistory = (event: string, payload: number | number[]) => {
  const handler = vi.mocked(listen).mock.calls.filter(([name]) => name === event).at(-1)?.[1];
  act(() => handler?.({ event, id: 1, payload }));
};

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

it("从筛选后的第二页打开工件再返回 → 恢复筛选、页码和原工件焦点", async () => {
  const part = { ...burstPartSummary(1), projectId: "line-b", tags: ["待复核"] };
  const filteredPage = { project: "line-b", tag: "待复核", offset: 50 };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      const { offset, project, tag } = (args as { query: HistoryQuery }).query;
      const matching = offset === 50 && project === filteredPage.project && tag === filteredPage.tag;
      return { ...page([matching ? part : burstPartSummary(51)]), total: 51 };
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
  await user.click(await row(1));

  const back = await screen.findByRole("button", { name: "返回历史列表" });
  const beforeReturn = queries().length;
  await user.click(back);
  const returnedRow = await row(1);
  expect(returnedRow).toBeVisible();
  await waitFor(() => expect(returnedRow).toHaveFocus());
  expect(project()).toHaveValue("line-b");
  expect(tag()).toHaveValue("待复核");
  expect(queries()[beforeReturn]).toMatchObject(filteredPage);
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

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { burstPartSummary } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const row = (id: number) => screen.findByRole("button", { name: `查看 SN SN-${100 + id} 的记录` });
const meta = (id: string, name: string) => ({ schema: 1, id, name, description: "", createdAt: 0, createdBy: "", copiedFrom: null });
const projects = [{ id: "line-a", current: true, error: null, meta: meta("line-a", "A 线") }, { id: "line-b", current: false, error: null, meta: meta("line-b", "B 线") }];
const page = (items: ReturnType<typeof burstPartSummary>[]) => ({ total: items.length, counts: { ok: items.length, excursion: 0, ng: 0, err: 0 }, items });
const queries = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "history_query").map(([, args]) => (args as { query: { project?: string | null } }).query);

it("换筛选后查询失败 → 不把上一次条件的记录当成新条件的结果", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") return projects;
    if (command === "history_query") {
      if ((args as { query: { project: string } }).query.project === "line-a") throw new Error("记录库忙");
      return { total: 1, counts: { ok: 0, excursion: 0, ng: 1, err: 0 }, items: [burstPartSummary(1)] };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await userEvent.selectOptions(screen.getByLabelText("历史项目", { selector: "select" }), "当前项目（A 线）");
  expect(await screen.findByRole("alert")).toHaveTextContent("记录库忙");
  expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument();
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("共 0 件");
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
  await userEvent.click(screen.getByRole("button", { name: "7 天" }));
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

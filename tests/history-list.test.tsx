import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
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
      if ((args as { query: { scope: string } }).query.scope === "offline") throw new Error("记录库忙");
      return { total: 1, counts: { ok: 0, excursion: 0, ng: 1, err: 0 }, items: [burstPartSummary(1)] };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await userEvent.click(screen.getByRole("button", { name: "离线件" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("记录库忙");
  expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument();
  expect(screen.getByText(/共/, { selector: ".hist-counts span" })).toHaveTextContent("离线共 0 件");
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

// 防的回归：项目列表没读到时把所有记录都标成“已删除”，或已删除项目的记录不加标记、看起来像本机现有项目
it.each([
  ["项目列表读到、记录的项目不在列表里 → 显示编号并标已删除", true, true],
  ["项目列表没读到 → 只显示编号，不标已删除", false, false],
])("%s", async (_name, listed, deleted) => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "project_current") return projects[0].meta;
    if (command === "project_list") { if (listed) return projects; throw new Error("项目目录不可读"); }
    if (command === "history_query") return page([{ ...burstPartSummary(1), projectId: "line-b" }, { ...burstPartSummary(2), projectId: "line-gone" }]);
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  const gone = (await row(2)).closest("tr")!;
  await waitFor(() => expect(screen.getByLabelText("历史项目", { selector: "select" })).toHaveTextContent("当前项目（A 线）"));
  expect(within(gone).getByTitle("line-gone")).toHaveTextContent(deleted ? "line-gone已删除" : /^line-gone$/);
  if (listed) expect(within((await row(1)).closest("tr")!).getByTitle("line-b")).toHaveTextContent("B 线");
});

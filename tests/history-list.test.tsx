import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { deferred, partSummary } from "./fixtures";
import type { HistoryPage as Page } from "../src/features/history";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const row = (id: number) => screen.findByRole("button", { name: `查看 SN SN-${100 + id} 的记录` });

it("翻页结果未返回 → 保留上一页的统计、行与分页并标为加载中，返回后换成新页", async () => {
  const page = (ids: number[]): Page => ({ total: 120, counts: { ok: 0, excursion: 0, ng: 120, err: 0 }, items: ids.map(partSummary) });
  const next = deferred<Page>();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "history_query") return (args as { query: { offset: number } }).query.offset ? next.promise : page([1, 2]);
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await row(1);
  await userEvent.click(screen.getByRole("button", { name: "下一页" }));
  expect(screen.getByText(/真机共/)).toHaveTextContent("真机共 120 件");
  expect(await row(1)).toBeVisible();
  expect(screen.getByRole("table").closest("[aria-busy]")).toHaveAttribute("aria-busy", "true");
  expect(screen.getByRole("button", { name: "上一页" })).toBeInTheDocument();
  await act(async () => next.resolve(page([51])));
  expect(await row(51)).toBeVisible();
  expect(screen.queryByRole("button", { name: "查看 SN SN-101 的记录" })).not.toBeInTheDocument();
  expect(screen.getByRole("table").closest("[aria-busy]")).toHaveAttribute("aria-busy", "false");
});

it("换筛选后查询失败 → 不把上一次条件的记录当成新条件的结果", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "history_query") {
      if ((args as { query: { scope: string } }).query.scope === "offline") throw new Error("记录库忙");
      return { total: 1, counts: { ok: 0, excursion: 0, ng: 1, err: 0 }, items: [partSummary(1)] };
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

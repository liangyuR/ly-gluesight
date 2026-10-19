import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import { AppLogPage } from "../src/features/logs";
import type { AppLogRecord } from "../src/features/logs/types";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const record = (ts: number, cycleId: string): AppLogRecord => ({ ts, level: "info", ev: "判定", msg: `工件 ${cycleId} 合格`, cycleId });

it("翻页结果未返回 → 保留上一页日志与总条数并标为加载中", async () => {
  const next = deferred<{ total: number; items: AppLogRecord[]; files: number; bytes: number }>();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "applog_query") return (args as { query: { offset: number } }).query.offset ? next.promise : { total: 250, items: [record(1000, "cycle-a")], files: 1, bytes: 1 };
    if (command === "applog_usage") return { dir: "logs", files: 1, bytes: 1, keepDays: 30, maxTotalBytes: 1, segmentBytes: 1 };
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><AppLogPage /></MemoryRouter>);
  await screen.findByRole("button", { name: "cycle-a" });
  await userEvent.click(screen.getByRole("button", { name: "下一页" }));
  expect(screen.getByText("共 250 条")).toBeVisible();
  expect(screen.getByRole("button", { name: "cycle-a" })).toBeVisible();
  expect(screen.getByRole("table", { name: "运行日志" }).closest("[aria-busy]")).toHaveAttribute("aria-busy", "true");
  await act(async () => next.resolve({ total: 250, items: [record(900, "cycle-z")], files: 1, bytes: 1 }));
  expect(await screen.findByRole("button", { name: "cycle-z" })).toBeVisible();
  expect(screen.queryByRole("button", { name: "cycle-a" })).not.toBeInTheDocument();
});

it("刷新后新日志插到最前 → 已查到的工件记录链接保留在原日志行上", async () => {
  let items = [record(1000, "cycle-a")];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "applog_query") return { total: items.length, items, files: 1, bytes: 1 };
    if (command === "applog_usage") return { dir: "logs", files: 1, bytes: 1, keepDays: 30, maxTotalBytes: 1, segmentBytes: 1 };
    if (command === "history_find_cycle") return (args as { cycleId: string }).cycleId === "cycle-a" ? 7 : null;
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><AppLogPage /></MemoryRouter>);
  const rowOf = async (cycleId: string) => (await screen.findByRole("button", { name: cycleId.slice(0, 8) })).closest("tr")!;
  await userEvent.click(within(await rowOf("cycle-a")).getByRole("button", { name: "记录" }));
  expect(await within(await rowOf("cycle-a")).findByRole("link", { name: "#7" })).toBeVisible();
  items = [record(2000, "cycle-b"), ...items];
  await userEvent.click(screen.getByRole("button", { name: "刷新" }));
  await rowOf("cycle-b");
  expect(within(await rowOf("cycle-a")).getByRole("link", { name: "#7" })).toBeVisible();
});

it("查无记录后工件入库、新日志插到最前 → 该行可点“无记录，重查”得到历史链接", async () => {
  let items = [record(1000, "cycle-a")];
  let stored: number | null = null;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "applog_query") return { total: items.length, items, files: 1, bytes: 1 };
    if (command === "applog_usage") return { dir: "logs", files: 1, bytes: 1, keepDays: 30, maxTotalBytes: 1, segmentBytes: 1 };
    if (command === "history_find_cycle") return stored;
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><AppLogPage /></MemoryRouter>);
  const rowOf = async (cycleId: string) => (await screen.findByRole("button", { name: cycleId.slice(0, 8) })).closest("tr")!;
  await userEvent.click(within(await rowOf("cycle-a")).getByRole("button", { name: "记录" }));
  expect(await within(await rowOf("cycle-a")).findByRole("button", { name: "无记录，重查" })).toBeVisible();
  stored = 7;
  items = [record(2000, "cycle-b"), ...items];
  await userEvent.click(screen.getByRole("button", { name: "刷新" }));
  await rowOf("cycle-b");
  await userEvent.click(within(await rowOf("cycle-a")).getByRole("button", { name: "无记录，重查" }));
  expect(await within(await rowOf("cycle-a")).findByRole("link", { name: "#7" })).toBeVisible();
});

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { partSummary } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

it("切到候选调试历史 → 调试数量明确标注，切回真机不混入调试统计", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "history_query") {
      const debug = (args as { query: { scope: string } }).query.scope === "debug";
      return { total: debug ? 2 : 0, counts: { ok: debug ? 2 : 0, excursion: 0, ng: 0, err: 0 }, items: debug ? [1, 2].map(id => ({ ...partSummary(id), source: "offlineCandidate" })) : [] };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  await userEvent.click(screen.getByRole("button", { name: "候选调试" }));
  expect(await screen.findByText(/候选调试共/)).toHaveTextContent("候选调试共 2 件");
  expect(screen.queryByText(/真机共/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "真机件" }));
  expect(await screen.findByText(/真机共/)).toHaveTextContent("真机共 0 件");
  expect(screen.queryByText(/候选调试共/)).not.toBeInTheDocument();
});

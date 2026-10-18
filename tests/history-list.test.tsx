import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { partSummary } from "./fixtures";
import type { Verdict } from "../src/features/cycle/types";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

it("记录判定决定行语义色 → NG 整行标红、未测成用故障色、局部超差用警告色、OK 不标记", async () => {
  const verdicts: Verdict[] = ["NG_GAP", "ERR_INSPECT", "OK_WITH_EXCURSION", "OK"];
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "history_query") return { total: 4, counts: { ok: 1, excursion: 1, ng: 1, err: 1 }, items: verdicts.map((verdict, i) => ({ ...partSummary(i + 1), verdict })) };
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  const row = (id: number) => screen.findByRole("row", { name: `查看 SN SN-${100 + id} 的记录` });
  expect(await row(1)).toHaveClass("r-ng");
  expect(await row(2)).toHaveClass("r-err");
  expect(await row(3)).toHaveClass("r-exc");
  expect((await row(4)).className).toBe("");
});

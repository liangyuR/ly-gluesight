import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { TrialProgressPanel } from "../src/features/cycle/components/MaintenanceMode";
import type { HistoryQuery } from "../src/features/history";
import type { TrialProgress } from "../src/features/cycle/types";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const trial: TrialProgress[] = [{
  recipeId: "NEW-14",
  recipeVersion: 3,
  counts: { ok: 3, excursion: 0, ng: 3, err: 1 },
  ng: [{ verdict: "NG_WIDTH", count: 2 }, { verdict: "NG_GAP", count: 1 }],
}];

it("试判面板点复核 → 历史页只查该配方试判件的 NG 与 ERR、不限日期；改选正式检测 → 只查正式件", async () => {
  const queries: HistoryQuery[] = [];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "history_query") {
      queries.push((args as { query: HistoryQuery }).query);
      return { total: 0, counts: { ok: 0, excursion: 0, ng: 0, err: 0 }, items: [] };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter>
    <Routes>
      <Route path="/" element={<TrialProgressPanel trial={trial} recipes={[]} />} />
      <Route path="/history" element={<HistoryPage />} />
    </Routes>
  </MemoryRouter>);
  const panel = screen.getByRole("region", { name: "维护试判统计" });
  expect(panel).toHaveTextContent("试判 7 件 · OK 3 · 局部超差 0 · NG 3（胶宽超差 2、断胶 1） · ERR 1");

  await userEvent.click(screen.getByRole("button", { name: "复核" }));
  await waitFor(() => expect(queries.length).toBeGreaterThan(0));
  expect(queries.at(-1)).toMatchObject({
    recipeId: "NEW-14",
    modes: ["maintenanceShadow"],
    from: null,
    verdicts: ["NG_POSITION", "NG_WIDTH", "NG_ABSOLUTE", "NG_GAP", "ERR_INSPECT"],
  });

  await userEvent.click(screen.getByRole("button", { name: "正式检测" }));
  await waitFor(() => expect(queries.at(-1)?.modes).toEqual(["production"]));
});

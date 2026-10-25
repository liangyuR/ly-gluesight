import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import HistoryPage from "../src/pages/HistoryPage";
import { burstPartSummary } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

const row = (id: number) => screen.findByRole("button", { name: `查看 SN SN-${100 + id} 的记录` });

it("换筛选后查询失败 → 不把上一次条件的记录当成新条件的结果", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
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

it("维护件 → 采集件标“维护采集·未判定”不显示成 ERR，试判件标“试判”加原结论", async () => {
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "recipe_list") return { recipes: [], errors: [] };
    if (command === "history_query") return { total: 2, counts: { ok: 0, excursion: 0, ng: 1, err: 1 }, items: [
      { ...burstPartSummary(1), mode: "maintenanceCollect", verdict: "ERR_INSPECT", plcCode: 0, reason: "维护采集：只录图" },
      { ...burstPartSummary(2), mode: "maintenanceShadow", verdict: "NG_GAP", plcCode: 0 },
    ] };
    throw new Error(`Unexpected command: ${command}`);
  });
  render(<MemoryRouter><HistoryPage /></MemoryRouter>);
  const collect = (await row(1)).closest("tr")!;
  expect(collect).toHaveTextContent("维护采集·未判定");
  expect(collect).not.toHaveTextContent("ERR · 未测成");
  expect((await row(2)).closest("tr")).toHaveTextContent("试判 NG · 断胶");
});

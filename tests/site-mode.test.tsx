import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AppLayout from "../src/layout/AppLayout";
import OfflinePage from "../src/pages/OfflinePage";
import { useReadiness } from "../src/features/cycle/api";
import type { Readiness, SiteStatus } from "../src/features/cycle/types";
import { readiness, recipeOption, sampleSet, siteStatus } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

let site: SiteStatus;
let report: Readiness | null;
const listeners = new Map<string, (event: { payload: unknown }) => void>();
beforeEach(() => {
  listeners.clear();
  report = null;
  vi.mocked(listen).mockImplementation(async (event, listener) => {
    listeners.set(event, listener as (event: { payload: unknown }) => void);
    return () => {};
  });
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "site_status") return site;
    if (command === "cycle_recipes") return [];
    if (command === "cycle_readiness") return report;
    throw new Error(`未声明的桌面调用：${command}`);
  });
});

describe("顶栏站点模式标识", () => {
  it("site://status 依次推送真机、离线样本集、离线合成图 → 标识随之切换且指向离线控制台", async () => {
    site = siteStatus();
    render(<MemoryRouter initialEntries={["/guide"]}><Routes><Route element={<AppLayout />}><Route path="/guide" element={<div />} /></Route></Routes></MemoryRouter>);
    const header = within(screen.getByRole("banner"));
    expect(await header.findByRole("link", { name: "站点模式：真机" })).toHaveAttribute("href", "/offline");
    act(() => listeners.get("site://status")!({ payload: siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet("壳体样本") }) }));
    expect(header.getByRole("link", { name: "站点模式：离线 · 样本集 壳体样本" })).toBeVisible();
    act(() => listeners.get("site://status")!({ payload: siteStatus({ mode: "offline", source: "synthetic", measurement: "simulated" }) }));
    expect(header.getByRole("link", { name: "站点模式：离线 · 合成图" })).toBeVisible();
  });
});

describe("离线控制台工况", () => {
  it.each([
    ["sampleSet", ["normal", "lostFrame", "countMismatch", "motionTimeout", "ackMismatch"]],
    ["synthetic", ["normal", "excursion", "gap", "lostFrame", "locateFail", "countMismatch", "motionTimeout", "ackMismatch", "random"]],
  ] as const)("离线来源 %s → 工况只列该来源后端接受的工况", async (source, expected) => {
    site = siteStatus({ mode: "offline", source, measurement: source === "sampleSet" ? "lyFlow" : "simulated" });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const select = await screen.findByRole("combobox", { name: "离线工况" });
    expect(within(select).getAllByRole("option").map(o => (o as HTMLOptionElement).value)).toEqual(expected);
  });
});

describe("离线控制台配方下拉", () => {
  it("样本集下有不可运行的配方 → 只有可运行配方可选并默认选中第一个可运行配方", async () => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet() });
    report = readiness({ recipes: [recipeOption("A", false), recipeOption("B"), recipeOption("C")] });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const select = await screen.findByRole("combobox", { name: "离线配方" });
    await waitFor(() => expect(select).toHaveValue("B"));
    const options = within(select).getAllByRole("option") as HTMLOptionElement[];
    expect(options.filter(o => !o.disabled).map(o => o.value)).toEqual(["B", "C"]);
  });

  it("所选配方暂不可运行 → 清空选择而不改选别的，恢复可运行后回到所选", async () => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet() });
    report = readiness({ recipes: [recipeOption("A"), recipeOption("B")] });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const select = await screen.findByRole("combobox", { name: "离线配方" });
    await waitFor(() => expect(select).toHaveValue("A"));
    await userEvent.selectOptions(select, "B");
    report = readiness({ recipes: [recipeOption("A"), recipeOption("B", false)] });
    await act(async () => listeners.get("site://status")!({ payload: site }));
    await waitFor(() => expect(select).toHaveValue(""));
    report = readiness({ recipes: [recipeOption("A"), recipeOption("B")] });
    await act(async () => listeners.get("site://status")!({ payload: site }));
    await waitFor(() => expect(select).toHaveValue("B"));
  });
});

describe("开工就绪报告刷新", () => {
  it("上次查询未返回 → 定时刷新不再发新查询", async () => {
    vi.useFakeTimers();
    try {
      let calls = 0;
      vi.mocked(invoke).mockImplementation(async (command) => {
        if (command === "cycle_readiness") { calls++; return new Promise(() => {}); }
        return null;
      });
      renderHook(() => useReadiness());
      await act(async () => { await vi.advanceTimersByTimeAsync(7000); });
      expect(calls).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

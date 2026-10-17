import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import OfflinePage from "../src/pages/OfflinePage";
import { useReadiness } from "../src/features/cycle/api";
import InspectionReadiness from "../src/features/cycle/components/InspectionReadiness";
import type { Readiness, SiteStatus } from "../src/features/cycle/types";
import { readiness, recipeOption, sampleSet, siteStatus, snapshot } from "./fixtures";

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

describe("离线模式工况", () => {
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

describe("离线模式配方下拉", () => {
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
  it("就绪报告成功后刷新失败再恢复 → 旧报告仅供参考且成功前不再显示可开工", async () => {
    let failure = false;
    report = readiness();
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "cycle_readiness") {
        if (failure) throw new Error("连接已关闭");
        return report;
      }
      return null;
    });
    function Probe() {
      const { readiness: data, status, error, refresh } = useReadiness();
      return <><InspectionReadiness snapshot={snapshot()} readiness={data} status={status} error={error} /><button onClick={() => void refresh()}>刷新检查</button></>;
    }
    render(<MemoryRouter><Probe /></MemoryRouter>);
    expect(await screen.findByText("就绪，等待工件")).toBeVisible();
    failure = true;
    await userEvent.click(screen.getByRole("button", { name: "刷新检查" }));
    expect(await screen.findByText("开工条件更新失败")).toBeVisible();
    expect(screen.getByText("下列条件为上次报告，仅供参考")).toBeVisible();
    expect(screen.queryByText("就绪，等待工件")).not.toBeInTheDocument();
    await userEvent.click(screen.getByText("查看更新错误"));
    expect(screen.getByText("连接已关闭")).toBeVisible();
    failure = false;
    await userEvent.click(screen.getByRole("button", { name: "刷新检查" }));
    expect(await screen.findByText("就绪，等待工件")).toBeVisible();
    expect(screen.queryByText("连接已关闭")).not.toBeInTheDocument();
  });

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

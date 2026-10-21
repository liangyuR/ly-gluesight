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
    const previous = listeners.get(event);
    listeners.set(event, payload => { previous?.(payload); (listener as (event: { payload: unknown }) => void)(payload); });
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
  it("外部样本 ID 已导入 → 确认使用已有副本后才能进入，取消保留原选择", async () => {
    site = siteStatus({ mode: "real", last: null });
    const original = vi.mocked(invoke).getMockImplementation()!;
    const managed = "C:/ProgramData/ly/GlueSight-dev/samples/one";
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "site_pick_dir") return "C:/external/one";
      if (command === "site_preview_sample_set") {
        const dir = (args as { dir: string }).dir;
        return { ...sampleSet(), dir, managedDir: dir === managed ? null : managed };
      }
      if (command === "site_enter_offline") return { ...site, mode: "offline", source: "sampleSet", sampleSet: { ...sampleSet(), dir: managed } };
      return original(command, args, options);
    });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    await userEvent.click(await screen.findByRole("button", { name: "选择样本集目录" }));
    expect(screen.getByRole("button", { name: "进入离线" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "取消本次选择" }));
    expect(screen.getByText("未选择")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "选择样本集目录" }));
    await userEvent.click(screen.getByRole("button", { name: "使用已有副本" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "进入离线" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "进入离线" }));
    expect(invoke).toHaveBeenCalledWith("site_enter_offline", { source: { kind: "sampleSet", dir: managed } });
  });
  it.each([
    ["sampleSet", ["normal", "lostFrame", "countMismatch", "motionTimeout", "ackMismatch"]],
    ["synthetic", ["normal", "excursion", "gap", "lostFrame", "locateFail", "locateFailGap", "countMismatch", "motionTimeout", "ackMismatch", "random"]],
  ] as const)("离线来源 %s → 工况只列该来源后端接受的工况", async (source, expected) => {
    site = siteStatus({ mode: "offline", source, measurement: source === "sampleSet" ? "lyFlow" : "simulated" });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const select = await screen.findByRole("combobox", { name: "离线工况" });
    expect(Array.from(select.closest(".qs-select")!.querySelector("select")!.options).map(o => o.value)).toEqual(expected);
  });
});

describe("离线模式配方下拉", () => {
  it("生产版本未发布但运行候选调试 → 发布阻塞只属于回放区域，候选入口与停止仍可用", async () => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet() });
    report = readiness({ ready: false, recipes: [recipeOption("A", false)], items: readiness().items.map(item => item.key === "recipe"
      ? { ...item, state: "blocked", detail: "配方未发布", target: "workspace" } : item) });
    const invokeBeforeStop = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => command === "offline_stop" ? undefined : invokeBeforeStop(command, args, options));
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const published = within(await screen.findByRole("region", { name: "已发布配方离线回放" }));
    expect(await published.findByText("配方未发布")).toBeVisible();
    expect(published.getByRole("button", { name: "运行一件" })).toBeDisabled();
    const candidate = within(screen.getByRole("region", { name: "候选配方离线试运行" }));
    expect(candidate.getByRole("link", { name: "前往在线检测调试候选" })).toHaveAttribute("href", "/inspect");
    expect(candidate.queryByText("配方未发布")).not.toBeInTheDocument();
    site = { ...site, running: true, message: "候选调试运行中" };
    await act(async () => listeners.get("site://status")!({ payload: site }));
    const stop = within(screen.getByRole("region", { name: "当前离线运行" })).getByRole("button", { name: "立即停止" });
    expect(stop).toBeVisible();
    expect(stop).toBeEnabled();
    await userEvent.click(stop);
    expect(invoke).toHaveBeenCalledWith("offline_stop", { now: true });
  });

  it("样本集含未发布候选与不可运行配方 → 回放只列已发布版本且默认选中首个可运行版本", async () => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet() });
    report = readiness({ recipes: [recipeOption("candidate", true, { published: false }), recipeOption("A", false), recipeOption("B"), recipeOption("C")] });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const select = await screen.findByLabelText("离线配方", { selector: "select" }) as HTMLSelectElement;
    await waitFor(() => expect(select).toHaveValue("B"));
    const options = Array.from(select.options);
    expect(options.some(o => o.value === "candidate")).toBe(false);
    expect(options.filter(o => !o.disabled).map(o => o.value)).toEqual(["B", "C"]);
  });

  it("所选配方暂不可运行 → 清空选择而不改选别的，恢复可运行后回到所选", async () => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet() });
    report = readiness({ recipes: [recipeOption("A"), recipeOption("B")] });
    render(<MemoryRouter><OfflinePage /></MemoryRouter>);
    const select = await screen.findByLabelText("离线配方", { selector: "select" });
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

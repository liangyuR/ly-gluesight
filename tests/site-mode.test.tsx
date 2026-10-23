import { act, render, screen, waitFor, within } from "@testing-library/react";
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


});

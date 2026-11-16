import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useReadiness } from "../src/features/cycle/api";
import InspectionReadiness from "../src/features/cycle/components/InspectionReadiness";
import type { Readiness } from "../src/features/cycle/types";
import { readiness, snapshot } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

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
    if (command === "cycle_recipes") return [];
    if (command === "cycle_readiness") return report;
    throw new Error(`未声明的桌面调用：${command}`);
  });
});

describe("开工就绪报告刷新", () => {
  it("授权失效但设备仍就绪 → 刷新后显示无法开工并提供授权处理入口", async () => {
    report = readiness();
    function Probe() {
      const { readiness: data, status, refresh } = useReadiness();
      return <><InspectionReadiness snapshot={snapshot()} readiness={data} status={status} /><button onClick={() => void refresh()}>刷新检查</button></>;
    }
    render(<MemoryRouter><Probe /></MemoryRouter>);
    expect(await screen.findByText("就绪，等待工件")).toBeVisible();
    report = { ...report, ready: false, items: [...report.items, {
      key: "license", title: "软件授权", state: "blocked", detail: "授权已到期", hint: null, target: "license",
    }] };
    await userEvent.click(screen.getByRole("button", { name: "刷新检查" }));
    expect(await screen.findByText("无法开工 · 1 项阻塞")).toBeVisible();
    expect(screen.queryByText("就绪，等待工件")).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "去软件授权" })).toHaveAttribute("href", "/license");
  });

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

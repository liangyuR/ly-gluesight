import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SimControls from "../src/features/cycle/components/SimControls";
import { cycleApi } from "../src/features/cycle/api";
import { plcApi } from "../src/features/plc/api";
import type { RecipeSummary, SimStatus } from "../src/features/cycle/types";
import { deferred, summary } from "./fixtures";
import { plcConfig } from "./plc-fixtures";

let recipes: RecipeSummary[], status: SimStatus, connected: boolean, connectedSince: number;
const plcEvents = new Map<string, (payload: unknown) => void>();
vi.mock("../src/features/cycle/api", () => ({ cycleApi: { simStart: vi.fn(), simStop: vi.fn() }, useRecipes: () => recipes, useSimStatus: () => status }));
vi.mock("../src/features/plc/api", () => ({
  plcApi: { getConfig: vi.fn() },
  usePlcStatus: () => ({ state: connected ? "connected" : "disconnected", since: connectedSince }),
  subscribe: (event: string, callback: (payload: unknown) => void) => {
    plcEvents.set(event, callback);
    return () => { plcEvents.delete(event); };
  },
}));
beforeEach(() => {
  recipes = [summary(), { ...summary(), id: "B" }]; connected = true; connectedSince = 1; plcEvents.clear();
  status = { running: false, continuous: false, parts: 0, message: "就绪" };
  vi.mocked(plcApi.getConfig).mockReset().mockResolvedValue(plcConfig()); vi.mocked(cycleApi.simStart).mockReset().mockResolvedValue(undefined); vi.mocked(cycleApi.simStop).mockReset().mockResolvedValue(undefined);
});
async function show() { const page = render(<SimControls/>); await screen.findByRole("button", { name: "运行一件" }); return page; }
describe("离线模拟节拍控制", () => {

  it("启动请求中锁定所有启动操作，失败保留选择并可以重试", async () => {
    const request = deferred<void>(); vi.mocked(cycleApi.simStart).mockReturnValueOnce(request.promise);
    await show(); const button = screen.getByRole("button", { name: "运行一件" }); fireEvent.click(button); fireEvent.click(button);
    expect(cycleApi.simStart).toHaveBeenCalledTimes(1); expect(screen.getByRole("button", { name: "连续运行" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "模拟配方" })).toBeDisabled(); await act(async () => request.reject(new Error("启动失败")));
    expect(await screen.findByText("Error: 启动失败")).toBeVisible(); expect(button).toBeEnabled();
  });
  it("运行中禁止换型，连续运行可本件后停止，停止失败可重试", async () => {
    status = { running: true, continuous: true, parts: 2, message: "检测中" }; vi.mocked(cycleApi.simStop).mockRejectedValueOnce(new Error("停止失败"));
    await show(); expect(screen.getByRole("combobox", { name: "模拟配方" })).toBeDisabled(); expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "本件后停止" })); expect(await screen.findByText("Error: 停止失败")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "本件后停止" })); expect(cycleApi.simStop).toHaveBeenCalledTimes(2);
  });

  it("实际 PLC 协议不给模拟入口，配置读取失败显示原因", async () => {
    const actual = plcConfig(); actual.connection.protocol = "modbusTcp"; vi.mocked(plcApi.getConfig).mockResolvedValueOnce(actual);
    const page = render(<SimControls/>); await screen.findByText(/模拟节拍需要/); expect(screen.queryByRole("button", { name: "运行一件" })).toBeNull();
    page.unmount(); vi.mocked(plcApi.getConfig).mockRejectedValueOnce(new Error("配置读取失败")); render(<SimControls compact/>);
    expect(await screen.findByText("Error: 配置读取失败")).toBeVisible();
  });

  it("快速重连仍显示已连接时重新确认协议，未确认前禁止发出模拟启动", async () => {
    const page = await show(); const request = deferred<Awaited<ReturnType<typeof plcApi.getConfig>>>(); vi.mocked(plcApi.getConfig).mockReturnValueOnce(request.promise);
    connectedSince = 2; page.rerender(<SimControls/>);
    expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled();
    expect(screen.getByText("正在确认模拟 PLC 配置…")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "运行一件" })); expect(cycleApi.simStart).not.toHaveBeenCalled();
    const actual = plcConfig(); actual.connection.protocol = "modbusTcp"; await act(async () => request.resolve(actual));
    expect(screen.queryByRole("button", { name: "运行一件" })).toBeNull(); expect(screen.getByText(/模拟节拍需要/)).toBeVisible();
    expect(plcApi.getConfig).toHaveBeenCalledTimes(2);
  });

  it.each(["success", "error"])("旧 PLC 配置的 %s 不覆盖最新协议判断", async kind => {
    const old = deferred<Awaited<ReturnType<typeof plcApi.getConfig>>>(); vi.mocked(plcApi.getConfig).mockReturnValueOnce(old.promise);
    const page = render(<SimControls/>); connected = false; page.rerender(<SimControls/>);
    expect(await screen.findByRole("button", { name: "运行一件" })).toBeDisabled();
    const actual = plcConfig(); actual.connection.protocol = "mc";
    await act(async () => kind === "success" ? old.resolve(actual) : old.reject(new Error("过期配置错误")));
    expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled(); expect(screen.queryByText("Error: 过期配置错误")).toBeNull();
  });

  it("停止等待不能重复提交，真实连续状态结束后恢复换型", async () => {
    status = { running: true, continuous: true, parts: 2, message: "正在本件" }; const request = deferred<void>(); vi.mocked(cycleApi.simStop).mockReturnValueOnce(request.promise);
    const page = await show(); const stop = screen.getByRole("button", { name: "本件后停止" }); fireEvent.click(stop); fireEvent.click(stop);
    expect(cycleApi.simStop).toHaveBeenCalledTimes(1); expect(stop).toBeDisabled();
    await act(async () => request.resolve()); expect(screen.getByRole("button", { name: "连续运行" })).toBeDisabled();
    expect(stop).toBeDisabled(); expect(screen.getByText("已请求停止，等待本件完成…")).toBeVisible();
    fireEvent.click(stop); expect(cycleApi.simStop).toHaveBeenCalledTimes(1);
    status = { running: false, continuous: false, parts: 3, message: "本件已结束" }; page.rerender(<SimControls/>);
    expect(screen.getByRole("combobox", { name: "模拟配方" })).toBeEnabled(); expect(screen.getByRole("button", { name: "连续运行" })).toBeEnabled(); expect(stop).toBeDisabled();
    expect(screen.queryByText("已请求停止，等待本件完成…")).toBeNull(); expect(screen.getByText("本件已结束")).toBeVisible();
  });

});

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  recipes = [summary(), { ...summary(), id: "FOLLOW", mode: "follow" }]; connected = true; connectedSince = 1; plcEvents.clear();
  status = { running: false, continuous: false, parts: 0, message: "就绪" };
  vi.mocked(plcApi.getConfig).mockReset().mockResolvedValue(plcConfig()); vi.mocked(cycleApi.simStart).mockReset().mockResolvedValue(undefined); vi.mocked(cycleApi.simStop).mockReset().mockResolvedValue(undefined);
});
async function show() { const page = render(<SimControls/>); await screen.findByRole("button", { name: "运行一件" }); return page; }
describe("离线模拟节拍控制", () => {
  it("选配方和工况，单件/连续调用包含准确参数", async () => {
    await show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟工况" }), "gap");
    await userEvent.click(screen.getByRole("button", { name: "运行一件" })); expect(cycleApi.simStart).toHaveBeenCalledWith("A", "gap", false);
    await userEvent.click(screen.getByRole("button", { name: "连续运行" })); expect(cycleApi.simStart).toHaveBeenLastCalledWith("A", "gap", true);
  });
  it("切到随动配方后重置不支持的工况，提供胶宽不足", async () => {
    await show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟工况" }), "countMismatch");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟配方" }), "FOLLOW");
    expect(screen.getByRole("combobox", { name: "模拟工况" })).toHaveValue("normal");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟工况" }), "narrow");
    await userEvent.click(screen.getByRole("button", { name: "运行一件" })); expect(cycleApi.simStart).toHaveBeenCalledWith("FOLLOW", "narrow", false);
  });
  it("未连接模拟 PLC 时禁用启动，显示连接前置条件", async () => {
    connected = false; await show(); expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled(); expect(screen.getByText("请先连接模拟 PLC")).toBeVisible();
  });
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
  it("删除当前配方后选择仍存在的配方，列表为空禁用启动", async () => {
    const page = await show(); recipes = recipes.slice(1); page.rerender(<SimControls/>);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "模拟配方" })).toHaveValue("FOLLOW"));
    recipes = []; page.rerender(<SimControls/>); expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled();
  });
  it("实际 PLC 协议不给模拟入口，配置读取失败显示原因", async () => {
    const actual = plcConfig(); actual.connection.protocol = "modbusTcp"; vi.mocked(plcApi.getConfig).mockResolvedValueOnce(actual);
    const page = render(<SimControls/>); await screen.findByText(/模拟节拍需要/); expect(screen.queryByRole("button", { name: "运行一件" })).toBeNull();
    page.unmount(); vi.mocked(plcApi.getConfig).mockRejectedValueOnce(new Error("配置读取失败")); render(<SimControls compact/>);
    expect(await screen.findByText("Error: 配置读取失败")).toBeVisible();
  });

  it("配置读取失败可重试，恢复后清除错误并允许模拟", async () => {
    vi.mocked(plcApi.getConfig).mockRejectedValueOnce(new Error("模拟配置暂时不可读")); render(<SimControls compact/>);
    expect(await screen.findByText("Error: 模拟配置暂时不可读")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "重新加载模拟配置" }));
    expect(await screen.findByRole("button", { name: "连续运行" })).toBeEnabled();
    expect(screen.queryByText("Error: 模拟配置暂时不可读")).toBeNull(); await userEvent.click(screen.getByRole("button", { name: "连续运行" }));
    expect(cycleApi.simStart).toHaveBeenCalledWith("A", "normal", true);
  });

  it("PLC 状态变化重新确认协议，确认期间不能用旧模拟配置启动", async () => {
    const page = await show(); const request = deferred<Awaited<ReturnType<typeof plcApi.getConfig>>>(); vi.mocked(plcApi.getConfig).mockReturnValueOnce(request.promise);
    connected = false; page.rerender(<SimControls/>);
    expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled(); expect(screen.getByRole("combobox", { name: "模拟工况" })).toBeDisabled();
    const actual = plcConfig(); actual.connection.protocol = "s7"; await act(async () => request.resolve(actual));
    expect(screen.queryByRole("button", { name: "运行一件" })).toBeNull(); expect(screen.getByText(/模拟节拍需要/)).toBeVisible();
    expect(cycleApi.simStart).not.toHaveBeenCalled();
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

  it("配置保存通知不依赖连接状态变化，重新读取当前协议", async () => {
    await show(); const actual = plcConfig(); actual.connection.protocol = "s7"; vi.mocked(plcApi.getConfig).mockResolvedValueOnce(actual);
    await act(async () => plcEvents.get("plc://config")?.(null));
    expect(screen.queryByRole("button", { name: "运行一件" })).toBeNull(); expect(screen.getByText(/模拟节拍需要/)).toBeVisible();
    expect(plcApi.getConfig).toHaveBeenCalledTimes(2);
  });

  it("正常轮询不会反复读取配置，相同连接内保持工况选择", async () => {
    const page = await show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟工况" }), "gap");
    page.rerender(<SimControls/>); page.rerender(<SimControls/>);
    expect(plcApi.getConfig).toHaveBeenCalledTimes(1); expect(screen.getByRole("combobox", { name: "模拟工况" })).toHaveValue("gap");
  });

  it("配置保存后可从外部 PLC 切回模拟器，较早的配置响应不能撤销新配置", async () => {
    const actual = plcConfig(); actual.connection.protocol = "modbusTcp"; vi.mocked(plcApi.getConfig).mockResolvedValueOnce(actual);
    const page = render(<SimControls compact/>); await waitFor(() => expect(plcApi.getConfig).toHaveBeenCalledTimes(1));
    const old = deferred<Awaited<ReturnType<typeof plcApi.getConfig>>>(); vi.mocked(plcApi.getConfig).mockReturnValueOnce(old.promise);
    connectedSince = 2; page.rerender(<SimControls compact/>);
    expect(screen.queryByRole("button", { name: "运行一件" })).toBeNull();
    await act(async () => plcEvents.get("plc://config")?.(null));
    expect(await screen.findByRole("button", { name: "运行一件" })).toBeEnabled();
    await act(async () => old.resolve(actual));
    expect(screen.getByRole("button", { name: "运行一件" })).toBeEnabled();
    expect(plcApi.getConfig).toHaveBeenCalledTimes(3);
  });

  it.each(["success", "error"])("旧 PLC 配置的 %s 不覆盖最新协议判断", async kind => {
    const old = deferred<Awaited<ReturnType<typeof plcApi.getConfig>>>(); vi.mocked(plcApi.getConfig).mockReturnValueOnce(old.promise);
    const page = render(<SimControls/>); connected = false; page.rerender(<SimControls/>);
    expect(await screen.findByRole("button", { name: "运行一件" })).toBeDisabled();
    const actual = plcConfig(); actual.connection.protocol = "mc";
    await act(async () => kind === "success" ? old.resolve(actual) : old.reject(new Error("过期配置错误")));
    expect(screen.getByRole("button", { name: "运行一件" })).toBeDisabled(); expect(screen.queryByText("Error: 过期配置错误")).toBeNull();
  });

  it("启动失败后保留连续工况，重试发送相同选择", async () => {
    await show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟工况" }), "random");
    vi.mocked(cycleApi.simStart).mockRejectedValueOnce(new Error("连续启动失败"));
    await userEvent.click(screen.getByRole("button", { name: "连续运行" })); expect(await screen.findByText("Error: 连续启动失败")).toBeVisible();
    expect(screen.getByRole("combobox", { name: "模拟工况" })).toHaveValue("random");
    await userEvent.click(screen.getByRole("button", { name: "连续运行" })); expect(cycleApi.simStart).toHaveBeenLastCalledWith("A", "random", true);
    expect(cycleApi.simStart).toHaveBeenCalledTimes(2); expect(screen.queryByText("Error: 连续启动失败")).toBeNull();
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

  it("单件运行和过期 continuous 标志都不给停止入口", async () => {
    status = { running: true, continuous: false, parts: 0, message: "单件中" }; const page = await show();
    expect(screen.getByRole("button", { name: "本件后停止" })).toBeDisabled();
    status = { ...status, running: false, continuous: true }; page.rerender(<SimControls/>);
    expect(screen.getByRole("button", { name: "本件后停止" })).toBeDisabled(); expect(cycleApi.simStop).not.toHaveBeenCalled();
  });

  it("切换 PLC 状态后旧启动错误不覆盖当前连接提示", async () => {
    const page = await show(); const request = deferred<void>(); vi.mocked(cycleApi.simStart).mockReturnValueOnce(request.promise);
    fireEvent.click(screen.getByRole("button", { name: "连续运行" })); connected = false; page.rerender(<SimControls/>);
    await waitFor(() => expect(screen.getByText("请先连接模拟 PLC")).toBeVisible());
    await act(async () => request.reject(new Error("过期启动失败")));
    expect(screen.queryByText("Error: 过期启动失败")).toBeNull(); expect(screen.getByText("请先连接模拟 PLC")).toBeVisible();
  });

  it.each(["start", "stop"])("%s 请求在卸载后结束不会发送额外停止或新启动", async operation => {
    if (operation === "stop") status = { running: true, continuous: true, parts: 0, message: "运行中" };
    const request = deferred<void>(); vi.mocked(operation === "start" ? cycleApi.simStart : cycleApi.simStop).mockReturnValueOnce(request.promise);
    const page = await show(); fireEvent.click(screen.getByRole("button", { name: operation === "start" ? "连续运行" : "本件后停止" })); page.unmount();
    await act(async () => request.reject(new Error("已离开页面的结果")));
    expect(cycleApi.simStart).toHaveBeenCalledTimes(operation === "start" ? 1 : 0); expect(cycleApi.simStop).toHaveBeenCalledTimes(operation === "stop" ? 1 : 0);
  });
});

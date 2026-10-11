import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PlcPage from "../src/features/plc/pages/PlcPage";
import { plcApi } from "../src/features/plc/api";
import type { PlcHandshakeState, PlcStatus, PointValue } from "../src/features/plc/types";
import { deferred } from "./fixtures";
import { plcConfig } from "./plc-fixtures";

let status: PlcStatus;
let values: Record<string, PointValue>;
let operationLock: string;
let handshake: PlcHandshakeState | null;
vi.mock("../src/features/plc/api", () => ({
  plcApi: { getConfig: vi.fn(), saveConfig: vi.fn(), connect: vi.fn(), disconnect: vi.fn(), writePoint: vi.fn(), checkAddress: vi.fn(), s7Phase1Template: vi.fn() },
  usePlcStatus: () => status, usePlcValues: () => values,
  usePlcOperationState: () => ({ blockedReason: operationLock, handshake }),
}));
beforeEach(() => {
  operationLock = "";
  handshake = null;
  status = { state: "disconnected", message: "模拟器", since: 1, lastPoll: null, cycleMs: null, pollCount: 0, errorCount: 0 };
  values = { speed: { value: 123, error: null, ts: 1 } };
  vi.mocked(plcApi.getConfig).mockResolvedValue(plcConfig());
  for (const fn of [plcApi.saveConfig, plcApi.connect, plcApi.disconnect, plcApi.writePoint]) vi.mocked(fn).mockResolvedValue(undefined);
  vi.mocked(plcApi.checkAddress).mockResolvedValue("地址有效");
});

describe("一期 S7 点表与生产操作保护", () => {

  it("生产锁定配置、断开和普通点手写，已打开点位编辑可取消但不能提交", async () => {
    status.state = "connected"; const page = await show();
    await userEvent.click(row().getByTitle("编辑"));
    operationLock = "生产事务未结束"; page.rerender(<PlcPage />);
    expect(screen.getByRole("button", { name: "断开" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "载入一期 S7 点表" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "新增点位" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "确定" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "名称" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(row().getByTitle("写入")).toBeDisabled();
    expect(plcApi.saveConfig).not.toHaveBeenCalled(); expect(plcApi.disconnect).not.toHaveBeenCalled();
  });

  it.each(["disconnected", "error", "connecting"] as const)("未结事务且 %s 时使用已有配置重连，不应用无效草稿", async state => {
    const request = deferred<void>(); vi.mocked(plcApi.connect).mockReturnValueOnce(request.promise);
    const page = await show();
    fireEvent.change(screen.getByRole("spinbutton", { name: "日志保留天数（0 为永久）" }), { target: { value: "" } });
    status = { ...status, state }; operationLock = "生产事务未结束"; page.rerender(<PlcPage />);
    const reconnect = screen.getByRole("button", { name: "使用已保存配置重连" });
    expect(reconnect).toBeEnabled(); expect(screen.getByText(/当前草稿不会保存或应用/)).toBeVisible();
    expect(screen.getByRole("button", { name: "保存配置" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "还原" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "IP / 主机名" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "载入一期 S7 点表" })).toBeDisabled();
    fireEvent.click(reconnect); fireEvent.click(reconnect);
    expect(plcApi.connect).toHaveBeenCalledTimes(1); expect(reconnect).toBeDisabled();
    expect(plcApi.saveConfig).not.toHaveBeenCalled(); expect(plcApi.disconnect).not.toHaveBeenCalled();
    await act(async () => request.resolve());
    expect(screen.getByText("未保存")).toBeVisible();
    status = { ...status, state: "connected" }; page.rerender(<PlcPage />);
    expect(screen.getByRole("button", { name: "断开" })).toBeDisabled();
  });

  it("手写弹窗打开后开始生产，不发送保留的写请求", async () => {
    status.state = "connected"; const page = await show();
    await userEvent.click(row().getByTitle("写入"));
    operationLock = "生产事务未结束"; page.rerender(<PlcPage />);
    const dialog = within(screen.getByRole("dialog"));
    expect(dialog.getByRole("button", { name: "写入" })).toBeDisabled();
    fireEvent.keyDown(dialog.getByRole("textbox"), { key: "Enter" });
    expect(plcApi.writePoint).not.toHaveBeenCalled();
    await userEvent.click(dialog.getByRole("button", { name: "取消" }));
  });

  it("S7 握手和心跳始终禁止手写，普通点在空闲可写", async () => {
    const config = plcConfig(); config.connection.protocol = "s7"; config.connection.port = 102;
    config.points = [
      { ...config.points[0], id: "done", name: "结果有效", address: "DB1.DBX0.0", tags: ["done"], dataType: "bool" },
      { ...config.points[0], id: "beat", name: "通讯心跳", address: "DB1.DBX0.1", dataType: "bool" },
      { ...config.points[0], address: "DB1.DBD4" },
    ];
    config.heartbeat.pointId = "beat"; status.state = "connected";
    vi.mocked(plcApi.getConfig).mockResolvedValueOnce(config); await show();
    for (const name of ["结果有效", "通讯心跳"]) {
      expect(within(screen.getByText(name).closest("tr")!).getByRole("button", { name: "写入" })).toBeDisabled();
    }
    expect(row().getByTitle("写入")).toBeEnabled();
  });

  it("保存等待时生产开始，即使保存返回也不继续连接", async () => {
    const request = deferred<void>(); vi.mocked(plcApi.saveConfig).mockReturnValueOnce(request.promise);
    const page = await show(); editRetention(7);
    fireEvent.click(screen.getByRole("button", { name: "连接" }));
    operationLock = "生产事务未结束"; page.rerender(<PlcPage />);
    await act(async () => request.resolve());
    expect(plcApi.connect).not.toHaveBeenCalled();
  });
});
async function show() { const page = render(<PlcPage />); await screen.findByRole("button", { name: "保存配置" }); return page; }
const editRetention = (value: number) => fireEvent.change(screen.getByRole("spinbutton", { name: "日志保留天数（0 为永久）" }), { target: { value: String(value) } });
const row = () => within(screen.getByText("速度").closest("tr")!);

describe("PLC 通讯页面的基本操作", () => {

  it("保存失败不连接且保留草稿，再次连接可以重试", async () => {
    vi.mocked(plcApi.saveConfig).mockRejectedValueOnce(new Error("保存失败"));
    await show(); editRetention(7); await userEvent.click(screen.getByRole("button", { name: "连接" }));
    expect(await screen.findByText("Error: 保存失败")).toBeVisible(); expect(plcApi.connect).not.toHaveBeenCalled();
    expect(screen.getByText("未保存")).toBeVisible(); await userEvent.click(screen.getByRole("button", { name: "连接" }));
    expect(plcApi.connect).toHaveBeenCalledTimes(1);
  });
  it("连接和保存等待期间禁止重复操作与编辑", async () => {
    const request = deferred<void>(); vi.mocked(plcApi.saveConfig).mockReturnValueOnce(request.promise);
    await show(); editRetention(7); const connect = screen.getByRole("button", { name: "连接" });
    fireEvent.click(connect); fireEvent.click(connect);
    expect(plcApi.saveConfig).toHaveBeenCalledTimes(1); expect(connect).toBeDisabled();
    expect(screen.getByRole("spinbutton", { name: "日志保留天数（0 为永久）" })).toBeDisabled();
    await act(async () => request.resolve()); await waitFor(() => expect(connect).toBeEnabled()); expect(plcApi.connect).toHaveBeenCalledTimes(1);
  });

  it("JSON 替换地址表后清除被删除心跳点的引用", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "导入/导出" }));
    const dialog = within(screen.getByRole("dialog")); fireEvent.change(dialog.getByRole("textbox"), { target: { value: "[]" } });
    await userEvent.click(dialog.getByRole("button", { name: "应用到地址表" }));
    expect(screen.getByRole("combobox", { name: "心跳点位" })).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ points: [], heartbeat: expect.objectContaining({ pointId: null }) }));
  });

  it("保存中的连接操作在页面卸载后不再发起连接", async () => {
    const request = deferred<void>(); vi.mocked(plcApi.saveConfig).mockReturnValueOnce(request.promise);
    const page = await show(); editRetention(7); fireEvent.click(screen.getByRole("button", { name: "连接" }));
    page.unmount(); await act(async () => request.resolve()); expect(plcApi.connect).not.toHaveBeenCalled();
  });

});

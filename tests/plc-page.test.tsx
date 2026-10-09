import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PlcPage from "../src/features/plc/pages/PlcPage";
import { plcApi } from "../src/features/plc/api";
import type { PlcStatus, PointValue, ProtocolKind } from "../src/features/plc/types";
import { deferred } from "./fixtures";
import { plcConfig } from "./plc-fixtures";

let status: PlcStatus;
let values: Record<string, PointValue>;
vi.mock("../src/features/plc/api", () => ({
  plcApi: { getConfig: vi.fn(), saveConfig: vi.fn(), connect: vi.fn(), disconnect: vi.fn(), writePoint: vi.fn(), checkAddress: vi.fn() },
  usePlcStatus: () => status, usePlcValues: () => values,
}));
beforeEach(() => {
  status = { state: "disconnected", message: "模拟器", since: 1, lastPoll: null, cycleMs: null, pollCount: 0, errorCount: 0 };
  values = { speed: { value: 123, error: null, ts: 1 } };
  vi.mocked(plcApi.getConfig).mockResolvedValue(plcConfig());
  for (const fn of [plcApi.saveConfig, plcApi.connect, plcApi.disconnect, plcApi.writePoint]) vi.mocked(fn).mockResolvedValue(undefined);
  vi.mocked(plcApi.checkAddress).mockResolvedValue("地址有效");
});
async function show() { const page = render(<PlcPage />); await screen.findByRole("button", { name: "保存配置" }); return page; }
const editRetention = (value: number) => fireEvent.change(screen.getByRole("spinbutton", { name: "日志保留天数（0 为永久）" }), { target: { value: String(value) } });
const row = () => within(screen.getByText("速度").closest("tr")!);

describe("PLC 通讯页面的基本操作", () => {
  it("配置读取失败显示重试入口，重试恢复完整表单", async () => {
    vi.mocked(plcApi.getConfig).mockRejectedValueOnce(new Error("配置读取失败"));
    render(<PlcPage />); expect(await screen.findByRole("alert")).toHaveTextContent("配置读取失败");
    await userEvent.click(screen.getByRole("button", { name: "重新加载" }));
    expect(await screen.findByRole("button", { name: "保存配置" })).toBeDisabled();
  });
  it("编辑产生草稿，还原恢复保存值与心跳，不写后端", async () => {
    await show(); editRetention(5); await userEvent.selectOptions(screen.getByRole("combobox", { name: "心跳点位" }), "");
    expect(screen.getByRole("button", { name: "保存配置" })).toBeEnabled();
    await userEvent.click(screen.getByRole("button", { name: "还原" }));
    expect(screen.getByRole("spinbutton", { name: "日志保留天数（0 为永久）" })).toHaveValue(30);
    expect(screen.getByRole("combobox", { name: "心跳点位" })).toHaveValue("speed"); expect(plcApi.saveConfig).not.toHaveBeenCalled();
  });
  it("带草稿连接先保存再连接，保存成功清除未保存提示", async () => {
    await show(); editRetention(7); await userEvent.click(screen.getByRole("button", { name: "连接" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ logRetentionDays: 7 }));
    expect(plcApi.connect).toHaveBeenCalledTimes(1);
    expect(vi.mocked(plcApi.saveConfig).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(plcApi.connect).mock.invocationCallOrder[0]);
    expect(screen.queryByText("未保存")).toBeNull();
  });
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
  it("已连接可断开，断开失败显示原因", async () => {
    status.state = "connected"; vi.mocked(plcApi.disconnect).mockRejectedValueOnce(new Error("断开失败"));
    await show(); await userEvent.click(screen.getByRole("button", { name: "断开" }));
    expect(plcApi.disconnect).toHaveBeenCalledTimes(1); expect(await screen.findByText("Error: 断开失败")).toBeVisible();
  });
  it("只有保存过且已连接的可写点可打开写入弹窗", async () => {
    status.state = "connected"; await show();
    expect(row().getByTitle("写入")).toBeEnabled(); await userEvent.click(row().getByTitle("写入"));
    const dialog = within(screen.getByRole("dialog"));
    fireEvent.change(dialog.getByRole("textbox"), { target: { value: "42" } });
    await userEvent.click(dialog.getByRole("button", { name: "写入" })); expect(plcApi.writePoint).toHaveBeenCalledWith("speed", 42);
    editRetention(7); expect(row().getByTitle("写入")).toBeDisabled(); expect(row().queryByText("123")).toBeNull();
    expect(row().getByText("待保存")).toBeVisible();
  });
  it("删除心跳点后清除引用，还原可恢复，保存发送最终地址表", async () => {
    await show(); await userEvent.click(row().getByTitle("删除"));
    expect(screen.getByRole("combobox", { name: "心跳点位" })).toHaveValue(""); expect(screen.getByRole("spinbutton", { name: "心跳周期 (ms)" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ points: [], heartbeat: { pointId: null, intervalMs: 1000 } }));
  });
  it("JSON 替换地址表后清除被删除心跳点的引用", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "导入/导出" }));
    const dialog = within(screen.getByRole("dialog")); fireEvent.change(dialog.getByRole("textbox"), { target: { value: "[]" } });
    await userEvent.click(dialog.getByRole("button", { name: "应用到地址表" }));
    expect(screen.getByRole("combobox", { name: "心跳点位" })).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ points: [], heartbeat: expect.objectContaining({ pointId: null }) }));
  });
  it("修改点位名称与地址后须保存，心跳继续指向该点", async () => {
    await show(); await userEvent.click(row().getByTitle("编辑"));
    const dialog = within(screen.getByRole("dialog")); fireEvent.change(dialog.getByRole("textbox", { name: "名称" }), { target: { value: "新速度" } });
    fireEvent.change(dialog.getByRole("textbox", { name: /^地址/ }), { target: { value: "HR200" } });
    await userEvent.click(dialog.getByRole("button", { name: "确定" })); await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ points: [expect.objectContaining({ name: "新速度", address: "HR200" })], heartbeat: expect.objectContaining({ pointId: "speed" }) }));
  });

  it.each(["modbusTcp", "s7", "mc"] as ProtocolKind[])("%s 页面提交当前协议的全部连接参数", async protocol => {
    const config = plcConfig(); config.connection.protocol = protocol; config.connection.port = protocol === "modbusTcp" ? 502 : protocol === "s7" ? 102 : 5000;
    config.points[0].address = protocol === "s7" ? "DB1.DBD0" : protocol === "mc" ? "D100" : "HR100";
    vi.mocked(plcApi.getConfig).mockResolvedValueOnce(config); await show();
    const change = (name: string, value: string) => fireEvent.change(screen.getByRole("spinbutton", { name }), { target: { value } });
    fireEvent.change(screen.getByRole("textbox", { name: "IP / 主机名" }), { target: { value: "plc-line-2" } });
    change("端口", "1234"); change("通讯超时 (ms)", "600"); change("轮询周期 (ms)", "25"); change("重连间隔 (ms)", "500");
    if (protocol === "modbusTcp") change("站号 (Unit ID)", "255");
    if (protocol === "s7") {
      change("机架 (Rack)", "2"); change("插槽 (Slot)", "3");
      await userEvent.selectOptions(screen.getByRole("combobox", { name: "连接类型" }), "basic");
      await userEvent.selectOptions(screen.getByRole("combobox", { name: "PDU 长度" }), "960");
      fireEvent.change(screen.getByRole("textbox", { name: /本地 TSAP/ }), { target: { value: "0x0100" } });
      fireEvent.change(screen.getByRole("textbox", { name: /远端 TSAP/ }), { target: { value: "0x0203" } });
    }
    if (protocol === "mc") {
      change("网络号", "1"); change("PC 号", "2"); change("目标模块站号", "3");
      fireEvent.change(screen.getByRole("textbox", { name: "目标模块 IO 号" }), { target: { value: "0x0123" } });
      await userEvent.click(screen.getByRole("checkbox", { name: /X\/Y 使用八进制/ }));
    }
    await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    const saved = vi.mocked(plcApi.saveConfig).mock.calls[0][0];
    expect(saved.connection).toMatchObject({ protocol, host: "plc-line-2", port: 1234, timeoutMs: 600, pollIntervalMs: 25, reconnectIntervalMs: 500 });
    if (protocol === "modbusTcp") expect(saved.connection.modbus).toEqual({ unitId: 255 });
    if (protocol === "s7") expect(saved.connection.s7).toEqual({ rack: 2, slot: 3, connectionType: "basic", pduSize: 960, localTsap: 256, remoteTsap: 515 });
    if (protocol === "mc") expect(saved.connection.mc).toEqual({ networkNo: 1, pcNo: 2, moduleStation: 3, moduleIo: 291, xyOctal: true });
  });

  it("连接失败保持真实未连接状态，可重试并由状态更新显示实际值", async () => {
    vi.mocked(plcApi.connect).mockRejectedValueOnce(new Error("握手失败"));
    const page = await show(); expect(row().queryByText("123")).toBeNull(); expect(row().getByText("未连接")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "连接" })); expect(await screen.findByText("Error: 握手失败")).toBeVisible();
    expect(row().getByTitle("写入")).toBeDisabled(); await userEvent.click(screen.getByRole("button", { name: "连接" }));
    expect(plcApi.connect).toHaveBeenCalledTimes(2); expect(row().queryByText("123")).toBeNull();
    status = { ...status, state: "connected", pollCount: 4, cycleMs: 20, errorCount: 1, message: "连接成功" }; page.rerender(<PlcPage />);
    expect(row().getByText("123")).toBeVisible(); expect(row().getByTitle("写入")).toBeEnabled(); expect(screen.getByRole("button", { name: "断开" })).toBeVisible();
  });

  it("写入弹窗打开后连接断开不能继续写，恢复实际连接后可以重试", async () => {
    status.state = "connected"; const page = await show(); await userEvent.click(row().getByTitle("写入"));
    const dialog = within(screen.getByRole("dialog")); fireEvent.change(dialog.getByRole("textbox"), { target: { value: "42" } });
    status = { ...status, state: "disconnected" }; page.rerender(<PlcPage />);
    await userEvent.click(dialog.getByRole("button", { name: "写入" }));
    expect(await dialog.findByText(/连接状态或地址表已变化/)).toBeVisible(); expect(plcApi.writePoint).not.toHaveBeenCalled();
    status = { ...status, state: "connected" }; page.rerender(<PlcPage />); await userEvent.click(dialog.getByRole("button", { name: "写入" }));
    expect(plcApi.writePoint).toHaveBeenCalledWith("speed", 42);
  });

  it("保存中的连接操作在页面卸载后不再发起连接", async () => {
    const request = deferred<void>(); vi.mocked(plcApi.saveConfig).mockReturnValueOnce(request.promise);
    const page = await show(); editRetention(7); fireEvent.click(screen.getByRole("button", { name: "连接" }));
    page.unmount(); await act(async () => request.resolve()); expect(plcApi.connect).not.toHaveBeenCalled();
  });

  it("当前值展示保持各类型、读取错误和只读状态一致", async () => {
    const config = plcConfig(); status.state = "connected";
    const samples = [
      ["bool", true, "1"], ["u8", 255, "255"], ["i8", -128, "-128"], ["u16", 65535, "65535"],
      ["i16", -32768, "-32768"], ["u32", 4294967295, "4294967295"], ["i32", -2147483648, "-2147483648"],
      ["f32", 1.25, "1.25"], ["f64", 1.234567890123, "1.234567890123"],
    ] as const;
    config.points = samples.map(([dataType], i) => ({ ...config.points[0], id: dataType, name: dataType, address: dataType === "bool" ? "C10" : `HR${i * 4}`, dataType, access: "read" }));
    config.heartbeat.pointId = null;
    values = Object.fromEntries(samples.map(([id, value]) => [id, { value, error: null, ts: 1 }]));
    vi.mocked(plcApi.getConfig).mockResolvedValueOnce(config); const page = await show();
    for (const [id, , text] of samples) {
      const record = within(screen.getByText(id).closest("tr")!);
      expect(record.getByText(text)).toBeVisible(); expect(record.getByTitle("写入")).toBeDisabled();
    }
    values = { ...values, f32: { value: null, error: "读取超时", ts: 2 }, bool: { value: false, error: null, ts: 2 } }; page.rerender(<PlcPage />);
    expect(within(screen.getByText("f32").closest("tr")!).getByText("读取超时")).toBeVisible();
    expect(within(screen.getByText("bool").closest("tr")!).getByText("0")).toBeVisible();
  });

  it("心跳点的上下沿配置随地址表保存，还原取消新点位", async () => {
    await show();
    for (const [edge, label] of [["rising", "上升沿"], ["falling", "下降沿"], ["both", "双边沿"]]) {
      await userEvent.click(row().getByTitle("编辑")); const dialog = within(screen.getByRole("dialog"));
      await userEvent.selectOptions(dialog.getByRole("combobox", { name: "边沿事件" }), edge);
      await userEvent.click(dialog.getByRole("button", { name: "确定" })); expect(row().getByText(label)).toBeVisible();
      await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
      expect(plcApi.saveConfig).toHaveBeenLastCalledWith(expect.objectContaining({ points: [expect.objectContaining({ edge })], heartbeat: expect.objectContaining({ pointId: "speed" }) }));
    }
    await userEvent.click(screen.getByRole("button", { name: "新增点位" })); const dialog = within(screen.getByRole("dialog"));
    fireEvent.change(dialog.getByRole("textbox", { name: "名称" }), { target: { value: "启动信号" } });
    fireEvent.change(dialog.getByRole("textbox", { name: "ID" }), { target: { value: "start" } });
    fireEvent.change(dialog.getByRole("textbox", { name: /^地址/ }), { target: { value: "C10" } });
    await userEvent.selectOptions(dialog.getByRole("combobox", { name: "数据类型" }), "bool");
    await userEvent.click(dialog.getByRole("button", { name: "确定" })); expect(screen.getByText("启动信号")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "还原" })); expect(screen.queryByText("启动信号")).toBeNull();
    expect(row().getByText("双边沿")).toBeVisible();
  });

  it("JSON 把心跳点改为只读时清除心跳，保存仍保留该点位", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "导入/导出" }));
    const points = plcConfig().points.map(p => ({ ...p, access: "read" })); const dialog = within(screen.getByRole("dialog"));
    fireEvent.change(dialog.getByRole("textbox"), { target: { value: JSON.stringify(points) } });
    await userEvent.click(dialog.getByRole("button", { name: "应用到地址表" })); expect(screen.getByRole("combobox", { name: "心跳点位" })).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ points: [expect.objectContaining({ id: "speed", access: "read" })], heartbeat: expect.objectContaining({ pointId: null }) }));
  });

  it.each(["", "-1", "99", "100.5"])("心跳周期 %s 阻止保存与新连接", async value => {
    await show(); fireEvent.change(screen.getByRole("spinbutton", { name: "心跳周期 (ms)" }), { target: { value } });
    expect(screen.getByRole("alert")).toHaveTextContent("心跳周期"); expect(screen.getByRole("button", { name: "保存配置" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "连接" })).toBeDisabled(); expect(plcApi.saveConfig).not.toHaveBeenCalled();
  });

  it.each(["", "-1", "0.5", "4294967296"])("日志天数 %s 阻止保存，不能变成隐式永久保存", async value => {
    await show(); fireEvent.change(screen.getByRole("spinbutton", { name: "日志保留天数（0 为永久）" }), { target: { value } });
    expect(screen.getByRole("alert")).toHaveTextContent("日志保留天数"); expect(screen.getByRole("button", { name: "保存配置" })).toBeDisabled();
  });

  it("心跳最小周期、日志永久保存和自动连接可一起保存", async () => {
    await show(); fireEvent.change(screen.getByRole("spinbutton", { name: "心跳周期 (ms)" }), { target: { value: "100" } });
    editRetention(0); await userEvent.click(screen.getByRole("checkbox", { name: "启动时自动连接" }));
    await userEvent.click(screen.getByRole("button", { name: "保存配置" }));
    expect(plcApi.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ heartbeat: { pointId: "speed", intervalMs: 100 }, logRetentionDays: 0, autoConnect: true }));
  });

  it("自动 TSAP 被改成无效值仍属于草稿，可还原而不误当作自动值", async () => {
    const config = plcConfig(); config.connection.protocol = "s7"; config.connection.port = 102; config.points[0].address = "DB1.DBD0";
    vi.mocked(plcApi.getConfig).mockResolvedValueOnce(config); await show();
    fireEvent.change(screen.getByRole("textbox", { name: /本地 TSAP/ }), { target: { value: "0x10bad-tail" } });
    expect(screen.getByText("未保存")).toBeVisible(); expect(screen.getByRole("button", { name: "还原" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "连接" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "还原" }));
    expect(screen.getByRole("textbox", { name: /本地 TSAP/ })).toHaveValue(""); expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "连接" })).toBeEnabled(); expect(plcApi.saveConfig).not.toHaveBeenCalled();
  });
});

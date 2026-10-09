import { useState } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ConnectionForm from "../src/features/plc/components/ConnectionForm";
import PointEditor from "../src/features/plc/components/PointEditor";
import PointsJsonDialog from "../src/features/plc/components/PointsJsonDialog";
import { plcApi } from "../src/features/plc/api";
import type { ConnectionConfig, PlcPoint } from "../src/features/plc/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/plc/api", () => ({ plcApi: { checkAddress: vi.fn() } }));
const connection: ConnectionConfig = {
  protocol: "modbusTcp", host: "192.168.1.10", port: 502, timeoutMs: 1000, pollIntervalMs: 200, reconnectIntervalMs: 3000,
  modbus: { unitId: 1 }, s7: { rack: 0, slot: 1, connectionType: "pg", localTsap: null, remoteTsap: null, pduSize: 480 },
  mc: { networkNo: 0, pcNo: 255, moduleIo: 1023, moduleStation: 0, xyOctal: false },
};
const point: PlcPoint = { id: "speed", name: "速度", address: "HR100", dataType: "f32", wordOrder: null, access: "readWrite", edge: "none", logChanges: true, tags: [], description: "" };
beforeEach(() => { vi.mocked(plcApi.checkAddress).mockResolvedValue("地址有效"); });

describe("PLC 协议设置", () => {
  function Harness() { const [value, onChange] = useState(connection); return <ConnectionForm value={value} onChange={onChange} />; }
  it("切换协议选择默认端口，模拟器关闭主机与端口输入", async () => {
    render(<Harness />);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "协议" }), "s7");
    expect(screen.getByRole("spinbutton", { name: "端口" })).toHaveValue(102);
    expect(screen.getByRole("combobox", { name: "CPU 型号" })).toBeVisible();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "协议" }), "mc");
    expect(screen.getByRole("spinbutton", { name: "端口" })).toHaveValue(5000);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "协议" }), "simulator");
    expect(screen.getByRole("textbox", { name: "IP / 主机名" })).toBeDisabled();
    expect(screen.getByRole("spinbutton", { name: "端口" })).toBeDisabled();
  });

  it("S7 CPU 预设同步插槽与 TSAP", async () => {
    render(<Harness />); await userEvent.selectOptions(screen.getByRole("combobox", { name: "协议" }), "s7");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "CPU 型号" }), "s7300");
    expect(screen.getByRole("spinbutton", { name: "插槽 (Slot)" })).toHaveValue(2);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "CPU 型号" }), "s7200smart");
    expect(screen.getByRole("spinbutton", { name: "插槽 (Slot)" })).toHaveValue(1);
    expect(screen.getByRole("textbox", { name: /本地 TSAP/ })).toHaveValue("0x0102");
    expect(screen.getByRole("textbox", { name: /远端 TSAP/ })).toHaveValue("0x0201");
  });
});

describe("PLC 点位编辑", () => {
  function show(initial = point, isNew = true) {
    const onSave = vi.fn(); render(<PointEditor initial={initial} isNew={isNew} existingIds={["existing"]} connection={connection} tagPresets={[]} onSave={onSave} onClose={vi.fn()} />); return onSave;
  }
  it.each([["id", "ID 不能为空"], ["name", "名称不能为空"], ["address", "地址不能为空"]] as const)("必填 %s 验证阻止提交", async (key, error) => {
    const onSave = show({ ...point, [key]: " " });
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(screen.getByText(error)).toBeVisible(); expect(onSave).not.toHaveBeenCalled();
  });

  it("禁止重复 ID，保存时去除字段空白", async () => {
    const onSave = show({ ...point, id: "existing" });
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(screen.getByText("ID 已存在")).toBeVisible(); expect(onSave).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "ID" }), { target: { value: " new-speed " } });
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "  新速度 " } });
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ id: "new-speed", name: "新速度" }));
  });

  it("后端地址检查失败时不能保存", async () => {
    vi.mocked(plcApi.checkAddress).mockRejectedValue(new Error("地址不合法")); const onSave = show();
    await screen.findByText("Error: 地址不合法");
    await userEvent.click(screen.getByRole("button", { name: "确定" })); expect(onSave).not.toHaveBeenCalled();
  });

  it("布尔类型不允许字节序，自定义标签去重并可删除", async () => {
    const onSave = show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "数据类型" }), "bool");
    expect(screen.getByRole("combobox", { name: "字节序" })).toBeDisabled();
    const tags = screen.getByPlaceholderText("自定义标签，回车添加");
    await userEvent.type(tags, "quality{Enter}"); await userEvent.type(tags, "quality{Enter}");
    expect(screen.getAllByRole("button", { name: "quality ×" })).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "quality ×" }));
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ dataType: "bool", tags: [] }));
  });

  it("修改地址后旧失败返回不能覆盖最新检查", async () => {
    const old = deferred<string>();
    vi.mocked(plcApi.checkAddress).mockReturnValueOnce(old.promise).mockResolvedValueOnce("最新地址有效");
    const onSave = show();
    await waitFor(() => expect(plcApi.checkAddress).toHaveBeenCalledTimes(1));
    fireEvent.change(screen.getByPlaceholderText(/HR100/), { target: { value: "HR101" } });
    expect(await screen.findByText("最新地址有效")).toBeVisible();
    await act(async () => old.reject(new Error("旧地址失败")));
    expect(screen.queryByText(/旧地址失败/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ address: "HR101" }));
    expect(plcApi.checkAddress).toHaveBeenLastCalledWith(connection, "HR101", "f32");
  });

  it("立即保存也须检查当前地址，等待期间不能重复提交或改字段", async () => {
    const pending = deferred<string>();
    vi.mocked(plcApi.checkAddress).mockReturnValue(pending.promise);
    const onSave = show();
    fireEvent.click(screen.getByRole("button", { name: "确定" }));
    fireEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(screen.getByRole("button", { name: "确定" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "ID" })).toBeDisabled();
    expect(onSave).not.toHaveBeenCalled();
    await act(async () => pending.resolve("有效"));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
  });

  it("提交时地址失败可重试，卸载后返回不会保存", async () => {
    const pending = deferred<string>();
    vi.mocked(plcApi.checkAddress).mockRejectedValueOnce(new Error("检查暂不可用"));
    const onSave = vi.fn();
    const view = render(<PointEditor initial={point} isNew existingIds={[]} connection={connection} tagPresets={[]} onSave={onSave} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(await screen.findByText(/检查暂不可用/)).toBeVisible();
    vi.mocked(plcApi.checkAddress).mockReturnValue(pending.promise);
    fireEvent.click(screen.getByRole("button", { name: "确定" }));
    view.unmount();
    await act(async () => pending.resolve("有效"));
    expect(onSave).not.toHaveBeenCalled();
  });
});

describe("地址表导入", () => {
  it.each(["not-json", "{}"])("拒绝输入 %s，保留对话框", async input => {
    const onApply = vi.fn(), onClose = vi.fn(); render(<PointsJsonDialog points={[]} onApply={onApply} onClose={onClose} />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: input } });
    await userEvent.click(screen.getByRole("button", { name: "应用到地址表" }));
    expect(screen.getByText(/解析失败/)).toBeVisible(); expect(onApply).not.toHaveBeenCalled(); expect(onClose).not.toHaveBeenCalled();
  });

  it("有效数组填充默认字段并应用，取消不修改地址表", async () => {
    const onApply = vi.fn(), onClose = vi.fn(); render(<PointsJsonDialog points={[point]} onApply={onApply} onClose={onClose} />);
    await userEvent.click(screen.getByRole("button", { name: "取消" })); expect(onApply).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: '[{"id":"new","name":"新点","address":"HR2"}]' } });
    await userEvent.click(screen.getByRole("button", { name: "应用到地址表" }));
    expect(onApply).toHaveBeenCalledWith([expect.objectContaining({ id: "new", name: "新点", address: "HR2", tags: [], access: "read" })]);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it.each([
    [null], [4], [[]], [{ ...point, id: undefined }], [{ ...point, id: " " }],
    [{ ...point, tags: "tag" }], [{ ...point, tags: [4] }], [{ ...point, dataType: "bad" }],
    [{ ...point, access: "write" }], [{ ...point, edge: "bad" }], [{ ...point, wordOrder: "bad" }],
    [{ ...point, logChanges: "true" }], [{ ...point, description: null }],
    [point, { ...point, id: " speed " }],
  ])("拒绝无效点位字段 %#", async (...items) => {
    const onApply = vi.fn(); render(<PointsJsonDialog points={[]} onApply={onApply} onClose={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox", { name: "地址表 JSON" }), { target: { value: JSON.stringify(items) } });
    await userEvent.click(screen.getByRole("button", { name: "应用到地址表" }));
    expect(screen.getByRole("alert")).toHaveTextContent("解析失败");
    expect(onApply).not.toHaveBeenCalled();
  });

  it("导入规范化空白和重复标签，布尔字节序归零", async () => {
    const onApply = vi.fn(); render(<PointsJsonDialog points={[]} onApply={onApply} onClose={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: JSON.stringify([{ ...point, id: " enabled ", name: " 开关 ", address: " C2 ", tags: [" quality ", "quality", ""], dataType: "bool", wordOrder: "ABCD" }]) } });
    await userEvent.click(screen.getByRole("button", { name: "应用到地址表" }));
    expect(onApply).toHaveBeenCalledWith([expect.objectContaining({ id: "enabled", name: "开关", address: "C2", tags: ["quality"], wordOrder: null })]);
  });

  it("复制失败显示原因且可重试，等待期间不重复复制", async () => {
    const copy = vi.fn().mockRejectedValueOnce(new Error("剪贴板拒绝"));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: copy } });
    render(<PointsJsonDialog points={[point]} onApply={vi.fn()} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("剪贴板拒绝");
    const pending = deferred<void>(); copy.mockReturnValueOnce(pending.promise);
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    expect(copy).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "复制" })).toBeDisabled();
    await act(async () => pending.resolve());
    expect(screen.getByRole("button", { name: "已复制" })).toBeVisible();
    expect(copy).toHaveBeenCalledWith(JSON.stringify([point], null, 2));
  });
});

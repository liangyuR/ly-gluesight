import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

describe("PLC 点位编辑", () => {
  function show(initial = point, isNew = true) {
    const onSave = vi.fn(); render(<PointEditor initial={initial} isNew={isNew} existingIds={["existing"]} connection={connection} tagPresets={[]} onSave={onSave} onClose={vi.fn()} />); return onSave;
  }

  it("禁止重复 ID，保存时去除字段空白", async () => {
    const onSave = show({ ...point, id: "existing" });
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(screen.getByText("ID 已存在")).toBeVisible(); expect(onSave).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "ID" }), { target: { value: " new-speed " } });
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "  新速度 " } });
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ id: "new-speed", name: "新速度" }));
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

  it("地址检查等待时进入生产，旧校验成功不能提交点位", async () => {
    const pending = deferred<string>(); vi.mocked(plcApi.checkAddress).mockReturnValue(pending.promise);
    const onSave = vi.fn(), onClose = vi.fn();
    const view = render(<PointEditor initial={point} isNew existingIds={[]} connection={connection} tagPresets={[]} onSave={onSave} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "确定" }));
    view.rerender(<PointEditor initial={point} isNew existingIds={[]} connection={connection} tagPresets={[]} blockedReason="生产事务未结束" onSave={onSave} onClose={onClose} />);
    await act(async () => pending.resolve("有效"));
    expect(onSave).not.toHaveBeenCalled(); expect(screen.getByRole("button", { name: "确定" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "名称" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "取消" })); expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("地址表导入", () => {

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

});

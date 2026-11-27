import { act, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { usePlcOperationState } from "../src/features/plc/api";
import { useMaintenance, type CycleControlState } from "../src/features/cycle/control";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
let onControl: (event: { payload: CycleControlState }) => void;
const unlisten = vi.fn();
const idle: CycleControlState = { productionBusy: false, plcLocked: false, plcHandshake: null, maintenance: null };
function Harness() {
  const { blockedReason, handshake } = usePlcOperationState();
  return <><p>{blockedReason || "操作可用"}</p><output aria-label="握手快照">{JSON.stringify(handshake)}</output></>;
}
function MaintenanceBadge() {
  return <output>{useMaintenance()?.reason ?? "常规生产"}</output>;
}
beforeEach(() => {
  vi.mocked(listen).mockImplementation(async (_event, listener) => {
    onControl = listener as typeof onControl;
    return unlisten;
  });
  vi.mocked(invoke).mockResolvedValue(idle);
});

describe("PLC 操作锁", () => {
  it.each([
    { stage: "订阅", listens: 2, reads: 1 },
    { stage: "首查", listens: 1, reads: 2 },
  ])("标题栏控制状态在 $stage 阶段失败 → 打开 PLC 页后恢复", async ({ stage, listens, reads }) => {
    if (stage === "订阅") vi.mocked(listen).mockRejectedValueOnce(new Error("事件不可用"));
    else vi.mocked(invoke).mockRejectedValueOnce(new Error("状态不可用"));
    const page = render(<><MaintenanceBadge /></>);
    await act(async () => {});
    page.rerender(<><MaintenanceBadge /><Harness /></>);
    expect(await screen.findByText("操作可用")).toBeVisible();
    expect(listen).toHaveBeenCalledTimes(listens);
    expect(invoke).toHaveBeenCalledTimes(reads);
  });

  it("订阅尚未建立 → 保持锁定，订阅就绪后查询空闲才放开操作", async () => {
    const registration = deferred<() => void>();
    const initial = deferred<CycleControlState>();
    vi.mocked(listen).mockReturnValueOnce(registration.promise);
    vi.mocked(invoke).mockReturnValueOnce(initial.promise);
    render(<Harness />); expect(screen.getByText(/正在核对生产状态/)).toBeVisible();
    expect(invoke).not.toHaveBeenCalled();
    await act(async () => registration.resolve(unlisten));
    expect(listen).toHaveBeenCalledWith("cycle://control-state", expect.any(Function));
    expect(invoke).toHaveBeenCalledWith("cycle_control_state");
    await act(async () => initial.resolve(idle));
    expect(screen.getByText("操作可用")).toBeVisible();
  });

  it("最新事务事件优先于迟到的初始空闲响应，FAULT 未结事务仍锁定", async () => {
    const initial = deferred<CycleControlState>(); vi.mocked(invoke).mockReturnValueOnce(initial.promise);
    const view = render(<Harness />);
    await act(async () => {});
    expect(invoke).toHaveBeenCalledOnce();
    const handshake = { phase: "draining", requestSeq: 41, message: "收尾并写结果" } as const;
    act(() => onControl({ payload: { ...idle, plcLocked: true, plcHandshake: handshake } }));
    await act(async () => initial.resolve(idle));
    expect(screen.getByText(/生产或结果交付尚未结束/)).toBeVisible();
    expect(screen.getByLabelText("握手快照")).toHaveTextContent(JSON.stringify(handshake));
    act(() => onControl({ payload: idle }));
    expect(screen.getByText("操作可用")).toBeVisible();
    expect(screen.getByLabelText("握手快照")).toHaveTextContent("null"); view.unmount();
    expect(unlisten).toHaveBeenCalled();
  });

  it("订阅失败 → 生产状态不可用且 PLC 操作继续锁定", async () => {
    vi.mocked(listen).mockRejectedValueOnce(new Error("事件不可用"));
    render(<Harness />); expect(await screen.findByText(/生产状态不可用/)).toBeVisible();
    expect(screen.queryByText("操作可用")).toBeNull();
  });
});

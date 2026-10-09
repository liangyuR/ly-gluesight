import { act, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { plcApi, usePlcOperationState } from "../src/features/plc/api";
import type { PlcOperationState } from "../src/features/plc/types";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
let onSnapshot: (event: { payload: PlcOperationState }) => void;
const unlisten = vi.fn();
function Harness() {
  const { blockedReason, handshake } = usePlcOperationState();
  return <><p>{blockedReason || "操作可用"}</p><output aria-label="握手快照">{JSON.stringify(handshake)}</output></>;
}
beforeEach(() => {
  vi.mocked(listen).mockImplementation(async (_event, listener) => {
    onSnapshot = listener as typeof onSnapshot;
    return unlisten;
  });
  vi.mocked(invoke).mockResolvedValue({ phase: "IDLE", plcLocked: false });
});

describe("PLC 操作锁", () => {
  it("初次状态未知时锁定，空闲快照确认后才开放", async () => {
    const initial = deferred<PlcOperationState>(); vi.mocked(invoke).mockReturnValueOnce(initial.promise);
    render(<Harness />); expect(screen.getByText(/正在核对生产状态/)).toBeVisible();
    await act(async () => initial.resolve({ phase: "IDLE", plcLocked: false }));
    expect(screen.getByText("操作可用")).toBeVisible();
    expect(invoke).toHaveBeenCalledWith("cycle_snapshot", undefined);
  });

  it("最新事务事件优先于迟到的初始空闲响应，FAULT 未结事务仍锁定", async () => {
    const initial = deferred<PlcOperationState>(); vi.mocked(invoke).mockReturnValueOnce(initial.promise);
    const view = render(<Harness />);
    const handshake = { phase: "awaitAck", requestSeq: 41, resultSeq: 41, message: "结果等待确认" } as const;
    act(() => onSnapshot({ payload: { phase: "FAULT", plcLocked: true, plcHandshake: handshake } }));
    await act(async () => initial.resolve({ phase: "IDLE", plcLocked: false }));
    expect(screen.getByText(/生产或结果交付尚未结束/)).toBeVisible();
    expect(screen.getByLabelText("握手快照")).toHaveTextContent(JSON.stringify(handshake));
    act(() => onSnapshot({ payload: { phase: "FAULT", plcLocked: false } }));
    expect(screen.getByText("操作可用")).toBeVisible();
    expect(screen.getByLabelText("握手快照")).toHaveTextContent("null"); view.unmount();
    expect(unlisten).toHaveBeenCalled();
  });

  it("加载失败保持锁定，后续真实事件可恢复", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("后端不可用"));
    render(<Harness />); expect(await screen.findByText(/生产状态不可用/)).toBeVisible();
    act(() => onSnapshot({ payload: { phase: "IDLE" } })); expect(screen.getByText("操作可用")).toBeVisible();
    act(() => onSnapshot({ payload: { phase: "REPORT" } })); expect(screen.getByText(/生产或结果交付尚未结束/)).toBeVisible();
  });

  it("一期模板把 DB 号交给后端生成，不执行保存或连接", async () => {
    await plcApi.s7Phase1Template(100);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("plc_s7_phase1_template", { dbNumber: 100 });
  });

  it("订阅失败不能被随后到达的空闲读取解除锁定", async () => {
    const initial = deferred<PlcOperationState>();
    vi.mocked(invoke).mockReturnValueOnce(initial.promise);
    vi.mocked(listen).mockRejectedValueOnce(new Error("事件不可用"));
    render(<Harness />); expect(await screen.findByText(/生产状态不可用/)).toBeVisible();
    await act(async () => initial.resolve({ phase: "IDLE", plcLocked: false }));
    expect(screen.queryByText("操作可用")).toBeNull();
  });
});

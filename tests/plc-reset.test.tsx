import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, expect, it, vi } from "vitest";
import PlcPage from "../src/features/plc/pages/PlcPage";
import type { PlcHandshakeState, PlcOperationState } from "../src/features/plc/types";
import { plcConfig } from "./plc-fixtures";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const listeners = new Map<string, (event: { payload: unknown }) => void>();
const handshake = (resetBlockedReason: string | null | undefined, phase: PlcHandshakeState["phase"] = "resetRequired"): PlcHandshakeState =>
  ({ phase, requestSeq: 41, resultSeq: 41, message: "S7 连接会话已更新", resetBlockedReason });
const operation = (state = handshake(null)): PlcOperationState => ({ phase: "FAULT", plcLocked: true, plcHandshake: state });
function publish(state: PlcOperationState) { act(() => listeners.get("cycle://snapshot")!({ payload: state })); }
const page = (readOnly = false) => render(<SnackbarProvider><PlcPage readOnly={readOnly} /></SnackbarProvider>);

beforeEach(() => {
  listeners.clear();
  vi.mocked(listen).mockImplementation(async (event, listener) => {
    listeners.set(event, listener as (event: { payload: unknown }) => void);
    return () => { listeners.delete(event); };
  });
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "plc_get_config") return plcConfig();
    if (command === "plc_get_status") return { state: "connected", message: "已连接" };
    if (command === "plc_get_values") return {};
    if (command === "cycle_snapshot") return operation();
  });
});

it("未结事务锁定配置但复位条件满足 → 可提交一次复位并等待真实状态结果", async () => {
  const reset = deferred<void>();
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "plc_get_config") return plcConfig();
    if (command === "plc_get_status") return { state: "connected", message: "已连接" };
    if (command === "plc_get_values") return {};
    if (command === "cycle_snapshot") return operation();
    if (command === "cycle_reset") return reset.promise;
  });
  page();
  const button = await screen.findByRole("button", { name: "复位故障" });
  expect(screen.getByText(/当前阶段/)).toHaveTextContent("待复位");
  expect(screen.getByRole("button", { name: "新增点位" })).toBeDisabled();
  expect(button).toBeEnabled();
  fireEvent.click(button);
  fireEvent.click(button);
  expect(button).toBeDisabled();
  await act(async () => reset.resolve());
  expect(screen.getByText("复位请求已提交，等待执行结果。")).toBeVisible();
  expect(button).toBeDisabled();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "cycle_reset")).toHaveLength(1);
  publish({ phase: "IDLE", plcLocked: false, plcHandshake: handshake(null, "idle") });
  expect(screen.getByText(/当前阶段/)).toHaveTextContent("空闲");
  expect(screen.queryByText("复位请求已提交，等待执行结果。")).toBeNull();
  expect(screen.getByRole("button", { name: "新增点位" })).toBeEnabled();
});

it.each([
  ["等待 PLC 释放 partStart", "等待 PLC 释放 partStart"],
  [undefined, "复位条件尚未就绪"],
] as const)("复位阻塞原因 %s → 展示具体原因，实时解除后开放复位", async (reason, text) => {
  page();
  const button = await screen.findByRole("button", { name: "复位故障" });
  publish(operation(handshake(reason)));
  expect(button).toBeDisabled();
  expect(screen.getByText(new RegExp(`当前不能复位：${text}`))).toBeVisible();
  publish(operation());
  expect(button).toBeEnabled();
  expect(screen.queryByText(/当前不能复位/)).toBeNull();
});

it("复位命令已接收但执行失败 → 显示故障快照原因并允许重试", async () => {
  page();
  const button = await screen.findByRole("button", { name: "复位故障" });
  fireEvent.click(button);
  expect(await screen.findByText("复位请求已提交，等待执行结果。")).toBeVisible();
  publish(operation({ ...handshake(null, "fault"), message: "复位写入失败" }));
  expect(await screen.findByText(/复位请求失败：复位写入失败/)).toBeVisible();
  await waitFor(() => expect(button).toBeEnabled());
});

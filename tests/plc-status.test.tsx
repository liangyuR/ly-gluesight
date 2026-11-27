import { act, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Profiler } from "react";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import { usePlcStatus } from "../src/features/plc/api";
import type { PlcStatus } from "../src/features/plc/types";
import { WindowConnections } from "../src/layout/WindowStatus";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

function Status({ label }: { label: string }) {
  return <output aria-label={label}>{usePlcStatus()?.state ?? "未知"}</output>;
}

const status = (state: PlcStatus["state"]): PlcStatus => ({
  state, message: state, since: 1, lastPoll: null, cycleMs: null, pollCount: 0, errorCount: 0,
  dials: 0, outageDials: 0, lastDialError: null, lastError: null,
});

it("PLC 轮询统计变化 → 完整统计更新、标题栏只响应连接信息", async () => {
  const connected = status("connected");
  let emit = (_: PlcStatus) => {};
  vi.mocked(listen).mockImplementation(async (_event, listener) => {
    emit = state => (listener as (event: { payload: PlcStatus }) => void)({ payload: state });
    return () => {};
  });
  vi.mocked(invoke).mockImplementation(async command => command === "engine_status"
    ? { backend: "LyFlow", ready: true, message: "就绪" } : connected);
  const titlebarRenders = vi.fn();
  function PollCount() {
    return <output aria-label="轮询次数">{usePlcStatus()?.pollCount ?? 0}</output>;
  }
  await act(async () => {
    render(<MemoryRouter>
      <Profiler id="connections" onRender={titlebarRenders}><WindowConnections /></Profiler>
      <PollCount />
    </MemoryRouter>);
  });
  expect(screen.getByRole("link", { name: "PLC · 已连接" })).toBeVisible();
  titlebarRenders.mockClear();

  act(() => emit({ ...connected, pollCount: 5, lastPoll: 2 }));
  expect(screen.getByLabelText("轮询次数")).toHaveTextContent("5");
  expect(titlebarRenders).not.toHaveBeenCalled();

  act(() => emit({ ...connected, pollCount: 5, message: "连接提示已更新" }));
  expect(screen.getByRole("link", { name: "PLC · 已连接" })).toHaveAttribute("title", "连接提示已更新");
  act(() => emit(status("error")));
  expect(screen.getByRole("link", { name: "PLC · 异常" })).toBeVisible();
});

it.each(["成功", "失败"])("连接事件先到时，迟到首查%s不覆盖共享状态", async outcome => {
  const registration = deferred<() => void>();
  const first = deferred<PlcStatus>();
  const stop = vi.fn();
  let emit = (_: PlcStatus) => {};
  vi.mocked(listen).mockImplementation((_event, listener) => {
    emit = state => (listener as (event: { payload: PlcStatus }) => void)({ payload: state });
    return registration.promise;
  });
  vi.mocked(invoke).mockReturnValue(first.promise);
  const view = render(<><Status label="标题栏" /><Status label="PLC 页" /></>);
  expect(listen).toHaveBeenCalledExactlyOnceWith("plc://status", expect.any(Function));
  expect(invoke).not.toHaveBeenCalled();
  await act(async () => registration.resolve(stop));
  expect(invoke).toHaveBeenCalledExactlyOnceWith("plc_get_status", undefined);

  act(() => emit(status("error")));
  await act(async () => {
    if (outcome === "成功") first.resolve(status("connected"));
    else first.reject(new Error("首次读取失败"));
  });
  expect(screen.getByLabelText("标题栏")).toHaveTextContent("error");
  expect(screen.getByLabelText("PLC 页")).toHaveTextContent("error");

  view.rerender(<><Status label="标题栏" /></>);
  expect(stop).not.toHaveBeenCalled();
  act(() => emit(status("connected")));
  expect(screen.getByLabelText("标题栏")).toHaveTextContent("connected");
  view.unmount();
  expect(stop).toHaveBeenCalledOnce();
});

it("连接状态订阅失败时仍可显示一次读取，新消费者可重试订阅", async () => {
  vi.mocked(listen).mockRejectedValueOnce(new Error("事件不可用")).mockResolvedValue(() => {});
  vi.mocked(invoke).mockResolvedValueOnce(status("disconnected")).mockResolvedValue(status("connected"));
  const view = render(<><Status label="标题栏" /></>);
  await act(async () => {});
  expect(screen.getByLabelText("标题栏")).toHaveTextContent("disconnected");
  view.rerender(<><Status label="标题栏" /><Status label="PLC 页" /></>);
  await act(async () => {});
  expect(listen).toHaveBeenCalledTimes(2);
  expect(screen.getByLabelText("标题栏")).toHaveTextContent("connected");
  expect(screen.getByLabelText("PLC 页")).toHaveTextContent("connected");
});

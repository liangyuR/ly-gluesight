import { act, renderHook, waitFor } from "@testing-library/react";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cycleApi, useCycle } from "../src/features/cycle/api";
import type { Snapshot } from "../src/features/cycle/types";
import { burstPartView } from "./cycle-visual-fixtures";
import { deferred, snapshot } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const events = new Map<string, (value: unknown) => void>();
const emit = (event: string, value: unknown) => act(() => events.get(event)?.(value));
const state = (cycleId = "cycle-1", since = 1): Snapshot => ({ ...snapshot("ACQUIRE"), activeRecipeId: "S", since, part: burstPartView("1", cycleId) });

beforeEach(() => {
  events.clear();
  vi.mocked(listen).mockClear().mockImplementation(async (event, listener) => {
    events.set(event, payload => listener({ event, id: 1, payload } as TauriEvent<never>));
    return () => { events.delete(event); };
  });
  vi.spyOn(cycleApi, "snapshot").mockClear().mockResolvedValue(state());
});

describe("在线周期身份与迟到响应隔离", () => {
  it.each([false, true])("首次读取失败后重试（期间收到新周期=%s） → 恢复最新状态", async arrives => {
    const retry = deferred<Snapshot | null>();
    vi.mocked(cycleApi.snapshot).mockRejectedValueOnce(new Error("状态读失败")).mockReturnValueOnce(retry.promise);
    const hook = renderHook(useCycle);
    await waitFor(() => expect(hook.result.current.snapshotError).toBe("状态读失败"));

    act(() => hook.result.current.retrySnapshot());
    expect(hook.result.current.snapshotError).toBeNull();
    if (arrives) emit("cycle://snapshot", state("cycle-2", 2));
    await act(async () => retry.resolve(state()));
    expect(hook.result.current.snapshot?.part?.cycleId).toBe(arrives ? "cycle-2" : "cycle-1");
    expect(listen).toHaveBeenCalledOnce();
  });

  it("周期监听注册失败 → 重连后读取状态并接收新周期，旧监听失效", async () => {
    let oldEvent = (_: Snapshot) => {};
    vi.mocked(listen).mockImplementationOnce(async (event, listener) => {
      oldEvent = payload => listener({ event, id: 1, payload } as TauriEvent<never>);
      throw new Error("无法注册监听");
    });
    const hook = renderHook(useCycle);
    await waitFor(() => expect(hook.result.current.subscriptionError).toBe("无法注册监听"));
    expect(hook.result.current.snapshotCurrent).toBe(false);
    expect(cycleApi.snapshot).not.toHaveBeenCalled();

    act(() => hook.result.current.retrySnapshot());
    await waitFor(() => expect(hook.result.current.snapshotCurrent).toBe(true));
    expect(hook.result.current.subscriptionError).toBeNull();
    expect(cycleApi.snapshot).toHaveBeenCalledOnce();
    emit("cycle://snapshot", state("cycle-2", 2));
    act(() => oldEvent(state("expired-listener", 3)));
    expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-2");
  });

  it("同 SN 新周期已经开始 → 迟到的旧周期快照不能覆盖当前件", async () => {
    const hook = renderHook(useCycle);
    await waitFor(() => expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-1"));
    emit("cycle://snapshot", state("cycle-2", 2));
    emit("cycle://snapshot", state("cycle-1", 3));
    expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-2");
    expect(hook.result.current.snapshot?.part?.sn).toBe("1");
  });

  it("初始快照请求迟到不会覆盖已收到的新周期事件", async () => {
    const initial = deferred<Snapshot | null>();
    vi.mocked(cycleApi.snapshot).mockReturnValue(initial.promise);
    const hook = renderHook(useCycle);
    emit("cycle://snapshot", state("cycle-2", 2));
    await act(async () => initial.resolve(state()));
    expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-2");
  });
});

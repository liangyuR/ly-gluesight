import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cycleApi, useCycle } from "../src/features/cycle/api";
import { subscribe } from "../src/features/plc";
import type { Measured, Snapshot } from "../src/features/cycle/types";
import { cycleMeasurement, cyclePart } from "./cycle-visual-fixtures";
import { deferred, snapshot } from "./fixtures";

vi.mock("../src/features/plc", () => ({ subscribe: vi.fn() }));

const events = new Map<string, (value: unknown) => void>();
const emit = (event: string, value: unknown) => act(() => events.get(event)?.(value));
const state = (cycleId = "cycle-1", since = 1): Snapshot => ({ ...snapshot("ACQUIRE"), since, part: cyclePart(1, cycleId) });

beforeEach(() => {
  events.clear();
  vi.mocked(subscribe).mockImplementation((event, listener) => {
    events.set(event, listener as (value: unknown) => void);
    return () => { events.delete(event); };
  });
  vi.spyOn(cycleApi, "snapshot").mockResolvedValue(state());
  vi.spyOn(cycleApi, "logs").mockResolvedValue([]);
  vi.spyOn(cycleApi, "partData").mockResolvedValue([]);
});

describe("在线周期身份与迟到响应隔离", () => {
  it("同 SN 的新周期清除旧测量，旧周期事件不能挤掉当前值", async () => {
    const hook = renderHook(useCycle);
    await waitFor(() => expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-1"));
    emit("cycle://frame", cycleMeasurement());
    expect(hook.result.current.measured).toHaveLength(1);
    emit("cycle://snapshot", state("cycle-2", 2));
    expect(hook.result.current.measured).toEqual([]);
    emit("cycle://frame", { ...cycleMeasurement(1, "cycle-2"), score: .97 });
    emit("cycle://frame", { ...cycleMeasurement(), error: "旧线程迟到" });
    expect(hook.result.current.measured).toHaveLength(1);
    expect(hook.result.current.measured[0]).toMatchObject({ cycleId: "cycle-2", score: .97, error: null });
  });

  it.each(["shotId", "camera", "bundleHash"] as const)("拒绝同周期但 %s 不符的测量事件", async field => {
    const hook = renderHook(useCycle);
    await waitFor(() => expect(hook.result.current.snapshot?.part).not.toBeNull());
    emit("cycle://frame", { ...cycleMeasurement(), [field]: "other" });
    expect(hook.result.current.measured).toEqual([]);
    emit("cycle://frame", cycleMeasurement());
    emit("cycle://frame", cycleMeasurement());
    expect(hook.result.current.measured).toHaveLength(1);
  });

  it("初始快照请求迟到不会覆盖已收到的新周期事件", async () => {
    const initial = deferred<Snapshot | null>();
    vi.mocked(cycleApi.snapshot).mockReturnValue(initial.promise);
    const hook = renderHook(useCycle);
    emit("cycle://snapshot", state("cycle-2", 2));
    await act(async () => initial.resolve(state()));
    expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-2");
  });

  it("新周期到达后，旧周期快照和较早阶段快照均不回滚界面", async () => {
    const hook = renderHook(useCycle);
    await waitFor(() => expect(hook.result.current.snapshot?.part?.cycleId).toBe("cycle-1"));
    emit("cycle://snapshot", state("cycle-2", 5));
    emit("cycle://snapshot", state("cycle-1", 6));
    emit("cycle://snapshot", { ...state("cycle-2", 4), phase: "VALIDATE" });
    expect(hook.result.current.snapshot).toMatchObject({ phase: "ACQUIRE", since: 5, part: { cycleId: "cycle-2" } });
  });

  it("旧周期数据请求晚到不覆盖新周期；新周期回填也不覆盖较新的帧事件", async () => {
    const old = deferred<Measured[]>(), current = deferred<Measured[]>();
    vi.mocked(cycleApi.partData).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const hook = renderHook(useCycle);
    await waitFor(() => expect(cycleApi.partData).toHaveBeenCalledTimes(1));
    emit("cycle://snapshot", state("cycle-2", 2));
    await waitFor(() => expect(cycleApi.partData).toHaveBeenCalledTimes(2));
    emit("cycle://frame", { ...cycleMeasurement(1, "cycle-2"), score: .99 });
    await act(async () => old.resolve([cycleMeasurement()]));
    await act(async () => current.resolve([{ ...cycleMeasurement(1, "cycle-2"), score: .8 }]));
    expect(hook.result.current.measured).toHaveLength(1);
    expect(hook.result.current.measured[0]).toMatchObject({ cycleId: "cycle-2", score: .99 });
  });

  it("快照前到达的帧由本周期数据回填恢复，卸载后的失败请求已处理", async () => {
    const initial = deferred<Snapshot | null>(), data = deferred<Measured[]>();
    vi.mocked(cycleApi.snapshot).mockReturnValue(initial.promise);
    vi.mocked(cycleApi.partData).mockResolvedValueOnce([cycleMeasurement()]).mockReturnValueOnce(data.promise);
    const hook = renderHook(useCycle);
    emit("cycle://frame", cycleMeasurement());
    await act(async () => initial.resolve(state()));
    await waitFor(() => expect(hook.result.current.measured).toHaveLength(1));
    emit("cycle://snapshot", state("cycle-2", 2));
    await waitFor(() => expect(cycleApi.partData).toHaveBeenCalledTimes(2));
    hook.unmount();
    await act(async () => data.reject(new Error("卸载后请求失败")));
    expect(events.size).toBe(0);
  });
});

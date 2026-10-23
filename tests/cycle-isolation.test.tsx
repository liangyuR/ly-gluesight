import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cycleApi, useCycle } from "../src/features/cycle/api";
import { subscribe } from "../src/features/plc";
import type { Snapshot } from "../src/features/cycle/types";
import { burstPartView } from "./cycle-visual-fixtures";
import { deferred, snapshot } from "./fixtures";

vi.mock("../src/features/plc", () => ({ subscribe: vi.fn() }));

const events = new Map<string, (value: unknown) => void>();
const emit = (event: string, value: unknown) => act(() => events.get(event)?.(value));
const state = (cycleId = "cycle-1", since = 1): Snapshot => ({ ...snapshot("ACQUIRE"), activeRecipeId: "S", since, part: burstPartView("1", cycleId) });

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

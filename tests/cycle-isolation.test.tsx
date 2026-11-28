import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cycleApi, useCycle, useLayout, useRecipes } from "../src/features/cycle/api";
import type { Recipe, RecipeSummary, Snapshot } from "../src/features/cycle/types";
import { burstPartView } from "./cycle-visual-fixtures";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred, snapshot } from "./fixtures";

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

describe("在线配方布局更新", () => {
  it("同修订的旧布局请求在刷新后失败 → 两个观察者共用新布局", async () => {
    const layout = burstWorkspaceView().layout;
    const request = { recipeId: layout.id, revisionId: layout.revisionId };
    const initial = deferred<Recipe>();
    const refreshed = deferred<Recipe>();
    const ipc = vi.mocked(invoke).mockReset().mockResolvedValue(null)
      .mockReturnValueOnce(initial.promise).mockReturnValueOnce(refreshed.promise);
    const read = () => useLayout(request.recipeId, request.revisionId);
    const first = renderHook(read);
    await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith("cycle_layout", request));

    emit("workspace://changed", request.recipeId);
    await waitFor(() => expect(ipc).toHaveBeenNthCalledWith(2, "cycle_layout", request));
    await act(async () => initial.reject(new Error("旧布局读取失败")));

    const second = renderHook(read);
    await act(async () => refreshed.resolve(layout));
    await waitFor(() => {
      expect(first.result.current).toEqual(layout);
      expect(second.result.current).toEqual(layout);
    });
    expect(ipc).toHaveBeenCalledTimes(2);
  });

  it("其他配方发生变化 → 更新配方目录并保留在线件的锁定布局", async () => {
    const doc = burstRecipe({ id: "LOCKED" });
    const layout = burstWorkspaceView(doc).layout;
    const locked = burstSummary(doc);
    const other = burstSummary(burstRecipe({ id: "OTHER", name: "待更新配方", productCode: 3 }));
    const updated = { ...other, name: "新版配方", version: 2, revisionId: "OTHER-v2" };
    let recipes = [locked, other];
    const ipc = vi.mocked(invoke).mockReset().mockImplementation(async command => {
      if (command === "cycle_recipes") return recipes;
      if (command === "cycle_layout") return layout;
      throw new Error(`未声明的桌面调用：${command}`);
    });
    const hook = renderHook(() => ({ layout: useLayout(layout.id, layout.revisionId), recipes: useRecipes() }));
    await waitFor(() => expect(hook.result.current).toEqual({ layout, recipes: [locked, other] }));

    recipes = [locked, updated];
    emit("workspace://changed", updated.id);
    await waitFor(() => expect(hook.result.current).toEqual({ layout, recipes: [locked, updated] }));
    expect(ipc.mock.calls.filter(([command]) => command === "cycle_layout"))
      .toEqual([["cycle_layout", { recipeId: layout.id, revisionId: layout.revisionId }]]);
  });

  it("离开布局监听后发布新版 → 重进时读取当前配方布局", async () => {
    const previous = burstWorkspaceView(burstRecipe({ id: "LEASE" })).layout;
    const updated = { ...previous, version: 2, revisionId: "LEASE-v2" };
    const ipc = vi.mocked(invoke).mockReset()
      .mockResolvedValueOnce(previous).mockResolvedValueOnce(updated);
    const first = renderHook(() => useLayout(previous.id));
    await waitFor(() => expect(first.result.current?.revisionId).toBe(previous.revisionId));
    first.unmount();

    const second = renderHook(() => useLayout(previous.id));
    await waitFor(() => expect(second.result.current?.revisionId).toBe(updated.revisionId));
    expect(ipc).toHaveBeenCalledTimes(2);
  });
});

describe("配方目录刷新", () => {
  it.each(["成功", "失败"])("监听登记延迟，首读%s前收到连续变更 → 只补读一次新版目录", async outcome => {
    const registered = deferred<void>();
    const listenImmediately = vi.mocked(listen).getMockImplementation()!;
    vi.mocked(listen).mockImplementationOnce(async (event, handler, options) => {
      await registered.promise;
      return listenImmediately(event, handler, options);
    });
    const previous = burstSummary();
    const updated = { ...previous, name: "新版配方", version: 2, revisionId: "S-v2" };
    const initial = deferred<RecipeSummary[]>();
    const refreshed = deferred<RecipeSummary[]>();
    const ipc = vi.mocked(invoke).mockReset().mockResolvedValue([updated])
      .mockReturnValueOnce(initial.promise).mockReturnValueOnce(refreshed.promise);
    const hook = renderHook(useRecipes);
    const callsBeforeRegistration = ipc.mock.calls.length;

    await act(async () => registered.resolve(undefined));
    expect(callsBeforeRegistration).toBe(0);
    await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith("cycle_recipes", undefined));
    emit("workspace://changed", previous.id);
    emit("workspace://changed", "OTHER");
    emit("workspace://changed", previous.id);
    expect(ipc).toHaveBeenCalledTimes(1);

    await act(async () => {
      if (outcome === "成功") initial.resolve([previous]);
      else initial.reject(new Error("旧的目录查询失败"));
    });
    await waitFor(() => expect(ipc).toHaveBeenCalledTimes(2));
    expect(hook.result.current).toEqual([]);

    await act(async () => refreshed.resolve([updated]));
    await waitFor(() => expect(hook.result.current).toEqual([updated]));
    expect(ipc).toHaveBeenCalledTimes(2);
  });

  it("目录查询期间收到变更后离页 → 丢弃待补读，重进时读取新目录", async () => {
    const initial = deferred<RecipeSummary[]>();
    const updated = burstSummary(burstRecipe({ id: "NEW" }));
    const ipc = vi.mocked(invoke).mockReset().mockResolvedValue([updated])
      .mockReturnValueOnce(initial.promise);
    const first = renderHook(useRecipes);
    await waitFor(() => expect(ipc).toHaveBeenCalledTimes(1));
    const oldListener = events.get("workspace://changed")!;
    emit("workspace://changed", updated.id);
    first.unmount();

    await act(async () => initial.reject(new Error("已离页的目录查询失败")));
    expect(ipc).toHaveBeenCalledTimes(1);

    const reopened = renderHook(useRecipes);
    await waitFor(() => expect(reopened.result.current).toEqual([updated]));
    act(() => oldListener(updated.id));
    expect(ipc).toHaveBeenCalledTimes(2);
  });
});

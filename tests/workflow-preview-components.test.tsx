import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useTask } from "../src/features/workflow/components";

const finishTask = () => act(async () => { await vi.advanceTimersByTimeAsync(450); });

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe("预览任务的异步保护", () => {
  it("同次渲染重复操作只执行一次，失败可恢复，卸载取消待操作", async () => {
    const hook = renderHook(() => useTask()); const first = vi.fn(), second = vi.fn();
    act(() => { hook.result.current.run(first); hook.result.current.run(second); });
    expect(hook.result.current.busy).toBe(true); await finishTask(); expect(first).toHaveBeenCalledOnce(); expect(second).not.toHaveBeenCalled();
    act(() => hook.result.current.run(() => { throw new Error("示例操作失败"); })); await finishTask();
    expect(hook.result.current.error).toBe("示例操作失败"); expect(hook.result.current.busy).toBe(false);
    act(() => hook.result.current.run(second)); expect(hook.result.current.error).toBe(""); await finishTask(); expect(second).toHaveBeenCalledOnce();
    act(() => hook.result.current.run(first)); hook.unmount(); await finishTask(); expect(first).toHaveBeenCalledOnce();
  });

});

import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { Dialog, NumberField, useTask } from "../src/features/workflow/components";

const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
const finishTask = () => act(async () => { await vi.advanceTimersByTimeAsync(450); });

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe("预览组件的基本交互", () => {
  it("数值范围提示、离焦恢复和禁用状态", () => {
    function Numeric() { const [n, setN] = useState(5); return <NumberField label="数值" value={n} min={1} max={10} onChange={setN} />; }
    const view = render(<Numeric />);
    const field = screen.getByRole("spinbutton", { name: "数值" });
    fireEvent.change(field, { target: { value: "20" } }); expect(field).toHaveAttribute("aria-invalid", "true"); fireEvent.blur(field); expect(field).toHaveValue(10);
    fireEvent.change(field, { target: { value: "-1" } }); fireEvent.blur(field); expect(field).toHaveValue(1);
    const change = vi.fn(); view.rerender(<NumberField label="数值" value={3} onChange={change} disabled />); expect(screen.getByRole("spinbutton", { name: "数值" })).toBeDisabled();
  });

  it("同次渲染重复操作只执行一次，失败可恢复，卸载取消待操作", async () => {
    const hook = renderHook(() => useTask()); const first = vi.fn(), second = vi.fn();
    act(() => { hook.result.current.run(first); hook.result.current.run(second); });
    expect(hook.result.current.busy).toBe(true); await finishTask(); expect(first).toHaveBeenCalledOnce(); expect(second).not.toHaveBeenCalled();
    act(() => hook.result.current.run(() => { throw new Error("示例操作失败"); })); await finishTask();
    expect(hook.result.current.error).toBe("示例操作失败"); expect(hook.result.current.busy).toBe(false);
    act(() => hook.result.current.run(second)); expect(hook.result.current.error).toBe(""); await finishTask(); expect(second).toHaveBeenCalledOnce();
    act(() => hook.result.current.run(first)); hook.unmount(); await finishTask(); expect(first).toHaveBeenCalledOnce();
  });

  it("弹窗保持焦点循环与关闭后回到打开按钮", () => {
    vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([{ width: 20, height: 20 }] as unknown as DOMRectList);
    function Popup() { const [open, setOpen] = useState(false); return <><button onClick={() => setOpen(true)}>打开</button>{open && <Dialog title="操作确认" onClose={() => setOpen(false)}><button>第一项</button><button>最后项</button></Dialog>}</>; }
    render(<Popup />); const trigger = screen.getByRole("button", { name: "打开" }); trigger.focus(); click("打开");
    const close = screen.getByRole("button", { name: "关闭弹窗" }), last = screen.getByRole("button", { name: "最后项" });
    close.focus(); fireEvent.keyDown(close, { key: "Tab", shiftKey: true }); expect(last).toHaveFocus();
    fireEvent.keyDown(last, { key: "Tab" }); expect(close).toHaveFocus(); click("关闭弹窗"); expect(trigger).toHaveFocus();
  });
});

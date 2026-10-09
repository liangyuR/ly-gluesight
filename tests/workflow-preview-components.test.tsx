import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { Dialog, FrameCanvas, NumberField, useTask } from "../src/features/workflow/components";
import { click, finishTask, navigate, showWorkflow, stored } from "./workflow-preview-fixtures";

beforeEach(() => { sessionStorage.clear(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("预览图像与延迟操作", () => {
  it("缩放有上下界、适应窗口、切帧和换图回到初始倍率，叠加可显示与隐藏", () => {
    const view = render(<FrameCanvas id={3} imageId={43} overlay defect />);
    expect(screen.getByText("断胶 6.2 mm")).toBeVisible(); click("测量叠加");
    expect(screen.queryByText("断胶 6.2 mm")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "测量叠加" })).toHaveAttribute("aria-pressed", "false");
    click("测量叠加"); expect(screen.getByText("断胶 6.2 mm")).toBeVisible();
    for (let i = 0; i < 12; i++) click("放大图像"); expect(screen.getByText("300%")).toBeVisible();
    for (let i = 0; i < 12; i++) click("缩小图像"); expect(screen.getByText("75%")).toBeVisible();
    click("适应窗口"); expect(screen.getByText("100%")).toBeVisible(); click("放大图像");
    view.rerender(<FrameCanvas id={4} imageId={44} overlay defect />);
    expect(screen.getByText("100%")).toBeVisible(); expect(screen.getByRole("img", { name: "帧 k4 的冻结样本" })).toBeVisible();
    expect(screen.queryByText("断胶 6.2 mm")).not.toBeInTheDocument(); click("放大图像");
    view.rerender(<FrameCanvas id={4} imageId={80} overlay />); expect(screen.getByText("100%")).toBeVisible();
    view.rerender(<FrameCanvas id={4} imageId={80} missing overlay />); expect(screen.getByText("原图不可用", { selector: ".wf-image-footer span" })).toBeVisible();
    expect(screen.queryByRole("img")).not.toBeInTheDocument(); expect(screen.queryByRole("button", { name: "测量叠加" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "放大图像" })).toBeDisabled(); expect(screen.getByRole("button", { name: "缩小图像" })).toBeDisabled(); expect(screen.getByRole("button", { name: "适应窗口" })).toBeDisabled();
  });

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

class ControlledReader {
  static readers: ControlledReader[] = [];
  result: string | null = null;
  readyState = 0;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  abort = vi.fn(() => { this.readyState = 2; this.onabort?.(); });
  readAsDataURL = vi.fn(() => { this.readyState = 1; });
  constructor() { ControlledReader.readers.push(this); }
  succeed(result: string) { this.result = result; this.readyState = 2; this.onload?.(); }
  fail() { this.readyState = 2; this.onerror?.(); }
}
function upload(file: File) { fireEvent.change(screen.getByLabelText("导入工件背景图"), { target: { files: [file] } }); return ControlledReader.readers.at(-1)!; }

describe("总览背景导入", () => {
  beforeEach(() => { ControlledReader.readers = []; vi.stubGlobal("FileReader", ControlledReader); });

  it.each([new File(["invalid"], "document.txt", { type: "text/plain" }), new File([new Uint8Array(1024 * 1024 + 1)], "big.png", { type: "image/png" })])("拒绝不支持或过大文件 $name", file => {
    showWorkflow("overview"); upload(file); expect(screen.getByText("请选择小于 1 MB 的 PNG、JPEG 或 WebP 图片。")).toBeVisible(); expect(ControlledReader.readers).toHaveLength(0); expect(stored().overview.background).toBeNull();
  });

  it.each(["image/png", "image/jpeg", "image/webp"])("可导入 %s 并保存，恢复底图后允许重新选择同一个文件", type => {
    showWorkflow("overview"); const selected = new File(["illustration"], "sample", { type });
    const reader = upload(selected); expect(screen.getByRole("button", { name: "保存布局" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存布局并进入验证" })).toBeDisabled(); expect(screen.getByText("正在读取工件图…")).toBeVisible();
    act(() => reader.succeed("data:" + type + ";base64,new"));
    expect(document.querySelector(".wf-overview-svg image")).toHaveAttribute("href", "data:" + type + ";base64,new");
    click("保存布局"); expect(stored().overview.saved).toBe(true); click("恢复几何底图"); expect(stored().overview.background).toBeNull();
    const retry = upload(selected); act(() => retry.succeed("data:" + type + ";base64,again")); expect(stored().overview.background).toContain("again");
  });

  it("读取失败可重试，替换中的旧响应和卸载响应不覆盖布局", () => {
    const view = showWorkflow("overview"); const file = new File(["illustration"], "sample.png", { type: "image/png" });
    const failed = upload(file); act(() => failed.fail()); expect(screen.getByText("图像读取失败，请重试。")).toBeVisible(); expect(screen.getByRole("button", { name: "保存布局" })).toBeEnabled();
    const first = upload(file); const staleCallback = first.onload!;
    const second = upload(file); expect(first.abort).toHaveBeenCalledOnce(); act(() => second.succeed("data:image/png;base64,new"));
    first.result = "data:image/png;base64,old"; act(() => staleCallback()); expect(stored().overview.background).toContain("new");
    const third = upload(file); const unmountedCallback = third.onload!; view.unmount();
    expect(third.abort).toHaveBeenCalledOnce(); third.result = "data:image/png;base64,unmounted"; act(() => unmountedCallback()); expect(stored().overview.background).toContain("new");
  });

  it("移除底图会取消待替换文件，离开总览不再提交待导入图像", () => {
    showWorkflow("overview"); const file = new File(["x"], "image.png", { type: "image/png" });
    const first = upload(file); act(() => first.succeed("data:image/png;base64,original"));
    const next = upload(file); const stale = next.onload!; click("恢复几何底图"); next.result = "data:image/png;base64,stale"; act(stale);
    expect(stored().overview.background).toBeNull(); const last = upload(file); const left = last.onload!; navigate("配方库"); last.result = "data:image/png;base64,left"; act(left);
    expect(stored().overview.background).toBeNull(); expect(last.abort).toHaveBeenCalledOnce();
  });
});

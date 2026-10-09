import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import Modal from "../src/features/plc/components/Modal";
import WriteDialog from "../src/features/plc/components/WriteDialog";
import type { PlcPoint } from "../src/features/plc/types";
import { deferred } from "./fixtures";

const point: PlcPoint = { id: "speed", name: "速度", address: "D100", dataType: "f32", wordOrder: "ABCD",
  access: "readWrite", edge: "none", logChanges: true, tags: [], description: "" };

describe("PLC 写入", () => {
  it.each(["", "abc", "Infinity", "-Infinity"])("拒绝无效输入 %j", async value => {
    const onWrite = vi.fn(), onClose = vi.fn();
    render(<WriteDialog point={point} current={undefined} onWrite={onWrite} onClose={onClose} />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value } });
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    expect(screen.getByText("请输入有效数值")).toBeVisible();
    expect(onWrite).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("预填当前值并写入数值，成功后关闭", async () => {
    const onWrite = vi.fn().mockResolvedValue(undefined), onClose = vi.fn();
    render(<WriteDialog point={point} current={{ value: 12, error: null, ts: 1 }} onWrite={onWrite} onClose={onClose} />);
    expect(screen.getByRole("textbox")).toHaveValue("12");
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "2.5" } });
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    expect(onWrite).toHaveBeenCalledWith(2.5);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it.each([true, false])("布尔按钮写入 %s", async value => {
    const onWrite = vi.fn().mockResolvedValue(undefined);
    render(<WriteDialog point={{ ...point, dataType: "bool" }} current={undefined} onWrite={onWrite} onClose={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: value ? "置 1 (ON)" : "置 0 (OFF)" }));
    expect(onWrite).toHaveBeenCalledWith(value);
  });

  it("写入等待期间，回车不能重复提交", async () => {
    const request = deferred<void>(), onWrite = vi.fn(() => request.promise);
    render(<WriteDialog point={point} current={undefined} onWrite={onWrite} onClose={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "3" } });
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    expect(screen.getByRole("button", { name: "写入" })).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    expect(onWrite).toHaveBeenCalledTimes(1);
    await act(async () => request.resolve());
  });

  it("失败显示错误，保留输入并允许重试", async () => {
    const onWrite = vi.fn().mockRejectedValueOnce(new Error("通讯中断")).mockResolvedValue(undefined), onClose = vi.fn();
    render(<WriteDialog point={point} current={{ value: 8, error: null, ts: 1 }} onWrite={onWrite} onClose={onClose} />);
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    expect(await screen.findByText("Error: 通讯中断")).toBeVisible();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox")).toHaveValue("8");
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("写入完成前冻结输入并阻止取消、Esc 和遮罩关闭，失败后恢复关闭", async () => {
    const request = deferred<void>(), onClose = vi.fn();
    render(<WriteDialog point={point} current={{ value: 8, error: null, ts: 1 }} onWrite={() => request.promise} onClose={onClose} />);
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    expect(screen.getByRole("textbox")).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "关闭" })).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    fireEvent.mouseDown(screen.getByRole("dialog").parentElement!);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => request.reject(new Error("写入失败")));
    expect(screen.getByRole("textbox")).toBeEnabled();
    await userEvent.keyboard("{Escape}"); expect(onClose).toHaveBeenCalledTimes(1);
  });

  it.each(["resolve", "reject"])("关闭所在页面后写入 %s 不执行旧弹窗回调", async outcome => {
    const request = deferred<void>(), onClose = vi.fn();
    const { unmount } = render(<WriteDialog point={point} current={{ value: 8, error: null, ts: 1 }} onWrite={() => request.promise} onClose={onClose} />);
    await userEvent.click(screen.getByRole("button", { name: "写入" })); unmount();
    await act(async () => { if (outcome === "resolve") request.resolve(); else request.reject(new Error("页面已离开")); });
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("对话框键盘与焦点", () => {
  it("具备可访问名称，焦点限制在弹窗内，关闭后还原焦点", async () => {
    const trigger = document.createElement("button");
    document.body.append(trigger); trigger.focus();
    const { unmount } = render(<Modal title="参数" onClose={vi.fn()}><input aria-label="数值" /><button disabled>禁用</button><button>保存</button></Modal>);
    expect(screen.getByRole("dialog", { name: "参数" })).toHaveAttribute("aria-modal", "true");
    await waitFor(() => expect(screen.getByRole("textbox")).toHaveFocus());
    await userEvent.tab(); expect(screen.getByRole("button", { name: "保存" })).toHaveFocus();
    await userEvent.tab(); expect(screen.getByRole("button", { name: "关闭" })).toHaveFocus();
    await userEvent.tab({ shift: true }); expect(screen.getByRole("button", { name: "保存" })).toHaveFocus();
    unmount(); expect(trigger).toHaveFocus(); trigger.remove();
  });

  it("Escape 使用最新的关闭回调，点击内容不关闭，点击遮罩关闭", async () => {
    const first = vi.fn(), latest = vi.fn();
    const { rerender } = render(<Modal title="参数" onClose={first}><p>内容</p></Modal>);
    rerender(<Modal title="参数" onClose={latest}><p>内容</p></Modal>);
    await userEvent.keyboard("{Escape}");
    expect(first).not.toHaveBeenCalled(); expect(latest).toHaveBeenCalledTimes(1);
    fireEvent.mouseDown(screen.getByText("内容")); expect(latest).toHaveBeenCalledTimes(1);
    fireEvent.mouseDown(screen.getByRole("dialog").parentElement!); expect(latest).toHaveBeenCalledTimes(2);
  });
});

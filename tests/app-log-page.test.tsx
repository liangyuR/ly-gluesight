import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { expect, it, vi } from "vitest";
import type { OperationProgress } from "../src/features/workspace/operation";
import AppLogPage from "../src/pages/AppLogPage";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

it("直接导出失败后重试 → 显示文件路径与未能打包的资料", async () => {
  let firstExport = true;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "app_log_tail") return { cursor: 0, lines: [], skipped: false };
    if (command === "operation_status") return null;
    if (command !== "diagnostics_export") throw new Error("意外调用 " + command);
    if (firstExport) {
      firstExport = false;
      throw new Error("导出目录不可写");
    }
    return { path: "exports/diagnostics.zip", notes: ["本件原图已清理"] };
  });
  const user = userEvent.setup();
  render(<AppLogPage />);

  await user.click(screen.getByRole("button", { name: "导出诊断包" }));
  expect(await screen.findByText("导出目录不可写")).toBeVisible();
  await user.click(await screen.findByRole("button", { name: "导出诊断包" }));
  expect(await screen.findByText("exports/diagnostics.zip")).toBeVisible();
  expect(screen.getByText("本件原图已清理")).toBeVisible();
  expect(screen.queryByText("导出目录不可写")).not.toBeInTheDocument();
});

it("取消正在打包的诊断导出 → 向当前任务发取消请求并可重试", async () => {
  let receive!: (event: { payload: OperationProgress }) => void;
  vi.mocked(listen).mockImplementationOnce(async (_, callback) => {
    receive = callback as typeof receive;
    return () => {};
  });
  let taskId = "";
  let rejectExport!: (reason: string) => void;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "app_log_tail") return { cursor: 0, lines: [], skipped: false };
    if (command === "operation_status") return null;
    if (command === "operation_cancel") return true;
    if (command !== "diagnostics_export") throw new Error("意外调用 " + command);
    if (taskId) return { path: "exports/retry.zip", notes: [] };
    taskId = String((args as { taskId?: string } | undefined)?.taskId ?? "");
    return new Promise((_, reject) => { rejectExport = reject; });
  });
  const user = userEvent.setup();
  render(<AppLogPage />);

  await user.click(screen.getByRole("button", { name: "导出诊断包" }));
  await waitFor(() => expect(taskId).not.toBe(""));
  act(() => receive({ payload: {
    taskId, stage: "packing_records", completed: 0, total: 0, failed: 0, skipped: 0, elapsedMs: 500,
  } }));
  expect(screen.getByRole("status", { name: "导出诊断包进度" })).toHaveTextContent("正在打包检测记录");
  expect(screen.getByRole("progressbar", { name: "导出诊断包完成进度" })).not.toHaveAttribute("value");
  await user.click(screen.getByRole("button", { name: "取消" }));
  expect(invoke).toHaveBeenCalledWith("operation_cancel", { taskId });
  expect(screen.getByRole("status", { name: "导出诊断包进度" })).toHaveTextContent("正在取消导出");

  await act(async () => { rejectExport("OPERATION_CANCELLED: 任务已取消，原结果未更改"); });
  expect(screen.getByRole("status", { name: "导出诊断包进度" })).toHaveTextContent("已取消导出");
  expect(screen.queryByText("诊断包已导出")).not.toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  await user.click(await screen.findByRole("button", { name: "导出诊断包" }));
  expect(await screen.findByText("exports/retry.zip")).toBeVisible();
  expect(screen.queryByRole("status", { name: "导出诊断包进度" })).not.toBeInTheDocument();
});

it("向上查看旧日志 → 停止刷新，回到最新后从原游标继续", async () => {
  vi.useFakeTimers();
  try {
    const read = vi.mocked(invoke).mockReset()
      .mockResolvedValueOnce({ cursor: 2, lines: ["旧记录"], skipped: false })
      .mockResolvedValueOnce({ cursor: 500, lines: ["新记录"], skipped: true });
    render(<AppLogPage />);
    const view = screen.getByRole("region", { name: "日志内容" });
    await act(() => vi.advanceTimersByTimeAsync(0));
    Object.defineProperties(view, { clientHeight: { value: 100 }, scrollHeight: { value: 400 } });
    view.scrollTop = 50;
    fireEvent.scroll(view);
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(read).toHaveBeenCalledTimes(1);
    expect(view).toHaveTextContent("旧记录");

    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "回到最新" })); });
    expect(read).toHaveBeenLastCalledWith("app_log_tail", { after: 2 });
    expect(view).toHaveTextContent("新记录");
    expect(view).not.toHaveTextContent("旧记录");
    expect(screen.getByText("日志量较大，已跳至最近记录")).toBeVisible();
  } finally {
    vi.useRealTimers();
  }
});

it("慢请求和突发日志 → 串行续读游标、限制显示条数并在离页后停止", async () => {
  vi.useFakeTimers();
  try {
    let finish!: (result: unknown) => void;
    const read = vi.mocked(invoke).mockReset().mockImplementation(
      () => new Promise(resolve => { finish = resolve; }),
    );
    const page = render(<AppLogPage />);
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(read.mock.calls).toEqual([["app_log_tail", { after: null }]]);
    const lines = Array.from({ length: 200 }, (_, n) => `记录 [${n + 1}]`);
    await act(async () => finish({ cursor: 200, lines, skipped: false }));
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(read).toHaveBeenLastCalledWith("app_log_tail", { after: 200 });
    await act(async () => finish({ cursor: 201, lines: ["记录 [201]"], skipped: false }));
    const view = screen.getByRole("region", { name: "日志内容" });
    expect(view).not.toHaveTextContent("记录 [1]");
    expect(view).toHaveTextContent("记录 [2]");
    expect(view).toHaveTextContent("记录 [201]");

    page.unmount();
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(read).toHaveBeenCalledTimes(2);
  } finally {
    vi.useRealTimers();
  }
});

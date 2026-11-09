import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { desktopCall } from "../src/lib/desktop";
import { Operation, OperationStatus, useOperation, type OperationProgress } from "../src/features/workspace/operation";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: () => true, desktopCall: vi.fn() }));
let receive: (event: { payload: OperationProgress }) => void;
beforeEach(() => {
  vi.mocked(desktopCall).mockResolvedValue(null);
  vi.mocked(listen).mockImplementation(async (_, callback) => {
    receive = callback as typeof receive;
    return () => {};
  });
});
const progress = (taskId: string, stage: OperationProgress["stage"], completed = 0): OperationProgress => ({ taskId, stage, completed, total: 1000, failed: 0, skipped: 0, elapsedMs: 250 });

it("旧任务进度到达且测量结束尚未保存 → 只显示当前任务且不提前报完成", async () => {
  const operation = new Operation();
  let taskId = "";
  let finish!: (value: string) => void;
  render(<OperationStatus operation={operation} label="历史复算" />);
  let running!: Promise<string>;
  act(() => { running = operation.run(id => { taskId = id; return new Promise(resolve => { finish = resolve; }); }); });
  await waitFor(() => expect(taskId).not.toBe(""));
  act(() => receive({ payload: progress("old-task", "completed", 1000) }));
  expect(screen.getByRole("status")).toHaveTextContent("正在校验数据");
  act(() => receive({ payload: progress(taskId, "measuring", 250) }));
  expect(screen.getByRole("progressbar")).toHaveAttribute("value", "25");
  expect(screen.getByRole("button", { name: "取消" })).toBeEnabled();
  act(() => receive({ payload: progress(taskId, "saving", 1000) }));
  expect(screen.getByRole("progressbar")).toHaveAttribute("value", "99");
  expect(screen.getByRole("button", { name: "保存中" })).toBeDisabled();
  vi.mocked(desktopCall).mockResolvedValue({ ...progress(taskId, "completed", 1000), failed: 5 });
  await act(async () => { finish("saved-result"); await running; });
  expect(screen.getByRole("progressbar")).toHaveAttribute("value", "100");
  expect(screen.getByRole("status")).toHaveTextContent("已完成");
  expect(screen.getByRole("status")).toHaveTextContent("已处理 1000 / 1000");
  expect(screen.getByRole("status")).toHaveTextContent("测量失败 5");
});

it("取消回复先于终态事件 → 显示已取消且保留旧结果，再次运行不受旧进度影响", async () => {
  const operation = new Operation();
  let taskId = "";
  let fail!: (reason: string) => void;
  let saved = "previous-result";
  render(<OperationStatus operation={operation} label="测量示教件" />);
  let running!: Promise<void>;
  act(() => {
    running = operation.run<string>(id => { taskId = id; return new Promise((_, reject) => { fail = reject; }); })
      .then(value => { saved = value; }, () => {});
  });
  await waitFor(() => expect(taskId).not.toBe(""));
  act(() => receive({ payload: progress(taskId, "measuring", 120) }));
  vi.mocked(desktopCall).mockImplementation(async command => {
    if (command === "operation_cancel") { fail("OPERATION_CANCELLED: 任务已取消"); return true as never; }
    return null as never;
  });
  await userEvent.click(screen.getByRole("button", { name: "取消" }));
  await act(async () => { await running; });
  expect(screen.getByRole("status")).toHaveTextContent("已取消，原结果保留");
  expect(saved).toBe("previous-result");
  expect(screen.queryByRole("alert")).toBeNull();
  const oldTask = taskId;
  let finish!: (value: string) => void;
  let next!: Promise<string>;
  act(() => { next = operation.run(id => { taskId = id; return new Promise(resolve => { finish = resolve; }); }); });
  await waitFor(() => expect(taskId).not.toBe(oldTask));
  act(() => receive({ payload: progress(oldTask, "completed", 1000) }));
  expect(screen.getByRole("status")).toHaveTextContent("正在校验数据");
  await act(async () => { finish("new-result"); saved = await next; });
  expect(saved).toBe("new-result");
});

it("进度订阅尚未建立就离开页面 → 不再启动后台测量或覆盖原结果", async () => {
  let ready!: (stop: () => void) => void;
  vi.mocked(listen).mockImplementation(() => new Promise(resolve => { ready = resolve; }));
  let saved = "previous-result";
  let result!: Promise<string>;
  function Page() {
    const operation = useOperation();
    return <button onClick={() => {
      result = operation.run(async () => { saved = "replacement"; return saved; }).catch(error => error.message);
    }}>测量</button>;
  }
  const page = render(<Page />);
  await userEvent.click(screen.getByRole("button", { name: "测量" }));
  page.unmount();
  ready(() => {});
  expect(await result).toContain("任务已取消");
  expect(saved).toBe("previous-result");
});

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PlcLogPage from "../src/features/plc/pages/PlcLogPage";
import { plcApi } from "../src/features/plc/api";
import type { LogEntry, LogPage } from "../src/features/plc/types";
import { deferred } from "./fixtures";
import { plcConfig } from "./plc-fixtures";

const events = vi.hoisted(() => ({ callback: null as (() => void) | null, unsubscribe: vi.fn() }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getConfig: vi.fn(), queryLogs: vi.fn(), pointHistory: vi.fn() },
  subscribe: vi.fn((_event: string, cb: () => void) => { events.callback = cb; return events.unsubscribe; }),
}));
const entry: LogEntry = { id: 1, ts: 1, level: "info", category: "value", pointId: "speed", pointName: "速度",
  message: "第一条日志", oldValue: "0", newValue: "1" };
beforeEach(() => {
  events.callback = null; events.unsubscribe.mockReset();
  vi.mocked(plcApi.getConfig).mockResolvedValue(plcConfig()); vi.mocked(plcApi.queryLogs).mockImplementation(async q => ({ total: q.limit === 1000 ? 1 : 201, items: [entry] }));
  vi.mocked(plcApi.pointHistory).mockResolvedValue([{ ts: 1, value: "1" }]);
});
async function show() { const page = render(<PlcLogPage />); await screen.findByText("第一条日志"); return page; }
function downloads() {
  const create = vi.fn((_blob: Blob) => "blob:csv"); const revoke = vi.fn();
  Object.defineProperty(URL, "createObjectURL", { configurable: true, value: create }); Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: revoke });
  const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
  return { create, revoke, click };
}
const blobText = (blob: Blob) => new Promise<string>((resolve, reject) => {
  const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsText(blob);
});
describe("通讯日志查询与导出", () => {

  it("自定义时间停用跟随，错误时间段阻止请求并显示原因", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "自定义" }));
    expect(screen.getByRole("button", { name: "实时跟随" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("开始时间"), { target: { value: "2026-10-09T10:00:00" } });
    fireEvent.change(screen.getByLabelText("结束时间"), { target: { value: "2026-10-08T10:00:00" } });
    expect(await screen.findByText(/开始时间不得晚于结束时间/)).toBeVisible();
    const calls = vi.mocked(plcApi.queryLogs).mock.calls.length; await userEvent.click(screen.getByRole("button", { name: "刷新" }));
    expect(plcApi.queryLogs).toHaveBeenCalledTimes(calls);
    fireEvent.change(screen.getByLabelText("结束时间"), { target: { value: "2026-10-10T10:00:00" } });
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ start: new Date("2026-10-09T10:00:00").getTime(), end: new Date("2026-10-10T10:00:00").getTime() })));
  });
  it.each(["success", "error"])("旧查询的 %s 不覆盖新筛选结果", async kind => {
    const old = deferred<LogPage>(); vi.mocked(plcApi.queryLogs).mockReturnValueOnce(old.promise);
    render(<PlcLogPage />); await userEvent.click(screen.getByRole("button", { name: "15 分钟" }));
    expect(await screen.findByText("第一条日志")).toBeVisible();
    await act(async () => kind === "success" ? old.resolve({ total: 1, items: [{ ...entry, message: "过期日志" }] }) : old.reject(new Error("过期错误")));
    expect(screen.getByText("第一条日志")).toBeVisible(); expect(screen.queryByText("过期日志")).toBeNull(); expect(screen.queryByText("Error: 过期错误")).toBeNull();
  });

  it("实时事件合并刷新，关闭跟随与卸载时清理订阅", async () => {
    const page = await show(); const calls = vi.mocked(plcApi.queryLogs).mock.calls.length;
    act(() => { events.callback?.(); events.callback?.(); });
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenCalledTimes(calls + 1), { timeout: 1500 });
    await userEvent.click(screen.getByRole("button", { name: "实时跟随中" })); expect(events.unsubscribe).toHaveBeenCalledTimes(1);
    page.unmount();
  });
  it("按固定筛选完整导出、转义 CSV，等待期间不重复导出", async () => {
    const now = new Date("2026-10-09T12:00:00+08:00").getTime(); const clock = vi.spyOn(Date, "now").mockReturnValue(now);
    await show(); const request = deferred<LogPage>(); vi.mocked(plcApi.queryLogs).mockReturnValueOnce(request.promise).mockResolvedValueOnce({ total: 1001, items: [{ ...entry, message: '胶宽,"正常"\r下一行' }] });
    const { create, revoke } = downloads();
    const button = screen.getByRole("button", { name: "导出 CSV" }); fireEvent.click(button); fireEvent.click(button);
    expect(screen.getByRole("button", { name: "导出中…" })).toBeDisabled();
    clock.mockReturnValue(now + 30_000);
    await act(async () => request.resolve({ total: 1001, items: Array.from({ length: 1000 }, (_, i) => ({ ...entry, id: i })) }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(plcApi.queryLogs).toHaveBeenCalledWith(expect.objectContaining({ limit: 1000, offset: 0 }));
    expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 1000, offset: 1000 }));
    const batches = vi.mocked(plcApi.queryLogs).mock.calls.map(([q]) => q).filter(q => q.limit === 1000);
    expect(batches).toHaveLength(2); expect(batches.every(q => q.end === now && q.start === now - 3_600_000)).toBe(true);
    expect(revoke).toHaveBeenCalledWith("blob:csv");
    expect(await blobText(create.mock.calls[0][0])).toContain('"胶宽,""正常""\r下一行"');
  });

  it.each(["success", "error"])("切换筛选后旧导出的 %s 不下载或解锁新的导出", async kind => {
    await show(); const old = deferred<LogPage>(), current = deferred<LogPage>(); const { create } = downloads();
    vi.mocked(plcApi.queryLogs).mockReturnValueOnce(old.promise); fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    await userEvent.click(screen.getByRole("button", { name: "15 分钟" })); await screen.findByText("第一条日志");
    vi.mocked(plcApi.queryLogs).mockReturnValueOnce(current.promise); fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    await act(async () => kind === "success" ? old.resolve({ total: 1, items: [entry] }) : old.reject(new Error("过期导出失败")));
    expect(create).not.toHaveBeenCalled(); expect(screen.queryByText("Error: 过期导出失败")).toBeNull(); expect(screen.getByRole("button", { name: "导出中…" })).toBeDisabled();
    await act(async () => current.resolve({ total: 1, items: [entry] })); expect(create).toHaveBeenCalledTimes(1); expect(screen.getByRole("button", { name: "导出 CSV" })).toBeEnabled();
  });

  it("超过五万条继续完整导出，不静默截断", async () => {
    await show(); const { create } = downloads();
    vi.mocked(plcApi.queryLogs).mockImplementation(async q => {
      const offset = q.offset ?? 0;
      return { total: 50_001, items: Array.from({ length: Math.min(1000, 50_001 - offset) }, (_, i) => ({ ...entry, id: offset + i, message: offset + i === 50_000 ? "最后一条" : entry.message })) };
    });
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 1000, offset: 50_000 }));
    expect(await blobText(create.mock.calls[0][0])).toContain("最后一条");
  });

  it("导出途中数量不再完整时提示重试，不下载部分文件", async () => {
    await show(); const { create } = downloads();
    vi.mocked(plcApi.queryLogs).mockResolvedValueOnce({ total: 2, items: [entry] }).mockResolvedValueOnce({ total: 0, items: [] });
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" })); expect(await screen.findByText(/未下载不完整文件/)).toBeVisible();
    expect(create).not.toHaveBeenCalled(); expect(screen.getByRole("button", { name: "导出 CSV" })).toBeEnabled();
  });

  it("下载浏览器动作失败仍释放临时地址并可重试", async () => {
    await show(); const { create, revoke, click } = downloads(); click.mockImplementationOnce(() => { throw new Error("浏览器下载失败"); });
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" })); expect(await screen.findByText("Error: 浏览器下载失败")).toBeVisible();
    expect(revoke).toHaveBeenCalledWith("blob:csv"); await userEvent.click(screen.getByRole("button", { name: "导出 CSV" })); expect(create).toHaveBeenCalledTimes(2);
  });
});

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
  it("时间、级别、类别、点位和回车搜索形成组合条件，翻页和筛选复位", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "下一页" }));
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 100 })));
    await userEvent.click(screen.getByRole("button", { name: "警告" }));
    await userEvent.click(screen.getByRole("button", { name: "写入" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "日志点位" }), "speed");
    const keyword = screen.getByPlaceholderText("搜索内容或点位名称，回车确认");
    fireEvent.change(keyword, { target: { value: "  断胶  " } }); expect(plcApi.queryLogs).not.toHaveBeenCalledWith(expect.objectContaining({ keyword: "断胶" }));
    fireEvent.keyDown(keyword, { key: "Enter" });
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 0, limit: 100, levels: ["warn"], categories: ["write"], pointId: "speed", keyword: "断胶" })));
    expect(await screen.findByText("点位趋势 · 速度")).toBeVisible(); expect(plcApi.pointHistory).toHaveBeenCalledWith("speed", expect.any(Number), expect.any(Number));
    await userEvent.click(screen.getByRole("button", { name: "清空搜索" }));
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ keyword: null })));
  });
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
  it("查询失败可刷新恢复，点击日志点位开启趋势", async () => {
    vi.mocked(plcApi.queryLogs).mockRejectedValueOnce(new Error("查询失败")); render(<PlcLogPage />);
    expect(await screen.findByText("Error: 查询失败")).toBeVisible(); await userEvent.click(screen.getByRole("button", { name: "刷新" }));
    await screen.findByText("第一条日志"); await userEvent.click(screen.getByRole("button", { name: "速度" }));
    expect(await screen.findByText("点位趋势 · 速度")).toBeVisible();
  });
  it("实时事件合并刷新，关闭跟随与卸载时清理订阅", async () => {
    const page = await show(); const calls = vi.mocked(plcApi.queryLogs).mock.calls.length;
    act(() => { events.callback?.(); events.callback?.(); });
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenCalledTimes(calls + 1), { timeout: 1500 });
    await userEvent.click(screen.getByRole("button", { name: "实时跟随中" })); expect(events.unsubscribe).toHaveBeenCalledTimes(1);
    page.unmount();
  });
  it("导出沿用筛选，按批导出全部页并转义引号逗号，重复点击只导出一次", async () => {
    await show(); const request = deferred<LogPage>(); vi.mocked(plcApi.queryLogs).mockReturnValueOnce(request.promise).mockResolvedValueOnce({ total: 1001, items: [{ ...entry, message: '胶宽,"正常"' }] });
    const create = vi.fn((_blob: Blob) => "blob:csv"); const revoke = vi.fn();
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: create }); Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: revoke });
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const button = screen.getByRole("button", { name: "导出 CSV" }); fireEvent.click(button); fireEvent.click(button);
    expect(screen.getByRole("button", { name: "导出中…" })).toBeDisabled();
    await act(async () => request.resolve({ total: 1001, items: Array.from({ length: 1000 }, (_, i) => ({ ...entry, id: i })) }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(plcApi.queryLogs).toHaveBeenCalledWith(expect.objectContaining({ limit: 1000, offset: 0 }));
    expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 1000, offset: 1000 }));
    expect(revoke).toHaveBeenCalledWith("blob:csv");
    const text = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsText(create.mock.calls[0][0]); });
    expect(text).toContain('"胶宽,""正常"""');
  });
  it("导出请求失败显示原因并解锁，保留原查询结果", async () => {
    await show(); vi.mocked(plcApi.queryLogs).mockRejectedValueOnce(new Error("导出失败"));
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(await screen.findByText("Error: 导出失败")).toBeVisible(); expect(screen.getByRole("button", { name: "导出 CSV" })).toBeEnabled(); expect(screen.getByText("第一条日志")).toBeVisible();
  });

  it("所有预设时间窗口使用当前时刻，上一页返回首屏", async () => {
    const now = new Date("2026-10-09T12:00:00+08:00").getTime(); vi.spyOn(Date, "now").mockReturnValue(now);
    await show();
    for (const [label, ms] of [["15 分钟", 900_000], ["1 小时", 3_600_000], ["24 小时", 86_400_000], ["7 天", 604_800_000]] as const) {
      await userEvent.click(screen.getByRole("button", { name: label }));
      expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ start: now - ms, end: null, offset: 0 }));
      expect(screen.getByRole("button", { name: label })).toHaveAttribute("aria-pressed", "true");
    }
    await userEvent.click(screen.getByRole("button", { name: "下一页" }));
    await userEvent.click(screen.getByRole("button", { name: "上一页" }));
    expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 0 }));
    expect(screen.getByRole("button", { name: "上一页" })).toBeDisabled();
  });

  it("所有级别与类别可组合并逐个取消", async () => {
    await show();
    const levelNames = ["信息", "警告", "错误"], categoryNames = ["连接", "值变化", "写入", "边沿", "读取异常", "配置"];
    for (const name of [...levelNames, ...categoryNames]) await userEvent.click(screen.getByRole("button", { name }));
    expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ levels: ["info", "warn", "error"], categories: ["connection", "value", "write", "edge", "error", "config"] }));
    for (const name of [...levelNames, ...categoryNames]) await userEvent.click(screen.getByRole("button", { name }));
    expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ levels: [], categories: [] }));
    expect(screen.getByRole("button", { name: "信息" })).toHaveAttribute("aria-pressed", "false");
  });

  it("清空筛选一次恢复时间、所有类别级别、点位、搜索与跟随", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "警告" })); await userEvent.click(screen.getByRole("button", { name: "边沿" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "日志点位" }), "speed");
    const keyword = screen.getByPlaceholderText("搜索内容或点位名称，回车确认"); fireEvent.change(keyword, { target: { value: "异常" } }); fireEvent.keyDown(keyword, { key: "Enter" });
    await userEvent.click(screen.getByRole("button", { name: "自定义" })); await userEvent.click(screen.getByRole("button", { name: "清空筛选" }));
    await waitFor(() => expect(plcApi.queryLogs).toHaveBeenLastCalledWith(expect.objectContaining({ levels: [], categories: [], pointId: null, keyword: null, end: null, offset: 0 })));
    expect(screen.getByRole("combobox", { name: "日志点位" })).toHaveValue(""); expect(keyword).toHaveValue("");
    expect(screen.queryByLabelText("开始时间")).toBeNull(); expect(screen.getByRole("button", { name: "实时跟随中" })).toBeEnabled();
    expect(screen.queryByText("点位趋势 · 速度")).toBeNull();
  });

  it("清空自定义时间阻止查询与导出，修正后可恢复", async () => {
    await show(); await userEvent.click(screen.getByRole("button", { name: "自定义" }));
    fireEvent.change(screen.getByLabelText("开始时间"), { target: { value: "" } }); const calls = vi.mocked(plcApi.queryLogs).mock.calls.length;
    const { create } = downloads(); await userEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(plcApi.queryLogs).toHaveBeenCalledTimes(calls); expect(create).not.toHaveBeenCalled(); expect(screen.getAllByText(/请选择有效时间段/).length).toBeGreaterThan(0);
    await userEvent.click(screen.getByRole("button", { name: "清空筛选" })); await screen.findByText("第一条日志");
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" })); expect(create).toHaveBeenCalledTimes(1);
  });

  it("点位配置失败不会被日志成功抹掉，可单独重试加载", async () => {
    vi.mocked(plcApi.getConfig).mockRejectedValueOnce(new Error("点位配置不可读")); await show();
    expect(screen.getByRole("alert")).toHaveTextContent("点位配置不可读");
    await userEvent.click(screen.getByRole("button", { name: "刷新" })); expect(screen.getByRole("alert")).toHaveTextContent("点位配置不可读");
    await userEvent.click(screen.getByRole("button", { name: "重新加载点位" }));
    await waitFor(() => expect(screen.getByRole("option", { name: "速度" })).toBeInTheDocument()); expect(screen.queryByRole("alert")).toBeNull();
  });

  it("点位趋势失败保留已查询日志，刷新同时重试趋势", async () => {
    await show(); vi.mocked(plcApi.pointHistory).mockRejectedValueOnce(new Error("趋势不可读"));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "日志点位" }), "speed");
    expect(await screen.findByText("点位趋势：Error: 趋势不可读")).toBeVisible(); expect(screen.getByText("第一条日志")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "刷新" })); expect(await screen.findByText("点位趋势 · 速度")).toBeVisible();
    expect(screen.queryByText("点位趋势：Error: 趋势不可读")).toBeNull();
  });

  it.each(["success", "error"])("旧点位趋势的 %s 不污染取消点位后的日志", async kind => {
    await show(); const old = deferred<Awaited<ReturnType<typeof plcApi.pointHistory>>>(); vi.mocked(plcApi.pointHistory).mockReturnValueOnce(old.promise);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "日志点位" }), "speed");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "日志点位" }), ""); await screen.findByText("第一条日志");
    await act(async () => kind === "success" ? old.resolve([{ ts: 4, value: "旧趋势" }]) : old.reject(new Error("旧趋势失败")));
    expect(screen.queryByText(/点位趋势 ·/)).toBeNull(); expect(screen.queryByText(/旧趋势失败/)).toBeNull(); expect(screen.getByText("第一条日志")).toBeVisible();
  });

  it("已移除配置的日志点位仍可筛选，并明确显示历史 ID", async () => {
    vi.mocked(plcApi.queryLogs).mockResolvedValue({ total: 1, items: [{ ...entry, pointId: "removed", pointName: "旧点位" }] });
    render(<PlcLogPage />); await userEvent.click(await screen.findByRole("button", { name: "旧点位" }));
    expect(screen.getByRole("combobox", { name: "日志点位" })).toHaveValue("removed"); expect(screen.getByRole("option", { name: "removed（已移除）" })).toBeInTheDocument();
    expect(plcApi.pointHistory).toHaveBeenCalledWith("removed", expect.any(Number), expect.any(Number));
    expect(await screen.findByText("点位趋势 · removed")).toBeVisible();
  });

  it("导出失败后可重新下载，导出错误不被日志刷新掩盖", async () => {
    await show(); vi.mocked(plcApi.queryLogs).mockRejectedValueOnce(new Error("读取导出失败")); const { create } = downloads();
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" })); expect(await screen.findByText("Error: 读取导出失败")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "刷新" })); expect(screen.getByText("Error: 读取导出失败")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "导出 CSV" })); expect(create).toHaveBeenCalledTimes(1); expect(screen.queryByText("Error: 读取导出失败")).toBeNull();
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

  it("卸载停止旧导出的后续分页与下载", async () => {
    const page = await show(), request = deferred<LogPage>(); const { create } = downloads();
    vi.mocked(plcApi.queryLogs).mockReturnValueOnce(request.promise); fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    const calls = vi.mocked(plcApi.queryLogs).mock.calls.length; page.unmount();
    await act(async () => request.resolve({ total: 1001, items: Array.from({ length: 1000 }, (_, id) => ({ ...entry, id })) }));
    expect(create).not.toHaveBeenCalled(); expect(plcApi.queryLogs).toHaveBeenCalledTimes(calls);
  });

  it("分批导出固定截止时间，包含 CR 的字段完整转义", async () => {
    const now = new Date("2026-10-09T12:00:00+08:00").getTime(); const clock = vi.spyOn(Date, "now").mockReturnValue(now);
    await show(); const { create } = downloads(); const first = deferred<LogPage>();
    vi.mocked(plcApi.queryLogs).mockReturnValueOnce(first.promise).mockResolvedValueOnce({ total: 1001, items: [{ ...entry, id: 1001, message: "第一行\r第二行" }] });
    fireEvent.click(screen.getByRole("button", { name: "导出 CSV" })); clock.mockReturnValue(now + 30_000);
    await act(async () => first.resolve({ total: 1001, items: Array.from({ length: 1000 }, (_, id) => ({ ...entry, id })) }));
    const batches = vi.mocked(plcApi.queryLogs).mock.calls.map(([q]) => q).filter(q => q.limit === 1000);
    expect(batches).toHaveLength(2); expect(batches.every(q => q.end === now && q.start === now - 3_600_000)).toBe(true);
    expect(await blobText(create.mock.calls[0][0])).toContain('"第一行\r第二行"');
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

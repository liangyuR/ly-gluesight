import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SettingsPage from "../src/pages/SettingsPage";
import { cycleApi } from "../src/features/cycle/api";
import type { CycleSettings } from "../src/features/cycle/types";
import { getAppInfo, getEngineStatus, type AppInfo, type EngineStatus } from "../src/lib/api";
import { deferred, summary } from "./fixtures";

vi.mock("../src/lib/api", () => ({ getAppInfo: vi.fn(), getEngineStatus: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ cycleApi: { getSettings: vi.fn(), saveSettings: vi.fn() }, useRecipes: () => [summary()] }));
const settings: CycleSettings = { productSource: "plc", manualRecipeId: null, historyDays: 180,
  timeouts: { armMs: 200, motionMs: 30000, drainMs: 1000, procMs: 3000, ackMs: 5000 },
  lyflowCore: null, vision: false, record: "off", recordKeep: 100, recordMaxGb: 20 };
let current: CycleSettings;
beforeEach(() => {
  current = structuredClone(settings);
  vi.mocked(cycleApi.getSettings).mockReset().mockImplementation(async () => structuredClone(current));
  vi.mocked(cycleApi.saveSettings).mockReset().mockImplementation(async value => { current = structuredClone(value); });
  vi.mocked(getAppInfo).mockReset().mockResolvedValue({ name: "GlueSight · 胶路智检", version: "test" });
  vi.mocked(getEngineStatus).mockReset().mockResolvedValue({ backend: "LyFlow", ready: true, message: "就绪", version: "15" });
});

function panel(name: string) {
  return within(screen.getByRole("heading", { name }).closest(".panel")! as HTMLElement);
}
async function panels() {
  const rendered = render(<SettingsPage />);
  const timing = panel("检测节拍"), measure = panel("测量与帧录制");
  await timing.findByRole("combobox", { name: "型号来源" });
  await measure.findByRole("combobox", { name: "帧录制" });
  return { ...rendered, timing, measure };
}
function changeNumber(scope: ReturnType<typeof within>, name: string, value: string) {
  fireEvent.change(scope.getByRole("spinbutton", { name }), { target: { value } });
}

describe("系统设置读取与基本操作", () => {
  it("读取中禁用保存，显示分区等待和应用信息", async () => {
    const request = deferred<CycleSettings>();
    vi.mocked(cycleApi.getSettings).mockReturnValue(request.promise);
    render(<SettingsPage />);
    expect(panel("检测节拍").getByRole("status")).toHaveTextContent("正在读取检测节拍设置");
    expect(panel("测量与帧录制").getByRole("status")).toHaveTextContent("正在读取测量与帧录制设置");
    screen.getAllByRole("button", { name: "保存" }).forEach(button => expect(button).toBeDisabled());
    expect(await screen.findByText("GlueSight · 胶路智检")).toBeVisible();
    expect(screen.getByText("test")).toBeVisible();
    await act(async () => request.resolve(structuredClone(current)));
    expect(panel("检测节拍").getByRole("button", { name: "保存" })).toBeEnabled();
    expect(panel("测量与帧录制").getByText("LyFlow · 就绪 · 15")).toBeVisible();
  });

  it.each(["检测节拍", "测量与帧录制"])("%s 加载失败显示错误，重试只恢复自己的分区", async (name) => {
    vi.mocked(cycleApi.getSettings).mockRejectedValueOnce(new Error("读取节拍失败")).mockRejectedValueOnce(new Error("读取录制失败"));
    render(<SettingsPage />);
    const scope = panel(name);
    expect(await scope.findByRole("alert")).toHaveTextContent("读取");
    expect(scope.getByRole("button", { name: "保存" })).toBeDisabled();
    await userEvent.click(scope.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(scope.getByRole("button", { name: "保存" })).toBeEnabled());
    expect(scope.queryByRole("alert")).not.toBeInTheDocument();
    expect(cycleApi.getSettings).toHaveBeenCalledTimes(3);
    expect(panel(name === "检测节拍" ? "测量与帧录制" : "检测节拍").getByRole("alert")).toBeVisible();
  });

  it("人工型号来源可以选择和清空配方，PLC 来源隐藏配方选择", async () => {
    const { timing } = await panels();
    expect(timing.queryByRole("combobox", { name: "当前配方" })).not.toBeInTheDocument();
    await userEvent.selectOptions(timing.getByRole("combobox", { name: "型号来源" }), "manual");
    await userEvent.selectOptions(timing.getByRole("combobox", { name: "当前配方" }), "A");
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    expect(await timing.findByText("已保存，从下一个工件开始生效")).toBeVisible();
    expect(current).toMatchObject({ productSource: "manual", manualRecipeId: "A" });
    await userEvent.selectOptions(timing.getByRole("combobox", { name: "当前配方" }), "");
    expect(timing.queryByText("已保存，从下一个工件开始生效")).not.toBeInTheDocument();
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    await timing.findByText("已保存，从下一个工件开始生效");
    expect(current.manualRecipeId).toBeNull();
    await userEvent.selectOptions(timing.getByRole("combobox", { name: "型号来源" }), "plc");
    expect(timing.queryByRole("combobox", { name: "当前配方" })).not.toBeInTheDocument();
  });

  it("所有节拍字段按输入值保存，超时可用毫秒整数", async () => {
    const { timing } = await panels();
    changeNumber(timing, "记录保留（天）", "90");
    changeNumber(timing, "布防目标 T_arm（ms）", "0");
    changeNumber(timing, "运动超时 T_motion（ms）", "1001");
    changeNumber(timing, "收尾等待 T_drain（ms）", "201");
    changeNumber(timing, "单帧处理 T_proc（ms）", "202");
    changeNumber(timing, "结果确认 T_ack（ms）", "501");
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    await timing.findByText("已保存，从下一个工件开始生效");
    expect(current).toMatchObject({ historyDays: 90, timeouts: { armMs: 0, motionMs: 1001, drainMs: 201, procMs: 202, ackMs: 501 } });
  });

  it("测量类型、核心库路径、帧录制和两个限额保存，明确显示模拟测量", async () => {
    const { measure } = await panels();
    expect(measure.queryByRole("textbox", { name: "核心库路径（lyflow_core.dll）" })).not.toBeInTheDocument();
    expect(within(measure.getByRole("combobox", { name: "飞拍配方" })).getByRole("option", { selected: true })).toHaveTextContent("模拟测量（不看图像）");
    await userEvent.selectOptions(measure.getByRole("combobox", { name: "飞拍配方" }), "lyFlow");
    await userEvent.type(measure.getByRole("textbox", { name: "核心库路径（lyflow_core.dll）" }), "D:\\core\\lyflow_core.dll");
    await userEvent.selectOptions(measure.getByRole("combobox", { name: "帧录制" }), "all");
    changeNumber(measure, "录制最多保留（件）", "125");
    changeNumber(measure, "录制总大小上限（GB）", "20.25");
    await userEvent.click(measure.getByRole("button", { name: "保存" }));
    expect(await measure.findByText("已保存")).toBeVisible();
    expect(current).toMatchObject({ vision: true, lyflowCore: "D:\\core\\lyflow_core.dll", record: "all", recordKeep: 125, recordMaxGb: 20.25 });
    await waitFor(() => expect(getEngineStatus).toHaveBeenCalledTimes(2));
    await userEvent.clear(measure.getByRole("textbox", { name: "核心库路径（lyflow_core.dll）" }));
    await userEvent.selectOptions(measure.getByRole("combobox", { name: "飞拍配方" }), "sim");
    await userEvent.selectOptions(measure.getByRole("combobox", { name: "帧录制" }), "failed");
    await userEvent.click(measure.getByRole("button", { name: "保存" }));
    await measure.findByText("已保存");
    expect(current).toMatchObject({ vision: false, lyflowCore: null, record: "failed" });
    await userEvent.selectOptions(measure.getByRole("combobox", { name: "帧录制" }), "off");
    await userEvent.click(measure.getByRole("button", { name: "保存" }));
    await measure.findByText("已保存");
    expect(current.record).toBe("off");
  });

  it("引擎状态读取失败可以重试，未就绪及说明可见", async () => {
    vi.mocked(getEngineStatus).mockRejectedValueOnce(new Error("核心库不可读"));
    const { measure } = await panels();
    expect(await measure.findByRole("alert")).toHaveTextContent("引擎状态读取失败：Error: 核心库不可读");
    vi.mocked(getEngineStatus).mockResolvedValueOnce({ backend: "LyFlow", ready: false, message: "图像流程未配置" });
    await userEvent.click(measure.getByRole("button", { name: "重试引擎状态" }));
    expect(await measure.findByText("LyFlow · 未就绪")).toBeVisible();
    expect(measure.getByText("图像流程未配置")).toBeVisible();
    expect(measure.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("应用信息失败可重试", async () => {
    vi.mocked(getAppInfo).mockRejectedValueOnce(new Error("应用信息读取失败"));
    await panels();
    const about = panel("关于");
    expect(await about.findByRole("alert")).toHaveTextContent("应用信息读取失败");
    await userEvent.click(about.getByRole("button", { name: "重试应用信息" }));
    expect(await about.findByText("GlueSight · 胶路智检")).toBeVisible();
    expect(about.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("保存后引擎状态刷新失败不继续显示过期的就绪状态", async () => {
    const { measure } = await panels();
    expect(measure.getByText("LyFlow · 就绪 · 15")).toBeVisible();
    vi.mocked(getEngineStatus).mockRejectedValueOnce(new Error("引擎断开"));
    await userEvent.click(measure.getByRole("button", { name: "保存" }));
    await measure.findByText("已保存");
    expect(await measure.findByRole("alert")).toHaveTextContent("引擎断开");
    expect(measure.queryByText("LyFlow · 就绪 · 15")).not.toBeInTheDocument();
    await userEvent.click(measure.getByRole("button", { name: "重试引擎状态" }));
    expect(await measure.findByText("LyFlow · 就绪 · 15")).toBeVisible();
  });

  it("保存后的新引擎状态不会被较早的读取覆盖", async () => {
    const old = deferred<EngineStatus>();
    vi.mocked(getEngineStatus).mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce({ backend: "LyFlow", ready: false, message: "新核心库未就绪" });
    const { measure } = await panels();
    await userEvent.click(measure.getByRole("button", { name: "保存" }));
    expect(await measure.findByText("LyFlow · 未就绪")).toBeVisible();
    await act(async () => old.resolve({ backend: "旧引擎", ready: true, message: "旧核心库" }));
    expect(measure.getByText("新核心库未就绪")).toBeVisible();
    expect(measure.queryByText(/旧引擎/)).not.toBeInTheDocument();
    expect(measure.queryByText("旧核心库")).not.toBeInTheDocument();
  });
});

describe("系统设置保存校验", () => {
  it.each([
    ["记录保留（天）", "0"], ["记录保留（天）", "3651"], ["记录保留（天）", "1.5"],
    ["记录保留（天）", ""], ["布防目标 T_arm（ms）", "-1"], ["布防目标 T_arm（ms）", ""],
    ["布防目标 T_arm（ms）", "9007199254740992"], ["运动超时 T_motion（ms）", "999"],
    ["收尾等待 T_drain（ms）", "199"], ["单帧处理 T_proc（ms）", "199"], ["结果确认 T_ack（ms）", "499"],
    ["单帧处理 T_proc（ms）", "200.5"],
  ])("%s 输入 %s 阻止保存，修正后恢复", async (name, value) => {
    const { timing } = await panels();
    const original = (timing.getByRole("spinbutton", { name }) as HTMLInputElement).value;
    changeNumber(timing, name, value);
    expect(timing.getByRole("alert")).toHaveTextContent("整数");
    expect(timing.getByRole("button", { name: "保存" })).toBeDisabled();
    fireEvent.click(timing.getByRole("button", { name: "保存" }));
    expect(cycleApi.saveSettings).not.toHaveBeenCalled();
    changeNumber(timing, name, original);
    expect(timing.queryByRole("alert")).not.toBeInTheDocument();
    expect(timing.getByRole("button", { name: "保存" })).toBeEnabled();
  });

  it.each([
    ["录制最多保留（件）", "0"], ["录制最多保留（件）", "100001"], ["录制最多保留（件）", "1.5"],
    ["录制最多保留（件）", ""], ["录制总大小上限（GB）", "0.49"], ["录制总大小上限（GB）", "10001"],
    ["录制总大小上限（GB）", ""],
  ])("%s 输入 %s 阻止保存，修正后恢复", async (name, value) => {
    const { measure } = await panels();
    const original = (measure.getByRole("spinbutton", { name }) as HTMLInputElement).value;
    changeNumber(measure, name, value);
    expect(measure.getByRole("alert")).toHaveTextContent("帧录制");
    expect(measure.getByRole("button", { name: "保存" })).toBeDisabled();
    fireEvent.click(measure.getByRole("button", { name: "保存" }));
    expect(cycleApi.saveSettings).not.toHaveBeenCalled();
    changeNumber(measure, name, original);
    expect(measure.queryByRole("alert")).not.toBeInTheDocument();
    expect(measure.getByRole("button", { name: "保存" })).toBeEnabled();
  });

  it.each(["minimum", "maximum"])("后端允许的 %s 数值边界可以保存", async edge => {
    const { timing, measure } = await panels();
    changeNumber(timing, "记录保留（天）", edge === "minimum" ? "1" : "3650");
    for (const [name, value] of [
      ["布防目标 T_arm（ms）", "0"], ["运动超时 T_motion（ms）", "1000"],
      ["收尾等待 T_drain（ms）", "200"], ["单帧处理 T_proc（ms）", "200"], ["结果确认 T_ack（ms）", "500"],
    ]) changeNumber(timing, name, value);
    changeNumber(measure, "录制最多保留（件）", edge === "minimum" ? "1" : "100000");
    changeNumber(measure, "录制总大小上限（GB）", edge === "minimum" ? "0.5" : "10000");
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    await timing.findByText("已保存，从下一个工件开始生效");
    await userEvent.click(measure.getByRole("button", { name: "保存" }));
    await measure.findByText("已保存");
    expect(current).toMatchObject({ historyDays: edge === "minimum" ? 1 : 3650, recordKeep: edge === "minimum" ? 1 : 100000, recordMaxGb: edge === "minimum" ? .5 : 10000 });
  });
});

describe("系统设置分区保存与恢复", () => {
  it.each(["timing", "measure"] as const)("先保存 %s 再保存另一分区，不覆盖新值", async first => {
    const { timing, measure } = await panels();
    changeNumber(timing, "记录保留（天）", "60");
    await userEvent.selectOptions(measure.getByRole("combobox", { name: "帧录制" }), "all");
    for (const scope of first === "timing" ? [timing, measure] : [measure, timing]) {
      await userEvent.click(scope.getByRole("button", { name: "保存" }));
      await scope.findByText(/^已保存/);
    }
    expect(current).toMatchObject({ historyDays: 60, record: "all" });
  });

  it.each(["timing", "measure"] as const)("%s 先发起、两个分区同时保存时串行读回，保留两个改动", async first => {
    const request = deferred<void>();
    const { timing, measure } = await panels();
    vi.mocked(cycleApi.saveSettings).mockImplementationOnce(async value => {
      await request.promise;
      current = structuredClone(value);
    });
    changeNumber(timing, "记录保留（天）", "60");
    fireEvent.change(measure.getByRole("combobox", { name: "帧录制" }), { target: { value: "all" } });
    for (const scope of first === "timing" ? [timing, measure] : [measure, timing])
      fireEvent.click(scope.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1));
    expect(cycleApi.getSettings).toHaveBeenCalledTimes(3);
    for (const scope of [timing, measure]) {
      expect(scope.getByRole("button", { name: "保存中…" })).toBeDisabled();
      expect(scope.getByRole("button", { name: "保存中…" })).toHaveAttribute("aria-busy", "true");
    }
    expect(timing.getByRole("spinbutton", { name: "记录保留（天）" })).toBeDisabled();
    expect(measure.getByRole("combobox", { name: "帧录制" })).toBeDisabled();
    await act(async () => request.resolve());
    await timing.findByText("已保存，从下一个工件开始生效");
    await measure.findByText("已保存");
    expect(cycleApi.saveSettings).toHaveBeenCalledTimes(2);
    expect(current).toMatchObject({ record: "all", historyDays: 60 });
  });

  it.each(["timing", "measure"] as const)("%s 重复点击仅保存一次，成功后恢复编辑", async name => {
    const request = deferred<void>();
    const all = await panels(), scope = all[name];
    vi.mocked(cycleApi.saveSettings).mockReturnValueOnce(request.promise);
    const save = scope.getByRole("button", { name: "保存" });
    fireEvent.click(save); fireEvent.click(save);
    await waitFor(() => expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1));
    expect(scope.getByRole("button", { name: "保存中…" })).toBeDisabled();
    await act(async () => request.resolve());
    await scope.findByText(/^已保存/);
    expect(scope.getByRole("button", { name: "保存" })).toBeEnabled();
  });

  it.each(["timing", "measure"] as const)("%s 保存失败保留输入，可以重试并清理旧错误", async name => {
    const all = await panels(), scope = all[name];
    const field = name === "timing" ? "记录保留（天）" : "录制最多保留（件）";
    changeNumber(scope, field, "60");
    vi.mocked(cycleApi.saveSettings).mockRejectedValueOnce(new Error("保存失败"));
    await userEvent.click(scope.getByRole("button", { name: "保存" }));
    expect(await scope.findByRole("alert")).toHaveTextContent("Error: 保存失败");
    expect(scope.queryByText(/^已保存/)).not.toBeInTheDocument();
    expect(scope.getByRole("spinbutton", { name: field })).toHaveValue(60);
    expect(scope.getByRole("button", { name: "保存" })).toBeEnabled();
    expect(getEngineStatus).toHaveBeenCalledTimes(1);
    await userEvent.click(scope.getByRole("button", { name: "保存" }));
    expect(await scope.findByText(/^已保存/)).toBeVisible();
    expect(scope.queryByRole("alert")).not.toBeInTheDocument();
    expect(current[name === "timing" ? "historyDays" : "recordKeep"]).toBe(60);
  });

  it("保存前最新设置读取失败不写入，重试可以完成", async () => {
    const { timing } = await panels();
    vi.mocked(cycleApi.getSettings).mockRejectedValueOnce(new Error("最新设置读取失败"));
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    expect(await timing.findByRole("alert")).toHaveTextContent("最新设置读取失败");
    expect(cycleApi.saveSettings).not.toHaveBeenCalled();
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    expect(await timing.findByText("已保存，从下一个工件开始生效")).toBeVisible();
    expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1);
  });

  it("一个分区失败不会阻塞另一分区排队保存，重试保留已保存值", async () => {
    const request = deferred<void>();
    const { timing, measure } = await panels();
    vi.mocked(cycleApi.saveSettings).mockReturnValueOnce(request.promise);
    changeNumber(timing, "记录保留（天）", "90");
    fireEvent.change(measure.getByRole("combobox", { name: "帧录制" }), { target: { value: "all" } });
    fireEvent.click(timing.getByRole("button", { name: "保存" }));
    fireEvent.click(measure.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1));
    await act(async () => request.reject(new Error("节拍保存失败")));
    expect(await timing.findByRole("alert")).toHaveTextContent("节拍保存失败");
    expect(await measure.findByText("已保存")).toBeVisible();
    expect(current).toMatchObject({ historyDays: 180, record: "all" });
    await userEvent.click(timing.getByRole("button", { name: "保存" }));
    await timing.findByText("已保存，从下一个工件开始生效");
    expect(current).toMatchObject({ historyDays: 90, record: "all" });
  });
});

describe("系统设置卸载后的请求", () => {
  it("卸载后设置、引擎和应用信息读取失败均已处理，不影响新页面", async () => {
    const settingsRequest = deferred<CycleSettings>(), engineRequest = deferred<EngineStatus>(), infoRequest = deferred<AppInfo>();
    vi.mocked(cycleApi.getSettings).mockReturnValueOnce(settingsRequest.promise).mockReturnValueOnce(settingsRequest.promise);
    vi.mocked(getEngineStatus).mockReturnValueOnce(engineRequest.promise);
    vi.mocked(getAppInfo).mockReturnValueOnce(infoRequest.promise);
    const old = render(<SettingsPage />);
    await waitFor(() => expect(cycleApi.getSettings).toHaveBeenCalledTimes(2));
    old.unmount();
    const { timing, measure } = await panels();
    await act(async () => {
      settingsRequest.reject(new Error("旧设置请求"));
      engineRequest.reject(new Error("旧引擎请求"));
      infoRequest.reject(new Error("旧应用请求"));
    });
    expect(timing.getByRole("spinbutton", { name: "记录保留（天）" })).toHaveValue(180);
    expect(measure.getByText("LyFlow · 就绪 · 15")).toBeVisible();
    expect(screen.queryAllByRole("alert")).toHaveLength(0);
  });

  it("读回最新设置时卸载，不再提交保存", async () => {
    const request = deferred<CycleSettings>();
    const { timing, unmount } = await panels();
    vi.mocked(cycleApi.getSettings).mockReturnValueOnce(request.promise);
    fireEvent.click(timing.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(cycleApi.getSettings).toHaveBeenCalledTimes(3));
    unmount();
    await act(async () => request.resolve(structuredClone(current)));
    expect(cycleApi.saveSettings).not.toHaveBeenCalled();
  });

  it("卸载取消尚未开始的排队保存，已提交保存完成后不刷新引擎", async () => {
    const request = deferred<void>();
    const { timing, measure, unmount } = await panels();
    vi.mocked(cycleApi.saveSettings).mockReturnValueOnce(request.promise);
    fireEvent.click(measure.getByRole("button", { name: "保存" }));
    fireEvent.click(timing.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => request.resolve());
    expect(cycleApi.getSettings).toHaveBeenCalledTimes(3);
    expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1);
    expect(getEngineStatus).toHaveBeenCalledTimes(1);
  });

  it("卸载后已提交保存失败不会出现未处理 Promise", async () => {
    const request = deferred<void>();
    const { measure, unmount } = await panels();
    vi.mocked(cycleApi.saveSettings).mockReturnValueOnce(request.promise);
    fireEvent.click(measure.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(cycleApi.saveSettings).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => request.reject(new Error("卸载后的保存失败")));
    expect(getEngineStatus).toHaveBeenCalledTimes(1);
  });
});

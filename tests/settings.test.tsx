import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SettingsPage from "../src/pages/SettingsPage";
import { cycleApi } from "../src/features/cycle/api";
import type { CycleSettings } from "../src/features/cycle/types";
import { getAppInfo, getEngineStatus, type EngineStatus } from "../src/lib/api";
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

describe("系统设置分区保存与恢复", () => {

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

});

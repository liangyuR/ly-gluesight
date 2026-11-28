import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SnackbarProvider } from "qing-steel";
import { MemoryRouter } from "react-router-dom";
import SettingsPage from "../src/pages/SettingsPage";
import { WindowConnections } from "../src/layout/WindowStatus";
import type { CycleSettings } from "../src/features/cycle/types";
import type { EngineStatus } from "../src/lib/api";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

let saved: CycleSettings;
let disks: string[];
let readEngine: () => Promise<EngineStatus>;
const disksReply = () => ({ reserveBytes: 5 * 1024 ** 3, disks: disks.map((path, i) => ({ path, freeBytes: 100 * 1024 ** 3, writable: true, error: null, current: i === 0 })) });

beforeEach(() => {
  readEngine = async () => ({ backend: "LyFlow", ready: false, message: "" });
  disks = ["C:\\gs_logs", "D:\\gs_logs"];
  saved = {
    productSource: "plc", manualRecipeId: null, historyDays: 180,
    timeouts: { motionMs: 30000, drainMs: 1000, procMs: 3000, gateGuardMs: 80 },
    record: "failed", recordKeep: 100, recordMaxGb: 20, idleCompress: false,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "cycle_get_settings": return structuredClone(saved);
      case "cycle_save_settings": saved = structuredClone((args as { settings: CycleSettings }).settings); return;
      case "cycle_recipes": return [];
      case "engine_status": return readEngine();
      case "storage_status": return { environment: "development", root: "C:/ProgramData/ly/GlueSight-dev",
        logRoot: "C:/test/gs_logs", runtimeLogDir: "C:/test/gs_logs/runtime/development/instance", visionLogDir: "C:/test/gs_logs/vision/development/instance/project" };
      case "storage_open_directory": return;
      case "storage_recording_disks": return disksReply();
      case "storage_recording_disks_save": disks = (args as { paths: string[] }).paths; return disksReply();
      case "app_info": return { name: "GlueSight", version: "test" };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
});

function recordSaveButton() {
  return within(screen.getByRole("region", { name: "lyFlow 与帧录制" })).getByRole("button", { name: "保存" });
}

function renderEngineViews() {
  return render(<MemoryRouter><SnackbarProvider><WindowConnections /><SettingsPage /></SnackbarProvider></MemoryRouter>);
}

it("标题栏和设置共享引擎首读 → 重试后同时恢复", async () => {
  const first = deferred<EngineStatus>();
  const load = vi.fn<() => Promise<EngineStatus>>().mockReturnValueOnce(first.promise)
    .mockResolvedValue({ backend: "LyFlow", ready: true, message: "运行库已就绪", version: "2.0.4", path: "C:/test/lyflow" });
  readEngine = load;
  renderEngineViews();
  await act(async () => {});
  expect(load).toHaveBeenCalledOnce();
  await act(async () => first.reject(new Error("引擎暂不可读")));
  expect(screen.getByRole("link", { name: "后端未连接" })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "重试引擎状态" }));

  expect(await screen.findByText("LyFlow · 就绪 · 2.0.4")).toBeVisible();
  expect(screen.getByRole("button", { name: "打开运行库目录：C:/test/lyflow" })).toBeVisible();
  expect(screen.getByRole("link", { name: "LyFlow · 就绪" })).toHaveAttribute("title", "运行库已就绪");
  expect(load).toHaveBeenCalledTimes(2);
});

it("保存时轮询仍在进行 → 保存后的引擎刷新排队且离页释放轮询", async () => {
  const pendingPoll = deferred<EngineStatus>();
  const waiting = { backend: "LyFlow", ready: false, message: "旧状态" };
  const load = vi.fn<() => Promise<EngineStatus>>()
    .mockResolvedValueOnce(waiting).mockReturnValueOnce(pendingPoll.promise)
    .mockResolvedValue({ backend: "LyFlow", ready: true, message: "已刷新" });
  readEngine = load;
  vi.useFakeTimers();
  try {
    const view = renderEngineViews();
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(load).toHaveBeenCalledOnce();
    await act(() => vi.advanceTimersByTimeAsync(6000));
    expect(load).toHaveBeenCalledTimes(2);
    expect(screen.getByText("旧状态")).toBeVisible();

    fireEvent.click(recordSaveButton());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(invoke).toHaveBeenCalledWith("cycle_save_settings", expect.anything());
    expect(load).toHaveBeenCalledTimes(2);
    await act(async () => pendingPoll.resolve(waiting));
    expect(load).toHaveBeenCalledTimes(3);
    expect(screen.getByRole("link", { name: "LyFlow · 就绪" })).toHaveAttribute("title", "已刷新");

    view.unmount();
    await act(() => vi.advanceTimersByTimeAsync(6000));
    expect(load).toHaveBeenCalledTimes(3);
  } finally {
    vi.useRealTimers();
  }
});

describe("原图保留设置", () => {

  it("修改录制保留上限并保存 → 重新打开显示新值且保留其他分区设置", async () => {
    const page = render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    const keep = await screen.findByRole("spinbutton", { name: "真机录制最多保留（件）" });
    fireEvent.change(keep, { target: { value: "450" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "真机录制总大小上限（GB）" }), { target: { value: "12.5" } });
    await userEvent.click(screen.getByRole("checkbox", { name: "空闲时压缩历史原图" }));
    saved.historyDays = 90;
    await userEvent.click(recordSaveButton());
    await waitFor(() => expect(saved.recordKeep).toBe(450));
    page.unmount();
    render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    expect(await screen.findByRole("spinbutton", { name: "真机录制最多保留（件）" })).toHaveValue(450);
    expect(screen.getByRole("spinbutton", { name: "真机录制总大小上限（GB）" })).toHaveValue(12.5);
    expect(screen.getByRole("checkbox", { name: "空闲时压缩历史原图" })).toBeChecked();
    expect(screen.getByRole("spinbutton", { name: "记录保留（天）" })).toHaveValue(90);
    expect(screen.getByRole("combobox", { name: "真机帧录制" }).closest(".qs-select")!.querySelector("select")!).toHaveValue("failed");
  });

  it("勾选不限并保存 → 后端收到空值（不清理），重新打开仍显示不限", async () => {
    const page = render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    await userEvent.click(await screen.findByRole("checkbox", { name: "件数不限" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "大小不限" }));
    await userEvent.click(recordSaveButton());
    await waitFor(() => expect(saved.recordKeep).toBeNull());
    expect(saved.recordMaxGb).toBeNull();
    page.unmount();
    render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    expect(await screen.findByRole("checkbox", { name: "件数不限" })).toBeChecked();
    expect(screen.getByRole("spinbutton", { name: "真机录制最多保留（件）" })).toBeDisabled();
  });

});

describe("录制盘", () => {
  it("把第二块盘上移后保存 → 按界面顺序提交，新件先写这块盘", async () => {
    render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    const panel = await screen.findByRole("region", { name: "录制盘" });
    await userEvent.click(await within(panel).findByRole("button", { name: "上移 D:\\gs_logs" }));
    await userEvent.click(within(panel).getByRole("button", { name: "保存录制盘" }));
    await waitFor(() => expect(disks).toEqual(["D:\\gs_logs", "C:\\gs_logs"]));
    expect(within(panel).getAllByRole("listitem")[0].textContent).toContain("D:\\gs_logs");
  });
});

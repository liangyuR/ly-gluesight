import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SnackbarProvider } from "qing-steel";
import SettingsPage from "../src/pages/SettingsPage";
import type { CycleSettings } from "../src/features/cycle/types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

let saved: CycleSettings;

beforeEach(() => {
  saved = {
    productSource: "plc", manualRecipeId: null, historyDays: 180,
    timeouts: { motionMs: 30000, drainMs: 1000, procMs: 3000, ackMs: 5000 },
    record: "failed", recordKeep: 100, recordMaxGb: 20,
    offlineRecordKeep: 300, offlineRecordMaxGb: 10,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "cycle_get_settings": return structuredClone(saved);
      case "cycle_save_settings": saved = structuredClone((args as { settings: CycleSettings }).settings); return;
      case "cycle_recipes": return [];
      case "engine_status": return { backend: "LyFlow", ready: false, message: "" };
      case "storage_status": return { environment: "development", root: "C:/ProgramData/ly/GlueSight-dev", migrationStatus: "completed",
        logRoot: "C:/test/gs_logs", runtimeLogDir: "C:/test/gs_logs/runtime/development/instance", visionLogDir: "C:/test/gs_logs/vision/development/instance/project" };
      case "storage_open_directory": return;
      case "app_info": return { name: "GlueSight", version: "test" };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
});

function recordSaveButton() {
  return within(screen.getByRole("region", { name: "lyFlow 与帧录制" })).getByRole("button", { name: "保存" });
}

describe("离线原图保留设置", () => {
  it.each([
    [String.raw`\\?\C:\test\gs_logs`, String.raw`C:\test\gs_logs`],
    [String.raw`\\?\UNC\server\share\gs_logs`, String.raw`\\server\share\gs_logs`],
  ])("Windows 规范化路径 %s → 设置页显示可读路径并保留网络共享根", async (raw, displayed) => {
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "storage_status") return { environment: "development", root: `${raw}\\data`, migrationStatus: "ready",
        logRoot: raw, runtimeLogDir: `${raw}\\runtime`, visionLogDir: `${raw}\\vision\\project` };
      if (command === "app_info") return { name: "GlueSight", version: "test", logDir: `${raw}\\runtime` };
      if (command === "engine_status") return { backend: "LyFlow", ready: true, message: "", path: `${raw}\\lyflow` };
      return original(command, args);
    });
    render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    const storage = within(screen.getByRole("region", { name: "资料存储" }));
    expect(await storage.findByText(displayed)).toBeVisible();
    for (const suffix of ["data", "runtime", "vision\\project"]) expect(storage.getByText(`${displayed}\\${suffix}`)).toBeVisible();
    expect(screen.getAllByText(`${displayed}\\runtime`)).toHaveLength(1);
    expect(await within(screen.getByRole("region", { name: "lyFlow 与帧录制" })).findByText(`${displayed}\\lyflow`)).toBeVisible();
  });

  it("日志与项目分开存储 → 目录入口分别打开运行日志和当前项目原图", async () => {
    render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    const storage = within(screen.getByRole("region", { name: "资料存储" }));
    expect(await storage.findByText("C:/test/gs_logs/runtime/development/instance")).toBeVisible();
    expect(storage.getByText("C:/test/gs_logs/vision/development/instance/project")).toBeVisible();
    await userEvent.click(storage.getByRole("button", { name: /^打开日志根目录：/ }));
    expect(invoke).toHaveBeenCalledWith("storage_open_directory", { target: "logs" });
    await userEvent.click(storage.getByRole("button", { name: /^打开运行日志：/ }));
    expect(invoke).toHaveBeenCalledWith("storage_open_directory", { target: "runtime" });
    await userEvent.click(storage.getByRole("button", { name: /^打开视觉原图：/ }));
    expect(invoke).toHaveBeenCalledWith("storage_open_directory", { target: "vision" });
  });

  it("修改离线保留上限并保存 → 重新打开显示新值且保留其他分区设置", async () => {
    const page = render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    const keep = await screen.findByRole("spinbutton", { name: "离线原图最多保留（件）" });
    fireEvent.change(keep, { target: { value: "450" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "离线原图总大小上限（GB）" }), { target: { value: "12.5" } });
    saved.historyDays = 90;
    await userEvent.click(recordSaveButton());
    await waitFor(() => expect(saved.offlineRecordKeep).toBe(450));
    page.unmount();
    render(<SnackbarProvider><SettingsPage /></SnackbarProvider>);
    expect(await screen.findByRole("spinbutton", { name: "离线原图最多保留（件）" })).toHaveValue(450);
    expect(screen.getByRole("spinbutton", { name: "离线原图总大小上限（GB）" })).toHaveValue(12.5);
    expect(screen.getByRole("spinbutton", { name: "记录保留（天）" })).toHaveValue(90);
    expect(screen.getByRole("spinbutton", { name: "真机录制最多保留（件）" })).toHaveValue(100);
    expect(screen.getByRole("spinbutton", { name: "真机录制总大小上限（GB）" })).toHaveValue(20);
    expect(screen.getByRole("combobox", { name: "真机帧录制" }).closest(".qs-select")!.querySelector("select")!).toHaveValue("failed");
  });


});

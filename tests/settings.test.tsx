import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SettingsPage from "../src/pages/SettingsPage";
import type { CycleSettings } from "../src/features/cycle/types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

let saved: CycleSettings;

beforeEach(() => {
  saved = {
    productSource: "plc", manualRecipeId: null, historyDays: 180,
    timeouts: { armMs: 200, motionMs: 30000, drainMs: 1000, procMs: 3000, ackMs: 5000 },
    record: "failed", recordKeep: 100, recordMaxGb: 20,
    offlineRecordKeep: 300, offlineRecordMaxGb: 10,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "cycle_get_settings": return structuredClone(saved);
      case "cycle_save_settings": saved = structuredClone((args as { settings: CycleSettings }).settings); return;
      case "cycle_recipes": return [];
      case "engine_status": return { backend: "LyFlow", ready: false, message: "" };
      case "storage_status": return { environment: "development", root: "C:/ProgramData/ly/GlueSight-dev", migrationStatus: "completed" };
      case "app_info": return { name: "GlueSight", version: "test" };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
});

function recordSaveButton() {
  return within(screen.getByRole("heading", { name: "lyFlow 与帧录制" }).parentElement!).getByRole("button", { name: "保存" });
}

describe("离线原图保留设置", () => {
  it("修改离线保留上限并保存 → 重新打开显示新值且保留其他分区设置", async () => {
    const page = render(<SettingsPage />);
    const keep = await screen.findByRole("spinbutton", { name: "离线原图最多保留（件）" });
    fireEvent.change(keep, { target: { value: "450" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "离线原图总大小上限（GB）" }), { target: { value: "12.5" } });
    saved.historyDays = 90;
    await userEvent.click(recordSaveButton());
    await screen.findByText("已保存");
    page.unmount();
    render(<SettingsPage />);
    expect(await screen.findByRole("spinbutton", { name: "离线原图最多保留（件）" })).toHaveValue(450);
    expect(screen.getByRole("spinbutton", { name: "离线原图总大小上限（GB）" })).toHaveValue(12.5);
    expect(screen.getByRole("spinbutton", { name: "记录保留（天）" })).toHaveValue(90);
    expect(screen.getByRole("spinbutton", { name: "真机录制最多保留（件）" })).toHaveValue(100);
    expect(screen.getByRole("spinbutton", { name: "真机录制总大小上限（GB）" })).toHaveValue(20);
    expect(screen.getByRole("combobox", { name: "真机帧录制" })).toHaveValue("failed");
  });

  it.each([
    ["离线原图最多保留（件）", "", "1"],
    ["离线原图最多保留（件）", "0", "1"],
    ["离线原图最多保留（件）", "1.5", "2"],
    ["离线原图最多保留（件）", "100001", "100000"],
    ["离线原图总大小上限（GB）", "", "0.5"],
    ["离线原图总大小上限（GB）", "0.4", "0.5"],
    ["离线原图总大小上限（GB）", "10001", "10000"],
  ])("%s 输入 %s → 阻止保存，改为 %s 后可保存", async (label, invalid, valid) => {
    render(<SettingsPage />);
    const field = await screen.findByRole("spinbutton", { name: label });
    fireEvent.change(field, { target: { value: invalid } });
    expect(recordSaveButton()).toBeDisabled();
    expect(screen.getByRole("alert")).toBeVisible();
    fireEvent.change(field, { target: { value: valid } });
    await waitFor(() => expect(recordSaveButton()).toBeEnabled());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

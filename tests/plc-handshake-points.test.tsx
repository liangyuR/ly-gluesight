import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, expect, it, vi } from "vitest";
import PlcPage from "../src/features/plc/pages/PlcPage";
import type { PlcConfig } from "../src/features/plc/types";
import { plcConfig } from "./plc-fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

function configWithHandshakePoint(): PlcConfig {
  const config = plcConfig();
  config.points.push({ id: "p_plcState", name: "PLC 状态", address: "DB1.DBW4", dataType: "i16", wordOrder: null,
    access: "read", edge: "none", poll: true, logChanges: true, tags: ["plcState"], description: "" });
  return config;
}

beforeEach(() => {
  vi.mocked(listen).mockResolvedValue(() => {});
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "plc_get_config") return configWithHandshakePoint();
    if (command === "plc_get_status") return { state: "connected", message: "已连接", pollCount: 0, errorCount: 0 };
    if (command === "plc_get_values") return {};
    if (command === "cycle_snapshot") return { phase: "IDLE", plcLocked: false };
    if (command === "plc_check_address") return "地址有效";
  });
});

it("编辑握手点位 → 地址等模板字段不可改，只有描述和记录变化写回草稿", async () => {
  render(<MemoryRouter><SnackbarProvider><PlcPage /></SnackbarProvider></MemoryRouter>);
  const row = (await screen.findByText("DB1.DBW4")).closest("tr")!;
  const edit = within(row).getByRole("button", { name: "编辑" });
  await waitFor(() => expect(edit).toBeEnabled());
  fireEvent.click(edit);

  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByLabelText(/^地址/)).toBeDisabled();
  expect(within(dialog).getByLabelText("名称")).toBeDisabled();
  fireEvent.change(within(dialog).getByLabelText("描述"), { target: { value: "联调备注" } });
  fireEvent.click(within(dialog).getByRole("checkbox", { name: /记录值变化/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "确定" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

  fireEvent.click(screen.getByText("保存配置").closest("button")!);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("plc_save_config", expect.anything()));
  const savedConfig = vi.mocked(invoke).mock.calls.find(([command]) => command === "plc_save_config")![1] as { config: PlcConfig };
  expect(savedConfig.config.points.find(p => p.id === "p_plcState")).toEqual({
    ...configWithHandshakePoint().points[1], description: "联调备注", logChanges: false,
  });
});

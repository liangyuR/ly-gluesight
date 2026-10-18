import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import S7Phase1Dialog from "../src/features/plc/components/S7Phase1Dialog";
import type { PlcConfig } from "../src/features/plc/types";
import { plcConfig } from "./plc-fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));

function template({ dbNumber, snType }: { dbNumber: number; snType: string }): PlcConfig {
  const config = plcConfig();
  config.connection.s7 = { rack: 0, slot: 1, connectionType: "pg", localTsap: 0x0102, remoteTsap: 0x0201, pduSize: 240 };
  config.points = [{ ...config.points[0], id: "p_partSn", address: `DB${dbNumber}.DBB160.50`, dataType: snType as "string" | "smartString", tags: ["partSn"] }];
  return config;
}

beforeEach(() => {
  vi.mocked(invoke).mockImplementation(async (_command, args) => template(args as { dbNumber: number; snType: string }));
});

describe("载入一期 S7 点表", () => {
  it("S7-200 SMART 锁定 DB1 并用 200 SMART 字符串，切到 S7-1200 改用 S7 STRING 与对应连接参数", async () => {
    const existing = plcConfig();
    existing.points[0].address = "DB100.DBD0";
    const onApply = vi.fn();
    render(<S7Phase1Dialog config={existing} dirty={false} blockedReason="" onApply={onApply} onClose={() => {}} />);

    expect(screen.getByRole("spinbutton")).toBeDisabled();
    expect(screen.getByRole("spinbutton")).toHaveValue(1);
    await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("plc_s7_phase1_template", { dbNumber: 1, snType: "smartString" }));
    fireEvent.click(await screen.findByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "应用到草稿" }));
    expect(onApply.mock.calls[0][0].connection.s7).toMatchObject({ localTsap: 0x0102, remoteTsap: 0x0201, pduSize: 240 });
    expect(onApply.mock.calls[0][1]).toBe("s7200smart");

    fireEvent.change(screen.getByRole("combobox", { name: "一期 CPU 预设" }), { target: { value: "s71200" } });
    expect(screen.getByRole("spinbutton")).toBeEnabled();
    await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("plc_s7_phase1_template", { dbNumber: 100, snType: "string" }));
    fireEvent.click(await screen.findByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "应用到草稿" }));
    expect(onApply.mock.calls[1][0].connection.s7).toMatchObject({ localTsap: null, remoteTsap: null, pduSize: 480 });
  });
});

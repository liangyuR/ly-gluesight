import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import S7Phase1Dialog from "../src/features/plc/components/S7Phase1Dialog";
import type { PlcConfig } from "../src/features/plc/types";
import { plcConfig } from "./plc-fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));

function template({ plcDb, pcDb }: { plcDb: number; pcDb: number }): PlcConfig {
  const config = plcConfig();
  config.connection.s7 = { rack: 0, slot: 1, connectionType: "pg", localTsap: null, remoteTsap: null, pduSize: 480 };
  config.points = [
    { ...config.points[0], id: "p_partSn", address: `DB${plcDb}.DBB38.50`, dataType: "string", access: "read", tags: ["partSn"] },
    { ...config.points[0], id: "p_resultSn", address: `DB${pcDb}.DBB20.50`, dataType: "string", access: "readWrite", tags: ["resultSn"] },
  ];
  return config;
}

beforeEach(() => {
  vi.mocked(invoke).mockImplementation(async (_command, args) => template(args as { plcDb: number; pcDb: number }));
});

describe("载入一期 S7 点表", () => {
  it("协议 4 的单 DB 旧点表 → 默认 PLC 区 DB4、PC 区 DB2，并应用 S7-1200 连接参数", async () => {
    const existing = plcConfig();
    existing.points = existing.points.map(point => ({ ...point, address: point.address.replace(/^DB\d+\./, "DB1.") }));
    const onApply = vi.fn();
    render(<S7Phase1Dialog config={existing} dirty={false} blockedReason="" onApply={onApply} onClose={() => {}} />);

    await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("plc_s7_phase1_template", { plcDb: 4, pcDb: 2 }));
    fireEvent.click(await screen.findByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "应用到草稿" }));
    expect(onApply.mock.calls[0][0].connection.s7).toMatchObject({ rack: 0, slot: 1, localTsap: null, remoteTsap: null, pduSize: 480 });
    expect(onApply.mock.calls[0][1]).toBe("s71200");
  });

  it("两个区填同一个 DB → 提示必须分开且不请求后端生成", async () => {
    render(<S7Phase1Dialog config={plcConfig()} dirty={false} blockedReason="" onApply={vi.fn()} onClose={() => {}} />);
    await waitFor(() => expect(invoke).toHaveBeenCalled());
    vi.mocked(invoke).mockClear();
    fireEvent.change(screen.getByRole("spinbutton", { name: "PC 区 DB（PC 写）" }), { target: { value: "4" } });
    expect(await screen.findByText("PLC 区与 PC 区必须使用两个不同的 DB")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "应用到草稿" })).toBeDisabled();
    await new Promise(resolve => setTimeout(resolve, 300));
    expect(invoke).not.toHaveBeenCalled();
  });
});

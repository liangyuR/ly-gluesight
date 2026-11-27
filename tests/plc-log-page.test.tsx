import { fireEvent, render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import PlcLogPage from "../src/features/plc/pages/PlcLogPage";
import type { LogGroup } from "../src/features/plc/types";
import { plcConfig } from "./plc-fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

it("两件复用同一件序号 → 打开所选工件时只显示该次的点位变化", async () => {
  const older: LogGroup = {
    firstId: 10, lastId: 19, context: "partSeq=8", partSeq: 8, sn: "A001",
    firstTs: 10000, lastTs: 19000, entryCount: 10, warningCount: 0, errorCount: 0,
    ranges: [[10, 19]],
    productCode: 1, resultCode: 1, faultCode: 0, event: "resultWritten",
  };
  const newer: LogGroup = { ...older, firstId: 60, lastId: 69, firstTs: 60000, lastTs: 69000, ranges: [[60, 69]], sn: "B008" };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "plc_get_config") return plcConfig();
    if (command === "plc_query_log_groups") return { total: 2, items: [newer, older] };
    if (command === "plc_query_group_logs") {
      const { group } = args as { group: Pick<LogGroup, "ranges"> };
      const items = [older, newer].filter(item =>
        group.ranges.some(([from, to]) => from <= item.firstId && item.firstId <= to),
      ).map(match => ({
        id: match.firstId, ts: match.firstTs, level: "info", category: "value",
        pointId: "p_sn", pointName: "工件 SN", message: "读取到 " + match.sn,
        oldValue: null, newValue: match.sn, context: match.context,
      }));
      return { total: items.length, items };
    }
    throw new Error("意外调用 " + command);
  });
  render(<PlcLogPage />);
  const row = (await screen.findByText("A001")).closest("tr")!;
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "plc_query_group_logs")).toBe(false);
  fireEvent.click(within(row).getByRole("button", { name: /#8/ }));
  const dialog = await screen.findByRole("dialog");
  expect(await within(dialog).findByText("读取到 A001")).toBeVisible();
  expect(within(dialog).queryByText("读取到 B008")).not.toBeInTheDocument();
});

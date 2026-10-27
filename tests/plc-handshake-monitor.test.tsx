import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import HandshakeMonitor from "../src/features/plc/components/HandshakeMonitor";
import type { PlcHandshakeState, PointValue, PlcValue } from "../src/features/plc/types";

const tags = (values: Record<string, PlcValue>) => (tag: string): PointValue | undefined =>
  tag in values ? { value: values[tag], error: null, errorKind: null, ts: Date.now() } : undefined;
const session: PlcHandshakeState = { phase: "validating", requestSeq: 17, resultSeq: null, message: null };
const field = (side: "PLC" | "PC", label: string) =>
  within(screen.getByRole("group", { name: `${side} 生产寄存器` })).getByText(label).nextElementSibling;

it("PLC 断线且保留未结事务 → 不展示旧状态和旧数值，阶段写重连后需复位", () => {
  render(<HandshakeMonitor connected={false} tagValue={tags({ plcState: 10, requestSeq: 17, sn: "A24-000123" })}
    handshake={{ phase: "fault", requestSeq: 17, resultSeq: null, message: "S7 PLC 未连接；保留未结事务，重连后需复位" }} />);

  expect(screen.getByText("重连后需复位")).toBeVisible();
  expect(screen.getByText("连接后显示信号")).toBeVisible();
  expect(screen.queryByRole("group", { name: "PLC 生产寄存器" })).toBeNull();
  expect(screen.queryByText("17")).toBeNull();
  expect(screen.queryByText("A24-000123")).toBeNull();
});

it("PLC 故障 90 原因 11、PC 已布防 → 双方状态与原因按协议 7 码表显示中文", () => {
  render(<HandshakeMonitor connected handshake={session}
    tagValue={tags({ plcState: 90, plcReason: 11, pcState: 20, pcMode: 0, requestSeq: 17, seqEcho: 17 })} />);

  expect(field("PLC", "状态")).toHaveTextContent("故障");
  expect(field("PLC", "原因")).toHaveTextContent("等待 PC 布防超时");
  expect(field("PC", "状态")).toHaveTextContent("已布防");
  expect(field("PC", "模式")).toHaveTextContent("正常");
});

it("维护模式下 PC 结果有效且结果码 0 → 结果显示无结论（维护），不当作未知结果", () => {
  render(<HandshakeMonitor connected handshake={session}
    tagValue={tags({ plcState: 20, pcState: 30, pcMode: 1, resultCode: 0, faultCode: 0 })} />);

  expect(field("PC", "结果")).toHaveTextContent("无结论（维护）");
});

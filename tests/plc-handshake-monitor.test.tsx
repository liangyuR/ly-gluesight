import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import HandshakeMonitor from "../src/features/plc/components/HandshakeMonitor";
import type { PointValue } from "../src/features/plc/types";

it("PLC 断线且保留未结事务 → 不展示旧信号和旧数值，阶段写重连后需复位", () => {
  const stale = (tag: string): PointValue => ({ value: tag === "partSn" ? "A24-000123" : 1, error: null, errorKind: null, ts: Date.now() });
  render(<HandshakeMonitor connected={false} tagValue={stale}
    handshake={{ phase: "fault", requestSeq: 17, resultSeq: null, message: "S7 PLC 未连接；保留未结事务，重连后需复位" }} />);

  expect(screen.getByText("重连后需复位")).toBeVisible();
  expect(screen.getByText("PLC 未连接，信号不显示")).toBeVisible();
  expect(screen.queryByRole("img", { name: /工件开始/ })).toBeNull();
  expect(screen.queryByText("17")).toBeNull();
  expect(screen.queryByText("A24-000123")).toBeNull();
});

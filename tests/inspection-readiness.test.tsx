import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import InspectionReadiness from "../src/features/cycle/components/InspectionReadiness";
import { readiness, snapshot } from "./fixtures";

describe("在线检测设备状态", () => {
  function showDevice({ offlineSample, plcBlocked }: { offlineSample: boolean; plcBlocked: boolean }) {
    const report = readiness({ ready: false, items: readiness().items.map(item => item.key === "recipe"
      ? { ...item, state: "blocked", detail: "配方未发布", target: "workspace" }
      : item.key === "plc" && plcBlocked ? { ...item, state: "blocked", detail: "PLC 未连接", target: "plc" } : item) });
    render(<MemoryRouter><InspectionReadiness snapshot={snapshot()} readiness={report} offlineSample={offlineSample} /></MemoryRouter>);
    return within(screen.getByRole("region", { name: "设备状态" }));
  }

  it("离线样本仅生产配方未发布 → 豁免发布条件，报告离线设备条件已通过但不宣告就绪", () => {
    const device = showDevice({ offlineSample: true, plcBlocked: false });
    expect(device.getByText("离线设备条件已通过")).toBeVisible();
    expect(device.queryByText("配方未发布")).not.toBeInTheDocument();
    expect(device.getByText(/生产发布条件不用于候选调试/)).toBeVisible();
    expect(device.queryByText("就绪，等待工件")).not.toBeInTheDocument();
  });

  it("离线样本仍有 PLC 阻塞 → 只计 PLC 一项阻塞并给出 PLC 通讯入口", () => {
    const device = showDevice({ offlineSample: true, plcBlocked: true });
    expect(device.getByText("离线设备异常 · 1 项阻塞")).toBeVisible();
    expect(device.queryByText("配方未发布")).not.toBeInTheDocument();
    expect(device.getByText("PLC 未连接")).toBeVisible();
    expect(device.getByRole("link", { name: "去 PLC 通讯" })).toHaveAttribute("href", "/plc");
  });

  it("真机生产配方未发布 → 发布条件计为阻塞，无法开工", () => {
    const device = showDevice({ offlineSample: false, plcBlocked: false });
    expect(device.getByText("无法开工 · 1 项阻塞")).toBeVisible();
    expect(device.getByText("配方未发布")).toBeVisible();
  });

});

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import InspectionReadiness from "../src/features/cycle/components/InspectionReadiness";
import { readiness, snapshot } from "./fixtures";

describe("在线检测设备状态", () => {
  it.each([
    { condition: "没有设备快照", snapshot: null, readiness: readiness(), expected: "正在确认设备状态" },
    { condition: "尚未取得开工条件", snapshot: snapshot(), readiness: null, expected: "正在确认设备状态" },
    { condition: "开工条件通过但设备故障", snapshot: { ...snapshot("FAULT"), fault: "PLC 心跳中断" }, readiness: readiness(), expected: "无法开工 · 设备故障" },
    { condition: "尚未允许开工且没有阻塞明细", snapshot: snapshot(), readiness: readiness({ ready: false }), expected: "暂未就绪" },
    { condition: "设备空闲且开工条件通过", snapshot: snapshot(), readiness: readiness(), expected: "就绪，等待工件" },
    { condition: "没有阻塞但审计仍在等待", snapshot: snapshot(), readiness: readiness({ ready: true, items: readiness().items.map(item => item.key === "audit" ? { ...item, state: "waiting" as const } : item) }), expected: "等待就绪条件" },
    { condition: "设备正在采集", snapshot: snapshot("ACQUIRE"), readiness: readiness(), expected: "工件检测中" },
  ])("$condition → $expected", ({ snapshot: state, readiness: report, expected }) => {
    render(<MemoryRouter><InspectionReadiness snapshot={state} readiness={report} /></MemoryRouter>);
    const device = within(screen.getByRole("region", { name: "设备状态" }));
    expect(device.getByText(expected)).toBeVisible();
    if (expected !== "就绪，等待工件") expect(device.queryByText("就绪，等待工件")).not.toBeInTheDocument();
    if (state?.fault) expect(device.getByText(state.fault)).toBeVisible();
  });

  it("存在阻塞与已就绪项 → 阻塞直接可见且已就绪项展开后可见", async () => {
    const report = readiness({ ready: false, items: readiness().items.map(item => item.key === "plc"
      ? { ...item, state: "blocked", detail: "PLC 未连接", target: "plc" }
      : { ...item, detail: `${item.title} 已通过检查` }) });
    render(<MemoryRouter><InspectionReadiness snapshot={snapshot()} readiness={report} /></MemoryRouter>);
    expect(screen.getByText("无法开工 · 1 项阻塞")).toBeVisible();
    expect(screen.getByText("PLC 未连接")).toBeVisible();
    expect(screen.getByText("engine 已通过检查")).not.toBeVisible();
    await userEvent.click(screen.getByText("已就绪项目（5）"));
    expect(screen.getByText("engine 已通过检查")).toBeVisible();
  });

  it("多条告警包含完全重复项 → 默认收起且展开后保留不同的原始信息", async () => {
    const alarms = ["配方 A 不开工：未发布", "配方 B 不开工：未发布", "配方 A 不开工：未发布", "设备诊断：触发计数异常"];
    render(<MemoryRouter><InspectionReadiness snapshot={{ ...snapshot(), alarms }} readiness={readiness()} /></MemoryRouter>);
    expect(screen.getByText(alarms[0])).not.toBeVisible();
    await userEvent.click(screen.getByText("诊断详情（3）"));
    expect(screen.getAllByText(alarms[0])).toHaveLength(1);
    for (const alarm of new Set(alarms)) expect(screen.getByText(alarm)).toBeVisible();
  });
});

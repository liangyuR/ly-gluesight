import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
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

  it.each([
    { condition: "没有设备快照", snapshot: null, readiness: readiness(), expected: "正在确认设备状态" },
    { condition: "尚未取得开工条件", snapshot: snapshot(), readiness: null, expected: "正在确认设备状态" },
    { condition: "开工条件通过但设备故障", snapshot: { ...snapshot("FAULT"), fault: "PLC 心跳中断" }, readiness: readiness(), expected: "无法开工 · 设备故障" },
    { condition: "尚未允许开工且没有阻塞明细", snapshot: snapshot(), readiness: readiness({ ready: false }), expected: "暂未就绪" },
    { condition: "设备空闲且开工条件通过", snapshot: snapshot(), readiness: readiness(), expected: "就绪，等待工件" },
    { condition: "没有阻塞但审计仍在等待", snapshot: snapshot(), readiness: readiness({ ready: true, items: readiness().items.map(item => item.key === "audit" ? { ...item, state: "waiting" as const } : item) }), expected: "等待就绪条件" },
    { condition: "设备正在采集", snapshot: snapshot("ACQUIRE"), readiness: readiness(), expected: "本件检测中" },
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

  it.each([
    { condition: "出现阻塞和等待", plc: "blocked", inline: ["PLC 未连接", "等待追溯写入"], links: ["去 PLC 通讯", "去系统设置"], entry: "已就绪 4 项" },
    { condition: "只有多项等待", plc: "waiting", inline: ["PLC 未连接"], links: ["去 PLC 通讯"], entry: "另有 1 项未就绪 · 已就绪 4 项" },
  ])("本件采集中下一件$condition → 阻塞时全部原因与入口内联，只有等待时单行只放第一项、其余计入诊断入口", async ({ plc, inline, links, entry }) => {
    const openDiagnostics = vi.fn();
    const report = readiness({ ready: plc !== "blocked", items: readiness().items.map(item => item.key === "plc"
      ? { ...item, state: plc as "blocked" | "waiting", detail: "PLC 未连接", target: "plc" as const }
      : item.key === "audit" ? { ...item, state: "waiting" as const, detail: "等待追溯写入", target: "settings" as const } : item) });
    render(<MemoryRouter><InspectionReadiness snapshot={snapshot("ACQUIRE")} readiness={report} onDiagnostics={openDiagnostics} /></MemoryRouter>);
    const device = within(screen.getByRole("region", { name: "设备状态" }));
    expect(device.getByText("本件检测中／下一件条件异常")).toBeVisible();
    for (const detail of ["PLC 未连接", "等待追溯写入"]) {
      if (inline.includes(detail)) expect(device.getByText(detail)).toBeVisible();
      else expect(device.queryByText(detail)).not.toBeInTheDocument();
    }
    expect(device.getAllByRole("link").map(link => link.textContent)).toEqual(links);
    await userEvent.click(device.getByRole("button", { name: entry }));
    expect(openDiagnostics).toHaveBeenCalledOnce();
  });

  it("提供统一诊断入口 → 主区只留入口，提示展开后可见且不重复已就绪和告警明细", async () => {
    const openDiagnostics = vi.fn();
    const report = readiness({ ready: false, items: readiness().items.map(item => item.key === "recipe"
      ? { ...item, state: "blocked", detail: "配方未发布", hint: "请在工作台验证并发布", target: "workspace" } : item) });
    render(<MemoryRouter><InspectionReadiness snapshot={{ ...snapshot(), alarms: ["设备告警原文"] }} readiness={report} onDiagnostics={openDiagnostics} /></MemoryRouter>);
    expect(screen.queryByRole("heading", { name: "开工检查清单" })).not.toBeInTheDocument();
    expect(screen.queryByText("设备告警原文")).not.toBeInTheDocument();
    expect(screen.queryByText("请在工作台验证并发布")).not.toBeInTheDocument();
    await userEvent.click(screen.getByLabelText("recipe说明"));
    expect(screen.getByText("请在工作台验证并发布")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "已就绪 5 项 · 诊断详情 1 条" }));
    expect(openDiagnostics).toHaveBeenCalledOnce();
  });
});

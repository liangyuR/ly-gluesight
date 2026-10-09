import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import ShotStrip from "../src/features/cycle/components/ShotStrip";
import TrajectoryMap from "../src/features/cycle/components/TrajectoryMap";
import UnrolledCurve from "../src/features/cycle/components/UnrolledCurve";
import type { FrameView, PointVis } from "../src/features/cycle/types";
import { shotList, workspaceView } from "./fixtures";
import { cycleFrame, cycleMeasurement, cyclePart, widthLayout } from "./cycle-visual-fixtures";

const vis: PointVis[] = ["ok", "exc", "gap", "inv"];

describe("拍照点选择与真实状态显示", () => {
  it("拍照点条按拍照点编号与相机标注", () => {
    const layout = workspaceView().layout; layout.shots[1] = { ...layout.shots[1], id: "B7", poseId: "P1", camera: "CAM-2" };
    render(<ShotStrip layout={layout} part={null} vis={vis} />);
    expect(within(screen.getByRole("button", { name: "查看帧 k1" })).getByText("P1 · CAM-1")).toBeVisible();
    const second = within(screen.getByRole("button", { name: "查看帧 k2" })).getByText("B7 · CAM-2");
    expect(second).toHaveAttribute("title", "k2 · Pose P1");
  });

  it("鼠标和键盘可选帧，受控选中状态与一基帧号一致", async () => {
    const select = vi.fn(); const layout = workspaceView().layout;
    const page = render(<ShotStrip layout={layout} part={cyclePart()} vis={vis} selected={0} onSelect={select} />);
    const first = screen.getByRole("button", { name: "查看帧 k1" }), second = screen.getByRole("button", { name: "查看帧 k2" });
    expect(first).toHaveAttribute("aria-pressed", "true"); await userEvent.click(second); expect(select).toHaveBeenLastCalledWith(1);
    first.focus(); await userEvent.keyboard("{Enter}"); expect(select).toHaveBeenLastCalledWith(0);
    second.focus(); await userEvent.keyboard(" "); expect(select).toHaveBeenLastCalledWith(1);
    page.rerender(<ShotStrip layout={layout} part={cyclePart()} vis={vis} selected={1} onSelect={select} />);
    expect(second).toHaveAttribute("aria-pressed", "true"); expect(first).toHaveAttribute("aria-pressed", "false");
  });

  it.each([
    ["waiting", "等待", "—"], ["measuring", "测量", "到达 20 ms · 测量中"],
    ["done", "完成", "0.90 · 2 点 · 8 ms"], ["locateFailed", "定位失败", "定位分数 0.90"],
    ["error", "测量出错", "见事件日志"], ["missing", "未收到", "未到达"],
  ] as [FrameView["status"], string, string][])("%s 显示对应状态与来源信息", (status, label, foot) => {
    const part = cyclePart(); part.frames[0] = cycleFrame(status);
    render(<ShotStrip layout={workspaceView().layout} part={part} vis={vis} />);
    const shot = within(screen.getByRole("button", { name: "查看帧 k1" }));
    expect(shot.getByText(label)).toBeVisible(); expect(shot.getByText(foot)).toBeVisible();
  });

  it("缺胶和跳号显示异常，不虚构定位阈值或未知数值", () => {
    const part = cyclePart(); part.frames[0] = { ...cycleFrame("missing"), counterJump: true };
    part.frames[1] = { ...cycleFrame("done"), gapPoints: 2, score: null, ms: null };
    render(<ShotStrip layout={workspaceView().layout} part={part} vis={vis} />);
    expect(screen.getByText("帧计数跳号")).toBeVisible(); expect(screen.getByText("缺胶 2")).toBeVisible();
    expect(screen.getByText("— · 2 点 · — ms")).toBeVisible(); expect(screen.queryByText(/undefined|null|< 0.60/)).not.toBeInTheDocument();
  });

  it.each(["id", "hash"])("%s 不匹配的工件快照不使用旧帧状态", mismatch => {
    const part = cyclePart(); if (mismatch === "id") part.recipeId = "B"; else part.recipeHash = "old-hash";
    render(<ShotStrip layout={workspaceView().layout} part={part} vis={vis} />);
    expect(within(screen.getByRole("button", { name: "查看帧 k1" })).getByText("等待")).toBeVisible();
    expect(screen.queryByText("完成")).not.toBeInTheDocument();
  });
});

describe("轨迹显示范围与测量叠加", () => {
  it("选择帧使用该拍照点视野，图上的 k 与拍照点条一致", () => {
    const page = render(<TrajectoryMap layout={workspaceView().layout} vis={vis} focus={0} current={0} />);
    expect(screen.getByLabelText("检测轨迹")).toHaveAttribute("viewBox", "-35 -10 120 80");
    expect(screen.getByText("P1 · CAM-1")).toBeVisible(); expect(screen.getByText("P2 · CAM-1")).toBeVisible(); expect(screen.queryByText(/^k\d/)).not.toBeInTheDocument();
    page.rerender(<TrajectoryMap layout={workspaceView().layout} vis={vis} focus={1} />);
    expect(screen.getByLabelText("检测轨迹")).toHaveAttribute("viewBox", "15 -10 120 80");
  });

  it("三台相机的拍照点各画自己的视野，按相机区分颜色与线型", () => {
    const layout = workspaceView().layout;
    layout.shots = shotList([[25, 30], [75, 30], [50, 10], [50, 50]]).map((s, k) => ({ ...s, camera: ["cam1", "cam2", "cam3", "cam1"][k] }));
    layout.shots[1].fov = [60, 40]; layout.shots[2].fov = [200, 30];
    const page = render(<TrajectoryMap layout={layout} vis={vis} />);
    const frame = (id: string, camera: string) => screen.getByLabelText(`拍照点 ${id} · ${camera} 视野`).querySelector("rect")!;
    const style = (r: Element) => [r.getAttribute("stroke"), r.getAttribute("stroke-dasharray")].join("|");
    const [p1, p2, p3, p4] = [frame("P1", "cam1"), frame("P2", "cam2"), frame("P3", "cam3"), frame("P4", "cam1")];
    // 默认视野 120 × 80；P2、P3 用自己的
    expect([p1, p2, p3].map(r => ["x", "y", "width", "height"].map(a => Number(r.getAttribute(a))))).toEqual([[-35, -10, 120, 80], [45, 10, 60, 40], [-50, -5, 200, 30]]);
    expect(new Set([p1, p2, p3].map(style)).size).toBe(3);
    expect(style(p4)).toBe(style(p1));
    expect(p1.getAttribute("stroke")).toBe("var(--camera-1)"); expect(p3.getAttribute("stroke")).toBe("var(--camera-3)");
    for (const label of ["P1 · cam1", "P2 · cam2", "P3 · cam3", "P4 · cam1"]) expect(screen.getByText(label)).toBeVisible();
    // 整件范围包含最宽的 P3 视野
    const [x0, , w] = screen.getByLabelText("检测轨迹").getAttribute("viewBox")!.split(" ").map(Number);
    expect(x0).toBeLessThanOrEqual(-58); expect(x0 + w).toBeGreaterThanOrEqual(158);
    page.rerender(<TrajectoryMap layout={layout} vis={vis} focus={2} current={2} />);
    expect(screen.getByLabelText("检测轨迹")).toHaveAttribute("viewBox", "-50 -5 200 30");
    expect(frame("P3", "cam3")).toHaveAttribute("stroke", "var(--accent-text)");
  });

  it.each([-1, 2, 99, 1.5])("无效焦点 %s 恢复整件范围，不崩溃", focus => {
    render(<TrajectoryMap layout={workspaceView().layout} vis={vis} focus={focus} />);
    const values = screen.getByLabelText("检测轨迹").getAttribute("viewBox")!.split(" ").map(Number);
    expect(values.every(Number.isFinite)).toBe(true); expect(values[2]).toBeGreaterThan(120);
  });

  it("断胶叠加可见，缩略图省略说明，所选点定位到真实点坐标", () => {
    const page = render(<TrajectoryMap layout={workspaceView().layout} vis={vis} selectedPoint={1} />);
    expect(screen.getByText("断胶 1.0 mm")).toBeVisible();
    expect(screen.getByLabelText("选中测量点 2").querySelector("circle")).toHaveAttribute("cx", "25");
    page.rerender(<TrajectoryMap layout={workspaceView().layout} vis={vis} compact selectedPoint={99} />);
    expect(screen.queryByText(/断胶/)).not.toBeInTheDocument(); expect(screen.queryByLabelText(/选中测量点/)).not.toBeInTheDocument();
  });

  it("任意胶路显示实际轮廓", () => {
    const layout = workspaceView().layout; layout.closed = false; layout.path = { kind: "polyline", points: [[0, 0], [75, 0]], closed: false, radius: 0 };
    render(<TrajectoryMap layout={layout} vis={vis} />);
    const svg = screen.getByLabelText("检测轨迹");
    expect(svg.querySelector('polyline[stroke-width="10"]')).toHaveAttribute("points", "0.0,0.0 25.0,0.0 50.0,0.0 75.0,0.0");
    expect(svg.querySelector("rect[rx]")).not.toBeInTheDocument();
  });
});

describe("展开曲线选点与数据呈现", () => {
  it("曲线按实际值显示公差、断胶和超差，点击显示弧长和值", async () => {
    const select = vi.fn(); render(<UnrolledCurve layout={workspaceView().layout} measured={[cycleMeasurement()]} vis={vis} onSelect={select} />);
    const svg = screen.getByLabelText("位置测量曲线");
    vi.spyOn(svg, "getBoundingClientRect").mockReturnValue({ x: 0, y: 0, left: 0, top: 0, right: 800, bottom: 150, width: 800, height: 150, toJSON() {} });
    expect(svg.querySelector('rect[fill="var(--ok-soft)"]')).toBeInTheDocument();
    expect(svg.querySelector('line[stroke-width="2"]')).toHaveAttribute("stroke", "var(--ng)");
    fireEvent.click(svg, { clientX: 227 });
    expect(select).toHaveBeenLastCalledWith(1);
    expect(screen.getByRole("status")).toHaveTextContent("点 2 · s=1.00 mm · d=4.50 mm · 超公差");
    expect(screen.getByLabelText("曲线选中点 2")).toBeVisible();
    fireEvent.click(svg, { clientX: 10 }); expect(select).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "清除位置曲线选点" }));
    expect(select).toHaveBeenLastCalledWith(null); expect(screen.queryByLabelText("曲线选中点 2")).not.toBeInTheDocument();
  });

  it("键盘滑块可查看未测点，缺胶/缺帧不显示虚构测量值", async () => {
    const m = cycleMeasurement(); m.st = [0, 2, 1, 2];
    render(<UnrolledCurve layout={workspaceView().layout} measured={[m]} vis={["ok", "inv", "gap", "miss"]} />);
    const slider = screen.getByRole("slider", { name: "位置曲线选点" });
    await userEvent.click(slider); expect(screen.getByRole("status")).toHaveTextContent("点 1");
    fireEvent.change(slider, { target: { value: "1" } }); expect(screen.getByRole("status")).toHaveTextContent("d=— mm · 未测成");
    fireEvent.change(slider, { target: { value: "2" } }); expect(screen.getByRole("status")).toHaveTextContent("d=— mm · 断胶");
    fireEvent.change(slider, { target: { value: "3" } }); expect(screen.getByRole("status")).toHaveTextContent("d=— mm · 缺帧");
  });

  it("受控选点显示指定位置，交互回调不会自行改变父级选择", () => {
    const select = vi.fn(); const props = { layout: workspaceView().layout, measured: [cycleMeasurement()], vis, onSelect: select };
    const page = render(<UnrolledCurve {...props} selected={1} />);
    expect(screen.getByRole("status")).toHaveTextContent("点 2");
    fireEvent.change(screen.getByRole("slider"), { target: { value: "2" } });
    expect(select).toHaveBeenCalledWith(2); expect(screen.getByRole("status")).toHaveTextContent("点 2");
    page.rerender(<UnrolledCurve {...props} selected={2} />); expect(screen.getByRole("status")).toHaveTextContent("点 3");
  });

  it("同件后续测不成保留已测值，换件和换量清除旧选点", () => {
    const m = cycleMeasurement(); const failed = { ...m, st: [2, 2, 2, 2] };
    const page = render(<UnrolledCurve layout={widthLayout()} measured={[m, failed]} vis={vis} />);
    fireEvent.change(screen.getByRole("slider"), { target: { value: "1" } });
    expect(screen.getByRole("status")).toHaveTextContent("d=4.50 mm");
    page.rerender(<UnrolledCurve layout={widthLayout()} measured={[cycleMeasurement(2)]} vis={vis} />);
    expect(screen.getByRole("status")).toHaveTextContent("点击曲线");
    fireEvent.change(screen.getByRole("slider"), { target: { value: "1" } });
    page.rerender(<UnrolledCurve layout={widthLayout()} measured={[cycleMeasurement(2)]} vis={vis} quantity="w" />);
    expect(screen.getByRole("status")).toHaveTextContent("点击曲线");
    fireEvent.change(screen.getByRole("slider"), { target: { value: "1" } });
    expect(screen.getByRole("status")).toHaveTextContent("胶宽=2.50 mm");
  });

  it("胶宽空值显示未测，归属条按拍照点着色", () => {
    render(<UnrolledCurve layout={widthLayout()} measured={[cycleMeasurement()]} vis={vis} quantity="w" />);
    const bars = screen.getByLabelText("胶宽测量曲线").querySelectorAll('rect[height="4"]');
    expect(Array.from(bars).map(bar => bar.getAttribute("fill"))).toEqual(["var(--camera-1)", "var(--camera-2)"]);
    fireEvent.change(screen.getByRole("slider"), { target: { value: "2" } });
    expect(screen.getByRole("status")).toHaveTextContent("胶宽=— mm");
  });

  it("开放胶路归属条止于末点，单点不会产生无效 SVG 坐标", () => {
    const layout = workspaceView().layout; layout.closed = false;
    const page = render(<UnrolledCurve layout={layout} measured={[cycleMeasurement()]} vis={vis} />);
    const bars = Array.from(screen.getByLabelText("位置测量曲线").querySelectorAll('rect[height="4"]'));
    expect(bars.every(bar => Number(bar.getAttribute("x")) + Number(bar.getAttribute("width")) <= 792)).toBe(true);
    layout.points = { x: [0], y: [0], k: [0], seg: [0] }; layout.segments[0].s1 = 0;
    page.rerender(<UnrolledCurve layout={layout} measured={[]} vis={["none"]} />);
    expect(screen.getByLabelText("位置测量曲线").outerHTML).not.toMatch(/NaN|Infinity/);
  });

  it("无测量/无胶宽限值仍显示有限坐标，清理 ResizeObserver", () => {
    const layout = workspaceView().layout;
    const original = globalThis.ResizeObserver;
    const disconnect = vi.fn(), observe = vi.fn(); let resize!: ResizeObserverCallback;
    vi.stubGlobal("ResizeObserver", class { constructor(callback: ResizeObserverCallback) { resize = callback; } observe = observe; disconnect = disconnect; });
    try {
      const page = render(<UnrolledCurve layout={layout} measured={[]} vis={[]} quantity="w" />);
      expect(screen.getByLabelText("胶宽测量曲线").outerHTML).not.toMatch(/NaN|Infinity/);
      act(() => resize([{ contentRect: { width: 400, height: 120 } } as ResizeObserverEntry], {} as ResizeObserver));
      expect(screen.getByLabelText("胶宽测量曲线")).toHaveAttribute("width", "400");
      page.unmount(); expect(observe).toHaveBeenCalledTimes(1); expect(disconnect).toHaveBeenCalledTimes(1);
    } finally { vi.stubGlobal("ResizeObserver", original); }
  });
});

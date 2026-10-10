import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import ShotStrip from "../src/features/cycle/components/ShotStrip";
import ShotTiles from "../src/features/cycle/components/ShotTiles";
import UnrolledCurve from "../src/features/cycle/components/UnrolledCurve";
import { computeVis, shotGaps, shotState, taughtPercent, teachStatus } from "../src/features/cycle/vis";
import type { FrameView, PointVis } from "../src/features/cycle/types";
import { workspaceView } from "./fixtures";
import { cycleFrame, cycleMeasurement, cyclePart, cycleResult, widthLayout } from "./cycle-visual-fixtures";

const vis: PointVis[] = ["ok", "exc", "gap", "inv"];

describe("拍照点选择与真实状态显示", () => {
  it("拍照点条按拍照点编号与相机标注", () => {
    const layout = workspaceView().layout; layout.shots[1] = { ...layout.shots[1], id: "B7", poseId: "P1", camera: "CAM-2" };
    render(<ShotStrip layout={layout} part={null} vis={vis} />);
    expect(within(screen.getByRole("button", { name: "查看帧 k1" })).getByText("P1 · CAM-1 · 视角 1")).toBeVisible();
    const second = within(screen.getByRole("button", { name: "查看帧 k2" })).getByText("B7 · CAM-2 · 视角 1");
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

describe("逐拍照点视图", () => {
  /** 四个拍照点：P1、P2 已示教，P3 不检，P4 未示教。 */
  function fourShots() {
    const layout = workspaceView().layout;
    layout.shots = [...layout.shots, { ...layout.shots[0], id: "P3", poseId: "P3", camera: "CAM-2", view: 1, bead: "J2", skip: true }, { id: "P4", poseId: "P4", camera: "CAM-2", view: 1, bead: "J2", skip: false, path: [] }];
    return layout;
  }

  it("每个拍照点一格，按计划顺序标出编号 · 相机 · 胶条，不检与未示教单独显示", () => {
    render(<ShotTiles layout={fourShots()} vis={["ok", "ok", "ok", "gap"]} />);
    const tile = (id: string) => within(screen.getByRole("group", { name: `拍照点 ${id}` }));
    expect(screen.getAllByRole("group").map(g => g.getAttribute("aria-label"))).toEqual(["拍照点 P1", "拍照点 P2", "拍照点 P3", "拍照点 P4"]);
    expect(tile("P1").getByText("P1 · CAM-1 · 视角 1 · J1")).toHaveAttribute("title", "k1 · Pose P1");
    expect(tile("P1").getByText("合格")).toHaveClass("c-ok"); expect(tile("P1").getByText("1.0 mm · 2 站")).toBeVisible();
    expect(tile("P2").getByText("断胶")).toHaveClass("c-ng"); expect(tile("P2").getByText("断胶 s=1.0–2.0 mm")).toBeVisible();
    expect(tile("P3").getByText("不检 · 只要求这一帧到达")).toBeVisible(); expect(tile("P3").queryByLabelText(/示教中线/)).toBeNull();
    expect(tile("P4").getByText("未示教")).toBeVisible(); expect(tile("P4").getByText("未示教中线，不能开工")).toBeVisible();
    expect(screen.getByRole("group", { name: "拍照点 P3" })).toHaveClass("st-skip");
  });

  it.each([
    [["ok", "ok"], "ok", "合格"], [["ok", "exc"], "exc", "局部超差"], [["ng", "exc"], "ng", "NG"], [["gap", "ng"], "gap", "断胶"],
    [["inv", "ok"], "inv", "未测成"], [["miss", "miss"], "miss", "缺帧"], [["ok", "none"], "none", "待测"], [["exc", "inv"], "inv", "未测成"],
  ] as [PointVis[], string, string][])("段内测量点 %j 给拍照点着色为 %s", (states, expected, label) => {
    const layout = workspaceView().layout;
    const vis: PointVis[] = ["ok", "ok", ...states];
    expect(shotState(layout, vis, 1)).toBe(expected); expect(shotState(layout, vis, 0)).toBe("ok");
    render(<ShotTiles layout={layout} vis={vis} />);
    const tile = screen.getByRole("group", { name: "拍照点 P2" });
    expect(tile).toHaveClass(`st-${expected}`); expect(within(tile).getAllByText(label)[0]).toBeVisible();
  });

  it("示教中线与各站按这一帧图像的像素坐标画出，状态分段着色，缺胶处画圈", () => {
    render(<ShotTiles layout={workspaceView().layout} vis={["ok", "exc", "gap", "gap"]} selectedPoint={1} />);
    const first = screen.getByLabelText("拍照点 P1 示教中线");
    // 视图框包住中线并留边，不再有工件坐标的视野框
    const [x0, y0, w, h] = first.getAttribute("viewBox")!.split(" ").map(Number);
    expect(x0 + w / 2).toBeCloseTo(15); expect(y0 + h / 2).toBeCloseTo(10); expect(w).toBeGreaterThan(10);
    const lines = Array.from(first.querySelectorAll("polyline"));
    expect(lines[0]).toHaveAttribute("points", "10.0,10.0 20.0,10.0");
    expect(lines.slice(1).map(l => [l.getAttribute("stroke"), l.getAttribute("points")])).toEqual([["var(--ok)", "10.0,10.0"], ["var(--warn)", "10.0,10.0 20.0,10.0"]]);
    expect(within(first).getByLabelText("选中测量点 2").querySelector("circle")).toHaveAttribute("cx", "20");
    const second = screen.getByLabelText("拍照点 P2 示教中线");
    expect(within(second).getByLabelText("断胶 s=0.0–2.0 mm")).toBeInTheDocument();
    expect(within(second).queryByLabelText(/选中测量点/)).toBeNull();
    expect(document.body.innerHTML).not.toMatch(/NaN|Infinity/);
  });

  it("可点选拍照点，选中与正在采集的拍照点有标记", async () => {
    const select = vi.fn();
    const page = render(<ShotTiles layout={workspaceView().layout} vis={["none", "none", "none", "none"]} selected={0} current={1} onSelect={select} />);
    const p2 = screen.getByRole("button", { name: "拍照点 P2" });
    expect(screen.getByRole("button", { name: "拍照点 P1" })).toHaveAttribute("aria-pressed", "true");
    expect(p2).toHaveClass("current"); await userEvent.click(p2); expect(select).toHaveBeenLastCalledWith(1);
    page.rerender(<ShotTiles layout={workspaceView().layout} vis={[]} />);
    expect(screen.getAllByText("待测")).toHaveLength(2);
  });

  it("缺胶位置是拍照点内的段内弧长，示教状态与示教比例按要检的拍照点算", () => {
    const layout = fourShots();
    expect(shotGaps(layout, ["gap", "ok", "ok", "gap"], 0)).toEqual([{ s0: 0, s1: 1 }]);
    expect(shotGaps(layout, ["gap", "ok", "ok", "gap"], 1)).toEqual([{ s0: 1, s1: 2 }]);
    expect(shotGaps(layout, [], 3)).toEqual([]);
    expect(layout.shots.map(teachStatus)).toEqual(["已示教 2 点 · 1.0 mm", "已示教 2 点 · 1.0 mm", "不检", "未示教"]);
    expect(taughtPercent(layout.shots)).toBeCloseTo(200 / 3); expect(taughtPercent([])).toBe(100);
  });

  it("测量点按段的位置与胶宽限值判超差，整件 NG 的段标成 NG，缺帧的拍照点标缺帧", () => {
    const layout = widthLayout(), part = cyclePart();
    part.frames[1] = cycleFrame("missing");
    const m = { ...cycleMeasurement(), idx: [0, 1], d: [3, 3], w: [2, 3], st: [0, 0] };
    expect(computeVis(layout, part, [m], null)).toEqual(["ok", "exc", "miss", "miss"]);
    const result = cycleResult(); result.segments[0].verdict = "NG_WIDTH";
    expect(computeVis(layout, part, [m], result)).toEqual(["ok", "ng", "miss", "miss"]);
    // 不判位置时偏移不参与；不是本件配方的测量不着色
    layout.segments[0].position = null; m.d = [99, 99]; m.w = [2, 2];
    expect(computeVis(layout, part, [m], null).slice(0, 2)).toEqual(["ok", "ok"]);
    expect(computeVis(layout, { ...part, recipeHash: "old" }, [m], null)).toEqual(["none", "none", "none", "none"]);
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
    expect(screen.getByRole("status")).toHaveTextContent("点 2 · P1 · J1 · s=1.00 mm · d=4.50 mm · 超公差");
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

  it("横轴按拍照点分段：段内是弧长，段与段之间留断口、曲线不相连，点选落到最近的段", () => {
    const select = vi.fn();
    const page = render(<UnrolledCurve layout={workspaceView().layout} measured={[cycleMeasurement()]} vis={["ok", "ok", "ok", "ok"]} onSelect={select} />);
    const svg = screen.getByLabelText("位置测量曲线");
    // 两段各 1 mm，断口 1 mm：横轴总长 3 mm，第二段从 2 mm 起
    const starts = Array.from(svg.querySelectorAll('line[stroke="var(--border-strong)"]')).map(l => Number(l.getAttribute("x1")));
    expect(starts[0]).toBe(38); expect(starts[1]).toBeCloseTo(38 + 754 * 2 / 3);
    expect(svg.querySelectorAll('rect[fill="var(--bg-hover)"]')).toHaveLength(1);
    expect(svg.querySelectorAll('polyline[stroke="var(--text)"]')).toHaveLength(2);
    expect(within(svg).getByText("P1 · J1")).toBeInTheDocument(); expect(within(svg).getByText("P2 · J1")).toBeInTheDocument();
    vi.spyOn(svg, "getBoundingClientRect").mockReturnValue({ x: 0, y: 0, left: 0, top: 0, right: 800, bottom: 150, width: 800, height: 150, toJSON() {} });
    // 断口靠近第二段的一侧：落到第二段首站
    fireEvent.click(svg, { clientX: 38 + 754 * 1.8 / 3 }); expect(select).toHaveBeenLastCalledWith(2);
    expect(screen.getByRole("status")).toHaveTextContent("点 3 · P2 · J1 · s=0.00 mm · d=3.00 mm · 合格");
    fireEvent.click(svg, { clientX: 792 }); expect(select).toHaveBeenLastCalledWith(3);
    // 单点段不会产生无效坐标
    const layout = workspaceView().layout; layout.points = { x: [0], y: [0], k: [0], seg: [0] }; layout.segments = [{ ...layout.segments[0], count: 1 }];
    page.rerender(<UnrolledCurve layout={layout} measured={[]} vis={["none"]} />);
    expect(screen.getByLabelText("位置测量曲线").outerHTML).not.toMatch(/NaN|Infinity/);
    layout.segments = []; page.rerender(<UnrolledCurve layout={layout} measured={[]} vis={["none"]} />);
    fireEvent.click(screen.getByLabelText("位置测量曲线"), { clientX: 400 }); expect(select).toHaveBeenCalledTimes(2);
  });

  it("不判位置的段不画位置公差带", () => {
    const layout = workspaceView().layout; layout.segments[1].position = null;
    render(<UnrolledCurve layout={layout} measured={[cycleMeasurement()]} vis={[]} />);
    expect(screen.getByLabelText("位置测量曲线").querySelectorAll('rect[fill="var(--ok-soft)"]')).toHaveLength(1);
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

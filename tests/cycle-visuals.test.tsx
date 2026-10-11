import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import ShotTiles from "../src/features/cycle/components/ShotTiles";
import UnrolledCurve from "../src/features/cycle/components/UnrolledCurve";
import { computeVis, shotGaps, shotState, taughtPercent, teachStatus } from "../src/features/cycle/vis";
import type { PointVis } from "../src/features/cycle/types";
import { workspaceView } from "./fixtures";
import { cycleFrame, cycleMeasurement, cyclePart, cycleResult, widthLayout } from "./cycle-visual-fixtures";

const vis: PointVis[] = ["ok", "exc", "gap", "inv"];

describe("逐拍照点视图", () => {
  /** 四个拍照点：P1、P2 已示教，P3 不检，P4 未示教。 */
  function fourShots() {
    const layout = workspaceView().layout;
    layout.shots = [...layout.shots, { ...layout.shots[0], id: "P3", poseId: "P3", camera: "CAM-2", view: 1, bead: "J2", skip: true }, { id: "P4", poseId: "P4", camera: "CAM-2", view: 1, bead: "J2", skip: false, path: [] }];
    return layout;
  }

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
    expect(computeVis(layout, { ...part, recipeRevision: "old" }, [m], null)).toEqual(["none", "none", "none", "none"]);
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

  it("同 SN 新周期也清除曲线内部选点", () => {
    const page = render(<UnrolledCurve layout={widthLayout()} measured={[cycleMeasurement(1, "first")]} vis={vis} />);
    fireEvent.change(screen.getByRole("slider"), { target: { value: "1" } });
    expect(screen.getByRole("status")).toHaveTextContent("点 2");
    page.rerender(<UnrolledCurve layout={widthLayout()} measured={[cycleMeasurement(1, "retest")]} vis={vis} />);
    expect(screen.getByRole("status")).toHaveTextContent("点击曲线");
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

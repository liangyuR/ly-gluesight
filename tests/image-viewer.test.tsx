import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";

import { beforeEach, describe, expect, it, vi } from "vitest";
import { GrayViewer, useGrayImage } from "../src/features/workspace/components";
import { workspaceApi } from "../src/features/workspace/api";
import type { GrayImage } from "../src/features/workspace/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), recordImage: vi.fn() } }));
const image: GrayImage = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
beforeEach(() => {
  vi.mocked(workspaceApi.image).mockResolvedValue(image);
  vi.mocked(workspaceApi.recordImage).mockResolvedValue(image);
});

describe("图像查看与加载", () => {
  it("搜索带沿局部法向显示半宽并裁剪到原图，独立隐藏不影响测量结果", () => {
    const overlay = {
      path: [[5, 10], [5, 10], [5, 50], [25, 50]] as [number, number][],
      searchHalfPx: 10,
      segments: [{ from: [2, 20] as [number, number], to: [8, 20] as [number, number], color: "red", label: "候选 0.6 mm" }],
    };
    const { rerender } = render(<GrayViewer image={image} label="带界图像" overlay={overlay} />);
    const band = screen.getByLabelText("搜索带边界");
    const edges = [...band.querySelectorAll("polyline")].map(edge => edge.getAttribute("points")!.split(" ").map(p => p.split(",").map(Number)));
    expect(edges[0]).toHaveLength(31);
    expect(edges[0][0]).toEqual([-5, 10]);
    expect(edges[1][0]).toEqual([15, 10]);
    expect(edges[0][20][0]).toBeCloseTo(5 - Math.sqrt(50));
    expect(edges[0][20][1]).toBeCloseTo(50 + Math.sqrt(50));
    expect(edges[1][30]).toEqual([25, 40]);
    expect(band).toHaveAttribute("clip-path", `url(#${band.querySelector("clipPath")!.id})`);
    expect(band.querySelector("clipPath rect")).toHaveAttribute("width", "100");
    expect(band.querySelector("clipPath rect")).toHaveAttribute("height", "60");
    fireEvent.click(screen.getByRole("button", { name: "搜索带" }));
    expect(screen.queryByLabelText("搜索带边界")).not.toBeInTheDocument();
    expect(screen.getByText("候选 0.6 mm")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "搜索带" }));
    fireEvent.click(screen.getByRole("button", { name: "中线叠加" }));
    expect(screen.getByLabelText("搜索带边界")).toBeInTheDocument();
    for (const searchHalfPx of [NaN, Infinity, 0, -1]) {
      rerender(<GrayViewer image={image} label="带界图像" overlay={{ ...overlay, searchHalfPx }} />);
      expect(screen.queryByLabelText("搜索带边界")).not.toBeInTheDocument();
    }
  });
  it("从候选图切到历史原图后，迟到的旧图不能覆盖当前来源", async () => {
    const old = deferred<GrayImage>();
    vi.mocked(workspaceApi.image).mockReturnValueOnce(old.promise);
    const current = { ...image, url: "data:,history-current" };
    vi.mocked(workspaceApi.recordImage).mockResolvedValueOnce(current);
    const { result, rerender } = renderHook(({ history }) => useGrayImage(history ? null : "A", history ? null : "candidate-image", history, 1, 3), { initialProps: { history: null as number | null } });
    expect(result.current.loading).toBe(true);
    rerender({ history: 12 });
    await waitFor(() => expect(result.current.image).toEqual(current));
    await act(async () => old.resolve(image));
    expect(result.current.image).toEqual(current);
    expect(workspaceApi.recordImage).toHaveBeenLastCalledWith(12, 1, 3);
  });

  it("点空白处在末尾加点并可接着拖动，按中线点只拖动不加点；整次拖动使用按下时的原图坐标", () => {
    let offset = 20;
    const previous = globalThis.DOMPoint;
    class TestPoint {
      constructor(public x: number, public y: number) {}
      matrixTransform(matrix: { offset: number }) { return new TestPoint(this.x, this.y - matrix.offset); }
    }
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
    const edit = { add: vi.fn(), move: vi.fn(), select: vi.fn() };
    try {
      render(<GrayViewer image={image} label="示教图像" overlay={{ path: [[50, 30]] }} onEdit={edit} />);
      const svg = screen.getByRole("img", { name: "示教图像" });
      const group = svg.querySelector("g")!;
      Object.defineProperty(group, "getScreenCTM", { value: () => ({ inverse: () => ({ offset }) }) });
      Object.defineProperty(svg, "setPointerCapture", { value: vi.fn() });
      fireEvent.pointerDown(svg, { clientX: 10.04, clientY: 30, pointerId: 1 });
      expect(edit.add).toHaveBeenLastCalledWith([10, 10]); expect(edit.select).toHaveBeenLastCalledWith(1);
      offset = 60;
      fireEvent.pointerMove(svg, { clientX: 50, clientY: 65, pointerId: 1 });
      expect(edit.move).toHaveBeenLastCalledWith(1, [50, 45]);
      fireEvent.pointerMove(svg, { clientX: 500, clientY: -20, pointerId: 1 });
      expect(edit.move).toHaveBeenLastCalledWith(1, [100, 0]);
      fireEvent.pointerUp(svg, { pointerId: 1 });
      fireEvent.pointerMove(svg, { clientX: 80, clientY: 75, pointerId: 1 });
      expect(edit.move).toHaveBeenCalledTimes(2);
      fireEvent.pointerDown(screen.getByLabelText("中线点 1"), { clientX: 12, clientY: 80, pointerId: 2 });
      expect(edit.add).toHaveBeenCalledTimes(1); expect(edit.select).toHaveBeenLastCalledWith(0);
      fireEvent.pointerMove(svg, { clientX: 14, clientY: 82, pointerId: 2 });
      expect(edit.move).toHaveBeenLastCalledWith(0, [14, 22]);
      fireEvent.pointerCancel(svg, { pointerId: 2 });
      fireEvent.pointerMove(svg, { clientX: 1, clientY: 61, pointerId: 2 });
      expect(edit.move).toHaveBeenCalledTimes(3);
      fireEvent.click(screen.getByRole("button", { name: "中线叠加" }));
      fireEvent.pointerDown(svg, { clientX: 30, clientY: 70, pointerId: 3 });
      expect(edit.add).toHaveBeenCalledTimes(1);
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });
  it("刷选固定按下时图像坐标 → 不修改中线，中键和取消后的移动不继续刷选", () => {
    let offset = 20;
    const previous = globalThis.DOMPoint;
    class TestPoint {
      constructor(public x: number, public y: number) {}
      matrixTransform(matrix: { offset: number }) { return new TestPoint(this.x, this.y - matrix.offset); }
    }
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
    const onBrush = vi.fn(), edit = { add: vi.fn(), move: vi.fn(), select: vi.fn() };
    try {
      render(<GrayViewer image={image} label="刷选图像" overlay={{ path: [[50, 30]] }} onEdit={edit} onBrush={onBrush} />);
      const svg = screen.getByRole("img", { name: "刷选图像" });
      Object.defineProperty(svg.querySelector("g")!, "getScreenCTM", { value: () => ({ inverse: () => ({ offset }) }) });
      Object.defineProperty(svg, "getScreenCTM", { value: () => ({ inverse: () => ({ offset }) }) });
      fireEvent.pointerDown(svg, { clientX: 10, clientY: 30, pointerId: 1 });
      offset = 60;
      fireEvent.pointerMove(svg, { clientX: 50, clientY: 65, pointerId: 1 });
      expect(onBrush).toHaveBeenLastCalledWith([[10, 10], [50, 45]]);
      fireEvent.pointerMove(svg, { clientX: 500, clientY: 500, pointerId: 1 });
      expect(onBrush).toHaveBeenLastCalledWith([[10, 10], [50, 45], [99, 59]]);
      expect(edit.add).not.toHaveBeenCalled();
      expect(edit.move).not.toHaveBeenCalled();
      fireEvent.pointerCancel(svg, { pointerId: 1 });
      fireEvent.pointerMove(svg, { clientX: 80, clientY: 75, pointerId: 1 });
      fireEvent.pointerDown(svg, { button: 1, clientX: 10, clientY: 30, pointerId: 2 });
      fireEvent.pointerMove(svg, { clientX: 20, clientY: 40, pointerId: 2 });
      expect(onBrush).toHaveBeenCalledTimes(3);
      fireEvent.pointerUp(svg, { pointerId: 2 });
      fireEvent.pointerDown(svg, { clientX: 30, clientY: 80, pointerId: 3 });
      expect(onBrush).toHaveBeenLastCalledWith([[30, 20]]);
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });
  it("密集中线隐藏内部编辑圆点，展开后可拖动且测量横线始终位于上层", () => {
    const previous = globalThis.DOMPoint;
    class TestPoint {
      constructor(public x: number, public y: number) {}
      matrixTransform() { return this; }
    }
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
    const edit = { add: vi.fn(), move: vi.fn(), select: vi.fn() };
    try {
      const path = Array.from({ length: 158 }, (_, i) => [50, i / 3] as [number, number]);
      render(<GrayViewer image={image} label="测量图像" onEdit={edit} overlay={{ path, segments: [{ from: [40, 30], to: [60, 30], color: "green", label: "3.5 mm" }] }} />);
      const svg = screen.getByRole("img", { name: "测量图像" });
      Object.defineProperty(svg.querySelector("g")!, "getScreenCTM", { value: () => ({ inverse: () => ({}) }) });
      const measurement = screen.getByLabelText("测量横线");
      expect(svg.querySelectorAll("[data-vertex]")).toHaveLength(2);
      expect(measurement.querySelector("line")).toHaveAttribute("vector-effect", "non-scaling-stroke");
      fireEvent.click(screen.getByRole("button", { name: "编辑点" }));
      const vertex = screen.getByLabelText("中线点 80");
      expect(vertex.compareDocumentPosition(measurement) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
      fireEvent.pointerDown(vertex, { clientX: 50, clientY: 26, pointerId: 1 });
      fireEvent.pointerMove(svg, { clientX: 55, clientY: 27, pointerId: 1 });
      expect(edit.move).toHaveBeenLastCalledWith(79, [55, 27]);
      expect(edit.add).not.toHaveBeenCalled();
      fireEvent.pointerUp(svg, { pointerId: 1 });
      fireEvent.click(screen.getByRole("button", { name: "编辑点" }));
      expect(screen.queryByLabelText("中线点 80")).not.toBeInTheDocument();
      expect(screen.getByText("3.5 mm")).toBeInTheDocument();
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });
  it("Ctrl 滚轮以鼠标处缩放并限制倍率，普通滚轮仍滚动页面，键盘可复位", () => {
    const previous = globalThis.DOMPoint;
    class TestPoint {
      constructor(public x: number, public y: number) {}
      matrixTransform() { return this; }
    }
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
    try {
      render(<GrayViewer image={image} label="缩放图像" />);
      const svg = screen.getByRole("img", { name: "缩放图像" });
      Object.defineProperty(svg, "getScreenCTM", { value: () => ({ inverse: () => ({}) }) });
      const ordinary = new WheelEvent("wheel", { deltaY: -100, bubbles: true, cancelable: true });
      fireEvent(svg, ordinary);
      expect(ordinary.defaultPrevented).toBe(false);
      expect(screen.getByText("100%")).toBeVisible();
      const modified = new WheelEvent("wheel", { ctrlKey: true, deltaY: -100, clientX: 25, clientY: 15, bubbles: true, cancelable: true });
      fireEvent(svg, modified);
      expect(modified.defaultPrevented).toBe(true);
      expect(screen.getByText("122%")).toBeVisible();
      const transform = svg.querySelector("g")!.getAttribute("transform")!;
      const [tx, ty, scale] = transform.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
      expect(tx + (25 - image.width / 2) * scale).toBeCloseTo(25);
      expect(ty + (15 - image.height / 2) * scale).toBeCloseTo(15);
      for (let n = 0; n < 20; n++) fireEvent.wheel(svg, { ctrlKey: true, deltaY: -200 });
      expect(screen.getByText("400%")).toBeVisible();
      for (let n = 0; n < 20; n++) fireEvent.wheel(svg, { ctrlKey: true, deltaY: 200 });
      expect(screen.getByText("50%")).toBeVisible();
      fireEvent.keyDown(svg, { key: "0", ctrlKey: true });
      expect(screen.getByText("100%")).toBeVisible();
      fireEvent.keyDown(svg, { key: "+" });
      expect(screen.getByText("125%")).toBeVisible();
      fireEvent.keyDown(svg, { key: "-" });
      expect(screen.getByText("100%")).toBeVisible();
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });

  it("放大原图 → 中线编辑点在屏幕上保持原大小，不随倍率放大遮住胶路", () => {
    render(<GrayViewer image={image} label="编辑点图像" overlay={{ path: [[20, 20], [60, 30]] }} onEdit={{ add: vi.fn(), move: vi.fn(), select: vi.fn() }} />);
    const radius = () => Number(screen.getByLabelText("中线点 1").getAttribute("r"));
    const before = radius();
    fireEvent.keyDown(screen.getByRole("img", { name: "编辑点图像" }), { key: "+" });
    fireEvent.keyDown(screen.getByRole("img", { name: "编辑点图像" }), { key: "+" });
    expect(screen.getByText("156%")).toBeVisible();
    expect(radius() * 1.5625).toBeCloseTo(before);
  });

  it("原图缩小显示且站点很密 → 站点半径不超过半个站距，单个 NG 站点不被相邻站点盖住", () => {
    const width = vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(50);
    const height = vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(30);
    try {
      const stations = [10, 18, 26, 34].map(x => [x, 30] as [number, number]);
      render(<GrayViewer image={image} label="紧凑图像" overlay={{ path: [[0, 30], [50, 30]], stations, stationColors: ["var(--ok)", "var(--ng)", "var(--ok)", "var(--ok)"] }} />);
      const radii = [...screen.getByLabelText("中线叠加").querySelectorAll("circle")].map(c => Number(c.getAttribute("r")));
      expect(radii).toHaveLength(4);
      for (const r of radii) expect(r * 2).toBeLessThanOrEqual(8);
    } finally {
      width.mockRestore();
      height.mockRestore();
    }
  });

  it.each([false, true])("中键平移（框选模式 %s）不修改示教，取消拖动和换图后不残留视角", (regionMode) => {
    const previous = globalThis.DOMPoint;
    class TestPoint {
      constructor(public x: number, public y: number) {}
      matrixTransform() { return new TestPoint(this.x / 2, this.y / 2); }
    }
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
    const edit = { add: vi.fn(), move: vi.fn(), select: vi.fn() };
    const onRegion = vi.fn();
    try {
      const { rerender } = render(<GrayViewer image={image} label="平移图像" overlay={{ path: [[20,20]] }} onEdit={edit} onRegion={regionMode ? onRegion : undefined} />);
      const svg = screen.getByRole("img", { name: "平移图像" });
      Object.defineProperty(svg, "getScreenCTM", { value: () => ({ inverse: () => ({}) }) });
      fireEvent.pointerDown(svg, { button: 1, clientX: 20, clientY: 20, pointerId: 1 });
      fireEvent.pointerMove(svg, { clientX: 60, clientY: 40, pointerId: 1 });
      expect(svg.querySelector("g")).toHaveAttribute("transform", "translate(70 40) scale(1) translate(-50 -30)");
      expect(edit.add).not.toHaveBeenCalled();
      expect(edit.move).not.toHaveBeenCalled();
      expect(onRegion).not.toHaveBeenCalled();
      fireEvent.pointerCancel(svg, { pointerId: 1 });
      fireEvent.pointerMove(svg, { clientX: 100, clientY: 100, pointerId: 1 });
      expect(svg.querySelector("g")).toHaveAttribute("transform", "translate(70 40) scale(1) translate(-50 -30)");
      fireEvent.click(screen.getByRole("button", { name: "适应窗口" }));
      expect(svg.querySelector("g")).toHaveAttribute("transform", "translate(50 30) scale(1) translate(-50 -30)");
      fireEvent.keyDown(svg, { key: "+" });
      fireEvent.pointerDown(svg, { button: 1, clientX: 20, clientY: 20, pointerId: 2 });
      fireEvent.pointerMove(svg, { clientX: 60, clientY: 40, pointerId: 2 });
      rerender(<GrayViewer image={{ ...image, url: "data:,next-image" }} label="平移图像" />);
      expect(screen.getByText("100%")).toBeVisible();
      expect(svg.querySelector("g")).toHaveAttribute("transform", "translate(50 30) scale(1) translate(-50 -30)");
      fireEvent.pointerMove(svg, { clientX: 100, clientY: 100, pointerId: 2 });
      expect(svg.querySelector("g")).toHaveAttribute("transform", "translate(50 30) scale(1) translate(-50 -30)");
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });

});

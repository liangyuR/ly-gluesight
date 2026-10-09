import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GrayViewer, useGrayImage } from "../src/features/workspace/components";
import { workspaceApi } from "../src/features/workspace/api";
import type { GrayImage } from "../src/features/workspace/types";
import { deferred, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), recordImage: vi.fn() } }));
const image: GrayImage = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
beforeEach(() => {
  vi.mocked(workspaceApi.image).mockResolvedValue(image);
  vi.mocked(workspaceApi.recordImage).mockResolvedValue(image);
});

describe("图像查看与加载", () => {
  it("未取样、读取中、读取失败的提示各自明确", () => {
    const page = render(<GrayViewer image={null} label="k1" />);
    expect(screen.getByText("尚未冻结本帧图像")).toBeVisible();
    page.rerender(<GrayViewer image={null} loading label="k1" />);
    expect(screen.getByText("正在读取原图…")).toBeVisible();
    page.rerender(<GrayViewer image={null} error="文件已清理" label="k1" />);
    expect(screen.getByText("原图不可用")).toBeVisible(); expect(screen.getByText("文件已清理")).toBeVisible();
  });

  it("缩放有边界，切换图像恢复 100%，中线叠加可切换", async () => {
    const page = render(<GrayViewer image={image} label="k1" overlay={{ path: [[10, 10], [40, 20]] }} />);
    await userEvent.click(screen.getByRole("button", { name: "放大原图" })); expect(screen.getByText("125%")).toBeVisible();
    for (let i = 0; i < 5; i++) await userEvent.click(screen.getByRole("button", { name: "缩小原图" }));
    expect(screen.getByText("50%")).toBeVisible();
    expect(screen.getByLabelText("中线叠加")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "中线叠加" }));
    expect(screen.getByRole("button", { name: "中线叠加" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByLabelText("中线叠加")).toBeNull();
    page.rerender(<GrayViewer image={{ ...image, url: "data:,new" }} label="k2" />);
    expect(screen.getByText("100%")).toBeVisible(); expect(screen.queryByRole("button", { name: "中线叠加" })).toBeNull();
  });

  it("图像切换后旧读取不会覆盖当前图像", async () => {
    const old = deferred<GrayImage>();
    vi.mocked(workspaceApi.image).mockReturnValueOnce(old.promise).mockResolvedValueOnce({ ...image, url: "data:,latest" });
    const { result, rerender } = renderHook(({ id }) => useGrayImage("A", id), { initialProps: { id: "first" } });
    expect(result.current.loading).toBe(true); rerender({ id: "latest" });
    await waitFor(() => expect(result.current.image?.url).toBe("data:,latest"));
    await act(async () => old.resolve(image));
    expect(result.current.image?.url).toBe("data:,latest"); expect(result.current.loading).toBe(false);
  });

  it("叠加直接按图像像素画中线与各站，不做对齐变换；只读时没有可拖的中线点", () => {
    const view = workspaceView();
    const page = render(<GrayViewer image={image} label="示教原图" overlay={{ path: [[10, 10], [20, 10]], stations: [[10, 10], [20, 10]], stationColors: ["var(--ok)", "var(--ng)"] }} />);
    expect(page.container.querySelector("polyline")).toHaveAttribute("points", "10,10 20,10");
    const dots = page.container.querySelectorAll("circle");
    expect(Array.from(dots).map(c => [c.getAttribute("cx"), c.getAttribute("cy"), c.getAttribute("fill")])).toEqual([["10", "10", "var(--ok)"], ["20", "10", "var(--ng)"]]);
    expect(screen.queryByLabelText("中线点 1")).toBeNull();
    // 中线改了还没保存：各站淡显；只有一个点时不画折线
    page.rerender(<GrayViewer image={image} label="示教原图" overlay={{ path: [[5, 5]], stations: [[10, 10]], stale: true }} onEdit={{ add: vi.fn(), move: vi.fn(), select: vi.fn() }} />);
    expect(page.container.querySelector("polyline")).toBeNull();
    expect(page.container.querySelector("circle")).toHaveAttribute("opacity", "0.35");
    expect(screen.getByLabelText("中线点 1")).toHaveAttribute("cx", "5");
    expect(page.container.querySelector("svg")!.outerHTML).not.toMatch(/NaN|Infinity/);
    expect(view.layout.points.x).toEqual([10, 20, 30, 40]);
  });

  it("读取失败展示原因，重新选择后清除错误", async () => {
    vi.mocked(workspaceApi.image).mockRejectedValueOnce(new Error("原图不存在"));
    const { result, rerender } = renderHook(({ id }) => useGrayImage("A", id), { initialProps: { id: "missing" } });
    await waitFor(() => expect(result.current.error).toContain("原图不存在"));
    rerender({ id: "available" }); await waitFor(() => expect(result.current.image).toEqual(image));
    expect(result.current.error).toBe("");
  });

  it("历史原图按记录和帧号读取，无标识时不发请求", async () => {
    const { result, rerender } = renderHook(({ history }) => useGrayImage(null, null, history, 1), { initialProps: { history: null as number | null } });
    expect(workspaceApi.image).not.toHaveBeenCalled(); expect(workspaceApi.recordImage).not.toHaveBeenCalled();
    rerender({ history: 12 }); await waitFor(() => expect(result.current.image).toEqual(image));
    expect(workspaceApi.recordImage).toHaveBeenCalledWith(12, 1);
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
      // 坐标截在图内，取 0.1 px
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
      // 关掉叠加时不能编辑
      fireEvent.click(screen.getByRole("button", { name: "中线叠加" }));
      fireEvent.pointerDown(svg, { clientX: 30, clientY: 70, pointerId: 3 });
      expect(edit.add).toHaveBeenCalledTimes(1);
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });
});

import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";

import { beforeEach, describe, expect, it, vi } from "vitest";
import { GrayViewer, useGrayImage } from "../src/features/workspace/components";
import { workspaceApi } from "../src/features/workspace/api";
import type { GrayImage } from "../src/features/workspace/types";
import { deferred } from "./fixtures";
import { stubScreenCTM, withDOMPoint } from "./svg-pointer";

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


  it("刷选固定按下时图像坐标 → 不修改中线，中键和取消后的移动不继续刷选", () => {
    let offset = 20;
    const onBrush = vi.fn(), edit = { add: vi.fn(), move: vi.fn(), select: vi.fn() };
    withDOMPoint<{ offset: number }>((x, y, matrix) => [x, y - matrix.offset], () => {
      render(<GrayViewer image={image} label="刷选图像" overlay={{ path: [[50, 30]] }} onEdit={edit} onBrush={onBrush} />);
      const svg = screen.getByRole("img", { name: "刷选图像" });
      stubScreenCTM(svg.querySelector("g")!, () => ({ offset }));
      stubScreenCTM(svg, () => ({ offset }));
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
    });
  });









});

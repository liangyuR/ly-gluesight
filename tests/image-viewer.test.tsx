import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FrameViewer, useFrameImage } from "../src/features/workspace/components";
import { workspaceApi } from "../src/features/workspace/api";
import type { FrameImage } from "../src/features/workspace/types";
import { deferred } from "./fixtures";
import { stubScreenCTM, withDOMPoint } from "./svg-pointer";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn() } }));
const image: FrameImage = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
beforeEach(() => {
  vi.mocked(workspaceApi.image).mockReset().mockResolvedValue(image);
});

describe("图像查看与加载", () => {
  it("切换到另一张示教帧后，迟到的上一张图不能覆盖当前图", async () => {
    const old = deferred<FrameImage>();
    const current = { ...image, url: "data:,current" };
    vi.mocked(workspaceApi.image).mockReturnValueOnce(old.promise).mockResolvedValueOnce(current);
    const { result, rerender } = renderHook(({ imageId }) => useFrameImage("A", imageId), { initialProps: { imageId: "frame-1" } });
    expect(result.current.loading).toBe(true);
    rerender({ imageId: "frame-2" });
    await waitFor(() => expect(result.current.image).toEqual(current));
    await act(async () => old.resolve(image));
    expect(result.current.image).toEqual(current);
    expect(workspaceApi.image).toHaveBeenLastCalledWith("A", "frame-2");
  });

  it("刷选固定按下时图像坐标 → 不修改中线，中键和取消后的移动不继续刷选", () => {
    let offset = 20;
    const onBrush = vi.fn(), edit = { add: vi.fn(), move: vi.fn(), select: vi.fn() };
    withDOMPoint<{ offset: number }>((x, y, matrix) => [x, y - matrix.offset], () => {
      render(<FrameViewer image={image} label="刷选图像" overlay={{ path: [[50, 30]] }} onEdit={edit} onBrush={onBrush} />);
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

  it("长走廊切换全部点柄 → 已选的中间点仍可按按下时的坐标拖动", () => {
    const count = 160;
    function Editor() {
      const [path, setPath] = useState<[number, number][]>(() => Array.from({ length: count }, (_, i) => [5 + i % 30 * 3, 5 + Math.floor(i / 30) * 8]));
      const [selected, select] = useState<number | null>(null);
      return <FrameViewer image={image} label="长走廊" controlsAt="footer" overlay={{ path, selected }} onEdit={{
        add: point => setPath(previous => [...previous, point]),
        move: (index, point) => setPath(previous => previous.map((value, i) => i === index ? point : value)),
        select,
      }} />;
    }
    let offset = 20;
    const selectedLabel = "中线点 129";
    withDOMPoint<{ offset: number }>((x, y, matrix) => [x, y - matrix.offset], () => {
      render(<Editor />);
      const svg = screen.getByRole("img", { name: "长走廊" });
      stubScreenCTM(svg.querySelector("g")!, () => ({ offset }));
      const handles = screen.getByRole("button", { name: "点柄" });
      expect(screen.queryByLabelText(selectedLabel)).not.toBeInTheDocument();
      fireEvent.click(handles);
      fireEvent.pointerDown(screen.getByLabelText("中线点 2"), { clientX: 8, clientY: 25 });
      fireEvent.pointerUp(svg);
      expect(screen.getByLabelText("中线点 2")).toHaveAttribute("fill", "var(--ov-line)");
      fireEvent.pointerDown(screen.getByLabelText(selectedLabel), { clientX: 29, clientY: 57 });
      expect(screen.getByLabelText(selectedLabel)).toHaveAttribute("fill", "var(--ov-line)");
      expect(screen.getByLabelText("中线点 2")).toHaveAttribute("fill", "var(--ov-halo)");
      offset = 50;
      fireEvent.pointerMove(svg, { clientX: 61, clientY: 64 });
      expect(screen.getByLabelText(selectedLabel)).toHaveAttribute("cy", "44");
      fireEvent.pointerUp(svg);

      fireEvent.click(handles);
      expect(screen.queryByLabelText("中线点 2")).not.toBeInTheDocument();
      fireEvent.pointerDown(screen.getByLabelText(selectedLabel), { clientX: 61, clientY: 94 });
      fireEvent.pointerMove(svg, { clientX: 66, clientY: 94 });
      fireEvent.pointerUp(svg);
      fireEvent.pointerMove(svg, { clientX: 70, clientY: 94 });
      const polyline = svg.querySelector('[aria-label="中线叠加"] polyline');
      const points = polyline?.getAttribute("points")?.split(" ");
      expect(points).toHaveLength(count);
      expect(points?.[128]).toBe("66,44");
    });
  });

  it("搜索带在重复点和折返处保留原始边界，隐藏后重开使用当前路径", () => {
    const path: [number, number][] = [[3, 1], [3, 1], [7, 1], [3, 1], [2, 1]];
    const viewer = (points: typeof path) => <FrameViewer image={image} label="搜索带预览" overlay={{ path: points, searchHalfPx: 2 }} />;
    const { container, rerender } = render(viewer(path));
    const band = () => container.querySelector('[aria-label="搜索带边界"]');
    const edges = () => [...band()!.querySelectorAll("polyline")].map(line => line.getAttribute("points"));
    expect(edges()).toEqual(["3,3 5,3 7,3 5,-1 3,-1 2,-1", "3,-1 5,-1 7,-1 5,3 3,3 2,3"]);
    expect([...band()!.querySelectorAll("line")].map(line => line.getAttribute("x1"))).toEqual(["3", "7", "3", "2"]);

    fireEvent.click(screen.getByRole("button", { name: "中线叠加" }));
    expect(band()).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "搜索带" }));
    expect(band()).not.toBeInTheDocument();
    rerender(viewer(path.map(([x, y]) => [x, y + 4])));
    expect(band()).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "搜索带" }));
    expect(edges()).toEqual(["3,7 5,7 7,7 5,3 3,3 2,3", "3,3 5,3 7,3 5,7 3,7 2,7"]);
  });

  it("框选延伸到图像右下角 → 选区覆盖最后一行和一列", () => {
    const onRegion = vi.fn();
    withDOMPoint(null, () => {
      render(<FrameViewer image={image} label="框选图像" onRegion={onRegion} />);
      const svg = screen.getByRole("img", { name: "框选图像" });
      stubScreenCTM(svg.querySelector("g")!);
      fireEvent.pointerDown(svg, { clientX: 0, clientY: 0 });
      fireEvent.pointerMove(svg, { clientX: 120, clientY: 80 });
      fireEvent.pointerUp(svg);
      expect(onRegion).toHaveBeenLastCalledWith([0, 0, 100, 60]);
    });
  });
});

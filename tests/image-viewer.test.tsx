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

  it("图像切换后旧读取不会覆盖当前图像", async () => {
    const old = deferred<GrayImage>();
    vi.mocked(workspaceApi.image).mockReturnValueOnce(old.promise).mockResolvedValueOnce({ ...image, url: "data:,latest" });
    const { result, rerender } = renderHook(({ id }) => useGrayImage("A", id), { initialProps: { id: "first" } });
    expect(result.current.loading).toBe(true); rerender({ id: "latest" });
    await waitFor(() => expect(result.current.image?.url).toBe("data:,latest"));
    await act(async () => old.resolve(image));
    expect(result.current.image?.url).toBe("data:,latest"); expect(result.current.loading).toBe(false);
  });

  it("历史原图按记录和帧号读取，无标识时不发请求", async () => {
    const { result, rerender } = renderHook(({ history }) => useGrayImage(null, null, history, 1), { initialProps: { history: null as number | null } });
    expect(workspaceApi.image).not.toHaveBeenCalled(); expect(workspaceApi.recordImage).not.toHaveBeenCalled();
    rerender({ history: 12 }); await waitFor(() => expect(result.current.image).toEqual(image));
    expect(workspaceApi.recordImage).toHaveBeenCalledWith(12, 1, undefined);
  });

  it("历史视角切换后迟到的旧视角不能覆盖当前图像", async () => {
    const old = deferred<GrayImage>();
    vi.mocked(workspaceApi.recordImage).mockReturnValueOnce(old.promise);
    const current = {...image, url: "data:image/png;base64,current"};
    vi.mocked(workspaceApi.recordImage).mockResolvedValueOnce(current);
    const {result, rerender} = renderHook(({view}) => useGrayImage(null, null, 12, 1, view), {initialProps: {view: 2}});
    rerender({view: 3});
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

import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";

import { beforeEach, describe, expect, it, vi } from "vitest";
import { GrayViewer, useGrayImage } from "../src/features/workspace/components";
import { workspaceApi } from "../src/features/workspace/api";
import type { GrayImage } from "../src/features/workspace/types";
import { deferred } from "./fixtures";
import { stubScreenCTM, withDOMPoint } from "./svg-pointer";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn() } }));
const image: GrayImage = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
beforeEach(() => {
  vi.mocked(workspaceApi.image).mockReset().mockResolvedValue(image);
});

describe("图像查看与加载", () => {
  it("切换到另一张示教帧后，迟到的上一张图不能覆盖当前图", async () => {
    const old = deferred<GrayImage>();
    const current = { ...image, url: "data:,current" };
    vi.mocked(workspaceApi.image).mockReturnValueOnce(old.promise).mockResolvedValueOnce(current);
    const { result, rerender } = renderHook(({ imageId }) => useGrayImage("A", imageId), { initialProps: { imageId: "frame-1" } });
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

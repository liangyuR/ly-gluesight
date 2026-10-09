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

  it("缩放有边界，切换图像恢复 100%，叠加可切换", async () => {
    const page = render(<GrayViewer image={image} label="k1" params={workspaceView().workspace.frames[0].params} />);
    await userEvent.click(screen.getByRole("button", { name: "放大原图" })); expect(screen.getByText("125%")).toBeVisible();
    for (let i = 0; i < 5; i++) await userEvent.click(screen.getByRole("button", { name: "缩小原图" }));
    expect(screen.getByText("50%")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "测量叠加" }));
    expect(screen.getByRole("button", { name: "测量叠加" })).toHaveAttribute("aria-pressed", "false");
    page.rerender(<GrayViewer image={{ ...image, url: "data:,new" }} label="k2" />);
    expect(screen.getByText("100%")).toBeVisible();
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

  it.each([
    ["dx", NaN], ["dy", Infinity], ["deg", NaN], ["mmPerPx", NaN], ["mmPerPx", Infinity], ["mmPerPx", 0],
  ] as const)("参数 %s=%s 未填完时保留原图和模板，不生成无效坐标，填写后恢复叠加", (key, value) => {
    const view = workspaceView(), params = view.workspace.frames[0].params;
    const page = render(<GrayViewer image={image} label="示教原图" params={params} layout={view.layout} />);
    expect(page.container.querySelector("polyline")).not.toBeNull();
    page.rerender(<GrayViewer image={image} label="示教原图" params={{ ...params, [key]: value }} layout={view.layout} />);
    expect(screen.getByRole("img", { name: "示教原图" })).toBeVisible();
    expect(page.container.querySelector("polyline")).toBeNull(); expect(page.container.querySelector("rect")).not.toBeNull();
    expect(page.container.querySelector("svg")!.outerHTML).not.toMatch(/NaN|Infinity/);
    page.rerender(<GrayViewer image={image} label="示教原图" params={params} layout={view.layout} />);
    expect(page.container.querySelector("polyline")).not.toBeNull();
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

  it("拖模板过程中提示条改变图像位置，仍使用按下时的原图坐标", () => {
    let offset = 20;
    const previous = globalThis.DOMPoint;
    class TestPoint {
      constructor(public x: number, public y: number) {}
      matrixTransform(matrix: { offset: number }) { return new TestPoint(this.x, this.y - matrix.offset); }
    }
    Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: TestPoint });
    const onRect = vi.fn();
    try {
      render(<GrayViewer image={image} label="模板图像" onRect={onRect} />);
      const svg = screen.getByRole("img", { name: "模板图像" });
      const group = svg.querySelector("g")!;
      Object.defineProperty(group, "getScreenCTM", { value: () => ({ inverse: () => ({ offset }) }) });
      Object.defineProperty(svg, "setPointerCapture", { value: vi.fn() });
      fireEvent.pointerDown(svg, { clientX: 10, clientY: 30, pointerId: 1 });
      offset = 60;
      fireEvent.pointerMove(svg, { clientX: 50, clientY: 65, pointerId: 1 });
      expect(onRect).toHaveBeenLastCalledWith([10, 10, 40, 35]);
      fireEvent.pointerCancel(svg, { pointerId: 1 });
      fireEvent.pointerMove(svg, { clientX: 80, clientY: 75, pointerId: 1 });
      expect(onRect).toHaveBeenCalledTimes(1);
    } finally {
      Object.defineProperty(globalThis, "DOMPoint", { configurable: true, value: previous });
    }
  });
});

import { act, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import RuntimeFrame from "../src/features/workspace/RuntimeFrame";
import { workspaceApi } from "../src/features/workspace/api";
import { GrayViewer } from "../src/features/workspace/components";
import type { GrayImage } from "../src/features/workspace/types";
import type { FrameView, PartView, Recipe } from "../src/features/cycle/types";
import { cycleFrame, cyclePart } from "./cycle-visual-fixtures";
import { deferred, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { liveImage: vi.fn() } }));

const layout = workspaceView().layout;
const partWith = (cycleId: string, status: FrameView["status"] = "done"): PartView => ({ ...cyclePart("1", cycleId), frames: [cycleFrame(status), cycleFrame("done", 1)] });
const frame = (part: PartView) => <RuntimeFrame compact part={part} layout={layout} k={0} measured={[]} />;

beforeEach(() => {
  vi.mocked(workspaceApi.liveImage).mockReset().mockImplementation(async (cycleId, _revision, k) => ({ url: `live:${cycleId}/${k}`, width: 100, height: 60 }));
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("读图期间工件连续更换 → 读完只补读最新一件的帧，中间的不读", async () => {
  const first = deferred<GrayImage>();
  vi.mocked(workspaceApi.liveImage).mockReturnValueOnce(first.promise);
  const view = render(frame(partWith("cycle-1")));
  view.rerender(frame(partWith("cycle-2")));
  view.rerender(frame(partWith("cycle-3")));
  await act(async () => first.resolve({ url: "live:cycle-1/0", width: 100, height: 60 }));
  await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledTimes(2));
  expect(vi.mocked(workspaceApi.liveImage).mock.calls.map(call => call[0])).toEqual(["cycle-1", "cycle-3"]);
});

it("帧从等待到到达读一次原图，之后测量状态变化不再重读", async () => {
  const view = render(frame(partWith("cycle-1", "waiting")));
  await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledTimes(1));
  view.rerender(frame(partWith("cycle-1", "measuring")));
  await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledTimes(2));
  view.rerender(frame(partWith("cycle-1", "done")));
  view.rerender(frame(partWith("cycle-1", "error")));
  await act(async () => {});
  expect(workspaceApi.liveImage).toHaveBeenCalledTimes(2);
});

it("同一件切到下一拍照点、新图未返回 → 仍显示上一张图及其标签，返回后换成新图", async () => {
  const next = deferred<GrayImage>();
  const part = partWith("cycle-1");
  const shownHref = (container: HTMLElement) => container.querySelector("image")?.getAttribute("href");
  const view = render(<RuntimeFrame compact part={part} layout={layout} k={0} measured={[]} />);
  await waitFor(() => expect(shownHref(view.container)).toBe("live:cycle-1/0"));
  vi.mocked(workspaceApi.liveImage).mockReturnValueOnce(next.promise);
  view.rerender(<RuntimeFrame compact part={part} layout={layout} k={1} measured={[]} />);
  await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledTimes(2));
  expect(shownHref(view.container)).toBe("live:cycle-1/0");
  expect(view.container.querySelector(".wp-image-tools")).toHaveTextContent("k1");
  await act(async () => next.resolve({ url: "live:cycle-1/1", width: 100, height: 60 }));
  await waitFor(() => expect(shownHref(view.container)).toBe("live:cycle-1/1"));
  expect(view.container.querySelector(".wp-image-tools")).toHaveTextContent("k2");
});

it("换到下一件、新图未返回 → 不沿用上一件的图", async () => {
  const next = deferred<GrayImage>();
  const view = render(frame(partWith("cycle-1")));
  await waitFor(() => expect(view.container.querySelector("image")).not.toBeNull());
  vi.mocked(workspaceApi.liveImage).mockReturnValueOnce(next.promise);
  view.rerender(frame(partWith("cycle-2")));
  await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledTimes(2));
  expect(view.container.querySelector("image")).toBeNull();
  await act(async () => next.resolve({ url: "live:cycle-2/0", width: 100, height: 60 }));
  await waitFor(() => expect(view.container.querySelector("image")?.getAttribute("href")).toBe("live:cycle-2/0"));
});

it("实时帧带灰度像素 → 画进 canvas，每个像素 R=G=B=灰度且不透明，不生成图片资源", () => {
  const put = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: put } as unknown as CanvasRenderingContext2D);
  vi.stubGlobal("ImageData", class { data: Uint8ClampedArray; constructor(public width: number, public height: number) { this.data = new Uint8ClampedArray(width * height * 4); } });
  const { container } = render(<GrayViewer image={{ url: "live:1", width: 4, height: 2, pixels: { data: Uint8Array.from([0, 17, 128, 255]), width: 2, height: 2 } }} label="实时帧" />);
  expect(container.querySelector("image")).toBeNull();
  expect([...put.mock.calls[0][0].data]).toEqual([0, 0, 0, 255, 17, 17, 17, 255, 128, 128, 128, 255, 255, 255, 255, 255]);
});
it("生产帧带被采用的位姿 → 原图上中线、站点、内框与外圈都画在 T(·)，示教框只作淡显的示教位置", async () => {
  // 只平移 (5, 3) px：期望坐标逐个手算；转角的符号与转心由 pose-overlay 测试守住
  const located: Recipe = structuredClone(layout);
  located.shots[0].fixture = { rect: [40, 20, 16, 16], source: "A-image-0", searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1 };
  const part = partWith("cycle-1");
  part.frames[0].viewResults = [{ view: 1, verdict: "OK", error: null, score: 1, ms: 8, pose: { accepted: true, reason: null, score: .97, peakRatio: .3, angle: 0, offset: [5, 3], endShiftMm: [0, 0], excludedMm: [1, 1], endLimitMm: 9.6 } }];
  const { container } = render(<RuntimeFrame compact part={part} layout={located} k={0} measured={[]} />);
  await waitFor(() => expect(container.querySelector("image")).not.toBeNull());
  const line = container.querySelector('[aria-label="中线叠加"]')!;
  expect(line.querySelector("polyline")?.getAttribute("points")).toBe("15,13 25,13");
  expect([...line.querySelectorAll("circle")].map(c => [Number(c.getAttribute("cx")), Number(c.getAttribute("cy"))])).toEqual([[15, 13], [25, 13]]);
  const box = container.querySelector('[aria-label="定位框（按位姿）"]')!;
  expect(box.querySelector('[aria-label="定位框"]')?.getAttribute("points")).toBe("44.5,22.5 60.5,22.5 60.5,38.5 44.5,38.5");
  expect(box.querySelector('[aria-label="模板外圈"]')?.getAttribute("points")).toBe("40.5,18.5 64.5,18.5 64.5,42.5 40.5,42.5");
  expect(box.querySelector('[aria-label="定位框示教位置"]')?.getAttribute("points")).toBe("39.5,19.5 55.5,19.5 55.5,35.5 39.5,35.5");
});

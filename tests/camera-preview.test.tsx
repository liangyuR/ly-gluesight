import { act, render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { defaultCameraConfig } from "../src/features/camera/api";
import FramePreview from "../src/features/camera/components/FramePreview";
import type { CameraConfig, CameraStatus, Frame } from "../src/features/camera/types";
import { FRAME_HEADER_BYTES } from "../src/lib/frame";
import CameraPage from "../src/pages/CameraPage";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const left = { ...defaultCameraConfig, id: "cam1", name: "左相机", serial: "SN1" };
const right = { ...defaultCameraConfig, id: "cam2", name: "右相机", serial: "SN2", triggerSource: "Software" } as const;
const replacement = { ...right, id: "cam3", name: "新相机", serial: "SN3" };
const status = (cam: number, config: CameraConfig): CameraStatus => ({
  cam, id: config.id, name: config.name, source: config.source, ready: true, released: false, message: "",
  device: null, sdkVersion: null, frames: 0, fps: 0, configuredFrameRate: 10, maxFps: null,
  lostPackets: 0, droppedFrames: 0, warnings: [], imaging: { exposureUs: 60, gainDb: 6 },
});
const frame = (cameraId: string, frameCounter: number, session = 1): Frame => ({
  cameraId, cam: 1, session, frameCounter, triggerCounter: frameCounter, counter: "sdkFrame", lostPackets: 0, ts: Date.now(),
});
function thumbnail(width: number) {
  const buffer = new ArrayBuffer(FRAME_HEADER_BYTES + width);
  const header = new DataView(buffer);
  [width, 1, width, 1, 1].forEach((value, index) => header.setUint32(index * 4, value, true));
  return buffer;
}

const native = vi.mocked(invoke);
const previewCalls = () => native.mock.calls.filter(([command]) => command === "camera_preview").map(([, args]) => args);
const tick = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const preview = (config: CameraConfig, lastFrame: Frame) =>
  <FramePreview cam={1} config={config} status={status(1, config)} lastFrame={lastFrame} />;
const image = () => screen.getByRole("img", { name: "相机最新图像" });
const imageDataDescriptor = Object.getOwnPropertyDescriptor(globalThis, "ImageData");

beforeEach(() => {
  vi.useFakeTimers();
  native.mockReset().mockResolvedValue(new ArrayBuffer(0));
  vi.mocked(listen).mockReset().mockResolvedValue(() => {});
  Object.defineProperty(globalThis, "ImageData", {
    configurable: true,
    value: class {
      data: Uint8ClampedArray;
      constructor(width: number, height: number) { this.data = new Uint8ClampedArray(width * height * 4); }
    },
  });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});

afterEach(() => {
  vi.useRealTimers();
  if (imageDataDescriptor) Object.defineProperty(globalThis, "ImageData", imageDataDescriptor);
  else Reflect.deleteProperty(globalThis, "ImageData");
});

it("同相机缩略图未返回时到达末帧 → 保留首图并限速补读一次尾图", async () => {
  const initial = deferred<ArrayBuffer>(), tail = deferred<ArrayBuffer>();
  native.mockReturnValueOnce(initial.promise).mockReturnValueOnce(tail.promise);
  const view = render(preview(right, frame("cam2", 1)));
  await tick(0);
  expect(previewCalls()).toEqual([{ cam: 1, id: "cam2", view: 1 }]);

  view.rerender(preview(right, frame("cam2", 2)));
  view.rerender(preview(right, frame("cam2", 3)));
  await act(async () => initial.resolve(thumbnail(2)));
  expect(image()).toHaveAttribute("width", "2");
  await tick(249);
  expect(previewCalls()).toHaveLength(1);

  await tick(1);
  expect(previewCalls()).toEqual([{ cam: 1, id: "cam2", view: 1 }, { cam: 1, id: "cam2", view: 1 }]);
  await act(async () => tail.resolve(thumbnail(3)));
  expect(image()).toHaveAttribute("width", "3");
  await tick(250);
  expect(previewCalls()).toHaveLength(2);
});

it("换到占同一索引的另一相机 → 迟到的旧缩略图不会覆盖新相机", async () => {
  const old = deferred<ArrayBuffer>();
  native.mockReturnValueOnce(old.promise).mockResolvedValueOnce(thumbnail(4));
  const view = render(preview(right, frame("cam2", 1)));
  await tick(0);
  view.rerender(preview(replacement, frame("cam3", 1, 2)));
  await tick(0);
  expect(previewCalls()).toEqual([{ cam: 1, id: "cam2", view: 1 }, { cam: 1, id: "cam3", view: 1 }]);
  expect(image()).toHaveAttribute("width", "4");

  await act(async () => old.resolve(thumbnail(2)));
  expect(image()).toHaveAttribute("width", "4");
});

it("相机组索引沿用时收到旧相机帧事件 → 新相机卡片等待自己的帧", async () => {
  let receiveFrame: ((payload: Frame) => void) | undefined;
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "camera://frame") receiveFrame = payload => handler({ event, id: 1, payload } as TauriEvent<never>);
    return () => {};
  });
  let available = new ArrayBuffer(0);
  native.mockImplementation(async (command, args) => {
    switch (command) {
      case "camera_rig_config": return [left, replacement];
      case "camera_rig_status": return [status(0, left), status(1, replacement)];
      case "camera_preview": return (args as { id?: string }).id === "cam3" ? available : new ArrayBuffer(0);
      case "cycle_control_state": return { productionBusy: false, plcLocked: false, plcHandshake: null, maintenance: null };
      case "recipe_list": return { recipes: [], errors: [], unavailable: [] };
      default: return [];
    }
  });
  render(<MemoryRouter><CameraPage /></MemoryRouter>);
  await tick(500);
  await tick(1);
  const camera = within(screen.getByRole("article", { name: "新相机 cam3" }));
  const readsBefore = previewCalls().length;
  expect(previewCalls()).toContainEqual({ cam: 1, id: "cam3", view: 1 });
  expect(camera.getByText("等待第一帧")).toBeVisible();
  expect(receiveFrame).toBeDefined();

  available = thumbnail(2);
  act(() => receiveFrame!(frame("cam2", 7)));
  await tick(750);
  await tick(1);
  expect(camera.getByText("等待第一帧")).toBeVisible();
  expect(previewCalls()).toHaveLength(readsBefore);

  act(() => receiveFrame!(frame("cam3", 1, 2)));
  await tick(750);
  await tick(1);
  const shown = camera.getByRole("img", { name: "新相机 最新图像" });
  expect(shown).toBeVisible();
  expect(shown).toHaveAttribute("width", "2");
  expect(previewCalls().at(-1)).toEqual({ cam: 1, id: "cam3", view: 1 });
});

import { act, render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { Profiler } from "react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { defaultCameraConfig, useRigStatus } from "../src/features/camera/api";
import FramePreview from "../src/features/camera/components/FramePreview";
import { useCameraFrame, useCameraFrames } from "../src/features/camera/frames";
import type { CameraConfig, CameraStatus, Frame } from "../src/features/camera/types";
import { FRAME_HEADER_BYTES } from "../src/lib/frame";
import CameraPage from "../src/pages/CameraPage";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const left = { ...defaultCameraConfig, id: "cam1", name: "左相机", serial: "SN1" };
const right = { ...defaultCameraConfig, id: "cam2", name: "右相机", serial: "SN2", triggerSource: "Software" } as const;
const replacement = { ...right, id: "cam3", name: "新相机", serial: "SN3" };
const rig = [left, right];
const replacedRig = [left, replacement];
const status = (cam: number, config: CameraConfig): CameraStatus => ({
  cam, id: config.id, name: config.name, source: config.source, ready: true, released: false, message: "",
  device: null, sdkVersion: null, frames: 0, fps: 0, configuredFrameRate: 10, maxFps: null,
  lostPackets: 0, droppedFrames: 0, warnings: [], imaging: { exposureUs: 60, gainDb: 6 },
});
const frame = (cameraId: string, frameCounter: number, session = 1, cam = 1): Frame => ({
  cameraId, cam, session, frameCounter, triggerCounter: frameCounter, counter: "sdkFrame", lostPackets: 0, ts: Date.now(),
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
function Preview({ configs }: { configs: CameraConfig[] }) {
  const frames = useCameraFrames(configs);
  return <FramePreview cam={1} config={configs[1]} status={status(1, configs[1])} frames={frames} />;
}
const image = () => screen.getByRole("img", { name: "相机最新图像" });
const imageDataDescriptor = Object.getOwnPropertyDescriptor(globalThis, "ImageData");
let receiveFrame: ((payload: Frame) => void) | undefined;
const emitFrame = (payload: Frame) => {
  expect(receiveFrame).toBeDefined();
  act(() => receiveFrame!(payload));
};

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date", "performance"] });
  native.mockReset().mockResolvedValue(new ArrayBuffer(0));
  receiveFrame = undefined;
  vi.mocked(listen).mockReset().mockImplementation(async (event, handler) => {
    if (event === "camera://frame") receiveFrame = payload => handler({ event, id: 1, payload } as TauriEvent<never>);
    return () => {};
  });
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
  render(<Preview configs={rig} />);
  emitFrame(frame("cam2", 1));
  await tick(0);
  expect(previewCalls()).toEqual([{ cam: 1, id: "cam2", view: 1 }]);

  emitFrame(frame("cam2", 2));
  emitFrame(frame("cam2", 3));
  await tick(250);
  expect(previewCalls()).toHaveLength(1);

  await act(async () => initial.resolve(thumbnail(2)));
  expect(image()).toHaveAttribute("width", "2");
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
  const view = render(<Preview configs={rig} />);
  emitFrame(frame("cam2", 1));
  await tick(0);
  view.rerender(<Preview configs={replacedRig} />);
  emitFrame(frame("cam3", 1, 2));
  await tick(0);
  expect(previewCalls()).toEqual([{ cam: 1, id: "cam2", view: 1 }, { cam: 1, id: "cam3", view: 1 }]);
  expect(image()).toHaveAttribute("width", "4");

  await act(async () => old.resolve(thumbnail(2)));
  expect(image()).toHaveAttribute("width", "4");
});

it("相机组索引沿用时收到旧相机帧事件 → 新相机卡片等待自己的帧", async () => {
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

  available = thumbnail(2);
  emitFrame(frame("cam2", 7));
  await tick(750);
  await tick(1);
  expect(camera.getByText("等待第一帧")).toBeVisible();
  expect(previewCalls()).toHaveLength(readsBefore);

  emitFrame(frame("cam3", 1, 2));
  await tick(750);
  await tick(1);
  const shown = camera.getByRole("img", { name: "新相机 最新图像" });
  expect(shown).toBeVisible();
  expect(shown).toHaveAttribute("width", "2");
  expect(previewCalls().at(-1)).toEqual({ cam: 1, id: "cam3", view: 1 });
});

function FrameCounter({ frames, cam, id }: { frames: ReturnType<typeof useCameraFrames>; cam: number; id: string }) {
  const value = useCameraFrame(frames, cam, id);
  return <output aria-label={id}>{value?.frameCounter ?? "等待帧"}</output>;
}

function StatusCount() {
  const { statuses } = useRigStatus();
  return <output aria-label="相机状态数">{statuses.length}</output>;
}

it("相机首帧即时显示 → 其他相机和状态区不重绘，各相机单独合并尾帧", async () => {
  const leftRender = vi.fn(), rightRender = vi.fn(), statusRender = vi.fn();
  native.mockResolvedValue([status(0, left), status(1, right)]);
  function Probe() {
    const frames = useCameraFrames(rig);
    return <>
      <Profiler id="left" onRender={leftRender}><FrameCounter frames={frames} cam={0} id="cam1" /></Profiler>
      <Profiler id="right" onRender={rightRender}><FrameCounter frames={frames} cam={1} id="cam2" /></Profiler>
      <Profiler id="status" onRender={statusRender}><StatusCount /></Profiler>
    </>;
  }
  render(<Probe />);
  await tick(0);
  expect(screen.getByLabelText("相机状态数")).toHaveTextContent("2");
  leftRender.mockClear(); rightRender.mockClear(); statusRender.mockClear();
  const leftCount = screen.getByLabelText("cam1"), rightCount = screen.getByLabelText("cam2");

  emitFrame(frame("cam2", 5));
  expect(rightCount).toHaveTextContent("5");
  expect(leftCount).toHaveTextContent("等待帧");
  expect(rightRender).toHaveBeenCalledOnce();
  expect(leftRender).not.toHaveBeenCalled();
  expect(statusRender).not.toHaveBeenCalled();

  emitFrame(frame("cam2", 6));
  emitFrame(frame("cam2", 7));
  emitFrame(frame("cam1", 2, 2, 0));
  expect(leftCount).toHaveTextContent("2");
  expect(rightCount).toHaveTextContent("5");
  await tick(249);
  expect(rightCount).toHaveTextContent("5");
  await tick(1);
  expect(rightCount).toHaveTextContent("7");
  expect(rightRender).toHaveBeenCalledTimes(2);
  expect(leftRender).toHaveBeenCalledOnce();
  expect(statusRender).not.toHaveBeenCalled();
});

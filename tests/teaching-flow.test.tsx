import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import OverviewPage from "../src/features/workspace/OverviewPage";
import ValidationPage from "../src/features/workspace/ValidationPage";
import CapturePage from "../src/features/workspace/CapturePage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi, useCycle } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera";
import { cameraApi as captureCameraApi } from "../src/features/camera/api";
import { plcApi } from "../src/features/plc/api";
import { historyApi } from "../src/features/history/api";
import type { CaptureRound } from "../src/features/workspace/types";
import { snapshot, summary, tricamWorkspaceView, twoLines } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  get: vi.fn(), list: vi.fn(), image: vi.fn(), progress: vi.fn(),
  saveDraft: vi.fn(), saveParams: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(),
  saveOverview: vi.fn(), captureGet: vi.fn(), captureList: vi.fn(), captureStart: vi.fn(),
  captureSample: vi.fn(), captureImage: vi.fn(), validate: vi.fn(), publish: vi.fn(),
} }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn() }, useCycle: vi.fn() }));
vi.mock("../src/features/camera", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigStatus: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getStatus: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));
vi.mock("../src/features/plc", () => ({ subscribe: () => () => {} }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: () => true }));

it("从采集原图完成逐帧示教、总览、独立样本采集验证并确认发布", async () => {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });

  const candidate = tricamWorkspaceView();
  const taughtLayout = structuredClone(candidate.layout);
  const trials = candidate.workspace.frames.map(frame => structuredClone(frame.trial!));
  candidate.workspace.captureId = "captured-round";
  candidate.workspace.validation = null;
  candidate.workspace.overview.saved = false;
  candidate.workspace.sampleBank = [];
  candidate.workspace.samples = [];
  candidate.coverage = 0;
  candidate.workspace.doc.shots.forEach(shot => { shot.path = []; delete shot.mmPerPx; });
  candidate.workspace.frames.forEach(frame => { frame.trial = null; frame.saved = false; });
  candidate.layout.shots = structuredClone(candidate.workspace.doc.shots);
  candidate.layout.segments = [];
  candidate.layout.points = { x: [], y: [], seg: [], k: [] };

  vi.mocked(workspaceApi.get).mockResolvedValue(candidate);
  vi.mocked(workspaceApi.list).mockResolvedValue([candidate.workspace]);
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.progress).mockImplementation(async (_id, k, view) => ({
    ...candidate, workspace: { ...candidate.workspace, lastPosition: { k, view } },
  }));
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [summary(candidate)], errors: [] });
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([{
    id: "CAM-1", name: "拼接设备", source: "sim", viewCount: 3, serial: "DEVICE", acquisition: "triggered",
    fps: 20, triggerSource: "Line0", triggerActivation: "RisingEdge", triggerDelayUs: 0, debouncerUs: 0,
    exposureUs: 60, gainDb: 0, strobe: false, chunk: true, replayDir: "", replayChannel: 0,
  }]);
  vi.mocked(captureCameraApi.rigStatus).mockResolvedValue([{ id: "CAM-1", ready: true }] as Awaited<ReturnType<typeof captureCameraApi.rigStatus>>);
  vi.mocked(plcApi.getStatus).mockResolvedValue({ state: "connected", message: "PLC 已连接" } as Awaited<ReturnType<typeof plcApi.getStatus>>);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });

  let response = structuredClone(candidate);
  for (const k of [0, 1]) {
    response.workspace.revision++;
    response.workspace.doc.shots[k].path = structuredClone(twoLines[k]);
    response.workspace.doc.shots[k].mmPerPx = .1;
    response.layout.shots = structuredClone(response.workspace.doc.shots);
    vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(structuredClone(response));
    response.workspace.frames[k].trial = trials[k];
    vi.mocked(workspaceApi.trial).mockResolvedValueOnce(structuredClone(response));
    response.workspace.frames[k].saved = true;
    response.workspace.revision++;
    response.coverage = (k + 1) * 50;
    if (k === 1) response.layout = taughtLayout;
    vi.mocked(workspaceApi.saveTeach).mockResolvedValueOnce(structuredClone(response));
  }
  vi.mocked(workspaceApi.saveOverview).mockImplementation(async (id, revision, overview) => {
    expect([id, revision]).toEqual(["A", 11]);
    response.workspace.overview = { ...structuredClone(overview), saved: true };
    response.workspace.revision++;
    return structuredClone(response);
  });
  const independentRound: CaptureRound = {
    roundId: "validation-round", recipeId: "A", cameraId: "CAM-1", deviceSession: 2,
    plannedCount: 2, receivedCount: 2, state: "complete", createdAt: 2, endedAt: 3,
    error: null, simulated: true,
    frames: [0, 1].map(k => ({ shotId: `P${k + 1}`, ordinal: k + 1, frameCounter: k + 1,
      triggerCounter: k + 1, originalPath: `validation/k${k}.pgm`,
      views: [1, 2, 3].map(view => ({ view, path: `validation/k${k}-v${view}.pgm`, width: 100, height: 60 })),
    })),
  };
  let receiving = false;
  vi.mocked(workspaceApi.captureGet).mockImplementation(async () => receiving ? independentRound : null);
  vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
  vi.mocked(workspaceApi.captureStart).mockImplementation(async () => {
    receiving = true;
    return { ...independentRound, state: "waitingStart", receivedCount: 0, endedAt: null, frames: [] };
  });
  vi.mocked(workspaceApi.captureImage).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.captureSample).mockImplementation(async (id, revision, roundId, expected, confirmed) => {
    expect([id, revision, roundId, expected, confirmed]).toEqual(["A", 12, "validation-round", "OK", true]);
    expect(roundId).not.toBe(response.workspace.captureId);
    response.workspace.sampleBank = [{ id: roundId, name: "独立采集 2", geometryTag: response.workspace.doc.shots.map(shot => [shot.id, shot.poseId, shot.camera]), expected, createdAt: 2 }];
    response.workspace.samples = [{ historyId: null, sampleId: roundId, expected }];
    response.workspace.revision++;
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.validate).mockImplementation(async (id, revision, samples, confirmed) => {
    expect([id, revision, samples, confirmed]).toEqual(["A", 13, [{ historyId: null, sampleId: "validation-round", expected: "OK" }], true]);
    response.workspace.validation = { revision, passed: true, checkedAt: 4, environmentTag: {},
      checks: [{ name: "独立正常样本", passed: true, detail: "独立整圈与当前示教一致" }],
      samples: [{ historyId: null, sampleId: "validation-round", name: "独立采集 2", sn: 0, expected: "OK", actual: "OK", passed: true, reason: "" }],
    };
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.publish).mockImplementation(async (id, revision) => {
    expect([id, revision]).toEqual(["A", 13]);
    response.workspace.pending = { doc: structuredClone(response.workspace.doc), bundleId: "bundle-A-v2", baseRevision: response.workspace.baseRevision,
      revision, frames: structuredClone(response.workspace.frames), overview: structuredClone(response.workspace.overview), validation: structuredClone(response.workspace.validation!),
    };
    return structuredClone(response);
  });

  render(<MemoryRouter initialEntries={["/recipe/teach"]}><WorkspaceProvider><Routes>
    <Route path="/recipe/teach" element={<TeachingPage />} />
    <Route path="/recipe/overview" element={<OverviewPage />} />
    <Route path="/recipe/validation" element={<ValidationPage />} />
    <Route path="/recipe/capture" element={<CapturePage />} />
  </Routes></WorkspaceProvider></MemoryRouter>);
  const user = userEvent.setup();
  expect(await screen.findByText(/全部 0／2 幅/)).toBeVisible();

  for (const k of [0, 1]) {
    const imageId = `A-image-${k}`;
    const image = await screen.findByRole("img", { name: `冻结图像 · ${imageId}` });
    expect(screen.getByRole("button", { name: "选择帧 k" + (k + 1) })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    for (const [clientX, clientY] of twoLines[k]) {
      fireEvent.pointerDown(image, { clientX, clientY });
      fireEvent.pointerUp(image);
    }
    fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".1" } });
    expect(screen.getByLabelText("中线点 2")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "保存中线" }));
    expect(workspaceApi.saveParams).toHaveBeenNthCalledWith(k + 1, "A", 7 + 2 * k, k, { path: twoLines[k], mmPerPx: .1 });
    await waitFor(() => expect(screen.getByRole("button", { name: "试测当前帧" })).toBeEnabled());
    expect(screen.queryByText("示教中线有未保存的修改")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "试测当前帧" }));
    expect(workspaceApi.trial).toHaveBeenNthCalledWith(k + 1, "A", 8 + 2 * k, k, imageId);
    await waitFor(() => expect(screen.getByRole("button", { name: "保存并示教下一幅" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "保存并示教下一幅" }));
    expect(workspaceApi.saveTeach).toHaveBeenNthCalledWith(k + 1, "A", 8 + 2 * k, k, imageId);
    await screen.findByText(new RegExp(`全部 ${k + 1}／2 幅`));
  }

  expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
  await user.click(screen.getByRole("link", { name: "布置总览" }));
  const firstPosition = await screen.findByRole("button", { name: "总览选择帧 k1" });
  fireEvent.keyDown(firstPosition, { key: "ArrowRight" });
  await user.click(screen.getByRole("button", { name: "保存总览" }));
  await screen.findByText("总览布置已保存，拍照与测量不受影响");
  expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A", 11, { background: null, positions: [[.26, .5], [.75, .5]], saved: false });
  await user.click(screen.getByRole("link", { name: "验证与发布" }));
  expect(await screen.findByRole("button", { name: "发布生产配方" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "运行整套图像验证" })).toBeDisabled();
  await user.click(screen.getByRole("link", { name: "重新实拍样本" }));
  const startCapture = await screen.findByRole("button", { name: "开始接收 / 整圈重采" });
  await waitFor(() => expect(startCapture).toBeEnabled());
  await user.click(startCapture);
  expect(workspaceApi.captureStart).toHaveBeenCalledWith("A", "CAM-1", 1500);
  const saveSample = await screen.findByRole("button", { name: "保存为独立正常验证样本" });
  expect(saveSample).toBeDisabled();
  await user.click(screen.getByRole("checkbox", { name: "已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致" }));
  await user.click(saveSample);
  await screen.findByText("正常验证样本已加入样本库");
  await user.click(screen.getByRole("link", { name: "进入验证" }));
  expect(await screen.findByRole("checkbox", { name: "选用样本 独立采集 2" })).toBeChecked();
  await user.click(screen.getByRole("checkbox", { name: "已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选" }));
  await user.click(screen.getByRole("button", { name: "运行整套图像验证" }));
  await screen.findByText("一致");
  const publish = screen.getByRole("button", { name: "发布生产配方" });
  await waitFor(() => expect(publish).toBeEnabled());
  await user.click(publish);
  expect(await screen.findByRole("dialog", { name: "发布生产配方" })).toBeVisible();
  expect(workspaceApi.publish).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "确认发布 v2" }));
  await screen.findByText("等待工件边界");
  expect(workspaceApi.publish).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
  expect(workspaceApi.saveParams).toHaveBeenCalledTimes(2);
  expect(workspaceApi.trial).toHaveBeenCalledTimes(2);
  expect(workspaceApi.saveTeach).toHaveBeenCalledTimes(2);
});

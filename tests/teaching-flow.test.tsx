import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import OverviewPage from "../src/features/workspace/OverviewPage";
import ValidationPage from "../src/features/workspace/ValidationPage";
import CapturePage from "../src/features/workspace/CapturePage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import { SnackbarProvider } from "qing-steel";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi, useCycle } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera";
import { cameraApi as captureCameraApi } from "../src/features/camera/api";
import { plcApi } from "../src/features/plc/api";
import { historyApi } from "../src/features/history/api";
import type { CaptureRound, ShotState, WorkspaceView } from "../src/features/workspace/types";
import { deferred, snapshot, summary, tricamWorkspaceView, twoLines } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  get: vi.fn(), list: vi.fn(), image: vi.fn(), progress: vi.fn(),
  selectGroupImage: vi.fn(), saveDraft: vi.fn(), saveParams: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(), extractCenterline: vi.fn(), checkCalibration: vi.fn(),
  saveOverview: vi.fn(), captureGet: vi.fn(), captureList: vi.fn(), captureStart: vi.fn(),
  captureSample: vi.fn(), captureImage: vi.fn(), validate: vi.fn(), publish: vi.fn(),
} }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn() }, useCycle: vi.fn(), useSiteStatus: () => ({ mode: "real" }) }));
vi.mock("../src/features/camera", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigStatus: vi.fn(), calibInfo: vi.fn(async () => null) } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getStatus: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));
vi.mock("../src/features/plc", () => ({ subscribe: () => () => {} }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: () => true }));

function shotState(view: WorkspaceView, k: number, state: ShotState) {
  const shot = view.progress.shots[k];
  shot.state = state; shot.completed = state === "taught" ? 1 : 0;
  shot.views[0].state = state;
}

function candidateView() {
  const candidate = tricamWorkspaceView();
  candidate.workspace.captureId = "captured-round";
  candidate.workspace.validation = null;
  candidate.workspace.overview.saved = false;
  candidate.workspace.sampleBank = [];
  candidate.workspace.samples = [];
  return candidate;
}

function groupedCandidate() {
  const candidate = candidateView();
  const shots = candidate.workspace.doc.shots;
  const frames = candidate.workspace.frames;
  const progress = candidate.progress.shots;
  candidate.workspace.doc.shots = shots.flatMap(shot => [1, 2, 3].map(camera => ({
    ...structuredClone(shot), id: `${shot.poseId}-cam${camera}`, camera: `CAM-${camera}`, skip: camera !== 1,
  })));
  candidate.workspace.frames = frames.flatMap((frame, group) => [1, 2, 3].map(camera => {
    const copy = structuredClone(frame);
    copy.k = group * 3 + camera - 1;
    copy.image = { ...copy.image!, id: `${frame.image!.id}-cam${camera}`, camera: `CAM-${camera}`,
      cameraTag: { id: `CAM-${camera}`, source: "sim", acquisition: "triggered", viewCount: 1 },
      geometryTag: [`P${group + 1}-cam${camera}`, `P${group + 1}`, `CAM-${camera}`, 1, `CAM-${camera}-v1`],
    };
    copy.views = [copy.image];
    if (copy.trial) {
      copy.trial.imageId = copy.image.id;
      copy.trial.geometryTag = copy.image.geometryTag;
    }
    return copy;
  }));
  candidate.progress.shots = progress.flatMap((shot, group) => [1, 2, 3].map(camera => ({
    ...structuredClone(shot), k: group * 3 + camera - 1, shotId: `P${group + 1}-cam${camera}`,
    state: camera === 1 ? shot.state : "skipped", total: camera === 1 ? 1 : 0,
    views: [{ ...structuredClone(shot.views[0]), enabled: camera === 1 }],
  })));
  candidate.layout.shots = structuredClone(candidate.workspace.doc.shots);
  candidate.layout.segments.forEach(segment => { segment.shot *= 3; });
  candidate.layout.points.k = candidate.layout.points.k.map(k => k * 3);
  return candidate;
}

function mockSite(candidate: WorkspaceView) {
  vi.mocked(workspaceApi.get).mockResolvedValue(candidate);
  vi.mocked(workspaceApi.list).mockResolvedValue([candidate.workspace]);
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.progress).mockImplementation(async (_id, k, view) => ({
    ...candidate, workspace: { ...candidate.workspace, lastPosition: { k, view } },
  }));
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [summary(candidate)], errors: [], unavailable: [] });
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([{
    id: "CAM-1", name: "拼接设备", source: "sim", viewCount: 3, serial: "DEVICE",
    triggerSource: "Line0", triggerActivation: "RisingEdge", triggerDelayUs: 0, debouncerUs: 0,
    strobe: false, chunk: true,
  }]);
  vi.mocked(captureCameraApi.rigStatus).mockResolvedValue([{ id: "CAM-1", ready: true }] as Awaited<ReturnType<typeof captureCameraApi.rigStatus>>);
  vi.mocked(plcApi.getStatus).mockResolvedValue({ state: "connected", message: "PLC 已连接" } as Awaited<ReturnType<typeof plcApi.getStatus>>);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
}

function renderWorkspace(path: string, strict = false) {
  const workspace = <MemoryRouter initialEntries={[path]}><SnackbarProvider><WorkspaceProvider><Routes>
    <Route path="/recipe/teach" element={<TeachingPage />} />
    <Route path="/recipe/overview" element={<OverviewPage />} />
    <Route path="/recipe/validation" element={<ValidationPage />} />
    <Route path="/recipe/capture" element={<CapturePage />} />
  </Routes></WorkspaceProvider></SnackbarProvider></MemoryRouter>;
  render(strict ? <StrictMode>{workspace}</StrictMode> : workspace);
  return userEvent.setup();
}

it("本图参数覆盖后恢复默认 → 搜索带与试测载荷使用配方默认，当量无效时隐藏", async () => {
  const candidate = groupedCandidate();
  candidate.workspace.doc.detect.searchMm = 15;
  candidate.workspace.doc.shots[0].detect = { searchMm: 4, polarity: "dark", widthRange: [1, 5] };
  candidate.workspace.doc.shots[0].limits = { ...structuredClone(candidate.workspace.doc.limits), position: null, maxGapLen: 3 };
  candidate.workspace.doc.shots[0].mmPerPx = .1;
  mockSite(candidate);
  const response = structuredClone(candidate);
  delete response.workspace.doc.shots[0].detect;
  delete response.workspace.doc.shots[0].limits;
  response.workspace.revision++;
  vi.mocked(workspaceApi.saveParams).mockResolvedValue(response);
  vi.mocked(workspaceApi.trial).mockResolvedValue(response);
  const user = renderWorkspace("/recipe/teach");
  await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  const edges = () => [...screen.getByLabelText("搜索带边界").querySelectorAll("polyline")].map(edge => {
    const points = edge.getAttribute("points")!.split(" ");
    return `${points[0]} ${points[points.length - 1]}`;
  });
  expect(edges()).toEqual(["10,50 20,50", "10,-30 20,-30"]);
  fireEvent.click(screen.getByText("检测参数", { exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "恢复配方默认检测参数" }));
  expect(edges()).toEqual(["10,160 20,160", "10,-140 20,-140"]);
  await user.click(screen.getByText("判定限值", { exact: true }));
  expect(screen.getByRole("checkbox", { name: "位置判定" })).not.toBeChecked();
  await user.click(screen.getByRole("button", { name: "恢复配方默认判定限值" }));
  await user.click(screen.getByRole("button", { name: "运行试测" }));
  await waitFor(() => expect(workspaceApi.saveParams).toHaveBeenLastCalledWith("A", 7, 0, { path: candidate.workspace.doc.shots[0].path, mmPerPx: .1 }));
  await waitFor(() => expect(workspaceApi.trial).toHaveBeenLastCalledWith("A", 8, 0, candidate.workspace.frames[0].image!.id));
  for (const value of ["", "0"]) {
    fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value } });
    expect(screen.queryByLabelText("搜索带边界")).not.toBeInTheDocument();
  }
});

it("手工落点并自动保存 → 不插入瞬时警告、保留缩放与中线，缺少当量仍阻止试测", async () => {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });
  const candidate = groupedCandidate();
  candidate.workspace.doc.shots[0].path = [];
  delete candidate.workspace.doc.shots[0].mmPerPx;
  candidate.workspace.frames[0].trial = null;
  candidate.workspace.frames[0].saved = false;
  shotState(candidate, 0, "unconfigured");
  mockSite(candidate);
  const saving = deferred<void>();
  vi.mocked(workspaceApi.saveDraft).mockImplementation(async (_id, revision, k, teach) => {
    await saving.promise;
    const response = structuredClone(candidate);
    Object.assign(response.workspace.doc.shots[k], teach);
    response.workspace.revision = revision + 1;
    return response;
  });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "放大原图" }));
  const transform = image.querySelector("g")!.getAttribute("transform");
  for (const [clientX, clientY] of [[10, 10], [20, 20]]) {
    fireEvent.pointerDown(image, { clientX, clientY });
    fireEvent.pointerUp(image);
  }
  const transientWarnings = ["示教参数有未保存的修改", "本组中线无效"].filter(title => screen.queryByText(title));
  const editButton = screen.getByRole("button", { name: "编辑点" });
  const point = screen.getByLabelText("中线点 2");
  fireEvent.pointerDown(point, { clientX: 20, clientY: 20 });
  await screen.findByText("保存中", {}, { timeout: 2500 });
  expect(screen.getByRole("button", { name: "编辑点" })).toBe(editButton);
  expect(editButton).toBeDisabled();
  expect(screen.getByLabelText("中线点 2")).toBe(point);
  fireEvent.pointerMove(image, { clientX: 30, clientY: 30 });
  expect(point).toHaveAttribute("cx", "20");
  fireEvent.pointerUp(image);
  fireEvent.pointerDown(image, { clientX: 30, clientY: 30 });
  fireEvent.pointerUp(image);
  expect(screen.queryByLabelText("中线点 3")).not.toBeInTheDocument();
  await act(async () => saving.resolve(undefined));
  await screen.findByText("草稿已保存", {}, { timeout: 2500 });
  expect(editButton).toBeEnabled();
  expect(screen.getByRole("img", { name: "P1 · CAM-1 · 图 1" })).toBe(image);
  expect(image.querySelector("g")).toHaveAttribute("transform", transform);
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cx", "20");
  expect(screen.getByRole("button", { name: "运行试测" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "运行试测" })).toHaveAttribute("title", expect.stringContaining("像素当量"));
  expect(transientWarnings).toEqual([]);
});

it("每组三相机图仅示教选中图 → 保存后进入下一组并沿用最新修订", async () => {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });

  const candidate = groupedCandidate();
  const taughtLayout = structuredClone(candidate.layout);
  const trials = candidate.workspace.frames.map(frame => structuredClone(frame.trial!));
  candidate.coverage = 0;
  candidate.workspace.doc.shots.forEach(shot => { shot.path = []; delete shot.mmPerPx; });
  candidate.workspace.frames.forEach(frame => { frame.trial = null; frame.saved = false; });
  [0, 3].forEach(k => shotState(candidate, k, "unconfigured"));
  candidate.layout.shots = structuredClone(candidate.workspace.doc.shots);
  candidate.layout.segments = [];
  candidate.layout.points = { x: [], y: [], seg: [], k: [] };
  mockSite(candidate);

  const response = structuredClone(candidate);
  for (const group of [0, 1]) {
    const k = group * 3;
    response.workspace.revision++;
    response.workspace.doc.shots[k].path = structuredClone(twoLines[group]);
    response.workspace.doc.shots[k].mmPerPx = .1;
    response.layout.shots = structuredClone(response.workspace.doc.shots);
    shotState(response, k, "pendingTrial");
    vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(structuredClone(response));
    response.workspace.frames[k].trial = trials[k];
    shotState(response, k, "trialPassed");
    vi.mocked(workspaceApi.trial).mockResolvedValueOnce(structuredClone(response));
    response.workspace.frames[k].saved = true;
    shotState(response, k, "taught");
    response.workspace.revision++;
    response.coverage = (group + 1) * 50;
    if (group === 1) response.layout = taughtLayout;
    vi.mocked(workspaceApi.saveTeach).mockResolvedValueOnce(structuredClone(response));
  }

  const user = renderWorkspace("/recipe/teach");
  expect(await screen.findByText(/已完成 0／2 组/)).toBeVisible();

  for (const group of [0, 1]) {
    const k = group * 3;
    const imageId = `A-image-${group}-cam1`;
    const image = await screen.findByRole("img", { name: `P${group + 1} · CAM-1 · 图 1` });
    expect(screen.getByRole("button", { name: "选择拍照组 P" + (group + 1) })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "运行试测" })).toBeDisabled();
    for (const [clientX, clientY] of twoLines[group]) {
      fireEvent.pointerDown(image, { clientX, clientY });
      fireEvent.pointerUp(image);
    }
    fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".1" } });
    expect(screen.getByLabelText("中线点 2")).toBeVisible();
    const actions = within(screen.getByRole("group", { name: "单帧示教操作" }));
    expect(actions.getByRole("button", { name: "保存本组示教" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "运行试测" }));
    expect(workspaceApi.saveParams).toHaveBeenNthCalledWith(group + 1, "A", 7 + 2 * group, k, { path: twoLines[group], mmPerPx: .1 });
    await waitFor(() => expect(workspaceApi.trial).toHaveBeenNthCalledWith(group + 1, "A", 8 + 2 * group, k, imageId));
    expect(screen.queryByText("示教参数有未保存的修改")).not.toBeInTheDocument();
    const saveLabel = group === 0 ? "保存并进入下一组" : "保存本组示教";
    await waitFor(() => expect(actions.getByRole("button", { name: saveLabel })).toBeEnabled());
    await user.click(actions.getByRole("button", { name: saveLabel }));
    expect(workspaceApi.saveTeach).toHaveBeenNthCalledWith(group + 1, "A", 8 + 2 * group, k, imageId);
    await screen.findByText(new RegExp(`已完成 ${group + 1}／2 组`));
  }

  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeDisabled();
  expect(workspaceApi.saveParams).toHaveBeenCalledTimes(2);
  expect(workspaceApi.trial).toHaveBeenCalledTimes(2);
  expect(workspaceApi.saveTeach).toHaveBeenCalledTimes(2);
});

it("刷选提取后运行一次算法 → 传递原图引导与点距，重采样后试测失效", async () => {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });
  const candidate = groupedCandidate();
  candidate.workspace.doc.shots[0].path = [];
  delete candidate.workspace.doc.shots[0].mmPerPx;
  candidate.workspace.frames[0].trial = null;
  candidate.workspace.frames[0].saved = false;
  shotState(candidate, 0, "unconfigured");
  mockSite(candidate);
  const extracted: [number, number][] = [[12, 20], [32, 20], [52, 20], [52, 40], [52, 60]];
  const response = structuredClone(candidate);
  vi.mocked(workspaceApi.saveDraft).mockImplementation(async (_id, _revision, k, params) => {
    Object.assign(response.workspace.doc.shots[k], params);
    response.workspace.revision++;
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.extractCenterline).mockResolvedValue({ path: extracted, message: "中线已提取" });
  vi.mocked(workspaceApi.saveParams).mockImplementation(async (_id, _revision, k, params) => {
    Object.assign(response.workspace.doc.shots[k], params);
    response.workspace.revision++;
    shotState(response, k, "pendingTrial");
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.trial).mockImplementation(async () => {
    response.workspace.frames[0].trial = {
      ...groupedCandidate().workspace.frames[0].trial!,
      passed: true, coverage: .5, elapsedMs: 8, verdict: "NG_GAP",
      measurement: { unit: "mm", ids: [0, 1], status: ["ok", "no_bead"], innerCenter: [.125, null],
        width: [2.5, null], px: [[12, 21], [22, 20]], edges: [[[12, 15], [12, 27]], null], candidateWidth: [null, 3.2], candidateEdges: [null, [[22, 12], [22, 28]]], coverage: .5, ms: 8 },
    };
    response.workspace.revision++;
    shotState(response, 0, "trialPassed");
    return structuredClone(response);
  });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  expect(screen.getByRole("button", { name: "结束选区 / 手工修正" })).toHaveAttribute("aria-pressed", "true");
  expect(workspaceApi.extractCenterline).not.toHaveBeenCalled();
  fireEvent.pointerDown(image, { clientX: 10.2, clientY: 12.4 });
  fireEvent.pointerMove(image, { clientX: 50.8, clientY: 42.7 });
  fireEvent.pointerUp(image);
  fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".2" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "中线点间距" }), { target: { value: "10" } });
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  expect(await screen.findByLabelText("中线点 5")).toBeVisible();
  expect(screen.queryByLabelText("刷选胶路区域")).not.toBeInTheDocument();
  expect(workspaceApi.saveDraft).toHaveBeenCalledWith("A", 7, 0, { path: [], mmPerPx: .2 });
  expect(workspaceApi.extractCenterline).toHaveBeenCalledWith("A", 8, 0, "A-image-0-cam1", [0, 0, 100, 60], { points: [[10.2, 12.4], [50.8, 42.7]], radiusPx: 60 }, 10);
  await user.click(screen.getByRole("button", { name: "运行试测" }));
  const table = await screen.findByRole("table", { name: "逐点试测结果" });
  expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 8, 0, { path: extracted, mmPerPx: .2 });
  expect(workspaceApi.trial).toHaveBeenCalledWith("A", 9, 0, "A-image-0-cam1");
  expect(within(table).getByText("0.125")).toBeVisible();
  expect(within(table).getByText("2.500")).toBeVisible();
  expect(within(table).getByText("缺胶")).toBeVisible();
  expect(image.querySelector('line[x1="12"][y1="15"][x2="12"][y2="27"]')).not.toBeNull();
  expect(image.querySelector('line[x1="22"][y1="12"][x2="22"][y2="28"][stroke="var(--ng)"]')).not.toBeNull();
  expect(within(table).getByText("3.200（候选，未通过）")).toHaveClass("c-ng");
  expect(screen.getByText("逐点试测结果 · 测得 1 / 2 · 未测 1")).toBeVisible();
  fireEvent.change(screen.getByRole("spinbutton", { name: "中线点间距" }), { target: { value: "80" } });
  await user.click(screen.getByRole("button", { name: "应用到当前中线" }));
  expect(screen.getByLabelText("中线点 1")).toHaveAttribute("cx", "12");
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cx", "52");
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cy", "20");
  expect(screen.getByLabelText("中线点 3")).toHaveAttribute("cx", "52");
  expect(screen.getByLabelText("中线点 3")).toHaveAttribute("cy", "60");
  expect(screen.queryByLabelText("中线点 4")).not.toBeInTheDocument();
  expect(screen.queryByRole("table", { name: "逐点试测结果" })).not.toBeInTheDocument();
  expect(image.querySelector('line[stroke="var(--ok)"]')).toBeNull();
  expect(image.querySelector('line[stroke="var(--ng)"]')).toBeNull();
  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeDisabled();
});

it("自动提取失败后按笔划生成 → 定位原因并保留原线，人工草稿保留转弯且须重新试测", async () => {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });
  const candidate = groupedCandidate();
  mockSite(candidate);
  const failure = { point: [40, 20] as [number, number], reason: "discontinuous_edges", message: "此处胶路宽度突变，请检查亮边与阴影。" };
  vi.mocked(workspaceApi.extractCenterline).mockResolvedValue({ path: [], message: failure.message, failure });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  const originalPoints = candidate.workspace.doc.shots[0].path.map(point => point.join(",")).join(" ");
  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "刷选胶路区域" }));
  fireEvent.change(screen.getByRole("spinbutton", { name: "中线点间距" }), { target: { value: "80" } });
  fireEvent.pointerDown(image, { clientX: 10, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 50 });
  fireEvent.pointerUp(image);
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  expect(await screen.findByLabelText("中线提取失败位置")).toBeVisible();
  expect(screen.getByText(failure.message, { selector: '[role="status"]' })).toBeVisible();
  expect(image.querySelector(`polyline[points="${originalPoints}"]`)).not.toBeNull();
  expect(screen.getByLabelText("刷选胶路区域").querySelector("polyline")).toHaveAttribute("points", "10,20 40,20 40,50");
  fireEvent.change(screen.getByRole("slider", { name: "拖动调整笔刷宽度" }), { target: { value: "60" } });
  expect(screen.queryByLabelText("中线提取失败位置")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  await screen.findByLabelText("中线提取失败位置");
  await user.click(screen.getByRole("button", { name: "笔划直接生成中线" }));
  expect(screen.queryByLabelText("中线提取失败位置")).not.toBeInTheDocument();
  for (const [index, x, y] of [[1, 10, 20], [2, 40, 20], [3, 40, 50]]) {
    expect(screen.getByLabelText(`中线点 ${index}`)).toHaveAttribute("cx", String(x));
    expect(screen.getByLabelText(`中线点 ${index}`)).toHaveAttribute("cy", String(y));
  }
  expect(screen.queryByLabelText("中线点 4")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "运行试测" })).toBeEnabled();
  expect(screen.queryByLabelText("本次算法结果")).not.toBeInTheDocument();
});

function extractionFailure(reason: string) {
  const failure = { point: [40, 20] as [number, number], reason, message: `提取失败：${reason}` };
  return { path: [], message: failure.message, failure };
}

async function brushOnTaughtGroup() {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });
  mockSite(groupedCandidate());
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "刷选胶路区域" }));
  fireEvent.pointerDown(image, { clientX: 10, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 20 });
  fireEvent.pointerUp(image);
  const extract = screen.getByRole("button", { name: "自动提取中线" }), stroke = screen.getByRole("button", { name: "笔划直接生成中线" });
  const failOnce = async () => { await user.click(extract); await screen.findByLabelText("中线提取失败位置"); };
  const primaries = () => ["自动提取中线", "笔划直接生成中线", "运行试测", "保存本组示教", "保存并进入下一组"]
    .map(name => screen.getByRole("button", { name })).filter(button => button.classList.contains("primary"));
  return { stroke, failOnce, primaries };
}

it("已有中线的组刷选提取连续两次找不到连续边缘 → 先提示加大笔刷，再把笔划直接生成中线设为唯一主操作", async () => {
  vi.mocked(workspaceApi.extractCenterline).mockResolvedValue(extractionFailure("discontinuity"));
  const { stroke, failOnce, primaries } = await brushOnTaughtGroup();
  expect(primaries()).toEqual([screen.getByRole("button", { name: "保存并进入下一组" })]);
  await failOnce();
  expect(screen.getByText(/可加大笔刷宽度（当前 120 px/)).toBeVisible();
  expect(stroke).not.toHaveClass("primary");
  fireEvent.change(screen.getByRole("slider", { name: "拖动调整笔刷宽度" }), { target: { value: "180" } });
  await failOnce();
  expect(screen.getByText(/请点「笔划直接生成中线」/)).toBeVisible();
  expect(primaries()).toEqual([stroke]);
});

it("两次找不到连续边缘之间夹一次多条候选失败 → 不算连续，第二次仍只提示加大笔刷", async () => {
  vi.mocked(workspaceApi.extractCenterline)
    .mockResolvedValueOnce(extractionFailure("discontinuity"))
    .mockResolvedValueOnce(extractionFailure("ambiguous"))
    .mockResolvedValueOnce(extractionFailure("missing_edges"));
  const { stroke, failOnce } = await brushOnTaughtGroup();
  await failOnce();
  await failOnce();
  await failOnce();
  expect(screen.getByText(/可加大笔刷宽度/)).toBeVisible();
  expect(screen.queryByText(/请点「笔划直接生成中线」/)).not.toBeInTheDocument();
  expect(stroke).not.toHaveClass("primary");
});

it("同组切换检测相机 → 原图取消选中且编辑切到新图，其他组保持原选择", async () => {
  const candidate = groupedCandidate();
  mockSite(candidate);
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.workspace.doc.shots[0].skip = true;
  response.workspace.doc.shots[1].skip = false;
  response.progress.shots[0].state = "skipped";
  response.progress.shots[0].views[0].enabled = false;
  response.progress.shots[1].state = "trialPassed";
  response.progress.shots[1].views[0].enabled = true;
  response.layout.shots = structuredClone(response.workspace.doc.shots);
  vi.mocked(workspaceApi.selectGroupImage).mockResolvedValue(response);
  const user = renderWorkspace("/recipe/teach");
  const secondCamera = await screen.findByRole("button", { name: "选择检测图 CAM-2 · 图 1" });
  expect(screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  await user.click(secondCamera);
  expect(workspaceApi.selectGroupImage).toHaveBeenCalledWith("A", 7, 1, 1);
  expect(await screen.findByRole("img", { name: "P1 · CAM-2 · 图 1" })).toBeVisible();
  expect(screen.getByRole("button", { name: "选择检测图 CAM-2 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" })).toHaveAttribute("aria-pressed", "false");
  await user.click(screen.getByRole("button", { name: "选择拍照组 P2" }));
  expect(await screen.findByRole("img", { name: "P2 · CAM-1 · 图 1" })).toBeVisible();
  expect(screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" })).toHaveAttribute("aria-pressed", "true");
});

it("未选检测图的组 → 默认保存首张，手动改选后返回该组仍保留选择", async () => {
  let candidate = groupedCandidate();
  candidate.workspace.lastPosition = { k: 4, view: 1 };
  for (const k of [3, 4, 5]) {
    candidate.workspace.doc.shots[k].skip = true;
    candidate.workspace.doc.shots[k].views?.forEach(view => { view.enabled = false; });
    shotState(candidate, k, "skipped");
  }
  mockSite(candidate);
  const selecting = deferred<void>();
  vi.mocked(workspaceApi.selectGroupImage).mockImplementation(async (_id, revision, k, view) => {
    await selecting.promise;
    const response = structuredClone(candidate);
    response.workspace.revision = revision + 1;
    response.workspace.doc.shots.forEach((shot, index) => {
      if (shot.poseId !== response.workspace.doc.shots[k].poseId) return;
      shot.skip = index !== k;
      shot.views?.forEach(image => { image.enabled = index === k && image.view === view; });
      shotState(response, index, index === k ? "pendingTrial" : "skipped");
    });
    candidate = response;
    return response;
  });
  const user = renderWorkspace("/recipe/overview", true);
  await screen.findByLabelText("工件总览，选择帧查看原图");
  await user.click(screen.getByRole("link", { name: /单帧示教/ }));
  await waitFor(() => expect(workspaceApi.selectGroupImage).toHaveBeenCalledWith("A", 7, 3, 1));
  await act(async () => selecting.resolve(undefined));
  expect(await screen.findByRole("img", { name: "P2 · CAM-1 · 图 1" })).toBeVisible();
  expect(workspaceApi.selectGroupImage).toHaveBeenCalledWith("A", 7, 3, 1);
  expect(screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("button", { name: "选择检测图 CAM-2 · 图 1" }));
  expect(await screen.findByRole("img", { name: "P2 · CAM-2 · 图 1" })).toBeVisible();
  await user.click(screen.getByRole("button", { name: "选择拍照组 P1" }));
  await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "选择拍照组 P2" }));
  expect(await screen.findByRole("img", { name: "P2 · CAM-2 · 图 1" })).toBeVisible();
  expect(screen.getByRole("button", { name: "选择检测图 CAM-2 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  expect(workspaceApi.selectGroupImage).toHaveBeenCalledTimes(2);
});

it("默认选图失败 → 显示错误并保留手动选图入口，不循环重试", async () => {
  const candidate = groupedCandidate();
  for (const k of [0, 1, 2]) {
    candidate.workspace.doc.shots[k].skip = true;
    candidate.workspace.doc.shots[k].views?.forEach(view => { view.enabled = false; });
    shotState(candidate, k, "skipped");
  }
  mockSite(candidate);
  const response = groupedCandidate();
  response.workspace.revision++;
  vi.mocked(workspaceApi.selectGroupImage).mockRejectedValueOnce(new Error("候选暂不可写")).mockResolvedValueOnce(response);
  const user = renderWorkspace("/recipe/teach");
  expect(await screen.findByText("候选暂不可写")).toBeVisible();
  const firstImage = screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" });
  expect(firstImage).toHaveAttribute("aria-pressed", "false");
  expect(firstImage).toBeEnabled();
  await user.click(firstImage);
  expect(await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" })).toBeVisible();
  expect(screen.queryByText("候选暂不可写")).not.toBeInTheDocument();
  expect(workspaceApi.selectGroupImage).toHaveBeenCalledTimes(2);
});

it.each([
  ["通过", true, true, "已验证"],
  ["未通过", false, false, "验证未通过"],
] as const)("人工验证返回%s → 采用后端判定，复用仅按人工选择提交", async (_label, passed, reuse, expectedStatus) => {
  const candidate = groupedCandidate();
  mockSite(candidate);
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.progress.shots[0].views[0].calibration = { state: passed ? "verified" : "failed", errorMm: passed ? .05 : .2 };
  vi.mocked(workspaceApi.checkCalibration).mockResolvedValue(response);
  const user = renderWorkspace("/recipe/teach");
  await user.click(await screen.findByText("标定", { exact: true }));
  await user.type(await screen.findByRole("spinbutton", { name: "实测尺寸" }), "10.05");
  expect(screen.getByText("+0.050 mm")).toBeVisible();
  expect(screen.getByText("未验证", { exact: true })).toBeVisible();
  const reuseInput = screen.getByRole("checkbox", { name: "复用到其他适用拍照点" });
  expect(reuseInput).not.toBeChecked();
  if (reuse) await user.click(reuseInput);
  await user.click(screen.getByRole("button", { name: "记录验证" }));
  expect(workspaceApi.checkCalibration).toHaveBeenLastCalledWith("A", 7, 0, 10, 10.05, reuse);
  expect(await screen.findByText(expectedStatus, { exact: true })).toBeVisible();
  expect(screen.getByText(new RegExp(`0.1 mm/px · 误差 ${passed ? "0.050" : "0.200"} mm`))).toBeVisible();
});

it("已勾选复用后切换拍照组 → 新图恢复单图验证，不沿用上一图的人工确认", async () => {
  const candidate = groupedCandidate();
  mockSite(candidate);
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.progress.shots[3].views[0].calibration = { state: "verified", errorMm: .02 };
  vi.mocked(workspaceApi.checkCalibration).mockResolvedValue(response);
  const user = renderWorkspace("/recipe/teach");
  await user.click(await screen.findByRole("checkbox", { name: "复用到其他适用拍照点" }));
  await user.click(screen.getByRole("button", { name: "选择拍照组 P2" }));
  expect(screen.getByRole("checkbox", { name: "复用到其他适用拍照点" })).not.toBeChecked();
  await user.type(screen.getByRole("spinbutton", { name: "实测尺寸" }), "10.02");
  await user.click(screen.getByRole("button", { name: "记录验证" }));
  expect(workspaceApi.checkCalibration).toHaveBeenLastCalledWith("A", 7, 3, 10, 10.02, false);
  expect(await screen.findByText("已验证", { exact: true })).toBeVisible();
});

it("修改样本集声明当量 → 草稿不沿用旧验证，保存试测后进入人工验证", async () => {
  const candidate = groupedCandidate();
  candidate.progress.shots[0].views[0].calibration = { state: "sampleSet", setId: "set-1", mmPerPx: .1 };
  mockSite(candidate);
  const response = structuredClone(candidate);
  vi.mocked(workspaceApi.saveParams).mockImplementation(async (_id, _revision, k, params) => {
    Object.assign(response.workspace.doc.shots[k], params);
    response.workspace.revision++;
    response.progress.shots[k].views[0].calibration = { state: "stale" };
    shotState(response, k, "pendingTrial");
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.trial).mockImplementation(async () => {
    shotState(response, 0, "trialPassed");
    return structuredClone(response);
  });
  const user = renderWorkspace("/recipe/teach");
  await user.click(await screen.findByText("标定", { exact: true }));
  expect(screen.getByText(/样本集 set-1 声明/)).toBeVisible();
  expect(screen.queryByRole("spinbutton", { name: "实测尺寸" })).not.toBeInTheDocument();
  await user.clear(screen.getByRole("spinbutton", { name: "像素当量" }));
  await user.type(screen.getByRole("spinbutton", { name: "像素当量" }), "0.2");
  expect(screen.getByText("需重新验证", { exact: true })).toBeVisible();
  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "记录验证" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "运行试测" }));
  expect(await screen.findByRole("spinbutton", { name: "实测尺寸" })).toHaveValue(null);
  expect(screen.getByRole("button", { name: "记录验证" })).toBeDisabled();
  expect(screen.queryByText("已验证", { exact: true })).not.toBeInTheDocument();
});

it("同位置三相机已选择检测图 → 总览每组一框绑定检测原图，移动保存共享坐标且保留选图", async () => {
  const candidate = groupedCandidate();
  candidate.workspace.doc.shots.forEach((shot, k) => {
    shot.skip = k % 3 !== 1;
    shot.views?.forEach(view => { view.enabled = !shot.skip; });
    shotState(candidate, k, shot.skip ? "skipped" : "taught");
  });
  candidate.layout.shots = structuredClone(candidate.workspace.doc.shots);
  candidate.workspace.overview.positions = [[.1, .1], [.2, .2], [.3, .3], [.4, .4], [.5, .5], [.6, .6]];
  const originalShots = structuredClone(candidate.workspace.doc.shots);
  mockSite(candidate);
  vi.mocked(workspaceApi.saveOverview).mockImplementation(async (_id, revision, overview) => {
    const response = structuredClone(candidate);
    response.workspace.overview = { ...structuredClone(overview), saved: true };
    response.workspace.revision = revision + 1;
    return response;
  });
  const user = renderWorkspace("/recipe/overview");
  const first = await screen.findByRole("button", { name: "总览选择帧 k2" });
  expect(screen.getAllByRole("button", { name: /^总览选择帧/ })).toHaveLength(2);
  expect(first).toHaveAttribute("aria-pressed", "true");
  expect(await screen.findByRole("img", { name: candidate.workspace.frames[1].image!.id })).toBeVisible();
  await user.click(screen.getByRole("button", { name: "总览选择帧 k5" }));
  expect(await screen.findByRole("img", { name: candidate.workspace.frames[4].image!.id })).toBeVisible();
  expect(workspaceApi.image).toHaveBeenLastCalledWith("A", candidate.workspace.frames[4].image!.id);
  await user.click(screen.getByRole("button", { name: "自动布置" }));
  fireEvent.keyDown(first, { key: "ArrowRight" });
  await user.click(screen.getByRole("button", { name: "保存总览" }));
  const saved = await screen.findByText("总览布置已保存，拍照与测量不受影响");
  expect(saved.closest(".wp-page")).toBeNull();
  expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A", 7, {
    background: null, positions: [[.26, .5], [.26, .5], [.26, .5], [.75, .5], [.75, .5], [.75, .5]], saved: false,
  });
  expect(candidate.workspace.doc.shots).toEqual(originalShots);
  expect(workspaceApi.selectGroupImage).not.toHaveBeenCalled();
});

it("示教完成后保存总览、实拍独立样本并验证 → 新样本自动选入验证，确认后按同一修订发布", async () => {
  const candidate = candidateView();
  candidate.workspace.frames.forEach(frame => { frame.saved = true; });
  [0, 1].forEach(k => shotState(candidate, k, "taught"));
  mockSite(candidate);

  const response = structuredClone(candidate);
  vi.mocked(workspaceApi.saveOverview).mockImplementation(async (id, revision, overview) => {
    expect([id, revision]).toEqual(["A", 7]);
    response.workspace.overview = { ...structuredClone(overview), saved: true };
    response.workspace.revision++;
    return structuredClone(response);
  });
  const independentRound: CaptureRound = {
    roundId: "validation-round", recipeId: "A", cameras: [{ cameraId: "CAM-1", deviceSession: 2, receivedCount: 2 }],
    plannedCount: 2, state: "complete", createdAt: 2, endedAt: 3,
    error: null, simulated: true,
    frames: [0, 1].map(k => ({ shotId: `P${k + 1}`, ordinal: k + 1, cameras: [{ cameraId: "CAM-1", frameCounter: k + 1,
      triggerCounter: k + 1, originalPath: `validation/k${k}.pgm`,
      views: [1, 2, 3].map(view => ({ view, path: `validation/k${k}-v${view}.pgm`, width: 100, height: 60 })),
    }] })),
  };
  let receiving = false;
  vi.mocked(workspaceApi.captureGet).mockImplementation(async () => receiving ? independentRound : null);
  vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
  vi.mocked(workspaceApi.captureStart).mockImplementation(async () => {
    receiving = true;
    return { ...independentRound, state: "waitingStart", cameras: [{ cameraId: "CAM-1", deviceSession: 2, receivedCount: 0 }], endedAt: null, frames: [] };
  });
  vi.mocked(workspaceApi.captureImage).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.captureSample).mockImplementation(async (id, revision, roundId, expected, confirmed) => {
    expect([id, revision, roundId, expected, confirmed]).toEqual(["A", 8, "validation-round", "OK", true]);
    expect(roundId).not.toBe(response.workspace.captureId);
    response.workspace.sampleBank = [{ id: roundId, name: "独立采集 2", geometryTag: response.workspace.doc.shots.map(shot => [shot.id, shot.poseId, shot.camera]), expected, createdAt: 2 }];
    response.workspace.samples = [{ historyId: null, sampleId: roundId, expected }];
    response.workspace.revision++;
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.validate).mockImplementation(async (id, revision, samples, confirmed) => {
    expect([id, revision, samples, confirmed]).toEqual(["A", 9, [{ historyId: null, sampleId: "validation-round", expected: "OK" }], true]);
    response.workspace.validation = { revision, passed: true, checkedAt: 4, environmentTag: {},
      checks: [{ name: "独立正常样本", passed: true, detail: "独立整圈与当前示教一致" }],
      samples: [{ historyId: null, sampleId: "validation-round", name: "独立采集 2", sn: "", expected: "OK", actual: "OK", passed: true, reason: "" }],
    };
    return structuredClone(response);
  });
  vi.mocked(workspaceApi.publish).mockImplementation(async (id, revision) => {
    expect([id, revision]).toEqual(["A", 9]);
    response.workspace.publish = { state: "waiting" };
    Object.assign(response.progress.steps.find(step => step.id === "publish")!, { status: "inProgress", detail: "v2 待生效：等待当前工件结束" });
    response.workspace.pending = { doc: structuredClone(response.workspace.doc), bundleId: "bundle-A-v2", baseRevision: response.workspace.baseRevision,
      revision, frames: structuredClone(response.workspace.frames), overview: structuredClone(response.workspace.overview), validation: structuredClone(response.workspace.validation!), validationSite: "real",
    };
    return structuredClone(response);
  });

  const user = renderWorkspace("/recipe/overview");
  const firstPosition = await screen.findByRole("button", { name: "总览选择帧 k1" });
  await waitFor(() => expect(screen.getByRole("button", { name: "保存总览" })).toBeEnabled());
  fireEvent.keyDown(firstPosition, { key: "ArrowRight" });
  await user.click(screen.getByRole("button", { name: "保存总览" }));
  const saved = await screen.findByText("总览布置已保存，拍照与测量不受影响");
  expect(saved.closest(".wp-page")).toBeNull();
  expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A", 7, { background: null, positions: [[.26, .5], [.75, .5]], saved: false });
  await user.click(screen.getByRole("link", { name: "验证与发布" }));
  expect(await screen.findByRole("button", { name: "发布生产配方" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "运行整套图像验证" })).toBeDisabled();
  await user.click(screen.getByRole("link", { name: "重新实拍样本" }));
  const startCapture = await screen.findByRole("button", { name: "开始接收" });
  await waitFor(() => expect(startCapture).toBeEnabled());
  await user.click(startCapture);
  expect(workspaceApi.captureStart).toHaveBeenCalledWith("A", ["CAM-1"], 1500);
  const saveSample = await screen.findByRole("button", { name: "保存为独立验证样本" });
  expect(saveSample).toBeDisabled();
  await user.click(screen.getByRole("checkbox", { name: "已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致" }));
  await user.click(saveSample);
  await screen.findByText("独立验证样本已加入样本库");
  await user.click(screen.getByRole("link", { name: "进入验证" }));
  expect(await screen.findByRole("checkbox", { name: "选用样本 独立采集 2" })).toBeChecked();
  await user.click(screen.getByRole("checkbox", { name: "已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选" }));
  await user.click(screen.getByRole("button", { name: "运行整套图像验证" }));
  await screen.findByText("符合");
  const publish = screen.getByRole("button", { name: "发布生产配方" });
  await waitFor(() => expect(publish).toBeEnabled());
  await user.click(publish);
  expect(await screen.findByRole("dialog", { name: "发布生产配方" })).toBeVisible();
  expect(workspaceApi.publish).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "确认发布 v2" }));
  await screen.findByText("v2 待生效：等待当前工件结束");
  expect(workspaceApi.publish).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
});


it("全部示教完成但标定未开始 → 导航显示示教完成并单独提示标定待验证", async () => {
  const candidate = groupedCandidate();
  candidate.progress.shots.forEach((shot, k) => { if (shot.state !== "skipped") shotState(candidate, k, "taught"); });
  candidate.progress.steps.find(step => step.id === "teach")!.status = "done";
  candidate.progress.steps.find(step => step.id === "calibration")!.status = "notStarted";
  mockSite(candidate);
  renderWorkspace("/recipe/teach");
  const navigation = await screen.findByRole("navigation", { name: "配方配置步骤" });
  const teaching = within(navigation).getByRole("link", { name: /单帧示教/ });
  expect(within(teaching).getByText("完成")).toBeVisible();
  expect(within(teaching).queryByText("未开始")).not.toBeInTheDocument();
  expect(within(teaching).getByText("标定待验证")).toBeVisible();
});

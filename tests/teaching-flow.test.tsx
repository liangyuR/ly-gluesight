import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { PointerEventsCheckLevel } from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import OverviewPage from "../src/features/workspace/OverviewPage";
import ValidationPage from "../src/features/workspace/ValidationPage";
import CapturePage from "../src/features/workspace/CapturePage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import { SnackbarProvider } from "qing-steel";
import { workspaceApi } from "../src/features/workspace/api";
import type { CaptureRound, ShotState, WorkspaceView } from "../src/features/workspace/types";
import { deferred, siteStatus, snapshot, summary, tricamWorkspaceView, twoLines } from "./fixtures";
import { stubSvgPointer } from "./svg-pointer";

// 只替身桌面后端：invoke 按命令名分派到 backend 夹具，其余全是真实模块。
// 例外：原图解码要用 canvas（jsdom 没有），workspaceApi.image / captureImage 直接给出解码结果。
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;
let backend: Record<string, Handler> = {};

/** 本次渲染能用到的全部桌面命令；handlers 覆盖或补充写操作。返回值复制一份，与 IPC 序列化一致。 */
function mockSite(candidate: WorkspaceView, handlers: Record<string, Handler> = {}) {
  backend = {
    storage_preferences_get: () => ({ legacyImported: true }),
    storage_preferences_update: ({ patch }) => ({ legacyImported: true, ...(patch as object) }),
    recipe_list: () => ({ recipes: [summary(candidate)], errors: [], unavailable: [] }),
    workspace_list: () => [candidate.workspace],
    workspace_get: () => candidate,
    workspace_progress: ({ k, view }) => ({ ...candidate, workspace: { ...candidate.workspace, lastPosition: { k, view } } }),
    camera_rig_config: () => [{ id: "CAM-1", name: "拼接设备", source: "sim", viewCount: 3, serial: "DEVICE",
      triggerSource: "Line0", triggerActivation: "RisingEdge", triggerDelayUs: 0, debouncerUs: 0, strobe: false, chunk: true }],
    camera_rig_status: () => [{ id: "CAM-1", ready: true }],
    vision_calib_info: () => null,
    plc_get_status: () => ({ state: "connected", message: "PLC 已连接" }),
    site_status: () => siteStatus(),
    cycle_snapshot: () => snapshot(),
    cycle_logs: () => [],
    history_query: () => ({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } }),
    ...handlers,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = backend[command];
    if (!handler) throw new Error(`未声明的桌面调用：${command}`);
    return structuredClone(await handler((args ?? {}) as Args));
  });
  vi.spyOn(workspaceApi, "image").mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
}

/** 依次返回给定响应，模拟后端逐次推进修订 */
const replies = (...responses: unknown[]): Handler => () => {
  const next = responses.shift();
  if (next instanceof Error) throw next;
  return next;
};
const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args);

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

/** 第一组 P1 还没有中线、当量与试测 */
function untaughtCandidate() {
  const candidate = groupedCandidate();
  candidate.workspace.doc.shots[0].path = [];
  delete candidate.workspace.doc.shots[0].mmPerPx;
  candidate.workspace.frames[0].trial = null;
  candidate.workspace.frames[0].saved = false;
  shotState(candidate, 0, "unconfigured");
  return candidate;
}

afterEach(() => {
  vi.useRealTimers();
  delete (globalThis as { jest?: unknown }).jest;
});

/** 假时钟下渲染：自动保存防抖、采集轮询只在测试推进时间时触发。 */
function renderWorkspace(path: string, strict = false) {
  vi.useFakeTimers();
  // Testing Library 只在全局有 jest 时才在 waitFor / findBy 里推进假时钟
  Object.assign(globalThis, { jest: { advanceTimersByTime: (ms: number) => vi.advanceTimersByTime(ms) } });
  const workspace = <MemoryRouter initialEntries={[path]}><SnackbarProvider><WorkspaceProvider><Routes>
    <Route path="/recipe/teach" element={<TeachingPage />} />
    <Route path="/recipe/overview" element={<OverviewPage />} />
    <Route path="/recipe/validation" element={<ValidationPage />} />
    <Route path="/recipe/capture" element={<CapturePage />} />
  </Routes></WorkspaceProvider></SnackbarProvider></MemoryRouter>;
  render(strict ? <StrictMode>{workspace}</StrictMode> : workspace);
  // jsdom 没有样式表，逐级检查 pointer-events 只是空耗
  return userEvent.setup({ advanceTimers: vi.advanceTimersByTime, pointerEventsCheck: PointerEventsCheckLevel.Never });
}

/** P1 本图覆盖了检测参数（搜索带 4 mm，配方默认 15 mm）与判定限值（关闭位置判定） */
function overriddenCandidate() {
  const candidate = groupedCandidate();
  candidate.workspace.doc.detect.searchMm = 15;
  candidate.workspace.doc.shots[0].detect = { searchMm: 4, polarity: "dark", widthRange: [1, 5] };
  candidate.workspace.doc.shots[0].limits = { ...structuredClone(candidate.workspace.doc.limits), position: null, maxGapLen: 3 };
  candidate.workspace.doc.shots[0].mmPerPx = .1;
  return candidate;
}

const searchBandEnds = () => [...screen.getByLabelText("搜索带边界").querySelectorAll("polyline")].map(edge => {
  const points = edge.getAttribute("points")!.split(" ");
  return `${points[0]} ${points[points.length - 1]}`;
});

it("本图检测参数覆盖后恢复默认 → 搜索带按配方默认搜索宽度重画", async () => {
  mockSite(overriddenCandidate());
  renderWorkspace("/recipe/teach");
  await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  expect(searchBandEnds()).toEqual(["10,50 20,50", "10,-30 20,-30"]);
  fireEvent.click(screen.getByText("检测参数", { exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "恢复配方默认检测参数" }));
  expect(searchBandEnds()).toEqual(["10,160 20,160", "10,-140 20,-140"]);
});

it("本图参数都恢复配方默认后试测 → 保存载荷不带本图覆盖，并按保存后的修订试测", async () => {
  const candidate = overriddenCandidate();
  const response = structuredClone(candidate);
  delete response.workspace.doc.shots[0].detect;
  delete response.workspace.doc.shots[0].limits;
  response.workspace.revision++;
  mockSite(candidate, { workspace_save_params: () => response, workspace_trial: () => response });
  const user = renderWorkspace("/recipe/teach");
  await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  fireEvent.click(screen.getByText("检测参数", { exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "恢复配方默认检测参数" }));
  await user.click(screen.getByText("判定限值", { exact: true }));
  expect(screen.getByRole("checkbox", { name: "位置判定" })).not.toBeChecked();
  await user.click(screen.getByRole("button", { name: "恢复配方默认判定限值" }));
  await user.click(screen.getByRole("button", { name: "运行试测" }));
  await waitFor(() => expect(calls("workspace_trial")).toEqual([{ id: "A", revision: 8, k: 0, imageId: candidate.workspace.frames[0].image!.id }]));
  expect(calls("workspace_save_params")).toEqual([{ id: "A", revision: 7, k: 0, params: { path: candidate.workspace.doc.shots[0].path, mmPerPx: .1, fixture: null } }]);
});

it.each(["", "0"])("像素当量改为「%s」 → 隐藏搜索带", async value => {
  mockSite(groupedCandidate());
  renderWorkspace("/recipe/teach");
  await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  expect(screen.getByLabelText("搜索带边界")).toBeInTheDocument();
  fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value } });
  expect(screen.queryByLabelText("搜索带边界")).not.toBeInTheDocument();
});

it("手工落点并自动保存 → 不插入瞬时警告、保留缩放与中线，缺少当量仍阻止试测", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = untaughtCandidate();
  const saving = deferred<void>();
  mockSite(candidate, { workspace_save_draft: async ({ revision, k, params }) => {
    await saving.promise;
    const response = structuredClone(candidate);
    Object.assign(response.workspace.doc.shots[k as number], params);
    response.workspace.revision = (revision as number) + 1;
    return response;
  } });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "放大原图" }));
  const transform = image.querySelector("g")!.getAttribute("transform");
  for (const [clientX, clientY] of [[10, 10], [20, 20]]) {
    fireEvent.pointerDown(image, { clientX, clientY });
    fireEvent.pointerUp(image);
  }
  const transientWarnings = ["示教参数有未保存的修改", "本组中线无效"].filter(title => screen.queryByText(title));
  const editButton = screen.getByRole("button", { name: "点柄" });
  const point = screen.getByLabelText("中线点 2");
  fireEvent.pointerDown(point, { clientX: 20, clientY: 20 });
  act(() => { vi.advanceTimersByTime(900); }); // 自动保存防抖到期，保存请求挂起
  expect(screen.getByText("保存中")).toBeVisible();
  expect(screen.getByRole("button", { name: "点柄" })).toBe(editButton);
  expect(editButton).toBeDisabled();
  expect(screen.getByLabelText("中线点 2")).toBe(point);
  fireEvent.pointerMove(image, { clientX: 30, clientY: 30 });
  expect(point).toHaveAttribute("cx", "20");
  fireEvent.pointerUp(image);
  fireEvent.pointerDown(image, { clientX: 30, clientY: 30 });
  fireEvent.pointerUp(image);
  expect(screen.queryByLabelText("中线点 3")).not.toBeInTheDocument();
  await act(async () => saving.resolve(undefined));
  expect(await screen.findByText("草稿已保存")).toBeVisible();
  expect(editButton).toBeEnabled();
  expect(screen.getByRole("img", { name: "P1 · CAM-1 · 图 1" })).toBe(image);
  expect(image.querySelector("g")).toHaveAttribute("transform", transform);
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cx", "20");
  expect(screen.getByRole("button", { name: "运行试测" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "运行试测" })).toHaveAccessibleDescription(expect.stringContaining("像素当量"));
  expect(transientWarnings).toEqual([]);
});

it("每组三相机图只示教选中图，保存并进入下一组 → 下一组的操作沿用保存后的最新修订", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = groupedCandidate();
  const trials = candidate.workspace.frames.map(frame => structuredClone(frame.trial!));
  candidate.coverage = 0;
  candidate.workspace.doc.shots.forEach(shot => { shot.path = []; delete shot.mmPerPx; });
  candidate.workspace.frames.forEach(frame => { frame.trial = null; frame.saved = false; });
  [0, 3].forEach(k => shotState(candidate, k, "unconfigured"));
  candidate.layout.shots = structuredClone(candidate.workspace.doc.shots);
  candidate.layout.segments = [];
  candidate.layout.points = { x: [], y: [], seg: [], k: [] };

  // 后端依次推进：保存参数 7→8、试测、保存示教 8→9
  const response = structuredClone(candidate);
  const step = (change: () => void) => { change(); return structuredClone(response); };
  const saved = step(() => {
    response.workspace.revision = 8;
    response.workspace.doc.shots[0].path = structuredClone(twoLines[0]);
    response.workspace.doc.shots[0].mmPerPx = .1;
    response.layout.shots = structuredClone(response.workspace.doc.shots);
    shotState(response, 0, "pendingTrial");
  });
  const tried = step(() => { response.workspace.frames[0].trial = trials[0]; shotState(response, 0, "trialPassed"); });
  const taught = step(() => { response.workspace.frames[0].saved = true; shotState(response, 0, "taught"); response.workspace.revision = 9; response.coverage = 50; });
  mockSite(candidate, { workspace_save_params: () => saved, workspace_trial: () => tried, workspace_save_teach: () => taught, workspace_select_group_image: () => taught });

  const user = renderWorkspace("/recipe/teach");
  const steps = within(await screen.findByRole("navigation", { name: "配方配置步骤" }));
  expect(await steps.findByText("0 / 2 组")).toBeVisible();
  expect(screen.getByRole("button", { name: "选择拍照组 P1" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("button", { name: "运行试测" })).toBeDisabled();
  const actions = within(screen.getByRole("group", { name: "单帧示教操作" }));
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  for (const [clientX, clientY] of twoLines[0]) {
    fireEvent.pointerDown(image, { clientX, clientY });
    fireEvent.pointerUp(image);
  }
  fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".1" } });
  expect(screen.getByLabelText("中线点 2")).toBeVisible();
  await user.click(actions.getByRole("button", { name: "运行试测" }));
  expect(calls("workspace_save_params")).toEqual([{ id: "A", revision: 7, k: 0, params: { path: twoLines[0], mmPerPx: .1, fixture: null } }]);
  await waitFor(() => expect(calls("workspace_trial")).toEqual([{ id: "A", revision: 8, k: 0, imageId: "A-image-0-cam1" }]));
  expect(screen.queryByText("示教参数有未保存的修改")).not.toBeInTheDocument();
  await waitFor(() => expect(actions.getByRole("button", { name: "保存并进入下一组" })).toBeEnabled());
  await user.click(actions.getByRole("button", { name: "保存并进入下一组" }));
  expect(calls("workspace_save_teach")).toEqual([{ id: "A", revision: 8, k: 0, imageId: "A-image-0-cam1" }]);
  expect(await steps.findByText("1 / 2 组")).toBeVisible();
  expect(await screen.findByRole("img", { name: "P2 · CAM-1 · 图 1" })).toBeVisible();
  expect(screen.getByRole("button", { name: "选择拍照组 P2" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("button", { name: "选择检测图 CAM-2 · 图 1" }));
  expect(calls("workspace_select_group_image")).toEqual([{ id: "A", revision: 9, k: 4, view: 1 }]);
});

it("最后一组试测通过后保存 → 主操作是保存本组示教，保存后示教完成且无可保存项", async () => {
  const candidate = groupedCandidate();
  shotState(candidate, 0, "taught");
  candidate.workspace.frames[0].saved = true;
  candidate.workspace.lastPosition = { k: 3, view: 1 };
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.workspace.frames[3].saved = true;
  shotState(response, 3, "taught");
  mockSite(candidate, { workspace_save_teach: () => response });
  const user = renderWorkspace("/recipe/teach");
  const steps = within(await screen.findByRole("navigation", { name: "配方配置步骤" }));
  await screen.findByRole("img", { name: "P2 · CAM-1 · 图 1" });
  expect(steps.getByText("1 / 2 组")).toBeVisible();
  const save = screen.getByRole("button", { name: "保存本组示教" });
  expect(save).toHaveAttribute("data-variant", "filled");
  await user.click(save);
  expect(calls("workspace_save_teach")).toEqual([{ id: "A", revision: 7, k: 3, imageId: "A-image-1-cam1" }]);
  expect(await steps.findByText("2 / 2 组")).toBeVisible();
  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeDisabled();
});

it("沿胶路刷选后自动提取中线 → 先保存草稿，再按刷选引导、笔刷半径与点距提取一次", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = untaughtCandidate();
  const response = structuredClone(candidate);
  mockSite(candidate, {
    workspace_save_draft: ({ k, params }) => {
      Object.assign(response.workspace.doc.shots[k as number], params);
      response.workspace.revision++;
      return response;
    },
    workspace_extract_centerline: () => ({ path: [[12, 20], [32, 20], [52, 20], [52, 40], [52, 60]], message: "中线已提取" }),
  });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  expect(screen.getByRole("button", { name: "沿胶路刷选" })).toHaveAttribute("aria-pressed", "true");
  expect(calls("workspace_extract_centerline")).toEqual([]);
  fireEvent.pointerDown(image, { clientX: 10.2, clientY: 12.4 });
  fireEvent.pointerMove(image, { clientX: 50.8, clientY: 42.7 });
  fireEvent.pointerUp(image);
  fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".2" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "中线点间距" }), { target: { value: "10" } });
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  expect(await screen.findByLabelText("中线点 5")).toBeVisible();
  expect(screen.queryByLabelText("刷选胶路区域")).not.toBeInTheDocument();
  expect(calls("workspace_save_draft")).toEqual([{ id: "A", revision: 7, k: 0, params: { path: [], mmPerPx: .2, fixture: null } }]);
  expect(calls("workspace_extract_centerline")).toEqual([{ id: "A", revision: 8, k: 0, imageId: "A-image-0-cam1", roi: [0, 0, 100, 60],
    guide: { points: [[10.2, 12.4], [50.8, 42.7]], radiusPx: 60 }, pointSpacingPx: 10 }]);
});

/** P1 的中线有一处转折；最近一次试测只测到第 1 站，第 2 站只找到一条未通过的候选胶条 */
function partlyMeasuredCandidate() {
  const candidate = groupedCandidate();
  candidate.workspace.doc.shots[0].path = [[12, 20], [32, 20], [52, 20], [52, 40], [52, 60]];
  candidate.workspace.doc.shots[0].mmPerPx = .2;
  Object.assign(candidate.workspace.frames[0].trial!, {
    passed: true, coverage: .5, elapsedMs: 8, verdict: "NG_GAP",
    measurement: { unit: "mm", ids: [0, 1], status: ["ok", "no_bead"], innerCenter: [.125, null],
      width: [2.5, null], px: [[12, 21], [22, 20]], edges: [[[12, 15], [12, 27]], null], candidateWidth: [null, 3.2], candidateEdges: [null, [[22, 12], [22, 28]]], coverage: .5, ms: 8 },
  });
  return candidate;
}

async function openTrialTable() {
  mockSite(partlyMeasuredCandidate());
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByText("逐点试测结果 · 测得 1 / 2 · 未测 1"));
  return { user, image, table: screen.getByRole("table", { name: "逐点试测结果" }) };
}

it("试测只测得部分站点 → 逐点表列出未通过的候选宽度，图上画出测得边缘与未通过的候选边缘", async () => {
  const { image, table } = await openTrialTable();
  expect(within(table).getByText("3.200（候选，未通过）")).toBeVisible();
  expect(image.querySelector('line[x1="12"][y1="15"][x2="12"][y2="27"]')).not.toBeNull();
  expect(image.querySelector('line[x1="22"][y1="12"][x2="22"][y2="28"][stroke="var(--ng)"]')).not.toBeNull();
});

it("试测后按新点距重采样当前中线 → 只留转折点，试测结果作废且不能保存示教", async () => {
  const { user, image } = await openTrialTable();
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
  stubSvgPointer({ pointerEvent: true });
  const candidate = groupedCandidate();
  const failure = { point: [40, 20] as [number, number], reason: "discontinuous_edges", message: "此处胶路宽度突变，请检查亮边与阴影。" };
  mockSite(candidate, { workspace_extract_centerline: () => ({ path: [], message: failure.message, failure }) });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  const originalPoints = candidate.workspace.doc.shots[0].path.map(point => point.join(",")).join(" ");
  expect(screen.getByRole("button", { name: "保存本组示教" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "沿胶路刷选" }));
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
  const slider = screen.getByRole("slider", { name: "拖动调整笔刷宽度" });
  for (let i = 0; i < 3; i++) fireEvent.keyDown(slider, { key: "PageDown" });
  expect(slider).toHaveAttribute("aria-valuenow", "60");
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

async function brushOnTaughtGroup(extract: Handler) {
  stubSvgPointer({ pointerEvent: true });
  mockSite(groupedCandidate(), { workspace_extract_centerline: extract });
  const user = renderWorkspace("/recipe/teach");
  const image = await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "沿胶路刷选" }));
  fireEvent.pointerDown(image, { clientX: 10, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 20 });
  fireEvent.pointerUp(image);
  const extractButton = screen.getByRole("button", { name: "自动提取中线" }), stroke = screen.getByRole("button", { name: "笔划直接生成中线" });
  const failOnce = async () => { await user.click(extractButton); await screen.findByLabelText("中线提取失败位置"); };
  const primaries = () => ["自动提取中线", "笔划直接生成中线", "运行试测", "保存本组示教", "保存并进入下一组"]
    .map(name => screen.getByRole("button", { name })).filter(button => button.dataset.variant === "filled");
  return { user, stroke, failOnce, primaries };
}

it("已有中线的组刷选提取连续两次找不到连续边缘 → 先提示加大笔刷，再把笔划直接生成中线设为唯一主操作", async () => {
  const { stroke, failOnce, primaries } = await brushOnTaughtGroup(() => extractionFailure("discontinuity"));
  expect(primaries()).toEqual([screen.getByRole("button", { name: "保存并进入下一组" })]);
  await failOnce();
  expect(screen.getByText(/可加大笔刷宽度（当前 120 px/)).toBeVisible();
  expect(stroke).toHaveAttribute("data-variant", "tonal");
  const slider = screen.getByRole("slider", { name: "拖动调整笔刷宽度" });
  for (let i = 0; i < 3; i++) fireEvent.keyDown(slider, { key: "PageUp" });
  expect(slider).toHaveAttribute("aria-valuenow", "180");
  await failOnce();
  expect(screen.getByText(/请点「笔划直接生成中线」/)).toBeVisible();
  expect(primaries()).toEqual([stroke]);
});

it("刷选后切回编辑点再自动提取 → 仍用上次刷选的选区与笔刷，不要求重新刷选", async () => {
  const { user } = await brushOnTaughtGroup(() => ({ path: [[12, 20], [38, 20]], message: "" }));
  await user.click(screen.getByRole("button", { name: "编辑点" }));
  await user.click(screen.getByRole("button", { name: "自动提取中线" }));
  await waitFor(() => expect(calls("workspace_extract_centerline")).toEqual([{ id: "A", revision: 7, k: 0, imageId: "A-image-0-cam1", roi: [0, 0, 100, 60],
    guide: { points: [[10, 20], [40, 20]], radiusPx: 60 }, pointSpacingPx: 20 }]));
  expect(screen.getByRole("button", { name: "编辑点" })).toHaveAttribute("aria-pressed", "true");
});

it("两次找不到连续边缘之间夹一次多条候选失败 → 不算连续，第二次仍只提示加大笔刷", async () => {
  const { stroke, failOnce } = await brushOnTaughtGroup(replies(extractionFailure("discontinuity"), extractionFailure("ambiguous"), extractionFailure("missing_edges")));
  await failOnce();
  await failOnce();
  await failOnce();
  expect(screen.getByText(/可加大笔刷宽度/)).toBeVisible();
  expect(screen.queryByText(/请点「笔划直接生成中线」/)).not.toBeInTheDocument();
  expect(stroke).toHaveAttribute("data-variant", "tonal");
});

it("同组切换检测相机 → 原图取消选中且编辑切到新图，其他组保持原选择", async () => {
  const candidate = groupedCandidate();
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.workspace.doc.shots[0].skip = true;
  response.workspace.doc.shots[1].skip = false;
  response.progress.shots[0].state = "skipped";
  response.progress.shots[0].views[0].enabled = false;
  response.progress.shots[1].state = "trialPassed";
  response.progress.shots[1].views[0].enabled = true;
  response.layout.shots = structuredClone(response.workspace.doc.shots);
  mockSite(candidate, { workspace_select_group_image: () => response });
  const user = renderWorkspace("/recipe/teach");
  const secondCamera = await screen.findByRole("button", { name: "选择检测图 CAM-2 · 图 1" });
  expect(screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  await user.click(secondCamera);
  expect(calls("workspace_select_group_image")).toEqual([{ id: "A", revision: 7, k: 1, view: 1 }]);
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
  const selecting = deferred<void>();
  mockSite(candidate, { workspace_select_group_image: async ({ revision, k, view }) => {
    await selecting.promise;
    const response = structuredClone(candidate);
    response.workspace.revision = (revision as number) + 1;
    response.workspace.doc.shots.forEach((shot, index) => {
      if (shot.poseId !== response.workspace.doc.shots[k as number].poseId) return;
      shot.skip = index !== k;
      shot.views?.forEach(image => { image.enabled = index === k && image.view === view; });
      shotState(response, index, index === k ? "pendingTrial" : "skipped");
    });
    candidate = response;
    return response;
  } });
  const user = renderWorkspace("/recipe/overview", true);
  await screen.findByLabelText("工件总览，选择帧查看原图");
  await user.click(screen.getByRole("link", { name: /单帧示教/ }));
  await waitFor(() => expect(calls("workspace_select_group_image")).toEqual([{ id: "A", revision: 7, k: 3, view: 1 }]));
  await act(async () => selecting.resolve(undefined));
  expect(await screen.findByRole("img", { name: "P2 · CAM-1 · 图 1" })).toBeVisible();
  expect(screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("button", { name: "选择检测图 CAM-2 · 图 1" }));
  expect(await screen.findByRole("img", { name: "P2 · CAM-2 · 图 1" })).toBeVisible();
  await user.click(screen.getByRole("button", { name: "选择拍照组 P1" }));
  await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" });
  await user.click(screen.getByRole("button", { name: "选择拍照组 P2" }));
  expect(await screen.findByRole("img", { name: "P2 · CAM-2 · 图 1" })).toBeVisible();
  expect(screen.getByRole("button", { name: "选择检测图 CAM-2 · 图 1" })).toHaveAttribute("aria-pressed", "true");
  expect(calls("workspace_select_group_image")).toHaveLength(2);
});

it("默认选图失败 → 显示错误并保留手动选图入口，不循环重试", async () => {
  const candidate = groupedCandidate();
  for (const k of [0, 1, 2]) {
    candidate.workspace.doc.shots[k].skip = true;
    candidate.workspace.doc.shots[k].views?.forEach(view => { view.enabled = false; });
    shotState(candidate, k, "skipped");
  }
  const response = groupedCandidate();
  response.workspace.revision++;
  mockSite(candidate, { workspace_select_group_image: replies(new Error("候选暂不可写"), response) });
  const user = renderWorkspace("/recipe/teach");
  expect(await screen.findByText("候选暂不可写")).toBeVisible();
  const firstImage = screen.getByRole("button", { name: "选择检测图 CAM-1 · 图 1" });
  expect(firstImage).toHaveAttribute("aria-pressed", "false");
  expect(firstImage).toBeEnabled();
  await user.click(firstImage);
  expect(await screen.findByRole("img", { name: "P1 · CAM-1 · 图 1" })).toBeVisible();
  expect(screen.queryByText("候选暂不可写")).not.toBeInTheDocument();
  expect(calls("workspace_select_group_image")).toHaveLength(2);
});

it.each([
  ["通过", { state: "verified", errorMm: .05 }, "已验证", "0.050"],
  ["未通过", { state: "failed", errorMm: .2 }, "验证未通过", "0.200"],
] as const)("人工勾选复用并记录验证、后端判定%s → 请求带上复用，状态与误差采用后端判定", async (_label, calibration, expectedStatus, errorMm) => {
  const candidate = groupedCandidate();
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.progress.shots[0].views[0].calibration = calibration;
  mockSite(candidate, { workspace_check_calibration: () => response });
  const user = renderWorkspace("/recipe/teach");
  await user.click(await screen.findByText("标定", { exact: true }));
  await user.type(await screen.findByRole("spinbutton", { name: "实测尺寸" }), "10.05");
  expect(screen.getByText("+0.050 mm")).toBeVisible();
  expect(screen.getByText("未验证", { exact: true })).toBeVisible();
  const reuseInput = screen.getByRole("checkbox", { name: "复用到其他适用拍照点" });
  expect(reuseInput).not.toBeChecked();
  await user.click(reuseInput);
  await user.click(screen.getByRole("button", { name: "记录验证" }));
  expect(calls("workspace_check_calibration")).toEqual([{ id: "A", revision: 7, k: 0, referenceMm: 10, measuredMm: 10.05, reuseMatching: true }]);
  expect(await screen.findByText(expectedStatus, { exact: true })).toBeVisible();
  expect(screen.getByText(new RegExp(`0.1 mm/px · 误差 ${errorMm} mm`))).toBeVisible();
});

it("已勾选复用后切换拍照组 → 新图恢复单图验证，不沿用上一图的人工确认", async () => {
  const candidate = groupedCandidate();
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.progress.shots[3].views[0].calibration = { state: "verified", errorMm: .02 };
  mockSite(candidate, { workspace_check_calibration: () => response });
  const user = renderWorkspace("/recipe/teach");
  await user.click(await screen.findByRole("checkbox", { name: "复用到其他适用拍照点" }));
  await user.click(screen.getByRole("button", { name: "选择拍照组 P2" }));
  expect(screen.getByRole("checkbox", { name: "复用到其他适用拍照点" })).not.toBeChecked();
  await user.type(screen.getByRole("spinbutton", { name: "实测尺寸" }), "10.02");
  await user.click(screen.getByRole("button", { name: "记录验证" }));
  expect(calls("workspace_check_calibration")).toEqual([{ id: "A", revision: 7, k: 3, referenceMm: 10, measuredMm: 10.02, reuseMatching: false }]);
  expect(await screen.findByText("已验证", { exact: true })).toBeVisible();
});

it("修改样本集声明当量 → 草稿不沿用旧验证，保存试测后进入人工验证", async () => {
  const candidate = groupedCandidate();
  candidate.progress.shots[0].views[0].calibration = { state: "sampleSet", setId: "set-1", mmPerPx: .1 };
  const response = structuredClone(candidate);
  mockSite(candidate, {
    workspace_save_params: ({ k, params }) => {
      Object.assign(response.workspace.doc.shots[k as number], params);
      response.workspace.revision++;
      response.progress.shots[k as number].views[0].calibration = { state: "stale" };
      shotState(response, k as number, "pendingTrial");
      return response;
    },
    workspace_trial: () => {
      shotState(response, 0, "trialPassed");
      return response;
    },
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
  mockSite(candidate, { workspace_save_overview: ({ revision, overview }) => {
    const response = structuredClone(candidate);
    response.workspace.overview = { ...structuredClone(overview as WorkspaceView["workspace"]["overview"]), saved: true };
    response.workspace.revision = (revision as number) + 1;
    return response;
  } });
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
  // 保存结果走全局提示，不在页面里留常驻横幅
  expect((await screen.findByText(/总览布置已保存/)).closest('[role="status"]')).not.toBeNull();
  expect(calls("workspace_save_overview")).toEqual([{ id: "A", revision: 7, overview: {
    background: null, positions: [[.26, .5], [.26, .5], [.26, .5], [.75, .5], [.75, .5], [.75, .5]], saved: false,
  } }]);
  expect(calls("workspace_select_group_image")).toEqual([]);
});

/** 示教全部完成、总览已保存，尚无独立验证样本 */
function taughtCandidate() {
  const candidate = candidateView();
  candidate.workspace.frames.forEach(frame => { frame.saved = true; });
  [0, 1].forEach(k => shotState(candidate, k, "taught"));
  candidate.workspace.overview.saved = true;
  return candidate;
}

it("验证样本模式下实拍并保存 → 以独立于示教的轮次入库，进入验证后自动选中新样本", async () => {
  const candidate = taughtCandidate();
  const response = structuredClone(candidate);
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
  mockSite(candidate, {
    recipe_capture_get: () => receiving ? independentRound : null,
    recipe_capture_list: () => [],
    recipe_capture_start: () => {
      receiving = true;
      return { ...independentRound, state: "waitingStart", cameras: [{ cameraId: "CAM-1", deviceSession: 2, receivedCount: 0 }], endedAt: null, frames: [] };
    },
    workspace_capture_sample: ({ roundId, expected }) => {
      response.workspace.sampleBank = [{ id: roundId as string, name: "独立采集 2", geometryTag: response.workspace.doc.shots.map(shot => [shot.id, shot.poseId, shot.camera]), expected: expected as "OK", createdAt: 2 }];
      response.workspace.samples = [{ historyId: null, sampleId: roundId as string, expected: expected as "OK" }];
      response.workspace.revision++;
      return response;
    },
  });
  vi.spyOn(workspaceApi, "captureImage").mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  const user = renderWorkspace("/recipe/capture?purpose=validation");
  const startCapture = await screen.findByRole("button", { name: "开始接收" });
  await waitFor(() => expect(startCapture).toBeEnabled());
  await user.click(startCapture);
  expect(calls("recipe_capture_start")).toEqual([{ recipeId: "A", cameraIds: ["CAM-1"], drainTimeoutMs: 1500 }]);
  const saveSample = await screen.findByRole("button", { name: "保存为独立验证样本" });
  expect(saveSample).toBeDisabled();
  await user.click(screen.getByRole("checkbox", { name: "已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致" }));
  await user.click(saveSample);
  await screen.findByText("独立验证样本已加入样本库");
  expect(calls("workspace_capture_sample")).toEqual([{ id: "A", revision: 7, roundId: "validation-round", expected: "OK", correspondenceConfirmed: true }]);
  expect(independentRound.roundId).not.toBe(candidate.workspace.captureId);
  await user.click(screen.getByRole("link", { name: "进入验证" }));
  expect(await screen.findByRole("checkbox", { name: "选用样本 独立采集 2" })).toBeChecked();
});

it("选用独立样本并确认适用后运行验证 → 按当前修订提交所选样本，验证通过后可发布", async () => {
  const candidate = taughtCandidate();
  const sample = { historyId: null, sampleId: "validation-round", expected: "OK" as const };
  candidate.workspace.sampleBank = [{ id: sample.sampleId, name: "独立采集 2", geometryTag: candidate.workspace.doc.shots.map(shot => [shot.id, shot.poseId, shot.camera]), expected: "OK", createdAt: 2 }];
  candidate.workspace.samples = [sample];
  mockSite(candidate, { workspace_validate: ({ revision }) => {
    const response = structuredClone(candidate);
    response.workspace.validation = { revision: revision as number, passed: true, checkedAt: 4, environmentTag: {},
      checks: [{ name: "独立正常样本", passed: true, detail: "独立整圈与当前示教一致" }],
      samples: [{ ...sample, name: "独立采集 2", sn: "", actual: "OK", passed: true, reason: "" }],
    };
    return response;
  } });
  const user = renderWorkspace("/recipe/validation");
  expect(await screen.findByRole("checkbox", { name: "选用样本 独立采集 2" })).toBeChecked();
  expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
  await user.click(screen.getByRole("checkbox", { name: "已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选" }));
  await user.click(screen.getByRole("button", { name: "运行整套图像验证" }));
  await screen.findByText("符合");
  expect(calls("workspace_validate")).toEqual([{ id: "A", revision: 7, samples: [sample], sourceConfirmed: true }]);
  await waitFor(() => expect(screen.getByRole("button", { name: "发布生产配方" })).toBeEnabled());
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

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
import { burstRecipe, burstSummary, burstWorkspaceView, captureRoundV2, deferred, siteStatus, snapshot, summary, tricamWorkspaceView, type BurstRecipeDoc } from "./fixtures";
import { stubSvgPointer } from "./svg-pointer";

// 只替身桌面后端：invoke 按命令名分派到 backend 夹具，其余全是真实模块。
// 例外：原图解码要用 canvas（jsdom 没有），workspaceApi.image / captureImage 直接给出解码结果。
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
// 整页集成：真实 Provider 加刷选交互，机器忙（并行编译、其余测试）时会超过默认的 5 s
vi.setConfig({ testTimeout: 20_000 });

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;
let backend: Record<string, Handler> = {};

/** 本次渲染能用到的全部桌面命令；handlers 覆盖或补充写操作。返回值复制一份，与 IPC 序列化一致。 */
function mockSite(candidate: WorkspaceView, handlers: Record<string, Handler> = {}) {
  backend = {
    storage_preferences_get: () => ({ legacyImported: true }),
    storage_preferences_update: ({ patch }) => ({ legacyImported: true, ...(patch as object) }),
    recipe_list: () => ({ recipes: [candidate.plan ? burstSummary(candidate.workspace.doc as BurstRecipeDoc) : summary(candidate)], errors: [], unavailable: [] }),
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
    recipe_capture_list: () => [],
    recipe_capture_get: () => null,
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

const searchBandEnds = () => [...screen.getByLabelText("搜索带边界").querySelectorAll("polyline")].map(edge => {
  const points = edge.getAttribute("points")!.split(" ");
  return `${points[0]} ${points[points.length - 1]}`;
});


// ───────────── 段配方：每台相机一条走廊 ─────────────

type Inspect = NonNullable<WorkspaceView["workspace"]["doc"]["inspect"]>[number];
const CAM2 = "CAM-2 · 示教帧";
const FRAME2 = "S-CAM-2-frame";
const bend: [number, number][] = [[12, 20], [32, 20], [52, 20], [52, 40], [52, 60]];

/** 段配方候选：CAM-2 已选示教帧；edit 改写 CAM-2 的走廊 / 当量（默认已有一条走廊与 0.1 mm/px） */
function corridorCandidate(edit: (inspect: Inspect, view: WorkspaceView) => void = () => {}) {
  const candidate = burstWorkspaceView(burstRecipe({ id: "A" }));
  edit(candidate.workspace.doc.inspect![1], candidate);
  return candidate;
}
/** CAM-2 还没有走廊与当量 */
const untaught = (inspect: Inspect) => { inspect.corridor = []; inspect.mmPerPx = null; };
/** CAM-2 已示教：一条有转折的走廊，0.2 mm/px */
const taught = (inspect: Inspect, view: WorkspaceView) => { inspect.corridor = structuredClone(bend); inspect.mmPerPx = .2; Object.assign(view.plan!.cameraTeaching[1], { saved: true, mmPerPx: .2 }); };
/** workspace_save_doc 的后端替身：整份配方写回并推进修订 */
const savingDoc = (candidate: WorkspaceView, gate?: Promise<void>): Handler => async ({ revision, doc }) => {
  await gate;
  const response = structuredClone(candidate);
  response.workspace.doc = doc as WorkspaceView["workspace"]["doc"];
  response.workspace.revision = (revision as number) + 1;
  return response;
};

it("本相机检测参数覆盖后恢复配方默认 → 搜索带按配方默认搜索宽度重画", async () => {
  const candidate = corridorCandidate((inspect, view) => {
    inspect.corridor = [[10, 10], [20, 10]]; inspect.mmPerPx = .1; inspect.detect = { searchMm: 4, polarity: "dark", widthRange: [1, 5] };
    view.workspace.doc.detect.searchMm = 15;
  });
  mockSite(candidate);
  renderWorkspace("/recipe/teach?camera=CAM-2");
  await screen.findByRole("img", { name: CAM2 });
  expect(searchBandEnds()).toEqual(["10,50 20,50", "10,-30 20,-30"]);
  fireEvent.click(screen.getByText("检测参数", { exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "恢复配方默认检测参数" }));
  expect(searchBandEnds()).toEqual(["10,160 20,160", "10,-140 20,-140"]);
});

it.each(["", "0"])("像素当量改为「%s」 → 隐藏搜索带", async value => {
  mockSite(corridorCandidate(inspect => { inspect.corridor = [[10, 10], [20, 10]]; }));
  renderWorkspace("/recipe/teach?camera=CAM-2");
  await screen.findByRole("img", { name: CAM2 });
  expect(screen.getByLabelText("搜索带边界")).toBeInTheDocument();
  fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value } });
  expect(screen.queryByLabelText("搜索带边界")).not.toBeInTheDocument();
});

it("手工落点并自动保存 → 保存期间保留缩放与走廊、不能再改点，保存后草稿并入配方且清空", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = corridorCandidate(untaught);
  const saving = deferred<void>();
  mockSite(candidate, { workspace_save_doc: savingDoc(candidate, saving.promise) });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2");
  const image = await screen.findByRole("img", { name: CAM2 });
  await user.click(screen.getByRole("button", { name: "放大原图" }));
  const transform = image.querySelector("g")!.getAttribute("transform");
  for (const [clientX, clientY] of [[10, 10], [20, 20]]) {
    fireEvent.pointerDown(image, { clientX, clientY });
    fireEvent.pointerUp(image);
  }
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
  expect(screen.getByRole("img", { name: CAM2 })).toBe(image);
  expect(image.querySelector("g")).toHaveAttribute("transform", transform);
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cx", "20");
  // 整份配方只改了这台相机：别的相机的走廊原样提交
  const [{ doc }] = calls("workspace_save_doc") as { doc: WorkspaceView["workspace"]["doc"] }[];
  expect(doc.inspect![1].corridor).toEqual([[10, 10], [20, 20]]);
  expect([doc.inspect![0], doc.inspect![2]]).toEqual([candidate.workspace.doc.inspect![0], candidate.workspace.doc.inspect![2]]);
});

it("沿胶路刷选后自动提取走廊 → 先保存草稿，再按刷选引导、笔刷半径与点距在示教帧上提取一次", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = corridorCandidate(untaught);
  mockSite(candidate, {
    workspace_save_doc: savingDoc(candidate),
    workspace_extract_centerline: () => ({ path: bend, message: "走廊已提取" }),
  });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2");
  const image = await screen.findByRole("img", { name: CAM2 });
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  expect(screen.getByRole("button", { name: "沿胶路刷选" })).toHaveAttribute("aria-pressed", "true");
  expect(calls("workspace_extract_centerline")).toEqual([]);
  fireEvent.pointerDown(image, { clientX: 10.2, clientY: 12.4 });
  fireEvent.pointerMove(image, { clientX: 50.8, clientY: 42.7 });
  fireEvent.pointerUp(image);
  fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".2" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "走廊点间距" }), { target: { value: "10" } });
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  expect(await screen.findByLabelText("中线点 5")).toBeVisible();
  expect(screen.queryByLabelText("刷选胶路区域")).not.toBeInTheDocument();
  const [saved] = calls("workspace_save_doc") as { id: string; revision: number; doc: WorkspaceView["workspace"]["doc"] }[];
  expect([saved.id, saved.revision, saved.doc.inspect![1]]).toEqual(["A", 3, { ...candidate.workspace.doc.inspect![1], corridor: [], mmPerPx: .2, detect: null }]);
  // 段配方没有拍照点：k 传这台相机在计划里的下标（CAM-2 = 1），图像是已冻结的示教帧
  expect(calls("workspace_extract_centerline")).toEqual([{ id: "A", revision: 4, k: 1, imageId: FRAME2, roi: [0, 0, 100, 60],
    guide: { points: [[10.2, 12.4], [50.8, 42.7]], radiusPx: 60 }, pointSpacingPx: 10 }]);
});

it("按新点距重采样当前走廊 → 只留转折点，走廊变成待保存", async () => {
  mockSite(corridorCandidate(taught));
  const user = renderWorkspace("/recipe/teach?camera=CAM-2");
  await screen.findByRole("img", { name: CAM2 });
  expect(screen.getByRole("button", { name: "保存走廊" })).toBeDisabled();
  fireEvent.change(screen.getByRole("spinbutton", { name: "走廊点间距" }), { target: { value: "80" } });
  await user.click(screen.getByRole("button", { name: "应用到当前走廊" }));
  expect(screen.getByLabelText("中线点 1")).toHaveAttribute("cx", "12");
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cx", "52");
  expect(screen.getByLabelText("中线点 2")).toHaveAttribute("cy", "20");
  expect(screen.getByLabelText("中线点 3")).toHaveAttribute("cx", "52");
  expect(screen.getByLabelText("中线点 3")).toHaveAttribute("cy", "60");
  expect(screen.queryByLabelText("中线点 4")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "保存走廊" })).toBeEnabled();
});

it("自动提取失败后按笔划生成 → 定位原因并保留原线，人工草稿保留转弯且走廊待保存", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = corridorCandidate(taught);
  const failure = { point: [40, 20] as [number, number], reason: "discontinuous_edges", message: "此处胶路宽度突变，请检查亮边与阴影。" };
  mockSite(candidate, { workspace_extract_centerline: () => ({ path: [], message: failure.message, failure }) });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2");
  const image = await screen.findByRole("img", { name: CAM2 });
  const originalPoints = bend.map(point => point.join(",")).join(" ");
  expect(screen.getByRole("button", { name: "保存走廊" })).toBeDisabled();
  await user.click(screen.getByRole("button", { name: "沿胶路刷选" }));
  fireEvent.change(screen.getByRole("spinbutton", { name: "走廊点间距" }), { target: { value: "80" } });
  fireEvent.pointerDown(image, { clientX: 10, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 50 });
  fireEvent.pointerUp(image);
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  expect(await screen.findByLabelText("中线提取失败位置")).toBeVisible();
  expect(screen.getByText(failure.message, { selector: '[role="status"]' })).toBeVisible();
  expect(image.querySelector(`polyline[points="${originalPoints}"]`)).not.toBeNull();
  expect(screen.getByLabelText("刷选胶路区域").querySelector("polyline")).toHaveAttribute("points", "10,20 40,20 40,50");
  const slider = screen.getByRole("slider", { name: "拖动调整笔刷宽度" });
  for (let i = 0; i < 3; i++) fireEvent.keyDown(slider, { key: "PageDown" });
  expect(slider).toHaveAttribute("aria-valuenow", "60");
  expect(screen.queryByLabelText("中线提取失败位置")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  await screen.findByLabelText("中线提取失败位置");
  await user.click(screen.getByRole("button", { name: "笔划直接生成走廊" }));
  expect(screen.queryByLabelText("中线提取失败位置")).not.toBeInTheDocument();
  for (const [index, x, y] of [[1, 10, 20], [2, 40, 20], [3, 40, 50]]) {
    expect(screen.getByLabelText(`中线点 ${index}`)).toHaveAttribute("cx", String(x));
    expect(screen.getByLabelText(`中线点 ${index}`)).toHaveAttribute("cy", String(y));
  }
  expect(screen.queryByLabelText("中线点 4")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "保存走廊" })).toBeEnabled();
});

function extractionFailure(reason: string) {
  const failure = { point: [40, 20] as [number, number], reason, message: `提取失败：${reason}` };
  return { path: [], message: failure.message, failure };
}

async function brushOnTaughtCamera(extract: Handler) {
  stubSvgPointer({ pointerEvent: true });
  mockSite(corridorCandidate(taught), { workspace_extract_centerline: extract });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2");
  const image = await screen.findByRole("img", { name: CAM2 });
  await user.click(screen.getByRole("button", { name: "沿胶路刷选" }));
  fireEvent.pointerDown(image, { clientX: 10, clientY: 20 });
  fireEvent.pointerMove(image, { clientX: 40, clientY: 20 });
  fireEvent.pointerUp(image);
  const extractButton = screen.getByRole("button", { name: "自动提取走廊" }), stroke = screen.getByRole("button", { name: "笔划直接生成走廊" });
  const failOnce = async () => { await user.click(extractButton); await screen.findByLabelText("中线提取失败位置"); };
  const primaries = () => ["自动提取走廊", "笔划直接生成走廊", "保存走廊", "下一台相机"]
    .map(name => screen.getByRole("button", { name })).filter(button => button.dataset.variant === "filled");
  return { user, stroke, failOnce, primaries };
}

it("已示教的相机刷选提取连续两次找不到连续边缘 → 先提示加大笔刷，再把笔划直接生成走廊设为唯一主操作", async () => {
  const { stroke, failOnce, primaries } = await brushOnTaughtCamera(() => extractionFailure("discontinuity"));
  expect(primaries()).toEqual([screen.getByRole("button", { name: "下一台相机" })]);
  await failOnce();
  expect(screen.getByText(/可加大笔刷宽度（当前 120 px/)).toBeVisible();
  expect(stroke).toHaveAttribute("data-variant", "tonal");
  const slider = screen.getByRole("slider", { name: "拖动调整笔刷宽度" });
  for (let i = 0; i < 3; i++) fireEvent.keyDown(slider, { key: "PageUp" });
  expect(slider).toHaveAttribute("aria-valuenow", "180");
  await failOnce();
  expect(screen.getByText(/请点「笔划直接生成走廊」/)).toBeVisible();
  expect(primaries()).toEqual([stroke]);
});

it("刷选后切回编辑点再自动提取 → 仍用上次刷选的选区与笔刷，不要求重新刷选", async () => {
  const { user } = await brushOnTaughtCamera(() => ({ path: [[12, 20], [38, 20]], message: "" }));
  await user.click(screen.getByRole("button", { name: "编辑点" }));
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  await waitFor(() => expect(calls("workspace_extract_centerline")).toEqual([{ id: "A", revision: 3, k: 1, imageId: FRAME2, roi: [0, 0, 100, 60],
    guide: { points: [[10, 20], [40, 20]], radiusPx: 60 }, pointSpacingPx: 20 }]));
  expect(screen.getByRole("button", { name: "编辑点" })).toHaveAttribute("aria-pressed", "true");
});

it("两次找不到连续边缘之间夹一次多条候选失败 → 不算连续，第二次仍只提示加大笔刷", async () => {
  const { stroke, failOnce } = await brushOnTaughtCamera(replies(extractionFailure("discontinuity"), extractionFailure("ambiguous"), extractionFailure("missing_edges")));
  await failOnce();
  await failOnce();
  await failOnce();
  expect(screen.getByText(/可加大笔刷宽度/)).toBeVisible();
  expect(screen.queryByText(/请点「笔划直接生成走廊」/)).not.toBeInTheDocument();
  expect(stroke).toHaveAttribute("data-variant", "tonal");
});

// 防的回归：选帧后后端冻结的是别的帧，或页面仍停在选帧面板、示教帧显示不出来
it("为未选示教帧的相机从采集轮次选一帧 → 按所选轮次、帧组冻结，选帧面板收起并显示这一帧", async () => {
  const candidate = corridorCandidate();
  const response = structuredClone(candidate);
  response.workspace.revision++;
  response.plan!.cameraTeaching[2].frame = { ...candidate.plan!.cameraTeaching[1].frame!, id: "S-CAM-3-frame", camera: "CAM-3" };
  const picture = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
  mockSite(candidate, {
    recipe_capture_list: () => [{ roundId: "round-1", recipeId: "A", cameraIds: ["CAM-1", "CAM-2", "CAM-3"], plannedCount: 12, receivedCount: 12, state: "complete", createdAt: 1, error: null }],
    recipe_capture_get: () => captureRoundV2({ recipeId: "A" }),
    workspace_teach_camera: () => response,
  });
  const frameImage = vi.spyOn(workspaceApi, "captureFrameImage").mockResolvedValue(picture);
  const user = renderWorkspace("/recipe/teach?camera=CAM-3&round=round-1");
  await screen.findByRole("region", { name: "示教帧选择" });
  fireEvent.change(await screen.findByRole("spinbutton", { name: "帧组序号" }), { target: { value: "7" } });
  await waitFor(() => expect(frameImage).toHaveBeenLastCalledWith("round-1", "CAM-3", 7));
  const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-3" });
  await waitFor(() => expect(choose).toBeEnabled());
  await user.click(choose);
  expect(calls("workspace_teach_camera")).toEqual([{ id: "A", revision: 3, camera: "CAM-3", round: "round-1", seq: 7, draft: { corridor: [[10, 30], [90, 30]], mmPerPx: .1, detect: null } }]);
  expect(await screen.findByRole("img", { name: "CAM-3 · 示教帧" })).toBeVisible();
  expect(screen.queryByRole("region", { name: "示教帧选择" })).not.toBeInTheDocument();
});


// ───────────── 旧拍照点配方：仍保留的总览、独立验证样本与步骤导航 ─────────────

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

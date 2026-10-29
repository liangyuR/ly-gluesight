import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent, { PointerEventsCheckLevel } from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import { SnackbarProvider } from "qing-steel";
import { workspaceApi } from "../src/features/workspace/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView, captureRoundV2, deferred, siteStatus, snapshot, type BurstRecipeDoc } from "./fixtures";
import { stubSvgPointer } from "./svg-pointer";
// 只替身桌面后端：invoke 按命令名分派到 backend 夹具，其余全是真实模块。
// 例外：原图解码要用 canvas（jsdom 没有），workspaceApi.image 直接给出解码结果。
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
    storage_preferences_get: () => ({}),
    storage_preferences_update: ({ patch }) => ({ ...(patch as object) }),
    recipe_list: () => ({ recipes: [burstSummary(candidate.workspace.doc as BurstRecipeDoc)], errors: [], unavailable: [] }),
    workspace_list: () => [candidate.workspace],
    workspace_get: () => candidate,
    camera_rig_config: () => [{ id: "CAM-1", name: "拼接设备", source: "sim", viewCount: 3, serial: "DEVICE",
      triggerSource: "Line0", triggerActivation: "LevelHigh", frameRate: 10, triggerDelayUs: 0, debouncerUs: 0, strobe: false, chunk: true }],
    camera_rig_status: () => [{ id: "CAM-1", ready: true }],
    vision_calib_info: () => null,
    plc_get_status: () => ({ state: "connected", message: "PLC 已连接" }),
    site_status: () => siteStatus(),
    cycle_snapshot: () => snapshot(),
    cycle_logs: () => [],
    history_query: () => ({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } }),
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

const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args);

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
    <Route path="/recipe/validation" element={<ValidationPage />} />
  </Routes></WorkspaceProvider></SnackbarProvider></MemoryRouter>;
  render(strict ? <StrictMode>{workspace}</StrictMode> : workspace);
  // jsdom 没有样式表，逐级检查 pointer-events 只是空耗
  return userEvent.setup({ advanceTimers: vi.advanceTimersByTime, pointerEventsCheck: PointerEventsCheckLevel.Never });
}

// ───────────── 段配方：每段每台检测相机一条走廊 ─────────────

type Inspect = NonNullable<WorkspaceView["workspace"]["doc"]["inspect"]>[number];
const CAM2 = "CAM-2 · 段 1 · 示教帧";
const FRAME2 = "S-CAM-2-frame";
const bend: [number, number][] = [[12, 20], [32, 20], [52, 20], [52, 40], [52, 60]];

/** 段配方候选：CAM-2 段 1 已选示教帧、还没画走廊（相机 0.1 mm/px）；edit 改写 CAM-2 的配方项与示教状态 */
function corridorCandidate(edit: (inspect: Inspect, view: WorkspaceView) => void = () => {}) {
  const candidate = burstWorkspaceView(burstRecipe({ id: "A" }));
  edit(candidate.workspace.doc.inspect![1], candidate);
  return candidate;
}
/** 这些件已确认分段边界：12 个帧组，段 1 为帧组 1–6 */
const confirmed = (view: WorkspaceView, ...rounds: string[]) => {
  view.workspace.captureReview = { rounds: Object.fromEntries(rounds.map(round => [round, { groupCount: 12, cuts: [6, 9], status: "confirmed" as const, excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null }])) };
};
/** workspace_save_doc 的后端替身：整份配方写回并推进修订 */
const savingDoc = (candidate: WorkspaceView, gate?: Promise<void>): Handler => async ({ revision, doc }) => {
  await gate;
  const response = structuredClone(candidate);
  response.workspace.doc = doc as WorkspaceView["workspace"]["doc"];
  response.workspace.revision = (revision as number) + 1;
  return response;
};

it("保存期间继续加点 → 不锁编辑、缩放与走廊不重置，保存回来后新点保留并按新修订再保存一次", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = corridorCandidate();
  const saving = deferred<void>();
  mockSite(candidate, { workspace_save_doc: savingDoc(candidate, saving.promise) });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2&segment=1");
  const image = await screen.findByRole("img", { name: CAM2 });
  await user.click(screen.getByRole("button", { name: "放大原图" }));
  const transform = image.querySelector("g")!.getAttribute("transform");
  for (const [clientX, clientY] of [[10, 10], [40, 40]]) {
    fireEvent.pointerDown(image, { clientX, clientY });
    fireEvent.pointerUp(image);
  }
  act(() => { vi.advanceTimersByTime(900); }); // 自动保存防抖到期，保存请求挂起
  expect(screen.getByText("保存中")).toBeVisible();
  expect(screen.getByRole("button", { name: "点柄" })).toBeEnabled();
  fireEvent.pointerDown(image, { clientX: 30, clientY: 30 });
  fireEvent.pointerUp(image);
  expect(screen.getByLabelText("中线点 3")).toBeInTheDocument();
  await act(async () => saving.resolve(undefined));
  expect(screen.getByLabelText("中线点 3")).toBeInTheDocument();
  act(() => { vi.advanceTimersByTime(900); });
  expect(await screen.findByText("草稿已保存")).toBeVisible();
  expect(screen.getByRole("img", { name: CAM2 })).toBe(image);
  expect(image.querySelector("g")).toHaveAttribute("transform", transform);
  const saves = calls("workspace_save_doc") as { revision: number; doc: WorkspaceView["workspace"]["doc"] }[];
  expect(saves.map(s => s.doc.inspect![1].segments)).toEqual([
    [{ segment: 1, corridor: [[10, 10], [40, 40]] }],
    [{ segment: 1, corridor: [[10, 10], [40, 40], [30, 30]] }],
  ]);
  expect(saves[1].revision).toBe(saves[0].revision + 1);
  // 整份配方只改了这台相机这一段：相机参数与别的相机原样提交
  expect({ ...saves[0].doc.inspect![1], segments: [] }).toEqual({ ...candidate.workspace.doc.inspect![1], segments: [] });
  expect([saves[0].doc.inspect![0], saves[0].doc.inspect![2]]).toEqual([candidate.workspace.doc.inspect![0], candidate.workspace.doc.inspect![2]]);
});

it("保存期间改相机参数 → 保存结果回来不覆盖新值，按新修订再保存一次", async () => {
  const candidate = corridorCandidate();
  const saving = deferred<void>();
  mockSite(candidate, { workspace_save_doc: savingDoc(candidate, saving.promise) });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2&segment=1");
  await user.click(await screen.findByRole("button", { name: "选择相机 CAM-2" }));
  const scale = await screen.findByRole("spinbutton", { name: /^像素当量/ });
  fireEvent.change(scale, { target: { value: "0.2" } });
  act(() => { vi.advanceTimersByTime(900); });
  expect(screen.getByText("保存中")).toBeVisible();
  fireEvent.change(screen.getByRole("spinbutton", { name: /^像素当量/ }), { target: { value: "0.3" } });
  await act(async () => saving.resolve(undefined));
  expect(screen.getByRole("spinbutton", { name: /^像素当量/ })).toHaveValue(0.3);
  act(() => { vi.advanceTimersByTime(900); });
  expect(await screen.findByText("草稿已保存")).toBeVisible();
  const saves = calls("workspace_save_doc") as { revision: number; doc: WorkspaceView["workspace"]["doc"] }[];
  expect(saves.map(s => s.doc.inspect![1].mmPerPx)).toEqual([0.2, 0.3]);
  expect(saves[1].revision).toBe(saves[0].revision + 1);
});

it("沿胶路刷选后自动提取走廊 → 先保存草稿，再按刷选引导、笔刷半径与点距在这一段的示教帧上提取一次", async () => {
  stubSvgPointer({ pointerEvent: true });
  const candidate = corridorCandidate();
  mockSite(candidate, {
    workspace_save_doc: savingDoc(candidate),
    workspace_extract_corridor: () => ({ path: bend, message: "走廊已提取" }),
  });
  const user = renderWorkspace("/recipe/teach?camera=CAM-2&segment=1");
  const image = await screen.findByRole("img", { name: CAM2 });
  fireEvent.pointerDown(image, { clientX: 10, clientY: 10 });
  fireEvent.pointerUp(image);
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  expect(screen.getByRole("button", { name: "沿胶路刷选" })).toHaveAttribute("aria-pressed", "true");
  expect(calls("workspace_extract_corridor")).toEqual([]);
  fireEvent.pointerDown(image, { clientX: 10.2, clientY: 12.4 });
  fireEvent.pointerMove(image, { clientX: 50.8, clientY: 42.7 });
  fireEvent.pointerUp(image);
  fireEvent.change(screen.getByRole("spinbutton", { name: "走廊点间距" }), { target: { value: "10" } });
  await user.click(screen.getByRole("button", { name: "自动提取走廊" }));
  expect(await screen.findByLabelText("中线点 5")).toBeVisible();
  expect(screen.queryByLabelText("刷选胶路区域")).not.toBeInTheDocument();
  const [saved] = calls("workspace_save_doc") as { id: string; revision: number; doc: WorkspaceView["workspace"]["doc"] }[];
  expect([saved.id, saved.revision, saved.doc.inspect![1]]).toEqual(["A", 3, { ...candidate.workspace.doc.inspect![1], segments: [{ segment: 1, corridor: [[10, 10]] }] }]);
  // 按相机 id 在这一段冻结的示教帧上提取（workspace_extract_corridor）
  expect(calls("workspace_extract_corridor")).toEqual([{ id: "A", revision: 4, camera: "CAM-2", imageId: FRAME2, roi: [0, 0, 100, 60],
    guide: { points: [[10.2, 12.4], [50.8, 42.7]], radiusPx: 60 }, pointSpacingPx: 10 }]);
});

// 防的回归（RB-1）：示教帧可以从示教件以外的轮次选取；重新选帧确认后仍显示旧帧
it("段已有示教帧时重新选帧 → 只在示教件这一段内选帧，确认后按示教件提交并显示新帧", async () => {
  const candidate = corridorCandidate();
  confirmed(candidate, "round-1");
  const response = structuredClone(candidate);
  response.workspace.revision++;
  const frame = { ...candidate.plan!.cameraTeaching[1].segments[0].frame, id: "S-CAM-2-new-frame", source: "capture:round-1" };
  response.plan!.cameraTeaching[1].segments[0].frame = frame;
  response.workspace.cameraTeaching = response.workspace.cameraTeaching.map(item => item.camera === "CAM-2" && item.segment === 1 ? { camera: "CAM-2", frame, round: "round-1", seq: 2, segment: 1 } : item);
  const picture = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
  mockSite(candidate, {
    recipe_capture_get: ({ roundId }) => captureRoundV2({ recipeId: "A", roundId: roundId as string }),
    workspace_teach_segment_corridor: () => response,
  });
  const frameImage = vi.spyOn(workspaceApi, "captureFrameImage").mockResolvedValue(picture);
  const user = renderWorkspace("/recipe/teach?camera=CAM-2&segment=1");
  await screen.findByRole("img", { name: CAM2 });
  await user.click(screen.getByRole("button", { name: "重新选帧" }));
  await screen.findByRole("region", { name: "示教帧选择" });
  expect(await screen.findByRole("img", { name: "CAM-2 · 帧组 4 · 候选示教帧" })).toBeVisible();
  expect(calls("recipe_capture_get")).toEqual([{ roundId: "round-1" }]);
  await user.click(await screen.findByRole("button", { name: "帧组 2 · CAM-2" }));
  await waitFor(() => expect(frameImage).toHaveBeenLastCalledWith("round-1", "CAM-2", 2));
  const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-2 · 段 1" });
  await waitFor(() => expect(choose).toBeEnabled());
  await user.click(choose);
  expect(calls("workspace_teach_segment_corridor")).toEqual([{ id: "A", revision: 3, camera: "CAM-2", segment: 1, round: "round-1", seq: 2 }]);
  expect(await screen.findByRole("img", { name: CAM2 })).toBeVisible();
  expect(screen.getByText("capture:round-1 · S-CAM-2-new-frame")).toBeVisible();
  expect(screen.queryByRole("region", { name: "示教帧选择" })).not.toBeInTheDocument();
});

import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { SnackbarProvider } from "qing-steel";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import type { CameraConfig } from "../src/features/camera/types";
import { desktopAvailable } from "../src/lib/desktop";
import { subscribe } from "../src/features/plc/api";
import type { WorkspaceCatalogEntry, WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred, workspaceCatalogEntry } from "./fixtures";
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDoc: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn() }));
let changed: (id: string) => void;
beforeEach(() => {
  const a = burstView(), b = burstView("B");
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [burstSummary(a.workspace.doc as ReturnType<typeof burstRecipe>), burstSummary(b.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] });
  vi.mocked(workspaceApi.list).mockResolvedValue([a.workspace, b.workspace].map(workspaceCatalogEntry));
  vi.mocked(workspaceApi.get).mockImplementation(async id => burstView(id));
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(a);
  vi.mocked(recipeApi.preview).mockResolvedValue(a.layout);
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  vi.mocked(subscribe).mockImplementation((event, callback) => {
    if (event === "workspace://changed") changed = callback as (id: string) => void;
    return () => {};
  });
});
afterEach(() => { vi.useRealTimers(); });
const burstView = (id = "A") => burstWorkspaceView(burstRecipe({ id }));
/** 段配方候选：走廊草稿按“相机#段号”存 */
async function openBurst() {
  const view = burstView();
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [burstSummary(view.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] });
  vi.mocked(workspaceApi.list).mockResolvedValue([workspaceCatalogEntry(view.workspace)]);
  vi.mocked(workspaceApi.get).mockImplementation(async () => burstView());
  vi.mocked(recipeApi.preview).mockResolvedValue(view.layout);
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(view);
  return open();
}
async function open() {
  const wrapper = ({ children }: { children: ReactNode }) => <SnackbarProvider><WorkspaceProvider>{children}</WorkspaceProvider></SnackbarProvider>;
  const hook = renderHook(() => useWorkspace(), { wrapper });
  // 不再自动打开配方：配方页按地址打开，这里直接打开 A
  await act(() => hook.result.current.select("A"));
  await waitFor(() => expect(hook.result.current.data?.workspace.doc.id).toBe("A"));
  return hook;
}

describe("配方库刷新", () => {
  it("后台读取期间操作再请求刷新 → 只提交后续目录，显式等待随目录完成", async () => {
    const { result } = await open();
    await waitFor(() => expect(result.current.drafts).toHaveLength(2));
    const previous = result.current.drafts;
    const oldList = deferred<WorkspaceCatalogEntry[]>(), newList = deferred<WorkspaceCatalogEntry[]>();
    const load = vi.mocked(workspaceApi.list).mockReturnValueOnce(oldList.promise).mockReturnValueOnce(newList.promise);
    await act(async () => { changed("B"); });
    const callsDuringRead = load.mock.calls.length;

    let finished = false;
    let refreshed!: Promise<void>;
    await act(async () => {
      refreshed = result.current.reloadList().then(() => { finished = true; });
      changed("B");
      changed("B");
    });
    const oldCandidate = { ...previous[1], name: "旧候选名称" };
    await act(async () => oldList.resolve([previous[0], oldCandidate]));
    expect(result.current.drafts[1].name).toBe(previous[1].name);
    expect(finished).toBe(false);
    expect(load).toHaveBeenCalledTimes(callsDuringRead + 1);

    await act(async () => {
      newList.resolve([previous[0]]);
      await refreshed;
    });
    expect(result.current.drafts.map(draft => draft.id)).toEqual(["A"]);
    expect(finished).toBe(true);
  });

  it("旧读取失败且已有后续刷新 → 跳过旧错误，后续失败仍可重试", async () => {
    const { result } = await open();
    await waitFor(() => expect(result.current.drafts).toHaveLength(2));
    const oldList = deferred<WorkspaceCatalogEntry[]>(), newList = deferred<WorkspaceCatalogEntry[]>();
    const oldCameras = deferred<CameraConfig[]>();
    const load = vi.mocked(workspaceApi.list).mockReturnValueOnce(oldList.promise).mockReturnValueOnce(newList.promise);
    vi.mocked(cameraApi.rigConfig).mockReturnValueOnce(oldCameras.promise);
    await act(async () => { changed("B"); });
    const callsDuringRead = load.mock.calls.length;

    let refreshed!: Promise<void>;
    await act(async () => {
      refreshed = result.current.reloadList();
      void refreshed.catch(() => {});
    });
    await act(async () => oldList.reject(new Error("旧目录读取失败")));
    expect(result.current.error).toBe("");
    expect(load).toHaveBeenCalledTimes(callsDuringRead);
    await act(async () => oldCameras.resolve([]));

    await act(async () => {
      newList.reject(new Error("当前目录读取失败"));
      await expect(refreshed).rejects.toThrow("当前目录读取失败");
    });
    expect(result.current.drafts.map(draft => draft.id)).toEqual(["A", "B"]);
    expect(result.current.error).toBe("当前目录读取失败");
    load.mockResolvedValueOnce([result.current.drafts[0]]);
    await act(() => result.current.reloadList());
    expect(result.current.drafts.map(draft => draft.id)).toEqual(["A"]);
    expect(result.current.error).toBe("");
  });
});

describe("候选工作台的主流程保护", () => {

  it("刷新请求发出后用户开始编辑，也不能覆盖草稿", async () => {
    const { result } = await open(), refresh = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockReturnValue(refresh.promise);
    act(() => changed("A"));
    act(() => result.current.setDoc({ ...result.current.doc!, name: "刷新期间编辑" }));
    await act(async () => refresh.resolve(burstView()));
    expect(result.current.doc?.name).toBe("刷新期间编辑"); expect(result.current.dirty).toBe(true);
  });

  it("操作途中离开再选回同一配方，旧操作结果也不能覆盖新选择", async () => {
    const { result } = await open(), operation = deferred<WorkspaceView>();
    act(() => { void result.current.act(() => operation.promise); });
    act(() => result.current.clearSelection());
    const latest = burstView(); latest.workspace.revision = 10;
    vi.mocked(workspaceApi.get).mockResolvedValueOnce(latest);
    await act(() => result.current.select("A"));
    await act(async () => operation.resolve(burstView()));
    expect(result.current.data?.workspace.revision).toBe(10);
  });

  it("某段换了示教帧 → 只清除这一段的旧走廊草稿，同相机别的段的草稿保留且仍算未保存",async()=>{
    const {result}=await openBurst();
    act(()=>{result.current.setCorridorDraft("CAM-1#1",{corridor:[[10,10],[24,10]],mmPerPx:.1,window:null});result.current.setCorridorDraft("CAM-1#2",{corridor:[[30,10],[54,10]],mmPerPx:.1,window:null});});
    const next=burstView();
    const teaching=next.plan!.cameraTeaching[0].segments[0];
    teaching.frame={...teaching.frame,id:"A-CAM-1-other-frame"};
    await act(()=>result.current.act(async()=>next));
    expect(result.current.corridorDrafts["CAM-1#1"]).toBeUndefined();expect(result.current.corridorDrafts["CAM-1#2"]).toEqual({corridor:[[30,10],[54,10]],mmPerPx:.1,window:null});
    expect(result.current.frameDirty).toBe(true);
  });

  it("保存失败保留走廊草稿，重试成功后才显示已保存；草稿并入这台相机这一段的走廊整份提交",async()=>{
    const {result}=await openBurst();
    const { revision, instanceId } = result.current.data!.workspace;
    const draft={corridor:[[10,10],[40,10]] as [number,number][],mmPerPx:.1,window:null};
    vi.mocked(workspaceApi.saveDoc).mockRejectedValueOnce(new Error("磁盘不可写"));
    vi.useFakeTimers();
    act(()=>result.current.setCorridorDraft("CAM-2#1",draft));
    await act(()=>vi.advanceTimersByTimeAsync(900));
    expect(result.current.saveState).toBe("failed");
    expect(result.current.corridorDrafts["CAM-2#1"]).toEqual(draft);
    const base=burstView().workspace.doc;
    const merged={...base,inspect:base.inspect!.map(item=>item.camera==="CAM-2"?{...item,segments:[{segment:1,corridor:draft.corridor}]}:item)};
    expect(workspaceApi.saveDoc).toHaveBeenLastCalledWith("A", revision, instanceId, merged);
    const next=burstView();next.workspace.revision=4;next.workspace.doc=merged;
    vi.mocked(workspaceApi.saveDoc).mockResolvedValue(next);
    act(()=>result.current.retrySave());
    await act(()=>vi.advanceTimersByTimeAsync(900));
    expect(workspaceApi.saveDoc).toHaveBeenLastCalledWith("A", revision, instanceId, merged);
    expect(result.current.saveState).toBe("saved");
    expect(result.current.frameDirty).toBe(false);
  });

  it("自动保存返回不同实例 → 保留草稿，同号候选重新打开后可保存相同修改", async () => {
    const { result } = await openBurst();
    const { revision, instanceId } = result.current.data!.workspace;
    const draft = { corridor: [[10, 10], [40, 10]] as [number, number][], mmPerPx: .1, window: null };
    const reopened = burstView();
    reopened.workspace.instanceId = instanceId + 1;
    const save = vi.mocked(workspaceApi.saveDoc).mockImplementation(async (_, sentRevision, _sentInstanceId, doc) => {
      const next = structuredClone(reopened);
      next.workspace.revision = sentRevision + 1;
      next.workspace.doc = doc;
      return next;
    });
    vi.useFakeTimers();
    act(() => result.current.setCorridorDraft("CAM-2#1", draft));
    await act(() => vi.advanceTimersByTimeAsync(900));
    expect(result.current.saveState).toBe("failed");
    expect(result.current.data?.workspace.instanceId).toBe(instanceId);
    expect(result.current.corridorDrafts["CAM-2#1"]).toEqual(draft);

    vi.mocked(workspaceApi.get).mockResolvedValueOnce(reopened);
    await act(() => result.current.select("A"));
    act(() => result.current.setCorridorDraft("CAM-2#1", draft));
    await act(() => vi.advanceTimersByTimeAsync(900));

    expect(save).toHaveBeenLastCalledWith("A", revision, reopened.workspace.instanceId, expect.any(Object));
    expect(result.current.data?.workspace.doc.inspect![1].segments![0].corridor).toEqual(draft.corridor);
    expect(result.current.saveState).toBe("saved");
  });

  it("夹有站数不足的走廊草稿 → 按输入顺序保存其余草稿并保留不足的段", async () => {
    const { result } = await openBurst();
    const instanceId = result.current.data!.workspace.instanceId;
    const first = { corridor: [[10, 10], [40, 10]] as [number, number][], mmPerPx: .1, window: null };
    const short = { ...first, corridor: [[10, 10], [20, 10]] as [number, number][] };
    const third = { ...first, corridor: [[10, 10], [55, 10]] as [number, number][] };
    const save = vi.mocked(workspaceApi.saveDoc).mockImplementation(async (_, revision, _sentInstanceId, doc) => {
      const next = burstView();
      next.workspace.revision = revision + 1;
      next.workspace.doc = doc;
      return next;
    });
    vi.useFakeTimers();
    act(() => {
      result.current.setCorridorDraft("CAM-1#3", third);
      result.current.setCorridorDraft("CAM-1#2", short);
      result.current.setCorridorDraft("CAM-1#1", first);
    });
    expect(result.current.pendingSave).toBe(true);
    expect(result.current.unsavable).toBe(true);

    await act(() => vi.advanceTimersByTimeAsync(900));
    await act(() => vi.advanceTimersByTimeAsync(900));
    const unchanged = [[10, 30], [90, 30]];
    expect(save.mock.calls.map(([, revision, sentInstanceId, doc]) => [revision, sentInstanceId, doc.inspect![0].segments!.map(segment => segment.corridor)])).toEqual([
      [3, instanceId, [unchanged, unchanged, third.corridor]],
      [4, instanceId, [first.corridor, unchanged, third.corridor]],
    ]);
    expect(result.current.corridorDrafts).toEqual({ "CAM-1#2": short });
    expect(result.current.frameDirty).toBe(true);
    expect(result.current.unsavable).toBe(true);
    expect(result.current.pendingSave).toBe(false);
  });
});

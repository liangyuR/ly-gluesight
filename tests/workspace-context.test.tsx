import { act, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { storageApi } from "../src/lib/storage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { SnackbarProvider } from "qing-steel";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import { subscribe } from "../src/features/plc/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred } from "./fixtures";
vi.mock("../src/lib/storage", () => ({ storageApi: { preferences: async () => ({}), updatePreferences: vi.fn(async () => ({})) } }));
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
  vi.mocked(workspaceApi.list).mockResolvedValue([a.workspace, b.workspace]);
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
  vi.mocked(workspaceApi.list).mockResolvedValue([view.workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async () => burstView());
  vi.mocked(recipeApi.preview).mockResolvedValue(view.layout);
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(view);
  return open();
}
async function open() {
  const wrapper = ({ children }: { children: ReactNode }) => <SnackbarProvider><WorkspaceProvider>{children}</WorkspaceProvider></SnackbarProvider>;
  const hook = renderHook(() => useWorkspace(), { wrapper });
  await waitFor(() => expect(hook.result.current.data?.workspace.doc.id).toBe("A"));
  return hook;
}

describe("候选工作台的主流程保护", () => {

  it("选择仅在当前请求真正接受后返回视图，旧请求返回 null",async()=>{
    const {result}=await open(),old=deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id=>id==="A"?old.promise:Promise.resolve(burstView(id)));
    let oldSelection!:Promise<WorkspaceView|null>;
    act(()=>{oldSelection=result.current.select("A");});
    await act(async()=>{const accepted=await result.current.select("B");expect(accepted?.workspace.doc.id).toBe("B");});
    await act(async()=>old.resolve(burstView()));expect(await oldSelection).toBeNull();
    expect(result.current.doc?.id).toBe("B");
    expect(storageApi.updatePreferences).toHaveBeenLastCalledWith({ lastWorkspace: "B" });
  });

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

  it("操作响应配方不一致时保留当前候选并允许重试", async () => {
    const { result } = await open();
    await act(async () => expect(await result.current.act(async () => burstView("B"), "已保存")).toBeNull());
    expect(result.current.doc?.id).toBe("A"); expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(result.current.error).toContain("候选响应与当前配方不一致");
    await act(async () => expect((await result.current.act(async () => burstView(), "当前配方已保存"))?.workspace.doc.id).toBe("A"));
    expect(result.current.error).toBe(""); expect(screen.getByRole("status")).toHaveTextContent("当前配方已保存");
  });

  it("某段换了示教帧 → 只清除这一段的旧走廊草稿，同相机别的段的草稿保留且仍算未保存",async()=>{
    const {result}=await openBurst();
    act(()=>{result.current.setCorridorDraft("CAM-1#1",{corridor:[[10,10],[24,10]],mmPerPx:.1,detect:null,window:null});result.current.setCorridorDraft("CAM-1#2",{corridor:[[30,10],[54,10]],mmPerPx:.1,detect:null,window:null});});
    const next=burstView();
    const teaching=next.plan!.cameraTeaching[0].segments[0];
    teaching.frame={...teaching.frame,id:"A-CAM-1-other-frame"};
    await act(()=>result.current.act(async()=>next));
    expect(result.current.corridorDrafts["CAM-1#1"]).toBeUndefined();expect(result.current.corridorDrafts["CAM-1#2"]).toEqual({corridor:[[30,10],[54,10]],mmPerPx:.1,detect:null,window:null});
    expect(result.current.frameDirty).toBe(true);
  });

  it("保存失败保留走廊草稿，重试成功后才显示已保存；草稿并入这台相机这一段的走廊整份提交",async()=>{
    const {result}=await openBurst();
    const draft={corridor:[[10,10],[20,10]] as [number,number][],mmPerPx:.1,detect:null,window:null};
    vi.mocked(workspaceApi.saveDoc).mockRejectedValueOnce(new Error("磁盘不可写"));
    vi.useFakeTimers();
    act(()=>result.current.setCorridorDraft("CAM-2#1",draft));
    await act(()=>vi.advanceTimersByTimeAsync(900));
    expect(result.current.saveState).toBe("failed");
    expect(result.current.corridorDrafts["CAM-2#1"]).toEqual(draft);
    const base=burstView().workspace.doc;
    const merged={...base,inspect:base.inspect!.map(item=>item.camera==="CAM-2"?{...item,segments:[{segment:1,corridor:draft.corridor}]}:item)};
    expect(workspaceApi.saveDoc).toHaveBeenLastCalledWith("A",3,merged);
    const next=burstView();next.workspace.revision=4;next.workspace.doc=merged;
    vi.mocked(workspaceApi.saveDoc).mockResolvedValue(next);
    act(()=>result.current.retrySave());
    await act(()=>vi.advanceTimersByTimeAsync(900));
    expect(result.current.saveState).toBe("saved");
    expect(result.current.frameDirty).toBe(false);
  });
});

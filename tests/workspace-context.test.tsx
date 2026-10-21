import { act, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { storageApi } from "../src/lib/storage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { SnackbarProvider } from "qing-steel";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import { subscribe } from "../src/features/plc/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { deferred, summary, workspaceView } from "./fixtures";

vi.mock("../src/lib/storage", () => ({ storageApi: { preferences: async () => ({}), updatePreferences: vi.fn(async () => ({})) } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDraft: vi.fn(), saveDoc: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn() }));
let changed: (id: string) => void;
let calibrationChanged: () => void;
beforeEach(() => {
  const a = workspaceView(), b = workspaceView("B");
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [summary(a), summary(b)], errors: [], unavailable: [] });
  vi.mocked(workspaceApi.list).mockResolvedValue([a.workspace, b.workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async id => workspaceView(id));
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(a);
  vi.mocked(workspaceApi.saveDraft).mockReset().mockResolvedValue(a);
  vi.mocked(recipeApi.preview).mockResolvedValue(a.layout);
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  vi.mocked(subscribe).mockImplementation((event, callback) => {
    if (event === "workspace://changed") changed = callback as (id: string) => void;
    if (event === "calibration://changed") calibrationChanged = callback as () => void;
    return () => {};
  });
});
async function open() {
  const wrapper = ({ children }: { children: ReactNode }) => <SnackbarProvider><WorkspaceProvider>{children}</WorkspaceProvider></SnackbarProvider>;
  const hook = renderHook(() => useWorkspace(), { wrapper });
  await waitFor(() => expect(hook.result.current.data?.workspace.doc.id).toBe("A"));
  return hook;
}

describe("候选工作台的主流程保护", () => {
  it("配置草稿不被环境刷新覆盖，保存后补读最新修订", async () => {
    const { result } = await open();
    const draft = { ...result.current.doc!, name: "保留用户编辑" };
    act(() => result.current.setDoc(draft));
    const calls = vi.mocked(workspaceApi.get).mock.calls.length;
    await act(async () => calibrationChanged());
    expect(workspaceApi.get).toHaveBeenCalledTimes(calls);
    expect(result.current.doc?.name).toBe(draft.name);
    const saved = workspaceView(); saved.workspace.doc = draft; saved.workspace.revision++;
    const refreshed = structuredClone(saved); refreshed.workspace.revision++;
    refreshed.workspace.frames[0].saved = false; refreshed.workspace.frames[0].trial = null;
    vi.mocked(workspaceApi.saveDoc).mockResolvedValue(saved);
    vi.mocked(workspaceApi.get).mockResolvedValue(refreshed);
    await act(() => result.current.saveDoc());
    await waitFor(() => expect(result.current.data?.workspace.revision).toBe(refreshed.workspace.revision));
    expect(workspaceApi.get).toHaveBeenCalledTimes(calls + 1);
    expect(result.current.doc?.name).toBe(draft.name);
    expect(result.current.dirty).toBe(false);
    expect(result.current.data?.workspace.frames[0].saved).toBe(false);
  });

  it("选择仅在当前请求真正接受后返回视图，旧请求返回 null",async()=>{
    const {result}=await open(),old=deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id=>id==="A"?old.promise:Promise.resolve(workspaceView(id)));
    let oldSelection!:Promise<WorkspaceView|null>;
    act(()=>{oldSelection=result.current.select("A");});
    await act(async()=>{const accepted=await result.current.select("B");expect(accepted?.workspace.doc.id).toBe("B");});
    await act(async()=>old.resolve(workspaceView()));expect(await oldSelection).toBeNull();
    expect(result.current.doc?.id).toBe("B");
    expect(storageApi.updatePreferences).toHaveBeenLastCalledWith({ lastWorkspace: "B" });
  });

  it("保存携带修订，等待期间阻止重复请求", async () => {
    const { result } = await open(), saving = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.saveDoc).mockReturnValue(saving.promise);
    const draft = { ...result.current.doc!, name: "更新名称" };
    act(() => result.current.setDoc(draft));
    act(() => { void result.current.saveDoc(); });
    expect(result.current.busy).toBe(true);
    await act(async () => { expect(await result.current.saveDoc()).toBeNull(); });
    expect(workspaceApi.saveDoc).toHaveBeenCalledTimes(1);
    expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, draft);
    const saved = workspaceView(); saved.workspace.doc = draft;
    await act(async () => saving.resolve(saved));
    expect(result.current.busy).toBe(false); expect(result.current.dirty).toBe(false);
    expect(screen.getByRole("status")).toHaveTextContent("候选配置已保存");
  });

  it("候选配置自动保存成功 → 不弹出全局提示，保存状态只看顶栏徽章", async () => {
    const { result } = await open();
    const draft = { ...result.current.doc!, name: "自动保存" };
    const saved = workspaceView(); saved.workspace.doc = draft;
    vi.mocked(workspaceApi.saveDoc).mockResolvedValue(saved);
    act(() => result.current.setDoc(draft));
    await waitFor(() => expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, draft), { timeout: 2500 });
    await waitFor(() => expect(result.current.saveState).toBe("saved"));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it.each([
    ["切换到其他配方", (workspace: ReturnType<typeof useWorkspace>) => workspace.select("B")],
    ["清空当前选择", (workspace: ReturnType<typeof useWorkspace>) => workspace.clearSelection()],
    ["开始下一次操作并失败", (workspace: ReturnType<typeof useWorkspace>) => workspace.act(async () => { throw new Error("试测未完成"); })],
  ])("上一次操作的成功提示 → %s时收起，不留在新的上下文里", async (_label, next) => {
    const { result } = await open();
    await act(() => result.current.act(async () => workspaceView(), "已提交发布，将在工件边界生效"));
    expect(screen.getByRole("status")).toHaveTextContent("已提交发布，将在工件边界生效");
    await act(async () => { await next(result.current); });
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
  });

  it("刷新请求发出后用户开始编辑，也不能覆盖草稿", async () => {
    const { result } = await open(), refresh = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockReturnValue(refresh.promise);
    act(() => changed("A"));
    act(() => result.current.setDoc({ ...result.current.doc!, name: "刷新期间编辑" }));
    await act(async () => refresh.resolve(workspaceView()));
    expect(result.current.doc?.name).toBe("刷新期间编辑"); expect(result.current.dirty).toBe(true);
  });

  it("操作途中离开再选回同一配方，旧操作结果也不能覆盖新选择", async () => {
    const { result } = await open(), operation = deferred<WorkspaceView>();
    act(() => { void result.current.act(() => operation.promise); });
    act(() => result.current.clearSelection());
    const latest = workspaceView(); latest.workspace.revision = 10;
    vi.mocked(workspaceApi.get).mockResolvedValueOnce(latest);
    await act(() => result.current.select("A"));
    await act(async () => operation.resolve(workspaceView()));
    expect(result.current.data?.workspace.revision).toBe(10);
  });

  it("操作响应配方不一致时保留当前候选并允许重试", async () => {
    const { result } = await open();
    await act(async () => expect(await result.current.act(async () => workspaceView("B"), "已保存")).toBeNull());
    expect(result.current.doc?.id).toBe("A"); expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(result.current.error).toContain("候选响应与当前配方不一致");
    await act(async () => expect((await result.current.act(async () => workspaceView(), "当前配方已保存"))?.workspace.doc.id).toBe("A"));
    expect(result.current.error).toBe(""); expect(screen.getByRole("status")).toHaveTextContent("当前配方已保存");
  });

  it("拍照点视角变化只清除该点的旧中线草稿",async()=>{
    const {result}=await open();
    act(()=>{result.current.setFrameDraft(0,{path:[[10,10],[24,10]],mmPerPx:.2,fixture:null});result.current.setFrameDraft(1,{path:[[30,10],[54,10]],mmPerPx:.3,fixture:null});});
    const next=workspaceView();
    next.workspace.doc.shots[0].view=3;
    next.workspace.doc.shots[0].path=[];delete next.workspace.doc.shots[0].mmPerPx;
    await act(()=>result.current.act(async()=>next));
    expect(result.current.frameDrafts[0]).toBeUndefined();expect(result.current.frameDrafts[1]).toEqual({path:[[30,10],[54,10]],mmPerPx:.3,fixture:null});
    expect(result.current.frameDirty).toBe(true);
  });

  it("保存失败保留编辑内容，重试成功后才显示已保存",async()=>{
    const {result}=await open();
    const draft={path:[[10,10],[20,10]] as [number,number][],mmPerPx:.2,fixture:null};
    vi.mocked(workspaceApi.saveDraft).mockRejectedValueOnce(new Error("磁盘不可写"));
    act(()=>result.current.setFrameDraft(0,draft));
    await waitFor(()=>expect(result.current.saveState).toBe("failed"),{timeout:2500});
    expect(result.current.frameDrafts[0]).toEqual(draft);
    const next=workspaceView();next.workspace.revision=8;next.workspace.doc.shots[0].mmPerPx=.2;
    vi.mocked(workspaceApi.saveDraft).mockResolvedValue(next);
    act(()=>result.current.retrySave());
    await waitFor(()=>expect(result.current.saveState).toBe("saved"),{timeout:2500});
    expect(result.current.frameDirty).toBe(false);
  });
});

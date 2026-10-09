import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import { subscribe } from "../src/features/plc/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { deferred, summary, workspaceView } from "./fixtures";

vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDoc: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn() }));
let changed: (id: string) => void;
let off = vi.fn<() => void>();
beforeEach(() => {
  const a = workspaceView(), b = workspaceView("B");
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [summary(a), summary(b)], errors: [] });
  vi.mocked(workspaceApi.list).mockResolvedValue([a.workspace, b.workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async id => workspaceView(id));
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(a);
  vi.mocked(recipeApi.preview).mockResolvedValue(a.layout);
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  off = vi.fn();
  vi.mocked(subscribe).mockImplementation((_event, callback) => { changed = callback as (id: string) => void; return off; });
});
async function open() {
  const hook = renderHook(() => useWorkspace(), { wrapper: WorkspaceProvider });
  await waitFor(() => expect(hook.result.current.data?.workspace.doc.id).toBe("A"));
  return hook;
}

describe("候选工作台状态与并发", () => {
  it("浏览器模式不读取桌面配置", () => {
    vi.mocked(desktopAvailable).mockReturnValue(false);
    const { result } = renderHook(() => useWorkspace(), { wrapper: WorkspaceProvider });
    expect(result.current.data).toBeNull();
    expect(workspaceApi.list).not.toHaveBeenCalled(); expect(subscribe).not.toHaveBeenCalled();
  });

  it("恢复上次配方，卸载后注销事件", async () => {
    localStorage.setItem("tujiao-last-workspace", "B");
    const { result, unmount } = renderHook(() => useWorkspace(), { wrapper: WorkspaceProvider });
    await waitFor(() => expect(result.current.doc?.id).toBe("B"));
    unmount(); expect(off).toHaveBeenCalledTimes(1);
  });

  it("已删除的上次配方回退到第一项", async () => {
    localStorage.setItem("tujiao-last-workspace", "deleted");
    await open(); expect(localStorage.getItem("tujiao-last-workspace")).toBe("A");
  });

  it("快速切换时只接受最后一次选择", async () => {
    const { result } = await open();
    const old = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id => id === "A" ? old.promise : Promise.resolve(workspaceView(id)));
    act(() => { void result.current.select("A"); });
    await act(() => result.current.select("B"));
    await act(async () => old.resolve(workspaceView()));
    expect(result.current.doc?.id).toBe("B");
    expect(localStorage.getItem("tujiao-last-workspace")).toBe("B");
  });

  it("选择仅在当前请求真正接受后返回视图，旧请求返回 null",async()=>{
    const {result}=await open(),old=deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id=>id==="A"?old.promise:Promise.resolve(workspaceView(id)));
    let oldSelection!:Promise<WorkspaceView|null>;
    act(()=>{oldSelection=result.current.select("A");});
    await act(async()=>{const accepted=await result.current.select("B");expect(accepted?.workspace.doc.id).toBe("B");});
    await act(async()=>old.resolve(workspaceView()));expect(await oldSelection).toBeNull();
    expect(result.current.doc?.id).toBe("B");
  });

  it.each(["reject","wrong-id"])("选择 %s 返回 null 并保留明确错误，修正后可再选择",async failure=>{
    const {result}=await open();
    if(failure==="reject")vi.mocked(workspaceApi.get).mockRejectedValueOnce(new Error("候选读取失败"));
    else vi.mocked(workspaceApi.get).mockResolvedValueOnce(workspaceView("other"));
    await act(async()=>expect(await result.current.select("B")).toBeNull());
    expect(result.current.data).toBeNull();expect(result.current.error).toContain(failure==="reject"?"候选读取失败":"候选响应与所选配方不一致");
    expect(localStorage.getItem("tujiao-last-workspace")).toBe("A");
    await act(async()=>expect((await result.current.select("B"))?.workspace.doc.id).toBe("B"));
    expect(result.current.error).toBe("");
  });

  it("清空选择后忽略未完成的请求", async () => {
    const { result } = await open(), request = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockReturnValue(request.promise);
    act(() => { void result.current.select("B"); });
    act(() => result.current.clearSelection());
    await act(async () => request.resolve(workspaceView("B")));
    expect(result.current.data).toBeNull(); expect(result.current.selectedId).toBeNull();
  });

  it("旧选择的失败不会覆盖新页面的错误状态", async () => {
    const { result } = await open(), old = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id => id === "A" ? old.promise : Promise.resolve(workspaceView(id)));
    act(() => { void result.current.select("A"); });
    await act(() => result.current.select("B"));
    await act(async () => old.reject(new Error("旧请求失败")));
    expect(result.current.doc?.id).toBe("B"); expect(result.current.error).toBe("");
  });

  it("未保存配置和帧草稿不被后台变更覆盖", async () => {
    const { result } = await open();
    act(() => result.current.setDoc({ ...result.current.doc!, name: "尚未保存" }));
    const count = vi.mocked(workspaceApi.get).mock.calls.length;
    await act(async () => changed("A"));
    expect(workspaceApi.get).toHaveBeenCalledTimes(count);
    expect(result.current.dirty).toBe(true); expect(result.current.doc?.name).toBe("尚未保存");
    act(() => result.current.setDoc(result.current.data!.workspace.doc));
    act(() => result.current.setFrameParams(0, { ...result.current.data!.workspace.frames[0].params, dx: 4 }));
    await act(async () => changed("A"));
    expect(result.current.frameDirty).toBe(true); expect(workspaceApi.get).toHaveBeenCalledTimes(count);
  });

  it("后台刷新途中切换配方，旧刷新不能覆盖新选择", async () => {
    const { result } = await open(), refresh = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id => id === "A" ? refresh.promise : Promise.resolve(workspaceView(id)));
    act(() => changed("A"));
    await act(() => result.current.select("B"));
    await act(async () => refresh.resolve(workspaceView()));
    expect(result.current.selectedId).toBe("B"); expect(result.current.doc?.id).toBe("B");
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
    expect(result.current.notice).toContain("候选配置已保存");
  });

  it("刷新请求发出后用户开始编辑，也不能覆盖草稿", async () => {
    const { result } = await open(), refresh = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockReturnValue(refresh.promise);
    act(() => changed("A"));
    act(() => result.current.setDoc({ ...result.current.doc!, name: "刷新期间编辑" }));
    await act(async () => refresh.resolve(workspaceView()));
    expect(result.current.doc?.name).toBe("刷新期间编辑"); expect(result.current.dirty).toBe(true);
  });

  it("连续后台变更只接受最后一次刷新", async () => {
    const { result } = await open(), old = deferred<WorkspaceView>();
    const latest = workspaceView(); latest.workspace.revision = 9;
    vi.mocked(workspaceApi.get).mockReturnValueOnce(old.promise).mockResolvedValueOnce(latest);
    act(() => changed("A")); await act(async () => changed("A"));
    await act(async () => old.resolve(workspaceView()));
    expect(result.current.data?.workspace.revision).toBe(9);
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

  it("操作失败显示错误并解锁，下次操作可以重试", async () => {
    const { result } = await open();
    await act(async () => { expect(await result.current.act(() => Promise.reject(new Error("修订冲突")))).toBeNull(); });
    expect(result.current.error).toContain("修订冲突"); expect(result.current.busy).toBe(false);
    await act(() => result.current.act(async () => workspaceView()));
    expect(result.current.error).toBe(""); expect(result.current.busy).toBe(false);
  });

  it.each(["resolve", "reject"])("切换配方后旧操作 %s 返回 null，不提示旧成功或错误", async outcome => {
    const { result } = await open(), operation = deferred<WorkspaceView>();
    let pending!: Promise<WorkspaceView | null>;
    act(() => { pending = result.current.act(() => operation.promise, "旧配方已保存"); });
    await act(() => result.current.select("B"));
    act(() => result.current.setError("新配方的提示"));
    await act(async () => {
      if (outcome === "resolve") operation.resolve(workspaceView());
      else operation.reject(new Error("旧配方保存失败"));
      expect(await pending).toBeNull();
    });
    expect(result.current.doc?.id).toBe("B");
    expect(result.current.notice).toBe(""); expect(result.current.error).toBe("新配方的提示");
    expect(result.current.busy).toBe(false);
  });

  it("保存后列表刷新途中切换配方，也不返回旧成功提示", async () => {
    const { result } = await open(), listing = deferred<ReturnType<typeof summary>[]>();
    vi.mocked(recipeApi.list).mockImplementationOnce(async () => ({ recipes: await listing.promise, errors: [] }));
    let pending!: Promise<WorkspaceView | null>;
    act(() => { pending = result.current.act(async () => workspaceView(), "旧配方已保存"); });
    await waitFor(() => expect(recipeApi.list).toHaveBeenCalledTimes(3));
    await act(() => result.current.select("B"));
    await act(async () => { listing.resolve([summary(workspaceView()), summary(workspaceView("B"))]); expect(await pending).toBeNull(); });
    expect(result.current.doc?.id).toBe("B"); expect(result.current.notice).toBe("");
  });

  it("操作响应配方不一致时保留当前候选并允许重试", async () => {
    const { result } = await open();
    await act(async () => expect(await result.current.act(async () => workspaceView("B"), "已保存")).toBeNull());
    expect(result.current.doc?.id).toBe("A"); expect(result.current.notice).toBe("");
    expect(result.current.error).toContain("候选响应与当前配方不一致");
    await act(async () => expect((await result.current.act(async () => workspaceView(), "当前配方已保存"))?.workspace.doc.id).toBe("A"));
    expect(result.current.error).toBe(""); expect(result.current.notice).toBe("当前配方已保存");
  });

  it("候选保存成功后列表刷新失败，不撤销成功结果或下一步资格", async () => {
    const { result } = await open(), next = workspaceView(); next.workspace.revision = 8;
    vi.mocked(recipeApi.list).mockRejectedValueOnce(new Error("配方目录暂不可读"));
    await act(async () => expect((await result.current.act(async () => next, "本帧示教已保存"))?.workspace.revision).toBe(8));
    expect(result.current.data?.workspace.revision).toBe(8); expect(result.current.notice).toBe("本帧示教已保存");
    expect(result.current.error).toContain("候选操作已完成，但配方列表刷新失败"); expect(result.current.busy).toBe(false);
    await act(() => result.current.reloadList());
    expect(result.current.data?.workspace.revision).toBe(8);
  });

  it("同一几何更新保留未提交帧草稿，几何改变清空草稿", async () => {
    const { result } = await open();
    act(() => result.current.setFrameParams(0, { ...result.current.data!.workspace.frames[0].params, dx: 4 }));
    await act(() => result.current.act(async () => workspaceView()));
    expect(result.current.frameDrafts[0].dx).toBe(4);
    const next = workspaceView(); next.layout.fov = [200, 80];
    await act(() => result.current.act(async () => next));
    expect(result.current.frameDrafts).toEqual({});
  });

  it("修改预览有防抖，旧预览晚到不覆盖新草稿", async () => {
    const { result } = await open(); vi.useFakeTimers();
    try {
      const first = deferred<WorkspaceView["layout"]>();
      vi.mocked(recipeApi.preview).mockReturnValueOnce(first.promise).mockResolvedValueOnce({ ...workspaceView().layout, name: "最新" });
      act(() => result.current.setDoc({ ...result.current.doc!, name: "第一版" }));
      await act(() => vi.advanceTimersByTimeAsync(249)); expect(recipeApi.preview).not.toHaveBeenCalled();
      await act(() => vi.advanceTimersByTimeAsync(1)); expect(recipeApi.preview).toHaveBeenCalledTimes(1);
      act(() => result.current.setDoc({ ...result.current.doc!, name: "最新" }));
      await act(() => vi.advanceTimersByTimeAsync(250));
      await act(async () => first.resolve({ ...workspaceView().layout, name: "旧预览" }));
      expect(result.current.preview?.name).toBe("最新");
    } finally { vi.useRealTimers(); }
  });
});

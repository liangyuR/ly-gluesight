import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { historyApi } from "../src/features/history/api";
import { workspaceApi } from "../src/features/workspace/api";
import { useWorkspace } from "../src/features/workspace/context";
import { deferred, partDetail, workspaceState, workspaceView } from "./fixtures";
import type { Comparison, RecordImages } from "../src/features/workspace/types";

vi.mock("../src/features/history/api", () => ({ historyApi: { detail: vi.fn(), recipe: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  recordImages: vi.fn(), recordImage: vi.fn(), comparisons: vi.fn(), compare: vi.fn(), compareOriginal: vi.fn(), historyCapture: vi.fn(), runtimeOverview: vi.fn(),
} }));
let ws: ReturnType<typeof workspaceState>;
let detail: ReturnType<typeof partDetail>;
const raw: RecordImages = { historyId: 1, complete: true, message: "原图完整", frames: [0, 1].map(k => ({ k, camera: "CAM-1", view: 1, file: `${k}.png`, ts: 1, available: true, error: null })) };
function element(){return <MemoryRouter initialEntries={["/history/1"]}><Link to="/history/2">打开另一件工件</Link><Routes><Route path="/history/:id" element={<HistoryDetailPage />} /><Route path="/recipe/teach" element={<p>进入示教页</p>} /><Route path="/history" element={<p>历史列表</p>}/></Routes></MemoryRouter>;}
function show() { return render(element()); }
function comparison():Comparison{return {id:"compare-1",historyId:1,source:"rules",cycleId:"cycle-1",bundleId:"bundle-A",candidateId:"A",candidateRevision:7,candidateRecipe:ws.data!.layout,
  originalVerdict:"NG_GAP",judgement:{...detail.judgement,verdict:"OK",reason:"候选合格"},measurements:[],createdAt:1};}
beforeEach(() => {
  ws = workspaceState(); ws.data!.workspace.frames.forEach(f => f.saved = true);
  detail = partDetail(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.detail).mockResolvedValue(detail); vi.mocked(historyApi.recipe).mockResolvedValue(ws.data!.layout);
  vi.mocked(workspaceApi.recordImages).mockResolvedValue(structuredClone(raw));
  vi.mocked(workspaceApi.recordImage).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.runtimeOverview).mockResolvedValue(null); vi.mocked(workspaceApi.comparisons).mockResolvedValue([]);
  vi.mocked(workspaceApi.historyCapture).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.compare).mockResolvedValue(comparison());
  vi.mocked(workspaceApi.compareOriginal).mockResolvedValue({...comparison(),source:"original"});
});

describe("历史工件复测", () => {

  it("缺少原图或发布身份时拒绝原包重现", async () => {
    detail.summary.bundleId = null;
    show(); await screen.findByText(/原图完整/);
    expect(screen.getByRole("button", {name: "按原发布包重现"})).toBeDisabled();
    expect(workspaceApi.compareOriginal).not.toHaveBeenCalled();
  });

  it("按实际保存视角查看三目原图，非检测视角不叠加中线或用于示教", async () => {
    detail.shots[0].rawFiles = [1, 2, 3].map(view => ({view, file: `k000_P1_CAM-1_v${view}.pgm`}));
    vi.mocked(workspaceApi.recordImages).mockResolvedValue({...raw, frames: [1, 2, 3].map(view => ({...raw.frames[0], view, available: view !== 3, error: view === 3 ? "原图校验失败" : null}))});
    show(); await screen.findByText(/原图完整/);
    expect(screen.getByRole("button", {name: "查看 k1 视角 3"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button", {name: "查看 k1 视角 2"}));
    await waitFor(() => expect(workspaceApi.recordImage).toHaveBeenLastCalledWith(1, 0, 2));
    const image = await screen.findByRole("img", {name: "原始 SN 101 · k1 · 视角 2"});
    expect(image.querySelector("polyline")).toBeNull();
    expect(screen.getByRole("button", {name: "将此帧用于示教"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button", {name: "追溯拍照点 P1"}));
    await waitFor(() => expect(workspaceApi.recordImage).toHaveBeenLastCalledWith(1, 0, 1));
    expect(screen.getByRole("button", {name: "将此帧用于示教"})).toBeEnabled();
  });

  it("无原图仍可规则重判，不能原图复测或取历史图示教", async () => {
    vi.mocked(workspaceApi.recordImages).mockResolvedValue({ ...raw, complete: false, frames: [], message: "原图已清理" });
    show(); await screen.findByText(/原图已清理/);
    expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "将此帧用于示教" })).toBeDisabled();
  });

  it.each(["dirty", "frame-dirty", "wrong-recipe", "incomplete-measurements"])("%s 禁止规则重判", async condition => {
    if (condition === "dirty") ws.dirty = true;
    if (condition === "frame-dirty") ws.frameDirty = true;
    if (condition === "wrong-recipe") ws.data!.workspace.doc.id = "B";
    if (condition === "incomplete-measurements") detail.points!.st[0] = 2;
    show(); await screen.findByRole("heading", { name: "复测与候选对照" });
    await waitFor(() => expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeDisabled());
  });

  it("历史帧绑定成功更新候选修订后，仍进入对应示教帧", async () => {
    const refresh = deferred<void>(), next = workspaceView(); next.workspace.revision = 8;
    vi.mocked(workspaceApi.historyCapture).mockResolvedValueOnce(next);
    ws.act = vi.fn(async request => { const result = await request(); await refresh.promise; return result; });
    const page = show(); await screen.findByText(/原图完整/);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "历史帧选择" }), "1");
    await userEvent.click(screen.getByRole("button", { name: "将此帧用于示教" }));
    ws.data = next; ws.doc = next.workspace.doc; page.rerender(element());
    await act(async () => refresh.resolve());
    expect(await screen.findByText("进入示教页")).toBeVisible();
    expect(workspaceApi.historyCapture).toHaveBeenCalledWith("A", 7, 1, 1);
  });

  it.each(["revision","recipe"])("候选 %s 改变后旧复测结果不能覆盖当前目标",async change=>{
    const pending=deferred<Comparison>();vi.mocked(workspaceApi.compare).mockReturnValueOnce(pending.promise);
    const old=comparison(),page=show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"按候选规则重判"}));
    if(change==="revision")ws.data!.workspace.revision++;else ws=workspaceState(workspaceView("B"));
    page.rerender(element());await act(async()=>pending.resolve(old));
    expect(screen.queryByRole("heading",{name:"规则重判结果"})).not.toBeInTheDocument();expect(screen.queryByText("候选合格")).not.toBeInTheDocument();
  });

  it.each(["resolve","reject"])("打开另一工件后旧复测 %s 不显示结果或报错",async finish=>{
    const pending=deferred<Comparison>();vi.mocked(workspaceApi.compare).mockReturnValueOnce(pending.promise);
    show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"按候选规则重判"}));
    const next=structuredClone(detail);next.summary.id=2;next.summary.sn=102;vi.mocked(historyApi.detail).mockResolvedValue(next);
    await userEvent.click(screen.getByRole("link",{name:"打开另一件工件"}));await screen.findByText("SN 102");
    await act(async()=>finish==="resolve"?pending.resolve(comparison()):pending.reject(new Error("旧工件错误")));
    expect(screen.queryByRole("heading",{name:"规则重判结果"})).not.toBeInTheDocument();expect(screen.queryByText("Error: 旧工件错误")).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"按候选规则重判"})).toBeEnabled();
  });

  it("候选或工件已切换时，旧示教取图完成不会突然跳转",async()=>{
    const pending=deferred<NonNullable<typeof ws.data>>();vi.mocked(workspaceApi.historyCapture).mockReturnValueOnce(pending.promise);
    const old=ws.data!,page=show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"将此帧用于示教"}));
    ws=workspaceState(workspaceView("B"));page.rerender(element());await act(async()=>pending.resolve(old));
    expect(screen.queryByText("进入示教页")).not.toBeInTheDocument();expect(screen.getByRole("heading",{name:"原始整件判定"})).toBeVisible();
  });

});

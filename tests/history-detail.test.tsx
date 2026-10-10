import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  it("原发布包重现不依赖当前候选且保留原始生产判定", async () => {
    const original = structuredClone(detail);
    ws.data!.workspace.doc.id = "B"; ws.dirty = true; ws.frameDirty = true;
    const reproduced = {...comparison(), source: "original" as const, judgement: {...detail.judgement, reason: "原包重现断胶"}};
    vi.mocked(workspaceApi.compareOriginal).mockResolvedValue(reproduced);
    show(); await screen.findByText(/原图完整/);
    expect(screen.getByRole("button", {name: "按候选规则重判"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button", {name: "按原发布包重现"}));
    expect(workspaceApi.compareOriginal).toHaveBeenCalledExactlyOnceWith(1);
    expect(workspaceApi.compare).not.toHaveBeenCalled();
    expect(await screen.findByRole("heading", {name: "原发布包重现结果"})).toBeVisible();
    expect(screen.getByText("原包重现断胶")).toBeVisible();
    expect(detail).toEqual(original);
  });

  it("缺少原图或发布身份时拒绝原包重现", async () => {
    detail.summary.bundleId = null;
    show(); await screen.findByText(/原图完整/);
    expect(screen.getByRole("button", {name: "按原发布包重现"})).toBeDisabled();
    expect(workspaceApi.compareOriginal).not.toHaveBeenCalled();
  });

  it("按实际保存视角查看三目原图，非检测视角不叠加中线或用于示教", async () => {
    detail.shots[0].rawFiles = [1, 2, 3].map(view => ({view, file: `k000_P1_CAM-1_v${view}.pgm`, revisionId: "fnv1a64:verified"}));
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

  it("测量出错、缺帧、PLC 确认和录制失败分别展示", async () => {
    detail.shots[0].status = "error"; detail.shots[0].error = "测量引擎超时";
    detail.shots[1].status = "missing"; detail.shots[1].error = "End 前未收到帧";
    detail.recording = {state: "failed", available: false, directory: null, errors: ["磁盘写入失败"]};
    show(); await screen.findByRole("heading", {name: "逐拍照点追溯"});
    expect(screen.getByText("测量引擎超时")).toBeVisible();
    expect(screen.getByText("End 前未收到帧")).toBeVisible();
    expect(screen.getByText("PLC 已确认")).toBeVisible();
    expect(screen.getByText("录制失败")).toBeVisible();
    expect(screen.getByText("磁盘写入失败")).toBeVisible();
    expect(screen.getByText("cycle-1")).toBeVisible();
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

  it("原图复测需要已保存的全部示教", async () => {
    ws.data!.workspace.frames[1].saved = false; show(); await screen.findByText(/原图完整/);
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeEnabled();
  });

  it("规则对照展示候选结论，原始生产结果保留", async () => {
    const original = structuredClone(detail); show(); await screen.findByText(/原图完整/);
    await userEvent.click(screen.getByRole("button", { name: "按候选规则重判" }));
    expect(workspaceApi.compare).toHaveBeenCalledWith("A", 7, 1, false);
    await screen.findByRole("heading", { name: "规则重判结果" });
    expect(screen.getByText("候选合格")).toBeVisible();
    expect(screen.getByRole("heading", { name: "原始整件判定" })).toBeVisible();
    expect(detail).toEqual(original);
  });

  it("原图复测携带 raw=true", async () => {
    show(); await screen.findByText(/原图完整/);
    await userEvent.click(screen.getByRole("button", { name: "从原图复测整件" }));
    expect(workspaceApi.compare).toHaveBeenCalledWith("A", 7, 1, true);
  });

  it("选中帧决定历史取样，成功才跳转示教", async () => {
    show(); await screen.findByText(/原图完整/);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "历史帧选择" }), "1");
    await userEvent.click(screen.getByRole("button", { name: "将此帧用于示教" }));
    expect(workspaceApi.historyCapture).toHaveBeenCalledWith("A", 7, 1, 1);
    expect(await screen.findByText("进入示教页")).toBeVisible();
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

  it("复测失败保留原始结果并允许重试", async () => {
    vi.mocked(workspaceApi.compare).mockRejectedValueOnce(new Error("图像不兼容")); show(); await screen.findByText(/原图完整/);
    await userEvent.click(screen.getByRole("button", { name: "从原图复测整件" }));
    expect(await screen.findByText("Error: 图像不兼容")).toBeVisible();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeEnabled();
    expect(screen.getByRole("heading", { name: "原始整件判定" })).toBeVisible();
  });

  it("重复触发复测只发起一次，等待中不能取样或切换候选",async()=>{
    const pending=deferred<Comparison>();vi.mocked(workspaceApi.compare).mockReturnValueOnce(pending.promise);
    show();await screen.findByText(/原图完整/);fireEvent.click(screen.getByRole("button",{name:"从原图复测整件"}));
    fireEvent.click(screen.getByRole("button",{name:"从原图复测整件"}));
    expect(workspaceApi.compare).toHaveBeenCalledTimes(1);expect(screen.getByRole("button",{name:"按候选规则重判"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"将此帧用于示教"})).toBeDisabled();expect(screen.getByRole("button",{name:"使用该配方候选"})).toBeDisabled();
    await act(async()=>pending.resolve(comparison()));expect(screen.getByRole("button",{name:"从原图复测整件"})).toBeEnabled();
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

  it("历史初次加载晚到不能替换新工件；返回入口可用",async()=>{
    const pending=deferred<typeof detail>();vi.mocked(historyApi.detail).mockReturnValueOnce(pending.promise);
    show();expect(screen.getByText("加载中…")).toBeVisible();
    const next=structuredClone(detail);next.summary.id=2;next.summary.sn=102;vi.mocked(historyApi.detail).mockResolvedValue(next);
    await userEvent.click(screen.getByRole("link",{name:"打开另一件工件"}));await screen.findByText("SN 102");
    await act(async()=>pending.resolve(detail));expect(screen.queryByText("SN 101")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"返回历史列表"}));expect(await screen.findByText("历史列表")).toBeVisible();
  });

  it("旧已保存对照查询不能吞掉刚生成的结果，选记录和清空都可操作",async()=>{
    const saved=deferred<Comparison[]>();vi.mocked(workspaceApi.comparisons).mockReturnValueOnce(saved.promise);
    show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"按候选规则重判"}));
    await screen.findByRole("heading",{name:"规则重判结果"});await act(async()=>saved.resolve([]));
    expect(screen.getByRole("combobox",{name:"已保存对照结果"})).toHaveValue("compare-1");
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"已保存对照结果"}),"");
    expect(screen.queryByRole("heading",{name:"规则重判结果"})).not.toBeInTheDocument();
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"已保存对照结果"}),"compare-1");expect(screen.getByText("候选合格")).toBeVisible();
  });

  it("历史帧取样失败保留页面，重试成功才跳转，重复点击只发一次",async()=>{
    const pending=deferred<NonNullable<typeof ws.data>>();vi.mocked(workspaceApi.historyCapture).mockReturnValueOnce(pending.promise);
    show();await screen.findByText(/原图完整/);fireEvent.click(screen.getByRole("button",{name:"将此帧用于示教"}));fireEvent.click(screen.getByRole("button",{name:"将此帧用于示教"}));
    expect(workspaceApi.historyCapture).toHaveBeenCalledTimes(1);await act(async()=>pending.reject(new Error("历史原图已清理")));
    expect(screen.getByText("Error: 历史原图已清理")).toBeVisible();expect(screen.queryByText("进入示教页")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"将此帧用于示教"}));expect(await screen.findByText("进入示教页")).toBeVisible();
  });

  it("候选或工件已切换时，旧示教取图完成不会突然跳转",async()=>{
    const pending=deferred<NonNullable<typeof ws.data>>();vi.mocked(workspaceApi.historyCapture).mockReturnValueOnce(pending.promise);
    const old=ws.data!,page=show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"将此帧用于示教"}));
    ws=workspaceState(workspaceView("B"));page.rerender(element());await act(async()=>pending.resolve(old));
    expect(screen.queryByText("进入示教页")).not.toBeInTheDocument();expect(screen.getByRole("heading",{name:"原始整件判定"})).toBeVisible();
  });

  it("没有完成任何测量时不能按规则重判，选择本配方候选可恢复操作",async()=>{
    detail.points={d:[],w:[],st:[]};ws.data!.workspace.doc.id="B";
    show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"使用该配方候选"}));
    expect(ws.select).toHaveBeenCalledWith("A");expect(screen.getByRole("button",{name:"按候选规则重判"})).toBeDisabled();
  });

  it("逐拍照点显示原始结论：缺胶的拍照点标断胶，断口按段内弧长与该拍照点的允许长度说明；可切到整件总览",async()=>{
    detail.points={d:[3,3,3,3],w:[null,null,null,null],st:[0,0,1,1]};
    detail.judgement={...detail.judgement,reason:"P2 · J1 断胶 2.0 mm > 0.5 mm · s=0.0–2.0",
      segments:[{verdict:"OK",min:3,max:3,excursionLen:0,wMin:null,wMax:null,wExcursionLen:0},{verdict:"NG_GAP",min:null,max:null,excursionLen:0,wMin:null,wMax:null,wExcursionLen:0}],
      gaps:[{segment:1,s0:0,s1:2,len:2,frames:[1]}]};
    detail.frames=[0,1].map(k=>({status:"done",cam:0,camera:"CAM-1",shotId:`P${k+1}`,view:1,session:1,ordinal:k+1,error:null,arrivedMs:1,frameCounter:1,triggerCounter:1,counterJump:false,score:.9,points:2,gapPoints:0,ms:5}));
    show();await screen.findByText(/原图完整/);
    const tiles=within(screen.getByLabelText("逐拍照点视图"));
    expect(within(tiles.getByRole("button",{name:"拍照点 P1"})).getByText("合格")).toBeVisible();
    expect(within(tiles.getByRole("button",{name:"拍照点 P2"})).getByText("断胶")).toBeVisible();
    expect(tiles.getByText("断胶 s=0.0–2.0 mm")).toBeVisible();
    expect(screen.getByText("段内 s 0.0 – 2.0 mm · 长度 2.0 mm（允许 ≤ 0.5 mm）")).toBeVisible();
    expect(screen.queryByText("跨帧合并")).toBeNull();
    await userEvent.click(screen.getByRole("button",{name:"k2 · P2"}));
    expect(screen.getByRole("combobox",{name:"历史帧选择"})).toHaveValue("1");
    expect(tiles.getByRole("button",{name:"拍照点 P2"})).toHaveAttribute("aria-pressed","true");
    // 原图上叠加这个拍照点的示教中线
    const image=await screen.findByRole("img",{name:"原始 SN 101 · k2 · 视角 1"});
    expect(image.querySelector("polyline")).toHaveAttribute("points","30,10 40,10");
    await userEvent.click(tiles.getByRole("button",{name:"拍照点 P1"}));expect(screen.getByRole("combobox",{name:"历史帧选择"})).toHaveValue("0");
    await userEvent.click(screen.getByRole("button",{name:"整件"}));
    expect(screen.getByLabelText("工件总览，选择帧查看原图")).toBeVisible();expect(screen.queryByLabelText("逐拍照点视图")).toBeNull();
    expect(screen.getByRole("button",{name:"总览选择帧 k2"})).toHaveAttribute("data-state","gap");
  });
});

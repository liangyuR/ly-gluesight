import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  recordImages: vi.fn(), recordImage: vi.fn(), comparisons: vi.fn(), compare: vi.fn(), historyCapture: vi.fn(), runtimeOverview: vi.fn(),
} }));
let ws: ReturnType<typeof workspaceState>;
let detail: ReturnType<typeof partDetail>;
const raw: RecordImages = { historyId: 1, complete: true, message: "原图完整", frames: [0, 1].map(k => ({ k, camera: "CAM-1", file: `${k}.png`, ts: 1, available: true })) };
function element(){return <MemoryRouter initialEntries={["/history/1"]}><Link to="/history/2">打开另一件工件</Link><Routes><Route path="/history/:id" element={<HistoryDetailPage />} /><Route path="/recipe/teach" element={<p>进入示教页</p>} /><Route path="/history" element={<p>历史列表</p>}/></Routes></MemoryRouter>;}
function show() { return render(element()); }
function comparison():Comparison{return {id:"compare-1",historyId:1,source:"rules",candidateId:"A",candidateRevision:7,candidateRecipe:ws.data!.layout,
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
});

describe("历史工件复测", () => {
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
    show(); await screen.findByRole("heading", { name: "候选对照" });
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
});

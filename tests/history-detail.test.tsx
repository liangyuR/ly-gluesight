import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { historyApi } from "../src/features/history/api";
import { workspaceApi } from "../src/features/workspace/api";
import { useWorkspace } from "../src/features/workspace/context";
import { deferred, partDetail, workspaceState } from "./fixtures";
import { cycleFrame } from "./cycle-visual-fixtures";
import type { Comparison, RecordImages } from "../src/features/workspace/types";
import type { Measured, Recipe } from "../src/features/cycle/types";

vi.mock("../src/features/history/api", () => ({ historyApi: { detail: vi.fn(), recipe: vi.fn(), neighbors: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  recordImages: vi.fn(), recordImage: vi.fn(), comparisons: vi.fn(), compare: vi.fn(), compareOriginal: vi.fn(), historyCapture: vi.fn(), runtimeOverview: vi.fn(),
} }));
let ws: ReturnType<typeof workspaceState>;
let detail: ReturnType<typeof partDetail>;
const raw: RecordImages = { imageSource: "recording", historyId: 1, complete: true, message: "原图完整", frames: [0, 1].map(k => ({ k, camera: "CAM-1", view: 1, file: `${k}.png`, ts: 1, available: true, error: null })) };
const frameSelect = () => screen.findByLabelText("历史帧选择", { selector: "select" }) as Promise<HTMLSelectElement>;
function element(){return <MemoryRouter initialEntries={["/history/1"]}><Routes><Route path="/history/:id" element={<HistoryDetailPage />} /></Routes></MemoryRouter>;}
function show() { return render(element()); }
function comparison():Comparison{return {id:"compare-1",historyId:1,source:"rules",cycleId:"cycle-1",bundleId:"bundle-A",candidateId:"A",candidateRevision:7,candidateRecipe:ws.data!.layout,
  originalVerdict:"NG_GAP",judgement:{...detail.judgement,verdict:"OK",reason:"候选合格"},measurements:[],createdAt:1};}
beforeEach(() => {
  ws = workspaceState(); ws.data!.workspace.frames.forEach(f => f.saved = true); ws.data!.progress.shots.forEach(s => s.state = "taught");
  detail = partDetail(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.detail).mockResolvedValue(detail); vi.mocked(historyApi.recipe).mockResolvedValue(ws.data!.layout);
  vi.mocked(historyApi.neighbors).mockResolvedValue({ older: null, newer: null });
  vi.mocked(workspaceApi.recordImages).mockResolvedValue(structuredClone(raw));
  vi.mocked(workspaceApi.recordImage).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.runtimeOverview).mockResolvedValue(null); vi.mocked(workspaceApi.comparisons).mockResolvedValue([]);
  vi.mocked(workspaceApi.historyCapture).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.compare).mockResolvedValue(comparison());
  vi.mocked(workspaceApi.compareOriginal).mockResolvedValue({...comparison(),source:"original"});
});

describe("历史工件复测", () => {
  it("分段判定不同 → 结果卡片使用对应的合格、超差、NG和错误颜色", async () => {
    detail.judgement.segments = (["OK", "OK_WITH_EXCURSION", "NG_WIDTH", "ERR_INSPECT"] as const).map(verdict => ({ verdict, min: 0, max: 0, excursionLen: 0, wMin: null, wMax: null, wExcursionLen: 0, referenceInvalid: false }));
    const page = show();
    await screen.findByText(/上方为中线示意/);
    const cards = page.container.querySelectorAll(".history-segment");
    expect(cards).toHaveLength(4);
    ["ok", "warn", "ng", "fault"].forEach((tone, i) => {
      expect(cards[i]).toHaveAttribute("data-tone", tone);
      expect(cards[i]).not.toHaveClass("ng-item");
    });
  });

  it("候选历史关闭录图但关联样本原图 → 可查看与复测并标明来源", async () => {
    detail.summary.source = "offlineCandidate";
    detail.summary.bundleId = null;
    detail.summary.recipeRevision = "candidate-A-r7";
    detail.recording = { state: "off", available: false, directory: null, errors: [] };
    ws.data!.progress.shots.forEach(shot => { shot.state = "recapture"; });
    detail.shots.forEach(shot => { shot.rawFiles = []; });
    vi.mocked(workspaceApi.recordImages).mockResolvedValue({ ...raw, imageSource: "offlineSample", message: "离线样本原图：2 幅可用" });
    show();
    expect(await screen.findByText("候选离线调试记录")).toBeVisible();
    expect(screen.queryByRole("button", { name: "按原发布包重现" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeEnabled();
    expect(screen.getByText(/此记录没有生产发布包/)).toBeVisible();
    expect(await screen.findByRole("button", { name: "查看 k1 视角 1" })).toBeEnabled();
    expect(screen.getByText("受管样本完整可用")).toBeVisible();
    expect(screen.queryByText("录制关闭")).not.toBeInTheDocument();
    expect(await screen.findByRole("img", { name: /离线样本原图 · 样本 SN/ })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "从原图复测整件" }));
    await waitFor(() => expect(workspaceApi.compare).toHaveBeenCalledWith("A", ws.data!.workspace.revision, 1, true));
  });

  it("历史关联的离线样本缺图 → 显示样本缺失原因并禁止整件复测", async () => {
    detail.summary.source = "offlineCandidate";
    vi.mocked(workspaceApi.recordImages).mockResolvedValue({ ...raw, imageSource: "offlineSample", complete: false, frames: raw.frames.map(frame => ({ ...frame, available: false, error: "历史样本集缺少所选视角" })), message: "历史样本集缺少所选视角" });
    show();
    expect(await screen.findByText("受管样本不完整或不可用")).toBeVisible();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "查看 k1 视角 1" })).toBeDisabled();
    expect(screen.getAllByText("历史样本集缺少所选视角").length).toBeGreaterThan(0);
    expect(screen.queryByText(/本帧原图未保存或已按保留策略清理/)).not.toBeInTheDocument();
  });

  it("录制失败明确显示原因，保留规则重判但禁止原图复测与示教", async () => {
    detail.recording = { state: "failed", available: false, directory: null, errors: ["原图写入失败：磁盘空间不足"] };
    vi.mocked(workspaceApi.recordImages).mockResolvedValue({ ...raw, complete: false, frames: [], message: "原图写入失败：磁盘空间不足" });
    show(); await screen.findByText("录制失败");
    expect(screen.getAllByRole("status").find(node => node.textContent?.includes("原图写入失败：磁盘空间不足"))).toBeVisible();
    expect(screen.queryByText(/原图完整|完整落盘|已核验落盘/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "按原发布包重现" })).toBeDisabled();
    expect(workspaceApi.compareOriginal).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "将此帧用于示教" })).toBeDisabled();
  });

  it("记录含无基准、超出检测范围的站 → 仍可按候选规则重判",async()=>{
    detail.points={d:[3,null,null,3],st:[0,4,5,0],w:[null,4,null,null]};
    show();await screen.findByText(/原图完整/);
    await userEvent.click(screen.getByRole("button",{name:"按候选规则重判"}));
    expect(workspaceApi.compare).toHaveBeenCalledWith(ws.data!.workspace.doc.id,ws.data!.workspace.revision,detail.summary.id,false);
  });

  it("候选含不检拍照点 → 其余点已示教即可从原图复测整件",async()=>{
    ws.data!.workspace.doc.shots[1].skip=true;ws.data!.workspace.frames[1].saved=false;ws.data!.progress.shots[1].state="skipped";
    show();await screen.findByText(/原图完整/);
    expect(screen.getByRole("button",{name:"从原图复测整件"})).toBeEnabled();
  });

  it("帧已保存但后端进度判为需重新采集 → 不能从原图复测整件",async()=>{
    ws.data!.progress.shots[0].state="recapture";
    show();await screen.findByText(/原图完整/);
    expect(screen.getByRole("button",{name:"从原图复测整件"})).toBeDisabled();
  });

  it("打开切换确认框 → 不显示之前的旧错误；自动保存失败时不再提示正在自动保存",async()=>{
    ws.data!.workspace.doc.id="B";ws.drafts=[ws.data!.workspace];ws.error="旧操作失败";ws.dirty=true;ws.saveState="failed";
    vi.mocked(ws.setError).mockImplementation(message=>{ws.error=message;});
    show();await screen.findByText(/原图完整/);
    await userEvent.click(screen.getByRole("button",{name:"使用该配方候选"}));
    const dialog=screen.getByRole("dialog");
    expect(dialog).not.toHaveTextContent("旧操作失败");
    expect(dialog).toHaveTextContent("自动保存失败");
    expect(dialog).not.toHaveTextContent("正在自动保存");
  });

  it("候选修订改变后旧复测结果不能覆盖当前目标",async()=>{
    const pending=deferred<Comparison>();vi.mocked(workspaceApi.compare).mockReturnValueOnce(pending.promise);
    const old=comparison(),page=show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"按候选规则重判"}));
    ws.data!.workspace.revision++;
    page.rerender(element());await act(async()=>pending.resolve(old));
    expect(screen.queryByRole("heading",{name:"规则重判结果"})).not.toBeInTheDocument();expect(screen.queryByText("候选合格")).not.toBeInTheDocument();
  });

  it("按原发布包重现后有一个拍照点与原结果不同 → 比对表只列该拍照点，点击后切到该帧", async () => {
    const shot = (k: number, d: number[], st: number[]): Measured => ({ cycleId: "cycle-1", shotId: `P${k + 1}`, camera: "CAM-1", bundleId: "bundle-A", sn: "SN-101", k, cam: 0,
      referenceFailed: [], score: 1, ms: 5, queueMs: null, engineMs: null, coreMs: null, error: null, idx: [2 * k, 2 * k + 1], d, w: [null, null], st, px: [] });
    vi.mocked(workspaceApi.compareOriginal).mockResolvedValue({ ...comparison(), source: "original", measurements: [shot(0, [3, 3], [0, 0]), shot(1, [3, 0], [0, 1])] });
    show(); await screen.findByText(/原图完整/);
    await userEvent.click(screen.getByRole("button", { name: "按原发布包重现" }));
    expect(await screen.findByText("1 / 2 个拍照点与原结果不同", { exact: false })).toBeVisible();
    const table = screen.getByRole("table", { name: "逐拍照点比对" });
    expect(within(table).queryByRole("button", { name: "比对查看拍照点 k1" })).not.toBeInTheDocument();
    await userEvent.click(within(table).getByRole("button", { name: "比对查看拍照点 k2" }));
    expect(await frameSelect()).toHaveValue("1");
  });

  it("原配方快照仍在加载 → 不提示缺失；确认缺失 → 提示原因并禁用按原发布包重现", async () => {
    const recipe = deferred<Recipe | null>(); vi.mocked(historyApi.recipe).mockReturnValue(recipe.promise);
    show(); await screen.findByText(/原图完整/);
    expect(screen.queryByText("不能按原发布包重跑")).not.toBeInTheDocument();
    await act(async () => recipe.resolve(null));
    expect(await screen.findByText(/原配方快照 revisionId-A 缺失/)).toBeVisible();
    expect(screen.getByRole("button", { name: "按原发布包重现" })).toBeDisabled();
  });

  it("选中不检拍照点后按 → → 跳到其后的检测拍照点而不是回到开头", async () => {
    const layout = ws.data!.layout;
    layout.shots = [0, 1, 2, 3].map(k => ({ ...layout.shots[0], id: `P${k + 1}`, poseId: `P${k + 1}`, skip: k === 2 }));
    show();
    const frame = await frameSelect();
    await userEvent.selectOptions(frame, "2");
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    expect(frame).toHaveValue("3");
  });
  it("记录的位姿未采用（贴边错锁 38 px）→ 原图上中线与站点不搬，框留在示教位置并标“定位未采用”", async () => {
    ws.data!.layout.shots[0].fixture = { rect: [40, 20, 16, 16], source: "A-image-0", searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1 };
    detail.frames = [{ ...cycleFrame("done", 0), referenceFailed: [{ view: 1, reason: "定位失败" }],
      viewResults: [{ view: 1, verdict: "ERR_INSPECT", error: null, score: 1, ms: 8, pose: { accepted: false, reason: "at_search_edge", score: .99, peakRatio: .5, angle: 0, offset: [38, 0], endShiftMm: null, excludedMm: null, endLimitMm: 9.6 } }] }, cycleFrame("done", 1)];
    show();
    const image = await screen.findByRole("img", { name: /原始 SN/ });
    await waitFor(() => expect(image.querySelector('[aria-label="中线叠加"] polyline')).not.toBeNull());
    expect(image.querySelector('[aria-label="中线叠加"] polyline')?.getAttribute("points")).toBe("10,10 20,10");
    expect([...image.querySelectorAll('[aria-label="中线叠加"] circle')].map(c => [Number(c.getAttribute("cx")), Number(c.getAttribute("cy"))])).toEqual([[10, 10], [20, 10]]);
    const box = image.querySelector('[aria-label^="定位未采用"]');
    expect(box?.querySelector('[aria-label="定位框"]')?.getAttribute("points")).toBe("39.5,19.5 55.5,19.5 55.5,35.5 39.5,35.5");
    expect(image.querySelector('[aria-label="定位框（按位姿）"]')).toBeNull();
  });
});
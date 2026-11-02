import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { historyApi } from "../src/features/history/api";
import { workspaceApi } from "../src/features/workspace/api";
import { burstImageApi } from "../src/features/workspace/burstImages";
import { useWorkspace } from "../src/features/workspace/context";
import { burstJudgement, burstPartDetail, burstWorkspaceView, frameMetrics, workspaceState } from "./fixtures";
import type { Comparison } from "../src/features/workspace/types";
import type { HistoryDetailState } from "../src/features/history/types";

vi.mock("../src/features/history/api", () => ({ historyApi: { detail: vi.fn(), neighbors: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  comparisons: vi.fn(), compare: vi.fn(), compareOriginal: vi.fn(), recordFrameView: vi.fn(),
} }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { live: vi.fn(), record: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
let detail: ReturnType<typeof burstPartDetail>;
type Entry = string | { pathname: string; state?: HistoryDetailState };
function element(entry: Entry = "/history/1"){return <MemoryRouter initialEntries={[entry]}><Routes><Route path="/history/:id" element={<HistoryDetailPage />} />
  <Route path="/history" element={<p>历史列表页</p>} /><Route path="/recipe/calibration" element={<p>试测页</p>} /></Routes></MemoryRouter>;}
function show(entry?: Entry) { return render(element(entry)); }
function comparison():Comparison{return {id:"compare-1",historyId:1,source:"rules",cycleId:"cycle-1",bundleId:"bundle-A",candidateId:"A",candidateRevision:7,candidateRecipe:ws.data!.layout,
  originalVerdict:"NG_GAP",judgement:{...detail.judgement,verdict:"OK",reason:"候选合格"},measurements:[],createdAt:1,burst:null};}
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView());
  detail = burstPartDetail(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.detail).mockResolvedValue(detail);
  vi.mocked(historyApi.neighbors).mockResolvedValue({ older: null, newer: null });
  vi.mocked(workspaceApi.comparisons).mockResolvedValue([]);
  vi.mocked(workspaceApi.compare).mockResolvedValue(comparison());
  vi.mocked(workspaceApi.compareOriginal).mockResolvedValue({...comparison(),source:"original"});
  vi.mocked(workspaceApi.recordFrameView).mockReset().mockResolvedValue({ segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5, edges: [], metrics: frameMetrics(0, 1) });
  vi.mocked(burstImageApi.record).mockReset().mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
});

describe("历史详情（段配方记录）", () => {
  // 防的回归：从试测“看详情”进来，返回按钮写死回历史列表，操作员要重新找到试测页
  it.each([
    ["从历史列表进入 → 返回历史列表", { pathname: "/history/1", state: { query: {} } }, "返回历史列表", "历史列表页"],
    ["从试测进入 → 返回试测", { pathname: "/history/1", state: { returnTo: { path: "/recipe/calibration", label: "返回试测" } } }, "返回试测", "试测页"],
  ] as const)("%s", async (_, entry, label, page) => {
    show(entry);
    await userEvent.click(await screen.findByRole("button", { name: label }));
    expect(screen.getByText(page)).toBeVisible();
  });

  it("按区域展示结果，同一帧组三台相机的原图并排、按 (相机, seq) 读取；缺帧的相机没有原图可读", async () => {
    show();
    const regions = within(await screen.findByRole("list", { name: "区域" }));
    expect(regions.getByText("段 1–3 · 未分段")).toBeVisible();
    expect(screen.getByText("检测不完整", { selector: "[data-flag]" })).toBeVisible();
    await waitFor(() => expect(burstImageApi.record).toHaveBeenCalledWith(detail.summary.id, 0, 1));
    await userEvent.click(screen.getByRole("button", { name: /^帧组 5 · CAM-2 ·/ }));
    expect(screen.getByLabelText("历史相机选择", { selector: "select" })).toHaveValue("1");
    await waitFor(() => expect(burstImageApi.record).toHaveBeenCalledWith(detail.summary.id, 0, 5));
    expect(await screen.findByText(/本帧组缺帧，没有原图/)).toBeVisible();
    expect(burstImageApi.record).not.toHaveBeenCalledWith(detail.summary.id, 1, 5);
  });

  it("按候选规则重判得到区域结果 → 逐区域比对表标出与原结果不同的区域", async () => {
    const burst = detail.burst!;
    ws.data!.workspace.doc.id = "S"; ws.data!.plan = burstWorkspaceView().plan;
    const replay = burstJudgement({ verdict: "NG_WIDTH", plcCode: 14, reason: "溢胶", regions: [{ ...burst.judgement.regions[0], verdict: "NG_WIDTH" }] });
    vi.mocked(workspaceApi.compare).mockResolvedValue({ ...comparison(), candidateId: "S", judgement: { ...detail.judgement, verdict: "NG_WIDTH", reason: "溢胶" },
      burst: { segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal, judgement: replay, frames: [] } });
    show();
    await userEvent.click(await screen.findByRole("button", { name: "按候选规则重判" }));
    await waitFor(() => expect(workspaceApi.compare).toHaveBeenCalledWith("S", ws.data!.workspace.revision, detail.summary.id, false));
    expect(await screen.findByText("1 / 1 个区域与原结果不同", { exact: false })).toBeVisible();
    const row = within(screen.getByRole("table", { name: "逐区域比对" })).getAllByRole("row")[1];
    expect(row).toHaveTextContent(/段 1–3 · 未分段.*OK.*NG · 胶宽超差.*结论.*不同/);
  });

  it("从试测“看详情”带对照进入 → 帧组与整件判定按对照的逐帧指标显示，原图上的测站与胶宽折线联动，可切回原始记录", async () => {
    const burst = detail.burst!;
    const edge = (y: number): [[number, number], [number, number]] => [[10, y], [20, y]];
    vi.mocked(workspaceApi.recordFrameView).mockResolvedValue({ segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5, edges: [edge(10), edge(20), edge(30)], metrics: frameMetrics(0, 1) });
    const replay = burstJudgement({ verdict: "NG_GAP", plcCode: 13, reason: "断胶" });
    vi.mocked(workspaceApi.comparisons).mockResolvedValue([{ ...comparison(), source: "raw", judgement: { ...detail.judgement, verdict: "NG_GAP", reason: "断胶" },
      burst: { segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal, judgement: replay,
        frames: burst.frames.map(f => frameMetrics(f.camera, f.seq, { gapMaxMm: f.camera === 0 && f.seq === 1 ? 12 : 0 })) } }]);
    show("/history/1?comparison=compare-1");
    expect(await screen.findByText("候选原图复测整件判定")).toBeVisible();
    expect(screen.getByText("12.00 mm")).toBeVisible();
    await waitFor(() => expect(workspaceApi.recordFrameView).toHaveBeenCalledWith(detail.summary.id, "compare-1", 0, 1));
    // 只有选中相机的原图能悬停测站；悬停第 2 站的测量线 → 胶宽折线标出第 2 站，原图上这条线加粗
    const hit = await waitFor(() => { const lines = document.querySelectorAll('[data-station="1"]'); expect(lines).toHaveLength(1); return lines[0]; });
    await userEvent.hover(hit);
    expect(screen.getByText(/逐站胶宽 · 第 2 站/)).toBeVisible();
    const lit = document.querySelectorAll('[aria-label="测量横线"] line[stroke-width="4"]');
    expect([...lit].map(line => line.getAttribute("y1"))).toEqual(["20"]);
    await userEvent.selectOptions(screen.getByLabelText("显示结果", { selector: "select" }), "original");
    expect(screen.getByText("原始整件判定")).toBeVisible();
    expect(screen.queryByText("12.00 mm")).toBeNull();
  });
});
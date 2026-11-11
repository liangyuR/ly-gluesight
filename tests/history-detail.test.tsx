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

vi.mock("../src/features/history/api", () => ({ historyApi: { overview: vi.fn(), frameDetail: vi.fn(), neighbors: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  comparisonSummaries: vi.fn(), comparisonGet: vi.fn(), compare: vi.fn(), compareOriginal: vi.fn(), recordFrameView: vi.fn(), historyFrameView: vi.fn(), historyCompareOptions: vi.fn(),
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
  vi.mocked(historyApi.overview).mockResolvedValue(detail);
  vi.mocked(historyApi.frameDetail).mockImplementation(async (_, camera, seq) => ({ ...detail, burst: { ...detail.burst!, frames: detail.burst!.frames.filter(frame => frame.camera === camera && frame.seq === seq) } }));
  vi.mocked(workspaceApi.comparisonGet).mockImplementation(async (_, id) => (await workspaceApi.comparisonSummaries(detail.summary.id)).find(item => item.id === id) as Comparison);
  vi.mocked(historyApi.neighbors).mockResolvedValue({ older: null, newer: null });
  vi.mocked(workspaceApi.comparisonSummaries).mockResolvedValue([]);
  vi.mocked(workspaceApi.compare).mockResolvedValue(comparison());
  vi.mocked(workspaceApi.compareOriginal).mockResolvedValue({...comparison(),source:"original"});
  vi.mocked(workspaceApi.recordFrameView).mockReset().mockResolvedValue({ segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5, edges: [], partEdges: [], metrics: frameMetrics(0, 1) });
  const yes = { available: true, reason: null };
  vi.mocked(workspaceApi.historyCompareOptions).mockReset().mockResolvedValue({ candidate: { id: "S", revision: 9, version: 2, name: "段配方" }, choices: [{ id: "S", revision: 9, version: 2, name: "段配方" }], original: yes, rules: yes, raw: yes });
  vi.mocked(workspaceApi.historyFrameView).mockReset().mockResolvedValue({ segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5, edges: [], partEdges: [], metrics: frameMetrics(0, 1) });
  vi.mocked(burstImageApi.record).mockReset().mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
});

describe("历史详情（段配方记录）", () => {
  it("对照有生产跳过区域 → 帧格、详情和区域显示跳过，切回原始记录恢复真实NG", async () => {
    const burst = detail.burst!;
    burst.frameVerdicts = burst.frameVerdicts.map(v => v.camera === 0 && v.seq === 1 ? { ...v, verdict: "NG_GAP" } : v);
    const base = burst.judgement.regions[0];
    const skipped = { ...base, region: { firstSeq: 1, lastSeq: 4, kind: { type: "skipped" as const, segment: 1, pass: 1 } }, frameCount: 4 };
    vi.mocked(workspaceApi.comparisonSummaries).mockResolvedValue([{ ...comparison(), burst: {
      segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal,
      judgement: { ...burst.judgement, regions: [skipped, { ...base, region: { ...base.region, firstSeq: 5 } }] },
      frames: burst.frames.map(f => frameMetrics(f.camera, f.seq, { presentRatio: 0 })),
      frameVerdicts: burst.frameVerdicts,
    } } as Comparison]);
    show("/history/1?comparison=compare-1");
    // 文件里第一个用例要承担首次渲染的预热，CI 上常超过默认 1 s
    const cell = await screen.findByRole("button", { name: "帧组 1 · CAM-1 · 生产跳过" }, { timeout: 5000 });
    expect(cell).toHaveStyle({ background: "var(--outline-variant)" });
    expect(screen.getByText("生产跳过 4 帧组 · 不参与判定")).toBeVisible();
    await userEvent.click(cell);
    expect(screen.getByText(/不参与判定，原图与测量数据仅供诊断/)).toBeVisible();
    const region = screen.getByRole("list", { name: "区域" }).querySelector('[data-kind="skipped"]')!;
    expect(region).toHaveAttribute("data-tone", "neutral");
    expect(within(region as HTMLElement).getByText("生产跳过")).toBeVisible();
    expect(within(region as HTMLElement).queryByText("OK")).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText("显示结果", { selector: "select" }), "original");
    expect(await screen.findByRole("button", { name: "帧组 1 · CAM-1 · NG" })).toHaveStyle({ background: "var(--ng)" });
    expect(screen.queryByText("生产跳过 4 帧组 · 不参与判定")).toBeNull();
    expect(screen.queryByText(/不参与判定，原图与测量数据仅供诊断/)).toBeNull();
  });

  it("加载旧对照期间重新复算 → 迟到的旧对照不能覆盖新结果", async () => {
    const old = { ...comparison(), judgement: { ...comparison().judgement, reason: "旧对照结果" } };
    vi.mocked(workspaceApi.comparisonSummaries).mockResolvedValue([old]);
    let finish!: (value: Comparison) => void;
    vi.mocked(workspaceApi.comparisonGet).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const next = { ...comparison(), id: "new-comparison", judgement: { ...comparison().judgement, reason: "最新复算结果" } };
    vi.mocked(workspaceApi.compare).mockResolvedValue(next);
    show();
    await userEvent.selectOptions(await screen.findByLabelText("已保存对照结果", { selector: "select" }), old.id);
    await userEvent.click(screen.getByRole("button", { name: "按候选规则重判" }));
    expect(await screen.findByText("最新复算结果")).toBeVisible();
    finish(old);
    await waitFor(() => expect(screen.queryByText("正在读取对照结果…")).toBeNull());
    expect(screen.getByText("最新复算结果")).toBeVisible();
    expect(screen.queryByText("旧对照结果")).toBeNull();
  });

  // 防的回归：从试测“看详情”进来，返回按钮写死回历史列表，操作员要重新找到试测页
  it("从试测进入历史详情 → 返回试测页", async () => {
    show({ pathname: "/history/1", state: { returnTo: { path: "/recipe/calibration", label: "返回试测" } } });
    await userEvent.click(await screen.findByRole("button", { name: "返回试测" }));
    expect(screen.getByText("试测页")).toBeVisible();
  });

  // 防的回归：区域 NG 时帧组条把整段的帧都涂红；格子只按本帧结论着色，区域结论放在段条上
  it("区域 NG 时只有超差帧是红色，同段其它帧仍按本帧结论显示，段条标出区域结论", async () => {
    const region = { ...detail.burst!.judgement.regions[0], verdict: "NG_WIDTH" as const, reason: "胶宽超差" };
    detail.burst = { ...detail.burst!, judgement: { ...detail.burst!.judgement, verdict: "NG_WIDTH", regions: [region] },
      frameVerdicts: detail.burst!.frameVerdicts.map(v => v.camera === 0 && v.seq === 3 ? { ...v, verdict: "NG_WIDTH" as const } : v) };
    show();
    expect(await screen.findByRole("button", { name: "帧组 3 · CAM-1 · NG" })).toBeVisible();
    expect(screen.getByRole("button", { name: "帧组 3 · CAM-2 · OK" })).toBeVisible();
    expect(screen.getByRole("button", { name: "帧组 4 · CAM-1 · OK" })).toBeVisible();
    expect(screen.getByRole("img", { name: "段 1–3 · 未分段 · NG · 胶宽超差：胶宽超差" })).toBeVisible();
  });

  it("按候选规则重判得到区域结果 → 逐区域比对表标出与原结果不同的区域", async () => {
    const burst = detail.burst!;
    const replay = burstJudgement({ verdict: "NG_WIDTH", plcCode: 14, reason: "溢胶", regions: [{ ...burst.judgement.regions[0], verdict: "NG_WIDTH" }] });
    vi.mocked(workspaceApi.compare).mockResolvedValue({ ...comparison(), candidateId: "S", judgement: { ...detail.judgement, verdict: "NG_WIDTH", reason: "溢胶" },
      burst: { segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal, judgement: replay, frames: [] } });
    show();
    await userEvent.click(await screen.findByRole("button", { name: "按候选规则重判" }));
    // 工作台选的是别的配方（多配方并行）：仍按记录配方 S 的候选对照
    await waitFor(() => expect(workspaceApi.compare).toHaveBeenCalledWith("S", 9, detail.summary.id, false, expect.any(String)));
    expect(await screen.findByText("1 / 1 个区域与原结果不同", { exact: false })).toBeVisible();
    const row = within(screen.getByRole("table", { name: "逐区域比对" })).getAllByRole("row")[1];
    expect(row).toHaveTextContent(/段 1–3 · 未分段.*OK.*NG · 胶宽超差.*结论.*不同/);
  });

  it("从试测“看详情”带对照进入 → 帧组与整件判定按对照的逐帧指标显示，原图上的测站与胶宽折线联动，可切回原始记录", async () => {
    const burst = detail.burst!;
    const edge = (y: number): [[number, number], [number, number]] => [[10, y], [20, y]];
    vi.mocked(workspaceApi.recordFrameView).mockResolvedValue({ segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5, edges: [edge(10), edge(20), edge(30)], partEdges: [], metrics: frameMetrics(0, 1) });
    const replay = burstJudgement({ verdict: "NG_GAP", plcCode: 13, reason: "断胶" });
    vi.mocked(workspaceApi.comparisonSummaries).mockResolvedValue([{ ...comparison(), source: "raw", judgement: { ...detail.judgement, verdict: "NG_GAP", reason: "断胶" },
      burst: { segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal, judgement: replay,
        frames: burst.frames.map(f => frameMetrics(f.camera, f.seq, { gapMaxMm: f.camera === 0 && f.seq === 1 ? 12 : 0 })) } } as Comparison]);
    show("/history/1?comparison=compare-1");
    expect(await screen.findByText("候选原图复测整件判定")).toBeVisible();
    expect(screen.getByText("12.00 mm")).toBeVisible();
    await waitFor(() => expect(workspaceApi.recordFrameView).toHaveBeenCalledWith(detail.summary.id, "compare-1", 0, 1, expect.any(AbortSignal)));
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

  // 防的回归：复测按钮只在工作台选中本配方时可用、禁用时不说原因，现场多配方并行时用不了
  it("复测不能做时按钮禁用并写明原因；工作台正在编辑本配方且有未保存改动时先要求保存", async () => {
    vi.mocked(workspaceApi.historyCompareOptions).mockResolvedValue({ candidate: null, choices: [{ id: "S", revision: 9, version: 2, name: "段配方" }], original: { available: true, reason: null },
      rules: { available: false, reason: "这个配方在本机没有候选" }, raw: { available: false, reason: "这个配方在本机没有候选" } });
    const view = show();
    const reasons = within(await screen.findByRole("list", { name: "不能复测的原因" }));
    expect(reasons.getByRole("listitem")).toHaveTextContent("按候选规则重判、从原图复测：这个配方在本机没有候选");
    expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "按原发布包重现" })).toBeEnabled();
    view.unmount();
    ws.data!.workspace.doc.id = "S"; ws.dirty = true;
    vi.mocked(workspaceApi.historyCompareOptions).mockResolvedValue({ candidate: { id: "S", revision: 9, version: 2, name: "段配方" }, choices: [{ id: "S", revision: 9, version: 2, name: "段配方" }], original: { available: true, reason: null },
      rules: { available: true, reason: null }, raw: { available: true, reason: null } });
    show();
    expect(within(await screen.findByRole("list", { name: "不能复测的原因" })).getByRole("listitem")).toHaveTextContent("先保存候选配置和走廊示教，再执行对照。");
    expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeDisabled();
  });

  // 防的回归（D-SB-58）：只能用记录所属配方的候选复测，换别的配方（型号对不上、别的项目）验证这件记录做不到
  it("选别的配方复测 → 按所选候选查能不能做，并用它发起原图复测", async () => {
    const yes = { available: true, reason: null };
    const S = { id: "S", revision: 9, version: 2, name: "段配方" }, B = { id: "B", revision: 4, version: 1, name: "别的型号" };
    vi.mocked(workspaceApi.historyCompareOptions).mockImplementation(async (_, candidate) =>
      ({ candidate: candidate === "B" ? B : S, choices: [B, S], original: yes, rules: yes, raw: yes }));
    show();
    const picker = await screen.findByLabelText("复测配方", { selector: "select" });
    await userEvent.selectOptions(picker, "B");
    await waitFor(() => expect(workspaceApi.historyCompareOptions).toHaveBeenLastCalledWith(detail.summary.id, "B"));
    await waitFor(() => expect(picker).toHaveValue("B"));
    await userEvent.click(screen.getByRole("button", { name: "从原图复测整件" }));
    await waitFor(() => expect(workspaceApi.compare).toHaveBeenCalledWith("B", 4, detail.summary.id, true, expect.any(String)));
  });

  // 防的回归（D-SB-57）：有段没测的原图复测被当成整件结论，操作员以为没测的段也合格
  it("原图复测有段没测时，复测前写明哪些段不测，结果里列出没测的段", async () => {
    const yes = { available: true, reason: null };
    vi.mocked(workspaceApi.historyCompareOptions).mockResolvedValue({ candidate: { id: "S", revision: 9, version: 2, name: "段配方" }, choices: [{ id: "S", revision: 9, version: 2, name: "段配方" }], original: yes, rules: yes,
      raw: { ...yes, note: "这些段没有可用走廊，复测时不测：第 2 段相机 CAM-2：尚未示教检测走廊" } });
    const burst = detail.burst!;
    vi.mocked(workspaceApi.comparisonSummaries).mockResolvedValue([{ ...comparison(), source: "raw", burst: { segments: burst.segments, cameras: burst.cameras,
      expectedTotal: burst.expectedTotal, judgement: burst.judgement, frames: [], unmeasured: [{ segment: 2, camera: "CAM-2", reason: "第 2 段相机 CAM-2：尚未示教检测走廊" }] } } as Comparison]);
    show("/history/1?comparison=compare-1");
    expect(await screen.findByText(/从原图复测：这些段没有可用走廊，复测时不测/)).toBeVisible();
    expect(within(await screen.findByRole("list", { name: "没测的段" })).getByRole("listitem")).toHaveTextContent("第 2 段相机 CAM-2：尚未示教检测走廊");
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeEnabled();
  });

  // 防的回归：原始记录只有选了候选对照才有辅助线，现场多配方并行时看不到；原始记录按记录自己的发布包取叠图
  it("看原始记录时不需要对照，按记录的发布包为每台相机取叠图并画在原图上；缺帧的相机不取", async () => {
    const edge = (y: number): [[number, number], [number, number]] => [[10, y], [20, y]];
    vi.mocked(workspaceApi.historyFrameView).mockResolvedValue({ segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5, edges: [edge(10), edge(20)], partEdges: [], metrics: frameMetrics(0, 1) });
    show();
    expect(within(await screen.findByRole("list", { name: "区域" })).getByText("段 1–3 · 未分段")).toBeVisible();
    expect(screen.getByText("检测不完整", { selector: "[data-flag]" })).toBeVisible();
    await waitFor(() => expect(workspaceApi.historyFrameView).toHaveBeenCalledWith(detail.summary.id, 0, 1, expect.any(AbortSignal)));
    // 三台相机都测成，每幅原图都叠出两条测量横线和搜索带
    await waitFor(() => expect(document.querySelectorAll('[aria-label="测量横线"]')).toHaveLength(3));
    expect(document.querySelectorAll('[aria-label="测量横线"] line')).toHaveLength(6);
    expect(document.querySelectorAll('[aria-label="搜索带边界"]')).toHaveLength(3);
    await userEvent.click(screen.getByRole("button", { name: /^帧组 5 · CAM-2 ·/ }));
    await waitFor(() => expect(workspaceApi.historyFrameView).toHaveBeenCalledWith(detail.summary.id, 0, 5, expect.any(AbortSignal)));
    expect(vi.mocked(workspaceApi.historyFrameView).mock.calls.some(([id, camera, seq]) => id === detail.summary.id && camera === 1 && seq === 5)).toBe(false);
    expect(screen.getByLabelText("历史相机选择", { selector: "select" })).toHaveValue("1");
    expect(await screen.findByText(/本帧组缺帧，没有原图/)).toBeVisible();
    expect(burstImageApi.record).toHaveBeenCalledWith(detail.summary.id, 0, 5, expect.any(AbortSignal));
    expect(vi.mocked(burstImageApi.record).mock.calls.some(([id, camera, seq]) => id === detail.summary.id && camera === 1 && seq === 5)).toBe(false);
    expect(workspaceApi.recordFrameView).not.toHaveBeenCalled();
  });
});

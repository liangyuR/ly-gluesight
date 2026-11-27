import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { historyApi } from "../src/features/history/api";
import { workspaceApi } from "../src/features/workspace/api";
import { burstImageApi } from "../src/features/workspace/burstImages";
import { useWorkspace } from "../src/features/workspace/context";
import { scheduleFrameRead } from "../src/features/workspace/previewCache";
import * as desktop from "../src/lib/desktop";
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
  it("历史混胶报警帧 → 叠加生产记录冻结的轮廓并区分合成来源", async () => {
    const record = detail.burst!.frames.find(f => f.camera === 0 && f.seq === 1)!;
    const evidence = {
      modelId: "model-1", mmPerPx: 0.1, datasetKind: "synthetic" as const,
      incomplete: false, reason: null, directions: ["yellow" as const], shiftRuns: [],
      observation: {
        stations: [], background: { validPixels: 1, totalPixels: 1, delta: [0, 0] as [number, number], normalizedShift: 0 },
        regions: [{ direction: "yellow" as const, contours: [[[11, 12], [41, 12], [40, 15]] as [number, number][]],
          startPx: 1, endPx: 31, confirmedLengthPx: 30, spanPx: 30, maxWidthPx: 3, areaPx2: 80 }],
      },
    };
    record.metrics = frameMetrics(0, 1, { mixEvidence: { ...evidence, observation: null } });
    vi.mocked(historyApi.frameDetail).mockResolvedValue({ ...detail, burst: {
      ...detail.burst!, frames: [{ ...record, metrics: frameMetrics(0, 1, { mixEvidence: evidence }) }],
    } });
    show();
    const polygons = await screen.findByLabelText("混胶色差轮廓", {}, { timeout: 5000 });
    expect(polygons.querySelector("polygon")).toHaveAttribute("points", "11,12 41,12 40,15");
    expect(screen.getAllByText(/合成联调 · PC 混胶报警 · 偏黄/).length).toBeGreaterThan(0);
  });

  it("带定位和候选的历史链接 → 首次加载直接读取目标帧组和候选", async () => {
    const candidate = { id: "B", revision: 4, version: 3, name: "复测候选" };
    const yes = { available: true, reason: null };
    vi.mocked(workspaceApi.historyCompareOptions).mockResolvedValue({ candidate, choices: [candidate], original: yes, rules: yes, raw: yes });
    vi.mocked(workspaceApi.historyFrameView).mockImplementation(async (_, camera, seq) => ({
      segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5,
      edges: [[[10, seq + 10], [20, seq + 10]]], partEdges: [], metrics: frameMetrics(camera, seq),
    }));

    show("/history/1?camera=2&seq=5&candidate=B&view=original");

    expect(await screen.findByLabelText("复测配方", { selector: "select" })).toHaveValue("B");
    const image = await screen.findByRole("img", { name: "原始 SN SN-101 · CAM-3 · 帧组 5" });
    const lines = await within(image).findByLabelText("测量横线");
    expect(lines.querySelector("line")).toHaveAttribute("y1", "15");
    expect(await screen.findByText(/CAM-3 · 帧组 5 逐站胶宽/)).toBeVisible();

    expect(new Set(vi.mocked(burstImageApi.record).mock.calls.map(([, , seq]) => seq))).toEqual(new Set([5]));
    expect(new Set(vi.mocked(workspaceApi.historyFrameView).mock.calls.map(([, , seq]) => seq))).toEqual(new Set([5]));
    expect(new Set(vi.mocked(historyApi.frameDetail).mock.calls.map(([, camera, seq]) => `${camera}:${seq}`))).toEqual(new Set(["2:5"]));
    expect(new Set(vi.mocked(workspaceApi.historyCompareOptions).mock.calls.map(([, id]) => id))).toEqual(new Set(["B"]));
  });

  it("历史详情快速翻帧 → 跳过过期逐站读取，原图队列忙时仍显示目标帧指标", async () => {
    const actual = await vi.importActual<typeof import("../src/features/history/api")>("../src/features/history/api");
    vi.mocked(historyApi.frameDetail).mockImplementation(actual.historyApi.frameDetail);
    const burst = detail.burst!;
    const detailFor = (camera: number, seq: number) => ({ ...detail, burst: { ...burst,
      frames: burst.frames.filter(frame => frame.camera === camera && frame.seq === seq).map(frame => ({
        ...frame, metrics: frameMetrics(camera, seq, { widthMin: seq + 10, widthMean: seq + 11, widthMax: seq + 12 }),
      })),
    } });
    let finishFirst!: (value: typeof detail) => void;
    const first = new Promise<typeof detail>(resolve => { finishFirst = resolve; });
    const requested: number[] = [];
    const desktopCall = desktop.desktopCall;
    vi.spyOn(desktop, "desktopCall").mockImplementation(async (command, args) => {
      if (command !== "history_frame_detail") return desktopCall(command, args);
      const { camera, seq } = args as { camera: number; seq: number };
      requested.push(seq);
      return seq === 1 ? first : detailFor(camera, seq);
    });

    let releaseImages!: () => void;
    const blockedImage = new Promise<void>(resolve => { releaseImages = resolve; });
    const imageReads = [scheduleFrameRead(() => blockedImage), scheduleFrameRead(() => blockedImage)];
    try {
      show();
      const user = userEvent.setup();
      await waitFor(() => expect(requested).toEqual([1]));
      for (const seq of [3, 4, 6]) {
        await user.click(screen.getByRole("button", { name: `帧组 ${seq} · CAM-1 · OK` }));
      }
      expect(screen.getByRole("button", { name: "帧组 6 · CAM-1 · OK", pressed: true })).toBeVisible();
      expect(requested).toEqual([1]);
      expect(screen.getByText("正在读取逐站指标…")).toBeVisible();

      await act(async () => { finishFirst(detailFor(0, 1)); });
      expect(await screen.findByText(/CAM-1 · 帧组 6 逐站胶宽/)).toBeVisible();
      expect(screen.getByText("16.00 / 18.00 / 17.00 mm")).toBeVisible();
      expect(requested).toEqual([1, 6]);
    } finally {
      await act(async () => {
        finishFirst(detailFor(0, 1));
        releaseImages();
        await Promise.all(imageReads);
      });
    }
  });

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

  it("对照相机重排且少一台 → 逐帧追溯与显示切换仍按相机身份定位", async () => {
    const burst = detail.burst!;
    const cameras = ["CAM-3", "CAM-1"];
    const frames = burst.frames.flatMap(frame => {
      const camera = cameras.indexOf(burst.cameras[frame.camera]);
      return camera >= 0 && frame.metrics ? [{ ...frame.metrics, camera }] : [];
    });
    vi.mocked(workspaceApi.comparisonSummaries).mockResolvedValue([{ ...comparison(), source: "raw", burst: {
      segments: burst.segments, cameras, expectedTotal: burst.expectedTotal, judgement: burst.judgement, frames,
    } } as Comparison]);

    show("/history/1?comparison=compare-1");
    const user = userEvent.setup();
    const display = await screen.findByLabelText("显示结果", { selector: "select" });
    const trace = within(screen.getByRole("table", { name: "逐帧追溯" }));
    const cam1 = /^帧组 4 · CAM-1 ·/;
    await user.click(screen.getByRole("button", { name: cam1 }));
    expect(trace.getByRole("button", { name: "追溯 CAM-1 帧组 4" })).toHaveAttribute("aria-pressed", "true");
    expect(trace.getByRole("button", { name: "追溯 CAM-2 帧组 4" })).toHaveAttribute("aria-pressed", "false");

    await user.selectOptions(display, "original");
    expect(screen.getByRole("button", { name: cam1, pressed: true })).toBeVisible();
    await user.selectOptions(display, "comparison");
    expect(screen.getByRole("button", { name: cam1, pressed: true })).toBeVisible();

    await user.click(trace.getByRole("button", { name: "查看 CAM-2 帧组 6 原图" }));
    expect(display).toHaveValue("original");
    expect(trace.getByRole("button", { name: "追溯 CAM-2 帧组 6", pressed: true })).toBeVisible();
    expect(await screen.findByRole("img", { name: "原始 SN SN-101 · CAM-2 · 帧组 6" })).toBeVisible();
    expect(await screen.findByText(/CAM-2 · 帧组 6 逐站胶宽/)).toBeVisible();
    await user.selectOptions(display, "comparison");
    expect(screen.getByText("候选原图复测整件判定")).toBeVisible();
    expect(screen.getByRole("button", { name: /^帧组 6 · CAM-3 ·/, pressed: true })).toBeVisible();
  });

  it("键盘查看追溯原图 → 等待读图时就定位到相机，返回时恢复追溯焦点", async () => {
    const image = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
    let finishImage!: (value: typeof image) => void;
    const pending = new Promise<typeof image>(resolve => { finishImage = resolve; });
    vi.mocked(burstImageApi.record).mockImplementation(async (_, camera, seq) => camera === 1 && seq === 6 ? pending : image);
    render(<main className="main-body">{element()}</main>);
    const user = userEvent.setup();
    const row = await screen.findByRole("button", { name: "追溯 CAM-2 帧组 6" });
    row.focus();
    await user.keyboard("{Enter}");
    expect(row).toHaveFocus();

    const tile = screen.getByRole("group", { name: "CAM-2 · 帧组 6 原图" });
    const content = screen.getByRole("main");
    content.scrollTop = 600;
    vi.spyOn(content, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 54, 1000, 720));
    vi.spyOn(tile, "getBoundingClientRect").mockReturnValue(new DOMRect(10, -360, 280, 240));
    const openerName = "查看 CAM-2 帧组 6 原图";
    const opener = screen.getByRole("button", { name: openerName });
    opener.focus();
    await user.keyboard("{Enter}");
    expect(tile).toHaveFocus();
    expect(content.scrollTop).toBeLessThan(600);
    expect(within(tile).getByText("正在读取原图…")).toBeVisible();

    await act(async () => { finishImage(image); });
    expect(within(tile).getByRole("img", { name: "原始 SN SN-101 · CAM-2 · 帧组 6" })).toBeVisible();
    expect(tile).toHaveFocus();
    await user.tab();
    expect(within(tile).getByRole("button", { name: "返回逐帧追溯" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(opener).toHaveFocus();

    await user.keyboard("{Enter}");
    const cameraFilter = screen.getByLabelText("相机筛选", { selector: "select" });
    await user.selectOptions(cameraFilter, "0");
    expect(screen.queryByRole("button", { name: openerName })).not.toBeInTheDocument();
    await user.click(within(tile).getByRole("button", { name: "返回逐帧追溯" }));
    expect(screen.getByRole("group", { name: "逐帧追溯筛选" })).toHaveFocus();
    expect(cameraFilter).toHaveValue("0");
  });

  it("查看原图途中翻到下一页 → 返回定位到原逐帧追溯行和查看按钮", async () => {
    const burst = detail.burst!;
    burst.cameras = ["CAM-1"];
    burst.cameraFrames = [51];
    burst.judgement.groupCount = 51;
    burst.judgement.regions[0].region.lastSeq = 51;
    const frame = burst.frames[0];
    burst.frames = Array.from({ length: 51 }, (_, index) => {
      const seq = index + 1;
      return { ...frame, seq, frameCounter: seq, triggerCounter: seq, metrics: frameMetrics(0, seq), rawFile: `frames/CAM-1/${seq}.pgm` };
    });
    burst.frameVerdicts = burst.frames.map(({ seq }) => ({ camera: 0, seq, verdict: "OK" }));

    show("/history/1?seq=50&view=original");
    const user = userEvent.setup();
    const openerName = "查看 CAM-1 帧组 50 原图";
    await user.click(await screen.findByRole("button", { name: openerName }));
    const tile = screen.getByRole("group", { name: "CAM-1 · 帧组 50 原图" });
    expect(tile).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("button", { name: "查看 CAM-1 帧组 51 原图" })).toBeVisible();
    expect(screen.queryByRole("button", { name: openerName })).not.toBeInTheDocument();

    const currentTile = screen.getByRole("group", { name: "CAM-1 · 帧组 51 原图" });
    await user.click(within(currentTile).getByRole("button", { name: "返回逐帧追溯" }));
    const returned = await screen.findByRole("button", { name: openerName });
    await waitFor(() => expect(returned).toHaveFocus());
    expect(screen.getByText("第 1 / 2 页")).toBeVisible();
    expect(screen.getByRole("group", { name: "CAM-1 · 帧组 51 原图" })).toBeVisible();
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
    expect(workspaceApi.historyFrameView).not.toHaveBeenCalled();
    expect(historyApi.frameDetail).not.toHaveBeenCalled();
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
    expect(await screen.findByText(/CAM-1 · 帧组 1 逐站胶宽/)).toBeVisible();
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

  it("各相机原图就绪后才读取叠图 → 慢图和失败图不请求，旧图迟到不复算", async () => {
    const image = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
    let finishOld!: (value: typeof image) => void;
    const oldImage = new Promise<typeof image>(resolve => { finishOld = resolve; });
    vi.mocked(burstImageApi.record).mockImplementation(async (_, camera, seq) => {
      if (seq === 1 && camera === 0) return oldImage;
      if (seq === 1 && camera === 1) throw new Error("模拟录制原图读取失败");
      return image;
    });
    vi.mocked(workspaceApi.historyFrameView).mockImplementation(async (_, camera, seq) => ({
      segment: 1, corridor: [[0, 0], [10, 10]], searchHalfPx: 5,
      edges: [[[10, seq + 10], [20, seq + 10]]], partEdges: [], metrics: frameMetrics(camera, seq),
    }));
    const firstGroupOverlays = () => new Set(vi.mocked(workspaceApi.historyFrameView).mock.calls
      .filter(([, , seq]) => seq === 1).map(([, camera]) => camera));

    show();
    const ready = await screen.findByRole("img", { name: "原始 SN SN-101 · CAM-3 · 帧组 1" });
    expect(await within(ready).findByLabelText("测量横线")).toBeVisible();
    expect(within(screen.getByRole("group", { name: "CAM-1 · 帧组 1 原图" })).getByText("正在读取原图…")).toBeVisible();
    const failed = within(screen.getByRole("group", { name: "CAM-2 · 帧组 1 原图" }));
    expect(await failed.findByText("原图不可用")).toBeVisible();
    expect(failed.getByText("模拟录制原图读取失败")).toBeVisible();
    expect(firstGroupOverlays()).toEqual(new Set([2]));

    await userEvent.click(screen.getByRole("button", { name: "帧组 6 · CAM-1 · OK" }));
    const current = await screen.findByRole("img", { name: "原始 SN SN-101 · CAM-1 · 帧组 6" });
    const lines = await within(current).findByLabelText("测量横线");
    expect(lines.querySelector("line")).toHaveAttribute("y1", "16");

    await act(async () => { finishOld(image); });
    expect(firstGroupOverlays()).toEqual(new Set([2]));
    expect(current).toBeVisible();
    expect(lines.querySelector("line")).toHaveAttribute("y1", "16");
  });
});

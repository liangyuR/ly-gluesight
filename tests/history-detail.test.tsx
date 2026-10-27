import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { historyApi } from "../src/features/history/api";
import { workspaceApi } from "../src/features/workspace/api";
import { burstImageApi } from "../src/features/workspace/burstImages";
import { useWorkspace } from "../src/features/workspace/context";
import { burstJudgement, burstPartDetail, burstWorkspaceView, workspaceState } from "./fixtures";
import type { Comparison } from "../src/features/workspace/types";

vi.mock("../src/features/history/api", () => ({ historyApi: { detail: vi.fn(), neighbors: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  comparisons: vi.fn(), compare: vi.fn(), compareOriginal: vi.fn(),
} }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { live: vi.fn(), record: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
let detail: ReturnType<typeof burstPartDetail>;
function element(){return <MemoryRouter initialEntries={["/history/1"]}><Routes><Route path="/history/:id" element={<HistoryDetailPage />} /></Routes></MemoryRouter>;}
function show() { return render(element()); }
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
  vi.mocked(burstImageApi.record).mockReset().mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
});

describe("历史详情（段配方记录）", () => {
  it("维护采集记录 → 摘要显示未判定而不是 ERR", async () => {
    detail.summary.mode = "maintenanceCollect";
    detail.summary.verdict = "ERR_INSPECT";
    show();
    const summary = await screen.findByRole("region", { name: "历史记录摘要" });
    expect(summary).toHaveTextContent("维护采集·未判定");
    expect(summary).not.toHaveTextContent("ERR");
  });
  it("按区域展示结果，原图按 (相机, seq) 读取；缺帧的相机没有原图可读", async () => {
    show();
    const regions = within(await screen.findByRole("list", { name: "区域" }));
    expect(regions.getByText("段 1–3 · 未分段")).toBeVisible();
    expect(screen.getByText("检测不完整", { selector: "[data-flag]" })).toBeVisible();
    await waitFor(() => expect(burstImageApi.record).toHaveBeenLastCalledWith(detail.summary.id, 0, 1));
    await userEvent.click(screen.getByRole("button", { name: /^帧组 5 · CAM-1 ·/ }));
    await waitFor(() => expect(burstImageApi.record).toHaveBeenLastCalledWith(detail.summary.id, 0, 5));
    await userEvent.click(screen.getByRole("button", { name: /^帧组 5 · CAM-2 ·/ }));
    expect(screen.getByLabelText("历史相机选择", { selector: "select" })).toHaveValue("1");
    expect(await screen.findByText(/本帧组缺帧，没有原图/)).toBeVisible();
    expect(burstImageApi.record).toHaveBeenCalledTimes(2);
  });

  it("逐帧追溯每台相机每个帧组一行、缺帧也有 → 只看异常帧后只剩缺帧的那一行", async () => {
    show();
    const table = await screen.findByRole("table", { name: "逐帧追溯" });
    expect(within(table).getAllByRole("row")).toHaveLength(1 + 3 * 12);
    await userEvent.click(screen.getByRole("checkbox", { name: /只看异常帧/ }));
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent("CAM-2 · #5");
    expect(rows[0]).toHaveTextContent("缺帧");
  });

  it("按候选规则重判得到区域结果 → 逐区域比对表标出与原结果不同的区域", async () => {
    const burst = detail.burst!;
    ws.data!.workspace.doc.id = "S"; ws.data!.plan = burstWorkspaceView().plan;
    const replay = burstJudgement({ verdict: "NG_WIDTH", plcCode: 14, reason: "溢胶", regions: [{ ...burst.judgement.regions[0], verdict: "NG_WIDTH" }] });
    vi.mocked(workspaceApi.compare).mockResolvedValue({ ...comparison(), candidateId: "S", judgement: { ...detail.judgement, verdict: "NG_WIDTH", reason: "溢胶" },
      burst: { segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal, judgement: replay } });
    show();
    await userEvent.click(await screen.findByRole("button", { name: "按候选规则重判" }));
    await waitFor(() => expect(workspaceApi.compare).toHaveBeenCalledWith("S", ws.data!.workspace.revision, detail.summary.id, false));
    expect(await screen.findByText("1 / 1 个区域与原结果不同", { exact: false })).toBeVisible();
    const row = within(screen.getByRole("table", { name: "逐区域比对" })).getAllByRole("row")[1];
    expect(row).toHaveTextContent(/段 1–3 · 未分段.*OK.*NG · 胶宽超差.*结论.*不同/);
  });
});

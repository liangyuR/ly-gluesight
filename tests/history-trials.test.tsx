import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HistoryTrials } from "../src/features/workspace/HistoryTrials";
import { workspaceApi } from "../src/features/workspace/api";
import { useWorkspace } from "../src/features/workspace/context";
import type { Comparison, HistoryCandidate } from "../src/features/workspace/types";
import { burstWorkspaceView, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { historyCandidates: vi.fn(), compare: vi.fn() } }));

const row = { recordId: 7, cycleId: "cycle-7", ts: 1, sn: "SN-7", mode: "production", verdict: "OK", groupCount: 12, cameras: [], complete: true, recipeId: "S", projectId: "line-a",
  reasons: [], warnings: [], durationMs: null, measuredFps: null, setFps: null, importedRound: null, usage: null, fixture: null } satisfies HistoryCandidate;

let ws: ReturnType<typeof workspaceState>;
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView());
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.historyCandidates).mockResolvedValue({ total: 1, items: [row] });
});

describe("历史件试测", () => {
  // 防的回归（D-SB-57）：改段后有段没示教走廊时，试测按钮整体禁用，帧数一致的历史件也测不了
  it("部分段示教了走廊即可试测，并说明没示教的段不测；一段都没示教时才禁用", async () => {
    const view = render(<MemoryRouter><HistoryTrials recipeId="S" /></MemoryRouter>);
    expect(await screen.findByRole("button", { name: /^触发测试/ })).toBeEnabled();
    expect(screen.getByText(/走廊示教未完成（CAM-2、CAM-3）：没示教的段不测/)).toBeVisible();
    view.unmount();
    ws.data!.plan.cameraTeaching = ws.data!.plan.cameraTeaching.map(c => ({ ...c, ready: false, segments: c.segments.map(s => ({ ...s, saved: false })) }));
    render(<MemoryRouter><HistoryTrials recipeId="S" /></MemoryRouter>);
    expect(await screen.findByRole("button", { name: /^触发测试/ })).toBeDisabled();
    expect(screen.getByText(/还没有任何一段示教走廊/)).toBeVisible();
  });

  // 防的回归（D-SB-58）：试测只列本配方的历史件，型号对不上或别的项目的件不能用当前配方验证；原图不全的件整件不让测
  it("显示全部历史件后列出别的配方、别的项目的件，原图不全的件也能测", async () => {
    vi.mocked(workspaceApi.historyCandidates).mockImplementation(async (_, query) => query.ownRecipe
      ? { total: 1, items: [row] }
      : { total: 2, items: [row, { ...row, recordId: 8, sn: "SN-8", recipeId: "OTHER", projectId: "line-b", complete: false }] });
    render(<MemoryRouter><HistoryTrials recipeId="S" /></MemoryRouter>);
    await screen.findByRole("button", { name: /^触发测试/ });
    await userEvent.click(screen.getByRole("checkbox", { name: /显示全部历史件/ }));
    await waitFor(() => expect(workspaceApi.historyCandidates).toHaveBeenLastCalledWith("S", { ownRecipe: false, limit: 200 }));
    const other = await screen.findByRole("listitem", { name: /SN-8/ });
    expect(other).toHaveTextContent("OTHER · line-b");
    expect(other).toHaveTextContent("原图不全");
    expect(screen.getByRole("button", { name: /^触发测试.*SN-8/ })).toBeEnabled();
  });

  it("取消历史件重新测试 → 保留原对照并可再次测试，只显示取消状态", async () => {
    const { workspace, layout } = ws.data!;
    const saved: Comparison = {
      id: "saved-trial", historyId: row.recordId, cycleId: row.cycleId, source: "raw", bundleId: null,
      candidateId: workspace.doc.id, candidateRevision: workspace.revision, candidateRecipe: layout,
      originalVerdict: row.verdict, judgement: { verdict: "OK", plcCode: 1, faultCode: 0,
        reason: "上一次试测结果", segments: [], gaps: [] }, measurements: [], createdAt: 1, burst: null,
    };
    const compare = vi.mocked(workspaceApi.compare);
    const user = userEvent.setup();
    render(<MemoryRouter><HistoryTrials recipeId="S" /></MemoryRouter>);
    const item = within(await screen.findByRole("listitem", { name: /SN-7/ }));
    const run = item.getByRole("button", { name: /^触发测试/ });
    compare.mockResolvedValueOnce(saved);
    await user.click(run);
    expect(await item.findByText(saved.judgement.reason)).toBeVisible();
    expect(item.getByRole("link", { name: "看详情" })).toHaveAttribute("href", "/history/7?comparison=saved-trial");

    compare.mockRejectedValueOnce("OPERATION_CANCELLED: 任务已取消");
    await user.click(run);
    const status = screen.getByRole("status", { name: "历史件试测进度" });
    expect(status).toHaveTextContent("已取消，原结果保留");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(item.getByText(saved.judgement.reason)).toBeVisible();
    expect(item.getByRole("link", { name: "看详情" })).toHaveAttribute("href", "/history/7?comparison=saved-trial");
    expect(run).toBeEnabled();
    expect(run).toHaveTextContent("重新测试");

    compare.mockResolvedValueOnce({ ...saved, id: "retried-trial", judgement: { ...saved.judgement, reason: "重新试测完成" } });
    await user.click(run);
    expect(await item.findByText("重新试测完成")).toBeVisible();
    expect(item.getByRole("link", { name: "看详情" })).toHaveAttribute("href", "/history/7?comparison=retried-trial");
  });
});

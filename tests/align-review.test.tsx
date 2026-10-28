import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AlignReview from "../src/features/workspace/AlignReview";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { burstRecipe, burstWorkspaceView, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({
  workspaceApi: { captureRoundThumbs: vi.fn(), captureReviewSuggest: vi.fn(), captureReviewConfirmCuts: vi.fn(), calibrationSync: vi.fn() },
}));

beforeEach(() => {
  vi.mocked(workspaceApi.captureRoundThumbs).mockResolvedValue({ width: 0, height: 0, frames: Array(9).fill(null) });
  vi.mocked(workspaceApi.captureReviewSuggest).mockResolvedValue({
    round: "round-2", templateRound: "round-1", templateCuts: [3, 6],
    cuts: [
      { cut: 4, proportional: 3, sharpness: 2, reason: "deviates", review: true },
      { cut: 6, proportional: 6, sharpness: 2, reason: null, review: false },
    ],
  });
});

describe("标定件的边界复核", () => {
  // 防的回归：微调越过相邻边界；可信边界也要逐个确认（D-SB-51），或需重点复核的边界没确认就能保存；
  // 确认后没按新修订同步标定件，这一件确认了边界却没加入标定
  it("只确认需重点复核的边界即可保存，微调不越过下一个边界，一次提交整件边界后按新修订同步标定件", async () => {
    const view = burstWorkspaceView(burstRecipe({ segments: [pathSegment(1, "straight"), pathSegment(2, "corner"), pathSegment(3, "straight")] }));
    const piece = { ...view.plan.pieces[1], review: "draft" as const, inCalibration: false };
    vi.mocked(useWorkspace).mockImplementation(() => workspaceState(view));
    vi.mocked(workspaceApi.captureReviewConfirmCuts).mockResolvedValue({ ...view, workspace: { ...view.workspace, revision: 4 } });
    const synced = [{ roundId: "round-2", added: true, issue: null }];
    vi.mocked(workspaceApi.calibrationSync).mockResolvedValue({ view, pieces: synced });
    const onSynced = vi.fn();
    const user = userEvent.setup();
    render(<MemoryRouter><AlignReview plan={view.plan} piece={piece} locked="" onSynced={onSynced} /></MemoryRouter>);

    const body = await screen.findByLabelText("边界复核");
    expect(workspaceApi.captureReviewSuggest).toHaveBeenCalledWith(view.workspace.doc.id, "round-2");
    expect(screen.getByLabelText("段 2 → 段 3 的边界")).toHaveTextContent("对齐可信");
    expect(screen.getByText("需重点复核：偏离比例换算")).toBeVisible();
    body.focus();
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}");
    expect(screen.getByLabelText("段 1 边界")).toHaveTextContent("5");

    const save = screen.getByRole("button", { name: "确认本件边界" });
    expect(save).toBeDisabled();
    await user.keyboard("{Enter}");
    await user.click(save);
    expect(workspaceApi.captureReviewConfirmCuts).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceApi.captureReviewConfirmCuts).mock.calls[0].slice(2)).toEqual(["round-2", [5, 6]]);
    expect(workspaceApi.calibrationSync).toHaveBeenCalledWith(view.workspace.doc.id, 4);
    expect(onSynced).toHaveBeenCalledWith(synced);
  });
});

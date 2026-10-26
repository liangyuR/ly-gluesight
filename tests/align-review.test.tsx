import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AlignReview from "../src/features/workspace/AlignReview";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { burstCameras, burstRecipe, burstWorkspaceView, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({
  workspaceApi: { captureList: vi.fn(), captureRoundThumbs: vi.fn(), captureReviewSuggest: vi.fn(), captureReviewConfirmCuts: vi.fn() },
}));

const round = (roundId: string, createdAt: number) =>
  ({ roundId, recipeId: "A", cameraIds: burstCameras, plannedCount: null, receivedCount: 9, state: "complete" as const, createdAt, error: null });

beforeEach(() => {
  vi.mocked(workspaceApi.captureList).mockResolvedValue([round("capture-1-0", 1), round("capture-2-0", 2)]);
  vi.mocked(workspaceApi.captureRoundThumbs).mockResolvedValue({ width: 0, height: 0, frames: Array(9).fill(null) });
  vi.mocked(workspaceApi.captureReviewSuggest).mockResolvedValue({
    round: "capture-2-0", templateRound: "capture-1-0", templateCuts: [3, 6],
    cuts: [{ cut: 4, proportional: 3, review: false }, { cut: 6, proportional: 6, review: false }],
  });
});

describe("其余轮次的对齐复核", () => {
  it("→ 微调不越过下一个边界；两行都确认后才保存，且一次提交整轮边界", async () => {
    const view = burstWorkspaceView(burstRecipe({
      segments: [pathSegment(1, "straight"), pathSegment(2, "corner"), pathSegment(3, "straight")],
    }));
    view.workspace.captureReview = {
      rounds: {},
      template: { roundId: "capture-1-0", groupCount: 9, cuts: [3, 6], segmentSignature: null, captureSignature: null, version: 1 },
    };
    const ws = workspaceState(view);
    vi.mocked(useWorkspace).mockImplementation(() => ws);
    vi.mocked(workspaceApi.captureReviewConfirmCuts).mockResolvedValue(view);
    const user = userEvent.setup();
    render(<MemoryRouter><AlignReview plan={view.plan!} locked="" /></MemoryRouter>);

    await user.click(await screen.findByRole("button", { name: /capture-2-0/ }));
    const body = await screen.findByLabelText("边界复核");
    body.focus();
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}");
    expect(screen.getByLabelText("段 1 边界")).toHaveTextContent("5");

    const save = screen.getByRole("button", { name: "确认本轮边界" });
    await user.keyboard("{Enter}");
    expect(save).toBeDisabled();
    await user.keyboard("{Enter}");
    await user.click(save);
    expect(workspaceApi.captureReviewConfirmCuts).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceApi.captureReviewConfirmCuts).mock.calls[0].slice(2)).toEqual(["capture-2-0", [5, 6]]);
  });
});

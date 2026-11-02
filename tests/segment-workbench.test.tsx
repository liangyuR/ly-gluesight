import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SegmentWorkbench from "../src/features/workspace/SegmentWorkbench";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { burstRecipe, burstWorkspaceView, deferred, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({
  workspaceApi: { captureRoundThumbs: vi.fn(), captureFrameImage: vi.fn(), segmentTemplateSave: vi.fn(), segmentTemplatePreview: vi.fn() },
}));

const ROUND = "capture-1-0";

beforeEach(() => {
  vi.mocked(workspaceApi.captureRoundThumbs).mockResolvedValue({ width: 0, height: 0, frames: Array(9).fill(null) });
  vi.mocked(workspaceApi.captureFrameImage).mockReturnValue(deferred<never>().promise);
});

describe("分段标定工作台改变段结构", () => {
  // 防的回归：改段结构不提示会清掉哪些走廊、示教件测量失效；保存没有按示教件的帧组时间轴提交边界
  it("改段结构 → 保存前按预演列出要清掉的走廊与测量失效，确认后一次提交两段的段列表与示教件的边界", async () => {
    const view = burstWorkspaceView(burstRecipe({
      segments: [pathSegment(1, "straight", { expectedFrames: 3 }), pathSegment(2, "corner", { expectedFrames: 3 }), pathSegment(3, "straight", { expectedFrames: 3 })],
    }));
    view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: ROUND };
    const ws = workspaceState(view);
    vi.mocked(useWorkspace).mockImplementation(() => ws);
    vi.mocked(workspaceApi.segmentTemplateSave).mockResolvedValue(view);
    vi.mocked(workspaceApi.segmentTemplatePreview).mockResolvedValue({ restructured: true, dropped: ["原第 3 段（直线）· CAM-1"], untaught: [], measurementCleared: true });
    const user = userEvent.setup();
    render(<MemoryRouter><SegmentWorkbench plan={view.plan!} locked="" onDirty={() => {}} /></MemoryRouter>);

    await user.click(await screen.findByRole("button", { name: "删除帧组 6 之后的边界" }));
    expect(workspaceApi.captureRoundThumbs).toHaveBeenCalledWith(ROUND, expect.any(String));
    await user.click(screen.getByRole("button", { name: "保存分段" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("原第 3 段（直线）· CAM-1");
    expect(dialog).toHaveTextContent("重新测量示教件");
    expect(workspaceApi.segmentTemplateSave).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "保存分段" }));
    expect(workspaceApi.segmentTemplateSave).toHaveBeenCalledTimes(1);
    const [, , segments, cuts] = vi.mocked(workspaceApi.segmentTemplateSave).mock.calls[0];
    expect(cuts).toEqual([3]);
    expect(segments.map(s => [s.index, s.kind, s.expectedFrames])).toEqual([[1, "straight", null], [2, "corner", null]]);
  });
});

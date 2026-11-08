import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SegmentWorkbench from "../src/features/workspace/SegmentWorkbench";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { scheduleFrameRead } from "../src/features/workspace/previewCache";
import type { FrameImage } from "../src/features/workspace/types";
import type { InspectionCoverage } from "../src/features/plan/types";
import { burstRecipe, burstWorkspaceView, deferred, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({
  workspaceApi: { captureRoundThumbs: vi.fn(), captureFrameImage: vi.fn(), segmentTemplateSave: vi.fn(), segmentTemplatePreview: vi.fn() },
}));

const ROUND = "capture-1-0";

beforeEach(() => {
  vi.mocked(workspaceApi.captureRoundThumbs).mockResolvedValue({ width: 0, height: 0, channels: 1, frames: Array(9).fill(null) });
  vi.mocked(workspaceApi.captureFrameImage).mockReturnValue(deferred<never>().promise);
});

it("分段预览快速切帧 → 取消过期排队图，空出读取槽后直接显示最新帧，卸载取消当前请求", async () => {
  const view = burstWorkspaceView();
  view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: "preview-cancel-round" };
  view.plan.pieces[0] = { ...view.plan.pieces[0], roundId: "preview-cancel-round", groupCount: 9 };
  vi.mocked(useWorkspace).mockImplementation(() => workspaceState(view));
  const first = deferred<FrameImage>(), obsolete = deferred<FrameImage>();
  const signals: AbortSignal[] = [];
  vi.mocked(workspaceApi.captureFrameImage).mockImplementation((_round, camera, seq, signal) => {
    if (signal) signals.push(signal);
    return scheduleFrameRead(() => seq === 1 ? first.promise : seq === 2 ? obsolete.promise : Promise.resolve({ url: `current-${camera}-${seq}.png`, width: 100, height: 60 }), signal);
  });
  const user = userEvent.setup();
  const page = render(<MemoryRouter><SegmentWorkbench plan={view.plan} locked="" onDirty={() => {}} /></MemoryRouter>);
  await user.click(await screen.findByRole("button", { name: "CAM-1 帧组 2" }));
  await user.click(screen.getByRole("button", { name: "CAM-1 帧组 3" }));
  try {
    await act(async () => first.resolve({ url: "old-frame.png", width: 100, height: 60 }));
    await waitFor(() => expect([...page.container.querySelectorAll(".swb-preview svg image")].map(image => image.getAttribute("href"))).toEqual(view.plan.cameras.map(camera => `current-${camera}-3.png`)));
    page.unmount();
    expect(signals.every(signal => signal.aborted)).toBe(true);
  } finally {
    await act(async () => obsolete.resolve({ url: "obsolete-frame.png", width: 100, height: 60 }));
  }
});

describe("分段标定工作台改变段结构", () => {
  // 防的回归：改段结构不提示会清掉哪些走廊、示教件测量失效；保存没有按示教件的帧组时间轴提交边界
  it("改段结构 → 保存前按预演列出要清掉的走廊与测量失效，确认后一次提交两段的段列表与示教件的边界", async () => {
    const view = burstWorkspaceView(burstRecipe({
      segments: [pathSegment(1, "straight", { expectedFrames: 3 }), pathSegment(2, "corner", { expectedFrames: 3 }), pathSegment(3, "straight", { expectedFrames: 3 })],
    }));
    view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: ROUND };
    view.plan.pieces[0] = { ...view.plan.pieces[0], roundId: ROUND, groupCount: 9 };
    const ws = workspaceState(view);
    vi.mocked(useWorkspace).mockImplementation(() => ws);
    vi.mocked(workspaceApi.segmentTemplateSave).mockResolvedValue(view);
    vi.mocked(workspaceApi.segmentTemplatePreview).mockResolvedValue({ restructured: true, dropped: ["原第 3 段（直线）· CAM-1"], untaught: [], measurementCleared: true, coverage: null });
    const user = userEvent.setup();
    render(<MemoryRouter><SegmentWorkbench plan={view.plan!} locked="" onDirty={() => {}} /></MemoryRouter>);

    await user.click(await screen.findByRole("button", { name: "删除帧组 6 之后的边界" }));
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

  it.each([
    { trusted: true, warning: "有检测段没有有效帧", details: "应测帧数为 0" },
    { trusted: false, warning: "检测范围尚无法确认", details: "不能据此认定各段已经覆盖" },
  ])("$warning → 即使不清除示教数据，也先显示覆盖风险再允许确认保存", async ({ trusted, warning, details }) => {
    const view = burstWorkspaceView();
    view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: ROUND };
    view.plan.pieces[0] = { ...view.plan.pieces[0], roundId: ROUND, groupCount: 9 };
    vi.mocked(useWorkspace).mockImplementation(() => workspaceState(view));
    vi.mocked(workspaceApi.segmentTemplateSave).mockResolvedValue(view);
    const coverage: InspectionCoverage = {
      groupCount: 9, boundaryGuard: 1, boundariesTrusted: trusted, boundaryRefinementPending: false,
      regions: [], excludedGroups: [], expectedCameraFrames: trusted ? 0 : null, availableCameraFrames: trusted ? 0 : null,
      zeroCoverageSegments: trusted ? [2] : [],
      segments: [{ segment: 2, kind: "straight", skipped: false, range: trusted ? { firstSeq: 5, lastSeq: 6 } : null,
        cameras: [{ camera: "CAM-1", ranges: [], expectedFrames: trusted ? 0 : null, availableFrames: trusted ? 0 : null }],
        expectedCameraFrames: trusted ? 0 : null, availableCameraFrames: trusted ? 0 : null,
        offset: { camera: "CAM-1", ranges: [], expectedFrames: 0, availableFrames: 0, unjudgedReasons: [trusted ? "noInteriorFrames" : "unsegmented"] } }],
    };
    vi.mocked(workspaceApi.segmentTemplatePreview).mockResolvedValue({ restructured: false, dropped: [], untaught: [], measurementCleared: false, coverage });
    const user = userEvent.setup();
    render(<MemoryRouter><SegmentWorkbench plan={view.plan!} locked="" onDirty={() => {}} /></MemoryRouter>);
    await screen.findByRole("button", { name: /^段 1（/ });
    await user.click(await screen.findByRole("button", { name: "保存分段" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(warning);
    expect(dialog).toHaveTextContent(details);
    expect(dialog).toHaveTextContent("实测帧数");
    expect(workspaceApi.segmentTemplateSave).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "保存分段" }));
    expect(workspaceApi.segmentTemplateSave).toHaveBeenCalled();
  });

  // 防的回归（PR #214 评审）：只把一段改成不检测、边界没动时，保存后的段列表里 skip 变了，工作台却还显示原来的检测状态，
  // 再次保存会悄悄把这一段改回检测
  it("保存后段列表里某段变为不检测 → 工作台按新的段列表显示“不检测”", async () => {
    const segments = [pathSegment(1, "straight", { expectedFrames: 3 }), pathSegment(2, "corner", { expectedFrames: 3 }), pathSegment(3, "straight", { expectedFrames: 3 })];
    const view = burstWorkspaceView(burstRecipe({ segments }));
    view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: ROUND };
    view.plan.pieces[0] = { ...view.plan.pieces[0], roundId: ROUND, groupCount: 9 };
    vi.mocked(useWorkspace).mockImplementation(() => workspaceState(view));
    const page = (plan: typeof view.plan) => <MemoryRouter><SegmentWorkbench plan={plan!} locked="" onDirty={() => {}} /></MemoryRouter>;
    const { rerender } = render(page(view.plan));
    expect(await screen.findByRole("button", { name: /^段 2（弯道/ })).toBeVisible();
    rerender(page({ ...view.plan!, segments: segments.map(s => s.index === 2 ? { ...s, skip: true } : s) }));
    expect(await screen.findByRole("button", { name: /^段 2（不检测/ })).toBeVisible();
  });
});


it("千帧件缩略图未完成 → 段落立即可编辑，跨窗口跳到末帧后保存原始帧号边界", async () => {
  const view = burstWorkspaceView(burstRecipe({ segments: [pathSegment(1, "straight", { expectedFrames: 1000 })] }));
  view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: "long-pending-round" };
  view.plan.pieces[0] = { ...view.plan.pieces[0], roundId: "long-pending-round", groupCount: 1000 };
  vi.mocked(useWorkspace).mockImplementation(() => workspaceState(view));
  const pending = deferred<Awaited<ReturnType<typeof workspaceApi.captureRoundThumbs>>>();
  vi.mocked(workspaceApi.captureRoundThumbs).mockReturnValue(pending.promise);
  vi.mocked(workspaceApi.segmentTemplatePreview).mockResolvedValue({ restructured: false, dropped: [], untaught: [], measurementCleared: false, coverage: null });
  vi.mocked(workspaceApi.segmentTemplateSave).mockResolvedValue(view);
  const user = userEvent.setup();
  const page = render(<MemoryRouter><SegmentWorkbench plan={view.plan} locked="" onDirty={() => {}} /></MemoryRouter>);
  try {
    expect(screen.getByRole("status", { name: "分段草稿" })).toHaveTextContent("1000 个帧组");
    expect(page.container.querySelectorAll(".swb-cell").length).toBeLessThan(100);
    const first = screen.getByRole("button", { name: "CAM-2 帧组 1" });
    first.focus();
    fireEvent.keyDown(first, { key: "End" });
    const last = screen.getByRole("button", { name: "CAM-2 帧组 1000" });
    expect(last).toHaveAttribute("aria-pressed", "true");
    expect(last).toHaveFocus();
    expect(screen.queryByRole("button", { name: "CAM-2 帧组 1" })).not.toBeInTheDocument();
    expect(page.container.querySelectorAll(".swb-cell").length).toBeLessThan(100);
    fireEvent.keyDown(last, { key: "ArrowLeft" });
    expect(screen.getByRole("button", { name: "CAM-2 帧组 999" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "在帧组 999 后插入边界" }));
    await user.click(screen.getByRole("button", { name: "保存分段" }));
    expect(vi.mocked(workspaceApi.segmentTemplateSave).mock.lastCall?.[3]).toEqual([999]);
  } finally {
    page.unmount();
    await act(async () => pending.resolve({ width: 0, height: 0, channels: 1, frames: Array(32).fill(null) }));
  }
});

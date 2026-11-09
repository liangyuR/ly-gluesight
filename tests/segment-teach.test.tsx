import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstWorkspaceView, deferred, pathSegment, snapshot, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({
  workspaceApi: { image: vi.fn(), captureGet: vi.fn(), captureRoundThumbs: vi.fn(), captureFrameImage: vi.fn(), segmentTemplateSave: vi.fn(), segmentTemplatePreview: vi.fn() },
}));

const noImpact = { restructured: false, dropped: [], untaught: [], measurementCleared: false, coverage: null };
let ws: ReturnType<typeof workspaceState>;
const page = () => <MemoryRouter initialEntries={["/recipe/teach"]}><TeachingPage /></MemoryRouter>;

/** 三段（直线 1–4、弯道 5–8、直线 9–12）已确认边界；示教件两笔 8 + 4 帧，笔末在帧组 8。 */
function twoPasses(segments = burstRecipe().segments): WorkspaceView {
  const view = burstWorkspaceView(burstRecipe({ segments }));
  view.workspace.teachingPiece!.passes = [8, 4];
  view.workspace.captureReview = { rounds: { "round-1": { groupCount: 12, cuts: [4, 8], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
  return view;
}

beforeEach(() => {
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [] });
  vi.mocked(workspaceApi.image).mockReturnValue(deferred<never>().promise);
  vi.mocked(workspaceApi.captureGet).mockResolvedValue(null);
  vi.mocked(workspaceApi.captureRoundThumbs).mockReturnValue(deferred<never>().promise);
  vi.mocked(workspaceApi.captureFrameImage).mockReturnValue(deferred<never>().promise);
  vi.mocked(workspaceApi.segmentTemplatePreview).mockReset().mockResolvedValue(noImpact);
  vi.mocked(workspaceApi.segmentTemplateSave).mockReset();
});

// 防的回归：分段改动没有自动保存（还要找“保存分段”按钮），或会清掉已示教走廊时不先提示就保存；保存的边界丢了笔末
it.each([
  { name: "不清走廊 → 防抖后直接保存", dropped: [] as string[] },
  { name: "会清掉走廊 → 先提示，确认后才保存", dropped: ["原第 2 段（弯道）· CAM-1"] },
])("删掉 1-1 与 1-2 之间的边界，$name：一次提交两段的段列表与含笔末的边界", async ({ dropped }) => {
  ws = workspaceState(twoPasses());
  vi.mocked(workspaceApi.segmentTemplatePreview).mockResolvedValue({ ...noImpact, restructured: true, dropped });
  vi.mocked(workspaceApi.segmentTemplateSave).mockResolvedValue(ws.data!);
  const user = userEvent.setup();
  render(page());
  await user.click(await screen.findByRole("button", { name: "删除帧组 4 之后的边界" }));
  await waitFor(() => expect(workspaceApi.segmentTemplatePreview).toHaveBeenCalledTimes(1), { timeout: 2000 });
  if (dropped.length) {
    expect(await screen.findByText(/原第 2 段（弯道）· CAM-1 的示教帧会落到别的段/)).toBeVisible();
    expect(workspaceApi.segmentTemplateSave).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "确认保存" }));
  }
  await waitFor(() => expect(workspaceApi.segmentTemplateSave).toHaveBeenCalledTimes(1));
  const [, , segments, cuts] = vi.mocked(workspaceApi.segmentTemplateSave).mock.calls[0];
  expect(cuts).toEqual([8]);
  expect(segments.map(s => [s.index, s.kind])).toEqual([[1, "straight"], [2, "straight"]]);
});

// 防的回归（PR #214 评审）：只把一段改成不检测、边界没动时，保存后的段列表里 skip 变了，帧条却还显示原来的检测状态，
// 再次保存会悄悄把这一段改回检测
it("保存后段列表里某段变为不检测 → 帧条按新的段列表显示“不检测”", async () => {
  const view = twoPasses();
  ws = workspaceState(view);
  const { rerender } = render(page());
  expect(await screen.findByRole("button", { name: /^1-2（弯道/ })).toBeVisible();
  const next = structuredClone(view);
  next.plan.segments = next.plan.segments.map(s => s.index === 2 ? { ...s, skip: true } : s);
  ws = workspaceState(next);
  rerender(page());
  expect(await screen.findByRole("button", { name: /^1-2（不检测/ })).toBeVisible();
});

// 防的回归：千帧件把整笔缩略图都渲染出来卡死界面；跳到窗口外的帧后插入的边界不是原始帧号
it("千帧件缩略图未完成 → 帧条立即可编辑，跳到末帧后保存原始帧号边界", async () => {
  const view = burstWorkspaceView(burstRecipe({ segments: [pathSegment(1, "straight", { expectedFrames: 1000 })] }));
  view.workspace.teachingPiece = { ...view.workspace.teachingPiece!, roundId: "long-round" };
  view.plan.pieces[0] = { ...view.plan.pieces[0], roundId: "long-round", groupCount: 1000 };
  view.workspace.captureReview = { rounds: { "long-round": { groupCount: 1000, cuts: [], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
  ws = workspaceState(view);
  vi.mocked(workspaceApi.segmentTemplateSave).mockResolvedValue(view);
  const user = userEvent.setup();
  const { container } = render(page());
  expect(screen.getByRole("status", { name: "分段草稿" })).toHaveTextContent("1000 组");
  expect(container.querySelectorAll(".swb-cell").length).toBeLessThan(100);
  const start = screen.getByRole("button", { name: "CAM-2 帧组 501" });
  start.focus();
  fireEvent.keyDown(start, { key: "End" });
  const last = screen.getByRole("button", { name: "CAM-2 帧组 1000" });
  expect(last).toHaveAttribute("aria-pressed", "true");
  expect(last).toHaveFocus();
  expect(container.querySelectorAll(".swb-cell").length).toBeLessThan(100);
  fireEvent.keyDown(last, { key: "ArrowLeft" });
  expect(screen.getByRole("button", { name: "CAM-2 帧组 999" })).toHaveFocus();
  await user.click(screen.getByRole("button", { name: "在帧组 999 后插入边界" }));
  await waitFor(() => expect(vi.mocked(workspaceApi.segmentTemplateSave).mock.lastCall?.[3]).toEqual([999]), { timeout: 2000 });
});

// 防的回归：右栏段参数写到当前选中以外的段，或顺带改了别的段
it("左栏选 1-2 再改长度 → 候选配置只改第 2 段的长度", async () => {
  ws = workspaceState(twoPasses());
  const original = structuredClone(ws.doc!.segments!);
  const user = userEvent.setup();
  render(page());
  await user.click(await screen.findByRole("button", { name: "选择 1-2 段" }));
  const params = await screen.findByRole("region", { name: "1-2 段参数" });
  await act(async () => { fireEvent.change(within(params).getByRole("spinbutton", { name: "1-2 长度" }), { target: { value: "130" } }); });
  expect(vi.mocked(ws.setDoc).mock.lastCall![0].segments).toEqual(original.map((segment, at) => at === 1 ? { ...segment, lengthMm: 130 } : segment));
});

it("已保存长度在分段重映射后被清空 → 再改跳过帧数不恢复旧长度", async () => {
  ws = workspaceState(twoPasses());
  const user = userEvent.setup();
  const { rerender } = render(page());
  await user.click(await screen.findByRole("button", { name: "选择 1-2 段" }));
  fireEvent.change(screen.getByRole("spinbutton", { name: "1-2 长度" }), { target: { value: "130" } });
  const saved = structuredClone(ws.data!);
  saved.workspace.doc = vi.mocked(ws.setDoc).mock.lastCall![0];
  saved.plan.segments = saved.workspace.doc.segments!;
  ws = workspaceState(saved);
  rerender(page());
  expect(screen.getByRole("spinbutton", { name: "1-2 长度" })).toHaveValue(130);

  const remapped = structuredClone(saved);
  remapped.workspace.captureReview!.rounds["round-1"].cuts = [3, 8];
  remapped.workspace.doc.segments![1] = { ...remapped.workspace.doc.segments![1], lengthMm: null, skipHead: 0, skipTail: 0, expectedFrames: 5 };
  remapped.plan.segments = remapped.workspace.doc.segments!;
  ws = workspaceState(remapped);
  rerender(page());
  expect(screen.getByRole("spinbutton", { name: "1-2 长度" })).toHaveValue(null);
  fireEvent.change(screen.getByRole("spinbutton", { name: "1-2 段尾跳过帧数" }), { target: { value: "1" } });
  expect(vi.mocked(ws.setDoc).mock.lastCall![0].segments![1]).toMatchObject({ lengthMm: null, skipHead: 0, skipTail: 1 });
});

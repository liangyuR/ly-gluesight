import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import CaptureReviewPanel from "../src/features/workspace/CaptureReviewPanel";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { CaptureRoundReview, WorkspaceView } from "../src/features/workspace/types";
import { burstWorkspaceView, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  captureReviewSave: vi.fn(), captureReviewConfirm: vi.fn(), captureReviewSetTemplate: vi.fn(),
  captureReviewApplyTemplate: vi.fn(), captureReviewExclude: vi.fn(),
} }));

let view: WorkspaceView;
const review = (over: Partial<CaptureRoundReview> = {}): CaptureRoundReview => ({
  groupCount: 12, cuts: [4, 8], status: "draft", excludedGroups: [],
  segmentSignature: null, captureSignature: null, templateVersion: null, ...over,
});
const withReview = (value: CaptureRoundReview) => ({ ...view, workspace: {
  ...view.workspace, revision: view.workspace.revision + 1,
  captureReview: { ...view.workspace.captureReview!, rounds: { r1: value } },
} });

function Harness({ round = "r1", ready = true }: { round?: string; ready?: boolean }) {
  const [data, setData] = useState(view);
  const [currentSeq, setCurrentSeq] = useState(5);
  vi.mocked(useWorkspace).mockReturnValue({ ...workspaceState(data), act: async request => {
    const next = await request(); view = next; setData(next); return next;
  } });
  return <><p>预览原始帧组 {currentSeq}</p><CaptureReviewPanel key={round} roundId={round} groupCount={12}
    currentSeq={currentSeq} currentFrameReady={ready} segments={data.workspace.doc.segments!}
    disabled={false} inCalibration={false} onNavigate={setCurrentSeq}/></>;
}

beforeEach(() => {
  vi.clearAllMocks();
  view = burstWorkspaceView();
  view.workspace.captureReview = { rounds: { r1: review() }, template: {
    roundId: "reference", groupCount: 12, cuts: [4, 8], segmentSignature: null, captureSignature: null, version: 1,
  } };
});

it("模板套用到轮次 → 只生成待确认草稿，人工复核后才能确认", async () => {
  vi.mocked(workspaceApi.captureReviewApplyTemplate).mockImplementation(async () => withReview(review({ cuts: [3, 7], templateVersion: 1 })));
  vi.mocked(workspaceApi.captureReviewConfirm).mockImplementation(async () => withReview({ ...view.workspace.captureReview!.rounds.r1, status: "confirmed" }));
  render(<Harness/>);
  await userEvent.click(screen.getByRole("button", { name: "套用模板" }));
  expect(screen.getByRole("spinbutton", { name: "第 1 段结束帧组" })).toHaveValue(3);
  expect(screen.getByText("草稿待确认")).toBeVisible();
  expect(screen.getByRole("button", { name: "确认分段" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "设为模板" })).toBeDisabled();
  await userEvent.click(screen.getByRole("button", { name: "查看第 2 段首帧" }));
  expect(screen.getByText("预览原始帧组 4")).toBeVisible();
  await userEvent.click(screen.getByRole("checkbox", { name: "已查看各段边界两侧图像并确认范围" }));
  await userEvent.click(screen.getByRole("button", { name: "确认分段" }));
  expect(screen.getByText("已确认")).toBeVisible();
  expect(screen.getByRole("button", { name: "设为模板" })).toBeEnabled();
});

it("屏蔽后恢复异常范围 → 原始总帧数、段边界和后续定位序号保持不变", async () => {
  vi.mocked(workspaceApi.captureReviewExclude).mockImplementation(async (_id, _revision, _round, start, end, excluded, reason) => withReview(review({
    excludedGroups: excluded ? Array.from({ length: end - start + 1 }, (_, i) => ({ seq: start + i, reason })) : [],
  })));
  render(<Harness/>);
  await userEvent.clear(screen.getByRole("spinbutton", { name: "屏蔽范围结束帧组" }));
  await userEvent.type(screen.getByRole("spinbutton", { name: "屏蔽范围结束帧组" }), "6");
  await userEvent.click(screen.getByRole("button", { name: "屏蔽所选范围" }));
  expect(screen.getByText("原始 12 组 · 屏蔽 2 组 · 未屏蔽 10 组")).toBeVisible();
  expect(screen.getByText("当前帧组 5 已屏蔽")).toBeVisible();
  expect(screen.getByRole("spinbutton", { name: "第 2 段结束帧组" })).toHaveValue(8);
  await userEvent.click(screen.getByRole("button", { name: "查看第 3 段首帧" }));
  expect(screen.getByText("预览原始帧组 9")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "恢复所选范围" }));
  expect(screen.getByText("原始 12 组 · 屏蔽 0 组 · 未屏蔽 12 组")).toBeVisible();
  expect(screen.getByText("预览原始帧组 9")).toBeVisible();
});

it("边界倒序或越界、屏蔽范围倒序 → 阻止保存和屏蔽，旧预览不能用于设边界", async () => {
  render(<Harness ready={false}/>);
  expect(screen.getByRole("button", { name: "以当前帧结束第 1 段" })).toBeDisabled();
  const boundary = screen.getByRole("spinbutton", { name: "第 1 段结束帧组" });
  for (const value of ["9", "12", "0", "2.5"]) {
    await userEvent.clear(boundary); await userEvent.type(boundary, value);
    expect(screen.getByRole("button", { name: "保存分段草稿" })).toBeDisabled();
  }
  await userEvent.clear(screen.getByRole("spinbutton", { name: "屏蔽范围起始帧组" }));
  await userEvent.type(screen.getByRole("spinbutton", { name: "屏蔽范围起始帧组" }), "7");
  expect(screen.getByRole("button", { name: "屏蔽所选范围" })).toBeDisabled();
  await userEvent.clear(boundary); await userEvent.type(boundary, "3");
  expect(screen.getByRole("button", { name: "保存分段草稿" })).toBeEnabled();
});

it("未保存边界切换轮次后返回 → 保留编辑并禁止模板覆盖，保存成功才可复核", async () => {
  vi.mocked(workspaceApi.captureReviewSave).mockImplementation(async (_id, _revision, _round, cuts) => withReview(review({ cuts })));
  const { rerender } = render(<Harness/>);
  await userEvent.click(screen.getByRole("button", { name: "以当前帧结束第 1 段" }));
  expect(screen.getByRole("spinbutton", { name: "第 1 段结束帧组" })).toHaveValue(5);
  expect(screen.getByRole("button", { name: "套用模板" })).toBeDisabled();
  rerender(<Harness round="r2"/>);
  rerender(<Harness round="r1"/>);
  expect(screen.getByRole("spinbutton", { name: "第 1 段结束帧组" })).toHaveValue(5);
  expect(screen.getByRole("checkbox", { name: "已查看各段边界两侧图像并确认范围" })).toBeDisabled();
  await userEvent.click(screen.getByRole("button", { name: "保存分段草稿" }));
  await waitFor(() => expect(screen.queryByText("有尚未保存的边界")).toBeNull());
  expect(screen.getByRole("spinbutton", { name: "第 1 段结束帧组" })).toHaveValue(5);
  expect(screen.getByRole("checkbox", { name: "已查看各段边界两侧图像并确认范围" })).toBeEnabled();
});

it("暂存边界期间已保存边界被修改 → 提示旧版本并要求明确采纳后才能覆盖", async () => {
  const { rerender } = render(<Harness key="first"/>);
  await userEvent.click(screen.getByRole("button", { name: "以当前帧结束第 1 段" }));
  view = withReview(review({ cuts: [3, 9] }));
  rerender(<Harness key="reopened"/>);
  expect(screen.getByRole("spinbutton", { name: "第 1 段结束帧组" })).toHaveValue(5);
  expect(screen.getByText("暂存边界基于旧版本，请核对后重新保存")).toBeVisible();
  expect(screen.getByRole("button", { name: "保存分段草稿" })).toBeDisabled();
  await userEvent.click(screen.getByRole("checkbox", { name: "已核对当前段序列及已保存边界，采用此暂存边界" }));
  expect(screen.getByRole("button", { name: "保存分段草稿" })).toBeEnabled();
  await userEvent.click(screen.getByRole("button", { name: "放弃未保存边界" }));
  expect(screen.getByRole("spinbutton", { name: "第 1 段结束帧组" })).toHaveValue(3);
  expect(screen.getByRole("spinbutton", { name: "第 2 段结束帧组" })).toHaveValue(9);
});

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CalibrationPage from "../src/features/workspace/CalibrationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstWorkspaceView, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { historyCandidates: vi.fn(), measureTeaching: vi.fn(), confirmTolerance: vi.fn(), calibrationSkipSuggest: vi.fn(), confirmSkip: vi.fn(), passSkipSuggest: vi.fn(), confirmPassSkip: vi.fn(), calibrationRoundFeatures: vi.fn(), calibrationFrame: vi.fn(), captureFrameImage: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter initialEntries={["/recipe/calibration"]}><CalibrationPage /></MemoryRouter>);

/** 单个直线段、示教件已分段（N_exp 12）、走廊示教完成，还没测量示教件 */
function singleSegment(): WorkspaceView {
  const view = burstWorkspaceView(burstRecipe({ id: "A", partCalib: null, segments: [pathSegment(1, "straight", { lengthMm: 120, expectedFrames: 12 })] }));
  view.plan.cameraTeaching = view.plan.cameraTeaching.map(item => ({ ...item, ready: true }));
  view.plan.calibration = { roundId: null, measured: false, expectedTotal: 12, toleranceTotal: 1, planVersionChange: false,
    segments: [{ index: 1, kind: "straight", nExp: 12, tolerance: 1, xRef: null, xRefPending: "还没测量示教件", hints: [] }] };
  return view;
}

beforeEach(() => {
  ws = workspaceState(singleSegment());
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([]);
  vi.mocked(workspaceApi.calibrationSkipSuggest).mockResolvedValue([]);
  vi.mocked(workspaceApi.passSkipSuggest).mockResolvedValue({ current: 0, suggested: null, passes: [], pending: "示教件还没有测量" });
  vi.mocked(workspaceApi.calibrationRoundFeatures).mockResolvedValue([]);
  for (const fn of [workspaceApi.measureTeaching, workspaceApi.confirmTolerance]) vi.mocked(fn).mockResolvedValue(ws.data!);
});

describe("试测", () => {
  // 防的回归：候选还有没保存的走廊或参数时测量示教件，测的是旧走廊、得出的 x_ref 与将要保存的配方不符；保存后测量没按当前修订提交
  it("有未保存的改动 → 测量示教件禁用；保存后点击 → 按当前修订测量示教件", async () => {
    ws.dirty = true;
    const { unmount } = show();
    expect(screen.getByRole("button", { name: "测量示教件" })).toBeDisabled();
    unmount();

    ws.dirty = false;
    show();
    await userEvent.click(screen.getByRole("button", { name: "测量示教件" }));
    expect(workspaceApi.measureTeaching).toHaveBeenCalledWith("A", 3);
  });

  // 防的回归：容差不经操作员确认就写入，或确认时提交的不是键入值；非整数被提交
  it("键入容差后点确认容差 → 按键入值提交；非整数不提交并报错", async () => {
    show();
    const input = screen.getByRole("spinbutton", { name: "段 1 帧数容差" });
    expect(input).toHaveValue(1);
    await userEvent.clear(input); await userEvent.type(input, "3");
    expect(workspaceApi.confirmTolerance).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "确认段 1 容差" }));
    expect(workspaceApi.confirmTolerance).toHaveBeenCalledWith("A", 3, 1, 3);
    await userEvent.clear(input); await userEvent.type(input, "1.5");
    await userEvent.click(screen.getByRole("button", { name: "确认段 1 容差" }));
    expect(workspaceApi.confirmTolerance).toHaveBeenCalledTimes(1);
    expect(ws.setError).toHaveBeenCalled();
  });
});

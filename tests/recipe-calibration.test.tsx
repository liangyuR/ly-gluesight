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
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { historyCandidates: vi.fn(), measureTeaching: vi.fn(), calibrationSkipSuggest: vi.fn(), confirmSkip: vi.fn(), passSkipSuggest: vi.fn(), confirmPassSkip: vi.fn(), calibrationRoundOverview: vi.fn(), calibrationFrame: vi.fn(), captureFrameImage: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter initialEntries={["/recipe/calibration"]}><CalibrationPage /></MemoryRouter>);

/** 单个直线段、示教件已分段（N_exp 12）、走廊示教完成，还没测量示教件 */
function singleSegment(): WorkspaceView {
  const view = burstWorkspaceView(burstRecipe({ id: "A", partCalib: null, segments: [pathSegment(1, "straight", { lengthMm: 120, expectedFrames: 12 })] }));
  view.plan.cameraTeaching = view.plan.cameraTeaching.map(item => ({ ...item, ready: true }));
  view.plan.calibration = { roundId: null, measured: false, expectedTotal: 12,
    segments: [{ index: 1, kind: "straight", nExp: 12, xRef: null, xRefPending: "还没测量示教件", hints: [] }] };
  return view;
}

beforeEach(() => {
  ws = workspaceState(singleSegment());
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.historyCandidates).mockResolvedValue({ total: 0, items: [] });
  vi.mocked(workspaceApi.calibrationSkipSuggest).mockResolvedValue([]);
  vi.mocked(workspaceApi.passSkipSuggest).mockResolvedValue({ current: 0, suggested: null, passes: [], pending: "示教件还没有测量" });
  vi.mocked(workspaceApi.calibrationRoundOverview).mockResolvedValue([]);
  vi.mocked(workspaceApi.measureTeaching).mockResolvedValue(ws.data!);
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
    expect(workspaceApi.measureTeaching).toHaveBeenCalledWith("A", 3, expect.any(String));
  });
});

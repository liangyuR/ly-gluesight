import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CalibrationPage from "../src/features/workspace/CalibrationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { CalibrationView } from "../src/features/plan/types";
import { burstRecipe, burstWorkspaceView, calibrationView, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { captureList: vi.fn(), calibrationApply: vi.fn(), calibrationAddRound: vi.fn(), calibrationRemoveRound: vi.fn(), confirmTolerance: vi.fn(), calibrationRoundFeatures: vi.fn().mockResolvedValue([]), calibrationFrame: vi.fn(), captureFrameImage: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter initialEntries={["/recipe/calibration"]}><CalibrationPage /></MemoryRouter>);

/** 整件标定的轮次：前 normal 轮是普通标定轮次，之后 standard 轮是标准件轮次 */
function withRounds(normal: number, standard = 0): CalibrationView {
  const view = calibrationView();
  const base = view.rounds[0];
  view.rounds = [
    ...Array.from({ length: normal }, (_, i) => ({ ...base, roundId: `round-${i + 1}`, standard: false })),
    ...Array.from({ length: standard }, (_, i) => ({ ...base, roundId: `standard-${i + 1}`, standard: true })),
  ];
  return view;
}
/** 只有一个直线段的计划：本轮只有单段计划能逐段标定 */
function singleSegment(calibration: CalibrationView, id = "A") {
  const doc = burstRecipe({ id, segments: [pathSegment(1, "straight", { lengthMm: 120 })] });
  const view = burstWorkspaceView(doc);
  calibration.segments = [{ index: 1, kind: "straight", outcome: { nExp: 12, calib: { rounds: 10, min: 11, max: 13, mean: 11.9 }, suggestedTolerance: 2 }, pending: null, tolerance: 1, xRef: null, xRefPending: "标准件轮次不足", hints: [] }];
  view.plan!.calibration = calibration;
  return view;
}
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ id: "A" }))); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
  for (const fn of [workspaceApi.calibrationApply, workspaceApi.calibrationAddRound, workspaceApi.calibrationRemoveRound, workspaceApi.confirmTolerance]) vi.mocked(fn).mockResolvedValue(ws.data!);
});

describe("多轮标定", () => {
  // 防的回归：轮数不够就能应用标定（用 2–3 轮的均值当 N_exp），或把标准件轮次算进轮数（规格 §2：同一程序连续采集 ≥ 配置轮数）
  it.each([
    ["普通轮次 7 轮，配置 10 轮 → 应用标定禁用并说明还差 3 轮", 7, 0, "还差 3 轮"],
    ["普通轮次 9 轮加 1 轮标准件 → 标准件不计入轮数，仍还差 1 轮", 9, 1, "还差 1 轮"],
  ])("%s", async (_, normal, standard, reason) => {
    ws.data!.plan!.calibration = withRounds(normal, standard);
    show();
    const apply = await screen.findByRole("button", { name: "应用标定" });
    expect(apply).toBeDisabled();
    expect(screen.getByText(new RegExp(`已加入 ${normal} / 10 轮，${reason}`))).toBeVisible();
    await userEvent.click(apply);
    expect(workspaceApi.calibrationApply).not.toHaveBeenCalled();
  });

  // 防的回归：建议容差在应用标定时被直接写进配方（规格 §2：按 min/max 给建议，操作员确认后才可改）
  it("应用标定后容差输入框预填建议值 → 应用本身不提交容差，点确认容差才按输入值提交；非整数不提交", async () => {
    ws.data = singleSegment(withRounds(10));
    show();
    const input = await screen.findByRole("spinbutton", { name: "段 1 帧数容差" });
    expect(input).toHaveValue(2);
    expect(workspaceApi.confirmTolerance).not.toHaveBeenCalled();
    await userEvent.clear(input); await userEvent.type(input, "3");
    await userEvent.click(screen.getByRole("button", { name: "确认段 1 容差" }));
    expect(workspaceApi.confirmTolerance).toHaveBeenCalledWith("A", 3, 1, 3);
    await userEvent.clear(input); await userEvent.type(input, "1.5");
    await userEvent.click(screen.getByRole("button", { name: "确认段 1 容差" }));
    expect(workspaceApi.confirmTolerance).toHaveBeenCalledTimes(1);
    expect(ws.setError).toHaveBeenCalled();
  });
});

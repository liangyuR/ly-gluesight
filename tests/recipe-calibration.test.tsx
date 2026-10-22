import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CalibrationPage from "../src/features/workspace/CalibrationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { CalibrationView } from "../src/features/plan/types";
import { burstRecipe, burstWorkspaceView, calibrationView, pathSegment, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { captureList: vi.fn(), calibrationApply: vi.fn(), calibrationAddRound: vi.fn(), calibrationRemoveRound: vi.fn(), confirmTolerance: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter initialEntries={["/recipe/calibration"]}><CalibrationPage /></MemoryRouter>);
const candidate = (roundId: string) => ({ roundId, recipeId: "A", cameraIds: ["CAM-1", "CAM-2", "CAM-3"], plannedCount: 12, receivedCount: 12, state: "complete" as const, createdAt: 1, error: null });
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
function singleSegment(calibration: CalibrationView) {
  const doc = burstRecipe({ id: "A", segments: [pathSegment(1, "straight", { lengthMm: 120 })] });
  const view = burstWorkspaceView(doc);
  calibration.segments = [{ index: 1, kind: "straight", outcome: { nExp: 12, calib: { rounds: 10, min: 11, max: 13, mean: 11.9 }, suggestedTolerance: 2 }, pending: null, tolerance: 1, xRef: null, xRefPending: "标准件轮次不足" }];
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

  it("普通轮次达到配置轮数 → 应用标定可用，按当前修订提交", async () => {
    ws.data!.plan!.calibration = withRounds(10, 1);
    show();
    const apply = await screen.findByRole("button", { name: "应用标定" });
    expect(apply).toBeEnabled();
    await userEvent.click(apply);
    expect(workspaceApi.calibrationApply).toHaveBeenCalledWith("A", 3);
  });

  // 防的回归：多段计划本轮没有分段算法，却给各段编出 N_exp；或因此把整件也判成“无法标定”（决策：整件帧组数标定仍可发布）
  it("多段计划 → 各段标“待标定（分段算法未就绪）”并提示无法逐段标定，整件仍给出预期帧组数与 ΣN_exp", async () => {
    show();
    expect(await screen.findByText("分段算法未就绪，本轮无法逐段标定")).toBeVisible();
    const table = within(screen.getByRole("table", { name: "逐段标定结果" }));
    expect(table.getAllByText(/待标定（分段算法未就绪）/)).toHaveLength(3);
    expect(screen.getByRole("group", { name: "整件标定" })).toHaveTextContent("整件预期 12 帧组");
    expect(screen.getByText("预期总数 12 ± 3")).toBeVisible();
  });

  it("单段计划标定后 → 显示 N_exp、各轮 min–max（均值）与建议容差，且不提示分段算法未就绪", async () => {
    ws.data = singleSegment(calibrationView()); ws.data.plan!.calibration!.segments[0].outcome!.suggestedTolerance = 2;
    show();
    const row = within(await screen.findByRole("row", { name: /段 1 · 直线/ }));
    expect(row.getByText("12")).toBeVisible();
    expect(row.getByText("11 – 13（11.9）")).toBeVisible();
    expect(row.getByText(/建议 ±2/)).toBeVisible();
    expect(screen.queryByText("分段算法未就绪，本轮无法逐段标定")).not.toBeInTheDocument();
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

  it.each([
    ["加入标定", false],
    ["设为标准件", true],
  ])("采集历史里没加入的完整轮次 → 点“%s”按所选类型加入", async (action, standard) => {
    vi.mocked(workspaceApi.captureList).mockResolvedValue([candidate("round-new"), { ...candidate("round-bad"), state: "failed" }]);
    show();
    await userEvent.click(await screen.findByRole("button", { name: `${action} round-new` }));
    expect(workspaceApi.calibrationAddRound).toHaveBeenCalledWith("A", 3, "round-new", standard);
    expect(screen.queryByRole("button", { name: `${action} round-bad` })).not.toBeInTheDocument();
  });

  it("已加入的轮次 → 移出只移这一轮", async () => {
    show();
    await userEvent.click(await screen.findByRole("button", { name: "移出轮次 round-3" }));
    expect(workspaceApi.calibrationRemoveRound).toHaveBeenCalledWith("A", 3, "round-3");
  });

  it("旧拍照点配方 → 只说明多轮标定只用于段配方", async () => {
    ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
    show();
    expect(await screen.findByText("当前配方按拍照点模型配置")).toBeVisible();
    expect(screen.queryByRole("button", { name: "应用标定" })).not.toBeInTheDocument();
  });
});

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CalibrationPage from "../src/features/workspace/CalibrationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { CalibrationView, PieceView } from "../src/features/plan/types";
import type { HistoryCandidate, WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstWorkspaceView, calibrationView, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { record: vi.fn(() => new Promise(() => {})) } }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { historyCandidates: vi.fn(), calibrationPick: vi.fn(), calibrationSync: vi.fn(), calibrationApply: vi.fn(), calibrationRemoveRound: vi.fn(), confirmTolerance: vi.fn(), calibrationSkipSuggest: vi.fn(), confirmSkip: vi.fn(), calibrationRoundFeatures: vi.fn().mockResolvedValue([]), calibrationFrame: vi.fn(), captureFrameImage: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter initialEntries={["/recipe/calibration"]}><CalibrationPage /></MemoryRouter>);

const piece = (roundId: string, role: PieceView["role"], over: Partial<PieceView> = {}): PieceView => ({
  roundId, role, recordId: 1, cycleId: `cycle-${roundId}`, sn: roundId.toUpperCase(), ts: 1, groupCount: 12,
  review: "confirmed", confirmedBy: "manual", inCalibration: true, excludedGroups: 0, ...over,
});
/** 已加入标定的件：前 normal 件计入件数（第 1 件是示教件），之后 standard 件是标准件 */
function withRounds(normal: number, standard = 0): CalibrationView {
  const view = calibrationView();
  const base = view.rounds[0];
  view.rounds = [
    ...Array.from({ length: normal }, (_, i) => ({ ...base, roundId: `round-${i + 1}`, standard: false })),
    ...Array.from({ length: standard }, (_, i) => ({ ...base, roundId: `standard-${i + 1}`, standard: true })),
  ];
  ws.data!.plan.pieces = [
    ...Array.from({ length: normal }, (_, i) => piece(`round-${i + 1}`, i ? "calibration" : "teaching")),
    ...Array.from({ length: standard }, (_, i) => piece(`standard-${i + 1}`, "standard")),
  ];
  return view;
}
/** 只有一个直线段的计划：本轮只有单段计划能逐段标定 */
function singleSegment(calibration: CalibrationView, id = "A") {
  const doc = burstRecipe({ id, segments: [pathSegment(1, "straight", { lengthMm: 120 })] });
  const view = burstWorkspaceView(doc);
  view.plan.pieces = ws.data!.plan.pieces;
  calibration.segments = [{ index: 1, kind: "straight", outcome: { nExp: 12, calib: { rounds: 10, min: 11, max: 13, mean: 11.9 }, suggestedTolerance: 2 }, pending: null, tolerance: 1, xRef: null, xRefPending: "标准件轮次不足", hints: [] }];
  view.plan!.calibration = calibration;
  return view;
}
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ id: "A" }))); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.calibrationSkipSuggest).mockResolvedValue([]);
  for (const fn of [workspaceApi.calibrationApply, workspaceApi.calibrationRemoveRound, workspaceApi.confirmTolerance, workspaceApi.confirmSkip]) vi.mocked(fn).mockResolvedValue(ws.data!);
});

describe("多轮标定", () => {
  // 防的回归：件数不够就能应用标定（用 2–3 件的均值当 N_exp），或把标准件算进件数（RB-3：含示教件、不含标准件 ≥ 配置件数）
  it.each([
    ["已加入 9 件加 1 件标准件 → 标准件不计入件数，仍还差 1 件", 9, 1, "还差 1 件"],
  ])("%s", async (_, normal, standard, reason) => {
    ws.data!.plan!.calibration = withRounds(normal, standard);
    show();
    const apply = await screen.findByRole("button", { name: "应用标定" });
    expect(apply).toBeDisabled();
    expect(screen.getByText(`标定需要 10 件（含示教件，不含标准件），已加入 ${normal} 件`)).toBeVisible();
    expect(screen.getByRole("note")).toHaveTextContent(reason);
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

  // 防的回归（RB-3）：选件后不分辨哪些件已加入标定、哪些要人工复核及原因；标准件标记没随选件提交
  it("选标定件 → 按所选记录与标准件标记提交，逐件列出已加入与未加入的原因，未加入的件可复核边界", async () => {
    const row = (recordId: number, sn: string): HistoryCandidate => ({ recordId, cycleId: `c-${recordId}`, ts: 1, sn, mode: "maintenanceCollect", groupCount: 12,
      cameras: ["CAM-1", "CAM-2", "CAM-3"].map(cameraId => ({ cameraId, frames: 12 })), complete: true, reasons: [], warnings: [],
      durationMs: null, measuredFps: null, setFps: null, importedRound: null, usage: null, fixture: null });
    vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([row(11, "SN-11"), row(12, "SN-12")]);
    const next: WorkspaceView = structuredClone(ws.data!);
    next.plan.pieces = [...next.plan.pieces, piece("round-11", "calibration", { sn: "SN-11", confirmedBy: "aligned" }),
      piece("round-12", "standard", { sn: "SN-12", review: "draft", confirmedBy: null, inCalibration: false })];
    const issue = "段 1 → 段 2：与比例换算差 4 帧（容许 2）";
    vi.mocked(workspaceApi.calibrationPick).mockResolvedValue({ view: next, pieces: [{ roundId: "round-11", added: true, issue: null }, { roundId: "round-12", added: false, issue }] });
    ws.act = vi.fn(async request => { const result = await request(); ws.data = result; return result; });
    const user = userEvent.setup();
    show();
    await user.click(screen.getByRole("button", { name: "选标定件" }));
    await user.click(await screen.findByRole("checkbox", { name: "选用 SN SN-11" }));
    await user.click(screen.getByRole("checkbox", { name: "选用 SN SN-12" }));
    await user.click(screen.getByRole("checkbox", { name: "SN SN-12 是标准件" }));
    await user.click(screen.getByRole("button", { name: "加入选中的 2 件" }));
    expect(workspaceApi.calibrationPick).toHaveBeenCalledWith("A", 3, [{ recordId: 11, standard: false }, { recordId: 12, standard: true }]);
    const result = within(await screen.findByRole("list", { name: "标定件同步结果" }));
    expect(result.getByText(/SN SN-11：已加入标定$/)).toBeVisible();
    expect(result.getByText(new RegExp(`SN SN-12：未加入（${issue.replace(/[()（）]/g, ".")}）$`))).toBeVisible();
    const pending = within(screen.getByRole("listitem", { name: /SN SN-12/ }));
    expect(pending.getByText("边界待复核")).toBeVisible();
    expect(pending.getByText("未加入")).toBeVisible();
    expect(pending.getByRole("button", { name: /^复核边界/ })).toBeEnabled();
  });

});

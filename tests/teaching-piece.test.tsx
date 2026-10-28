import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import TeachingPiecePage from "../src/features/workspace/TeachingPiecePage";
import { workspaceApi } from "../src/features/workspace/api";
import { burstImageApi } from "../src/features/workspace/burstImages";
import { useWorkspace } from "../src/features/workspace/context";
import type { HistoryCandidate, WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstWorkspaceView, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { historyCandidates: vi.fn(), setTeachingPiece: vi.fn(), captureFrameImage: vi.fn() } }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { record: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const picture = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
const row = (recordId: number, sn: string, reasons: string[] = [], usage: HistoryCandidate["usage"] = null): HistoryCandidate => ({
  recordId, cycleId: `c-${recordId}`, ts: 1, sn, mode: "maintenanceCollect", groupCount: 12,
  cameras: ["CAM-1", "CAM-2", "CAM-3"].map(cameraId => ({ cameraId, frames: 12 })), complete: !reasons.length, reasons, warnings: [],
  durationMs: null, measuredFps: null, setFps: null, importedRound: null, usage, fixture: null,
});
function show(edit: (view: WorkspaceView) => void = () => {}) {
  const view = burstWorkspaceView(burstRecipe({ id: "A" }));
  edit(view);
  ws = workspaceState(view);
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.setTeachingPiece).mockResolvedValue(view);
  return render(<MemoryRouter initialEntries={["/recipe/capture"]}><TeachingPiecePage /></MemoryRouter>);
}
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
  vi.mocked(burstImageApi.record).mockResolvedValue(picture);
  vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([row(7, "SN-7")]);
});

// 防的回归（RB-2、RB-8）：换示教件或改版候选首次设示教件时不提示就清空走廊、边界与标定；取消后仍提交
it.each<[string, (view: WorkspaceView) => void, string[], string[]]>([
  ["已有示教件，换一件", () => {}, ["示教件的分段边界", "全部走廊与示教帧", "全部标定件与标准件（9 件）", "标定结果"], []],
  ["改版候选还没有示教件，继承了走廊与标定结果", view => { view.workspace.teachingPiece = null; view.plan.pieces = []; }, ["全部走廊与示教帧", "标定结果"], ["示教件的分段边界", "标定件与标准件"]],
])("%s → 先列出将清空的内容，取消不提交，确认后才按所选记录设为示教件", async (_, edit, listed, absent) => {
  show(edit);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("checkbox", { name: "选用 SN SN-7" }));
  await user.click(screen.getByRole("button", { name: "设为示教件" }));
  const items = within(screen.getByRole("list", { name: "将清空的内容" }));
  for (const text of listed) expect(items.getByText(new RegExp(text))).toBeVisible();
  for (const text of absent) expect(items.queryByText(new RegExp(text))).toBeNull();
  await user.click(screen.getByRole("button", { name: "取消" }));
  expect(workspaceApi.setTeachingPiece).not.toHaveBeenCalled();

  await user.click(screen.getByRole("button", { name: "设为示教件" }));
  await user.click(screen.getByRole("button", { name: "清空并设为示教件" }));
  expect(workspaceApi.setTeachingPiece).toHaveBeenCalledWith("A", 3, 7);
});

// 防的回归（HI-1～HI-8、RB-6）：不可用或已被本候选使用的历史件也能选；预览读图传错相机下标或帧组从 0 起，后端读到别的相机或报缺帧
it("不可用或已使用的历史件不能勾选并写明原因；看图按记录号、相机下标、从 1 起的帧组读录制原图", async () => {
  vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([row(1, "SN-1", [], "teaching"), row(2, "SN-2", ["缺帧 1 帧"]), row(3, "SN-3")]);
  show();
  const user = userEvent.setup();
  expect(await screen.findByText("已使用（示教件）")).toBeVisible();
  expect(screen.getByText("缺帧 1 帧")).toBeVisible();
  expect(screen.getByRole("checkbox", { name: "选用 SN SN-1" })).toBeDisabled();
  expect(screen.getByRole("checkbox", { name: "选用 SN SN-2" })).toBeDisabled();
  expect(screen.getByRole("checkbox", { name: "选用 SN SN-3" })).toBeEnabled();

  await user.click(screen.getByRole("button", { name: "预览 SN SN-3" }));
  const preview = within(screen.getByRole("complementary", { name: "历史件原图预览" }));
  expect(burstImageApi.record).toHaveBeenLastCalledWith(3, 0, 1);
  await user.click(preview.getByRole("button", { name: "CAM-2" }));
  await user.click(preview.getByRole("button", { name: "下一帧组" }));
  expect(burstImageApi.record).toHaveBeenLastCalledWith(3, 1, 2);
});

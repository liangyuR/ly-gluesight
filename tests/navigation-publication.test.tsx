import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import { NavigationPublication } from "../src/features/workspace/WorkspaceNavigation";
import { useWorkspace } from "../src/features/workspace/context";
import { burstRecipe, burstWorkspaceView, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));

it.each([
  { single: false, status: "draft" as const, cuts: [4, 8], count: 12, ready: false },
  { single: false, status: "confirmed" as const, cuts: [4], count: 12, ready: false },
  { single: false, status: "confirmed" as const, cuts: [4, 8], count: 12, ready: true },
  { single: true, status: "draft" as const, cuts: [], count: 12, ready: true },
  { single: true, status: "draft" as const, cuts: [], count: 0, ready: false },
])("空间已确认但single=$single/status=$status/count=$count → 可冻结提示必须对应真实帧范围 ready=$ready", ({ single, status, cuts, count, ready }) => {
  const recipe = burstRecipe();
  if (single) recipe.segments = recipe.segments.slice(0, 1);
  const view = burstWorkspaceView(recipe);
  const roundId = view.workspace.teachingPiece!.roundId;
  view.plan.pieces = [{ roundId, role: "teaching", recordId: 1, cycleId: "cycle", sn: "fixture", ts: 0, groupCount: count, review: status, confirmedBy: null, inCalibration: false, excludedGroups: 0 }];
  view.workspace.captureReview = { rounds: { [roundId]: { groupCount: count, cuts, status, excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
  view.navigation = { map: { format: 1, image: { id: "image", width: 100, height: 100, mime: "image/png" }, points: [[0, 0], [1, 0]], closed: false, captureStart: 0, captureEnd: 1, cuts: [], confirmed: true, structureRevision: 1 }, imageAvailable: true, error: null, structureRevision: 1, published: null };
  vi.mocked(useWorkspace).mockReturnValue(workspaceState(view));
  render(<MemoryRouter><NavigationPublication /></MemoryRouter>);
  expect(screen.getByText(ready ? "空间对应已确认，发布时冻结独立辅助快照" : "空间对应已确认，但示教件范围未确认，暂不可冻结导航快照")).toBeInTheDocument();
});

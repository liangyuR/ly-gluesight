import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SegmentsPage from "../src/features/workspace/SegmentsPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { burstRecipe, burstWorkspaceView, deferred, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { updateSegments: vi.fn(), captureRoundThumbs: vi.fn(() => new Promise(() => {})) } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><SegmentsPage /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.updateSegments).mockReset().mockReturnValue(deferred<never>().promise);
});
afterEach(() => { vi.useRealTimers(); });

/** 三段都已标定：直线（带 x_ref）、弯道、直线。 */
function openCalibrated() {
  ws = workspaceState(burstWorkspaceView(burstRecipe({
    partCalib: null,
    segments: [
      pathSegment(1, "straight", { expectedFrames: 5, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
      pathSegment(2, "corner", { expectedFrames: 4 }), pathSegment(3, "straight", { expectedFrames: 3 }),
    ],
  })));
  show();
}

it("切换到第二段再修改长度 → 只保存第二段的改动", async () => {
  openCalibrated();
  const original = structuredClone(ws.data!.plan.segments);
  fireEvent.mouseDown(screen.getByRole("tab", { name: "段 2 · 弯道" }), { button: 0, ctrlKey: false });
  expect(screen.queryByRole("region", { name: "段 1" })).not.toBeInTheDocument();
  vi.useFakeTimers();
  fireEvent.change(screen.getByRole("spinbutton", { name: "段 2 长度" }), { target: { value: "130" } });
  await act(() => vi.advanceTimersByTimeAsync(1000));
  const sent = vi.mocked(workspaceApi.updateSegments).mock.calls[0][2];
  expect(sent).toEqual(original.map((segment, at) => at === 1 ? { ...segment, lengthMm: 130 } : segment));
});

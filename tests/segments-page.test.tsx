import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

/** 三段都已标定：直线（带 x_ref）、弯道、直线；返回第 1 段的区域。 */
function openCalibrated() {
  ws = workspaceState(burstWorkspaceView(burstRecipe({
    partCalib: null,
    segments: [
      pathSegment(1, "straight", { expectedFrames: 5, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
      pathSegment(2, "corner", { expectedFrames: 4 }), pathSegment(3, "straight", { expectedFrames: 3 }),
    ],
  })));
  const page = show();
  const first = within(screen.getByRole("region", { name: "段 1" }));
  return { first, page };
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

// 防抖（700 ms）还没到期就离开页面：编辑不能随卸载丢掉。换了配方再离开、离开后工作区拿到新修订，
// 这两条要经真实的 WorkspaceProvider 才观察得到（act 在这里是 mock），见 segments-page-leave.test.tsx
describe("离开段列表页", () => {

  // 防的回归：上一份还在提交时离开，新编辑直接丢掉，或仍用旧修订提交而被后端按修订冲突拒绝
  it("上一份编辑还在提交时又改了容差并离开 → 等上一份完成，用它返回的新修订提交新编辑", async () => {
    const { first, page } = openCalibrated();
    const inFlight = deferred<ReturnType<typeof burstWorkspaceView>>();
    vi.mocked(workspaceApi.updateSegments).mockReturnValueOnce(inFlight.promise);
    vi.useFakeTimers();
    fireEvent.change(first.getByRole("spinbutton", { name: "段 1 长度" }), { target: { value: "130" } });
    await act(() => vi.advanceTimersByTimeAsync(1000)); // 越过防抖：第一份开始提交
    expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
    fireEvent.change(first.getByRole("spinbutton", { name: "段 1 帧数容差" }), { target: { value: "2" } });

    page.unmount();
    expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1);
    const next = structuredClone(ws.data!);
    next.workspace.revision = 4;
    inFlight.resolve(next);

    await waitFor(() => expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(2));
    const [id, revision, sent] = vi.mocked(workspaceApi.updateSegments).mock.calls[1];
    expect([id, revision, sent[0].lengthMm, sent[0].tolerance]).toEqual(["S", 4, 130, 2]);
  });
});

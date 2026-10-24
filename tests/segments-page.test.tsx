import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SegmentsPage from "../src/features/workspace/SegmentsPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { PathSegment } from "../src/features/plan/types";
import { burstRecipe, burstWorkspaceView, deferred, pathSegment, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { updateSegments: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><SegmentsPage /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.updateSegments).mockReset().mockReturnValue(deferred<never>().promise);
});
afterEach(() => { vi.useRealTimers(); });

/** 三段都已标定：直线（带 x_ref）、弯道、直线；返回第 1 段的区域，检测项已展开。 */
async function openCalibrated() {
  ws = workspaceState(burstWorkspaceView(burstRecipe({
    partCalib: null,
    segments: [
      pathSegment(1, "straight", { expectedFrames: 5, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
      pathSegment(2, "corner", { expectedFrames: 4 }), pathSegment(3, "straight", { expectedFrames: 3 }),
    ],
  })));
  const user = userEvent.setup();
  const page = show();
  const first = within(screen.getByRole("region", { name: "段 1" }));
  await user.click(first.getByText(/检测项/));
  return { user, first, page };
}

// 改段类型走 restructure：清空标定并立即保存
describe("直线段改成弯道", () => {
  it("已标定的配方 → 各段预期帧数清空为待标定，并提示标定已清空", async () => {
    const { user, first } = await openCalibrated();
    expect(screen.getByLabelText("段 2 预期帧数")).toHaveTextContent("4");

    await user.selectOptions(first.getByLabelText("段 1 类型", { selector: "select" }), "corner");

    for (const index of [1, 2, 3]) expect(screen.getByLabelText(`段 ${index} 预期帧数`)).toHaveTextContent("待标定");
    expect(screen.getByText("各段预期帧数与标定已清空")).toBeVisible();
  });
});

// 改长度、容差、检测项走 patch：标定保留，防抖后保存
it.each<[string, (first: ReturnType<typeof within>) => void, (segment: PathSegment) => unknown, unknown]>([
  ["帧数容差改为 2", first => fireEvent.change(first.getByRole("spinbutton", { name: "段 1 帧数容差" }), { target: { value: "2" } }), segment => segment.tolerance, 2],
  ["长度改为 130 mm", first => fireEvent.change(first.getByRole("spinbutton", { name: "段 1 长度" }), { target: { value: "130" } }), segment => segment.lengthMm, 130],
  ["关掉溢胶检测", first => fireEvent.click(first.getByRole("switch", { name: "检溢胶" })), segment => segment.checks.overflow.enabled, false],
])("已标定的配方，%s → 各段预期帧数与胶偏参考保留，保存的段列表带这项改动", async (_, edit, field, expected) => {
  const { first } = await openCalibrated();
  vi.useFakeTimers();

  edit(first);
  await act(() => vi.advanceTimersByTimeAsync(1000)); // 越过自动保存的防抖

  expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1);
  const sent = vi.mocked(workspaceApi.updateSegments).mock.calls[0][2];
  expect(sent.map(segment => segment.expectedFrames)).toEqual([5, 4, 3]);
  expect(sent[0].offsetRef?.xRef).toBe(0.1);
  expect(field(sent[0])).toBe(expected);
});

// 防抖（700 ms）还没到期就离开页面：编辑不能随卸载丢掉。换了配方再离开、离开后工作区拿到新修订，
// 这两条要经真实的 WorkspaceProvider 才观察得到（act 在这里是 mock），见 segments-page-leave.test.tsx
describe("离开段列表页", () => {

  // 防的回归：上一份还在提交时离开，新编辑直接丢掉，或仍用旧修订提交而被后端按修订冲突拒绝
  it("上一份编辑还在提交时又改了容差并离开 → 等上一份完成，用它返回的新修订提交新编辑", async () => {
    const { user, first, page } = await openCalibrated();
    const inFlight = deferred<ReturnType<typeof burstWorkspaceView>>();
    vi.mocked(workspaceApi.updateSegments).mockReturnValueOnce(inFlight.promise);
    await user.selectOptions(first.getByLabelText("段 1 类型", { selector: "select" }), "corner"); // 改类型：立即提交
    await waitFor(() => expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1));
    fireEvent.change(first.getByRole("spinbutton", { name: "段 1 帧数容差" }), { target: { value: "2" } });

    page.unmount();
    expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1);
    const next = structuredClone(ws.data!);
    next.workspace.revision = 4;
    inFlight.resolve(next);

    await waitFor(() => expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(2));
    const [id, revision, sent] = vi.mocked(workspaceApi.updateSegments).mock.calls[1];
    expect([id, revision, sent[0].kind, sent[0].tolerance]).toEqual(["S", 4, "corner", 2]);
  });
});

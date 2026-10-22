import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import SegmentsPage from "../src/features/workspace/SegmentsPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { burstRecipe, burstWorkspaceView, deferred, pathSegment, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { updateSegments: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><SegmentsPage /></MemoryRouter>);
beforeEach(() => { vi.mocked(useWorkspace).mockImplementation(() => ws); });

it("新建段配方尚未标定 → 各段预期帧数显示待标定且没有可编辑的输入，整件预期总帧数待标定", () => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ partCalib: null })));
  show();
  for (const index of [1, 2, 3]) expect(screen.getByLabelText(`段 ${index} 预期帧数`)).toHaveTextContent("待标定");
  expect(screen.queryByRole("spinbutton", { name: /预期帧数/ })).not.toBeInTheDocument();
  expect(screen.getByLabelText("整件预期总帧数")).toHaveTextContent("待标定");
});

it("直线段改成弯道 → 各段预期帧数清空、弯道不再有胶偏检测，保存的段列表带清空后的值", async () => {
  const doc = burstRecipe({
    partCalib: null,
    segments: [
      pathSegment(1, "straight", { expectedFrames: 5, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: 0.1, spread: 0.02 } }),
      pathSegment(2, "corner", { expectedFrames: 4 }), pathSegment(3, "straight", { expectedFrames: 3 }),
    ],
  });
  ws = workspaceState(burstWorkspaceView(doc));
  vi.mocked(workspaceApi.updateSegments).mockReturnValue(deferred<never>().promise);
  const user = userEvent.setup();
  show();
  expect(screen.getByLabelText("段 2 预期帧数")).toHaveTextContent("4");
  const first = within(screen.getByRole("region", { name: "段 1" }));
  await user.click(first.getByText(/检测项/));
  expect(first.getByRole("switch", { name: "检胶偏" })).toBeInTheDocument();

  await user.selectOptions(first.getByLabelText("段 1 类型", { selector: "select" }), "corner");

  for (const index of [1, 2, 3]) expect(screen.getByLabelText(`段 ${index} 预期帧数`)).toHaveTextContent("待标定");
  expect(first.queryByRole("switch", { name: "检胶偏" })).not.toBeInTheDocument();
  expect(screen.getByText("各段预期帧数与标定已清空")).toBeVisible();
  await waitFor(() => expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1));
  const [id, revision, sent] = vi.mocked(workspaceApi.updateSegments).mock.calls[0];
  expect([id, revision]).toEqual(["S", 3]);
  expect(sent.map(segment => [segment.kind, segment.expectedFrames])).toEqual([["corner", null], ["corner", null], ["straight", null]]);
  expect(sent[0].checks.offset).toBeNull();
  expect(sent[0].offsetRef).toBeNull();
});

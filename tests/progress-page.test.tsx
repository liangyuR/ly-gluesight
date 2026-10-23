import { render, screen, within } from "@testing-library/react";
import { useSyncExternalStore } from "react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import ProgressPage from "../src/features/workspace/ProgressPage";
import { WorkspaceBar } from "../src/features/workspace/components";
import { useWorkspace } from "../src/features/workspace/context";
import { burstRecipe, burstWorkspaceView, pathSegment, planExport, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
beforeEach(() => { ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws); });

it("切换工作台页面 → 上一页的“操作未完成”错误从页面消失", async () => {
  let error = "候选配置已更新，请重新加载后操作";
  const listeners = new Set<() => void>();
  const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
  const setError = (next: string) => { error = next; listeners.forEach(listener => listener()); };
  vi.mocked(useWorkspace).mockImplementation(() => ({ ...ws, error: useSyncExternalStore(subscribe, () => error), setError }));
  render(<MemoryRouter initialEntries={["/recipe/teach"]}><Routes>
    <Route path="/recipe/teach" element={<><WorkspaceBar /><Link to="/recipe/capture">去采集</Link></>} />
    <Route path="/recipe/capture" element={<WorkspaceBar />} />
  </Routes></MemoryRouter>);
  expect(screen.getByText("候选配置已更新，请重新加载后操作")).toBeVisible();
  await userEvent.click(screen.getByRole("link", { name: "去采集" }));
  expect(screen.queryByText("候选配置已更新，请重新加载后操作")).not.toBeInTheDocument();
});

// 旧拍照点配方（segments 为空）：每个 Pose 组只选一张检测图，示教进度按“有效组”计数
const showBurst = (change: (view: ReturnType<typeof burstWorkspaceView>) => void = () => {}) => {
  const view = burstWorkspaceView();
  change(view);
  ws = workspaceState(view);
  render(<MemoryRouter><ProgressPage /></MemoryRouter>);
};

it("段配方整圈采集阻塞 → 走廊示教、多轮标定、验证与发布显示等待采集，段列表与默认参数不受影响", () => {
  showBurst(view => { view.plan!.progress.steps.find(step => step.id === "capture")!.status = "blocked"; });
  const navigation = within(screen.getByRole("navigation", { name: "配方配置步骤" }));
  for (const name of [/走廊示教/, /多轮标定/, /验证与发布/]) expect(within(navigation.getByRole("link", { name })).getByText("等待采集")).toBeVisible();
  for (const name of ["走廊示教状态", "多轮标定状态", "验证与发布状态"]) expect(screen.getByLabelText(name)).toHaveTextContent("等待采集");
  for (const name of ["段列表状态", "默认参数状态"]) expect(screen.getByLabelText(name)).not.toHaveTextContent("等待采集");
});

it("后端记走廊示教完成但仍有相机未示教 → 走廊示教显示进行中，主操作继续下一台未完成的相机", () => {
  showBurst(view => { view.plan!.progress.steps.find(step => step.id === "corridor")!.status = "done"; });
  expect(screen.getByLabelText("走廊示教状态")).toHaveTextContent("进行中");
  expect(screen.getByLabelText("走廊示教状态")).not.toHaveTextContent("完成");
  expect(screen.getByLabelText("已示教相机数")).toHaveTextContent("1 / 3 台已示教");
  const row = within(screen.getByRole("region", { name: "走廊示教进度" }));
  expect(row.getByRole("link", { name: "继续示教" })).toHaveAttribute("href", "/recipe/teach?camera=CAM-2");
});

it("段配方进度页复制计划导出 → 剪贴板里是带段序列、各段预期帧数和 ΣN_exp 的 JSON", async () => {
  const doc = burstRecipe({
    partCalib: null,
    segments: [pathSegment(1, "straight", { expectedFrames: 5 }), pathSegment(2, "corner", { expectedFrames: 4 }), pathSegment(3, "straight", { expectedFrames: 3 })],
  });
  const user = userEvent.setup();
  showBurst(view => { view.plan!.productionExport = planExport(doc); });
  await user.click(screen.getByRole("button", { name: "复制 JSON" }));
  const copied = JSON.parse(await navigator.clipboard.readText());
  expect(copied.segments.map((segment: { index: number; kind: string; expectedFrames: number }) => [segment.index, segment.kind, segment.expectedFrames]))
    .toEqual([[1, "straight", 5], [2, "corner", 4], [3, "straight", 3]]);
  expect(copied).toMatchObject({ expectedTotal: 12, toleranceTotal: 3 });
});

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

it("后端下一步指向标定验证 → 主操作保留对应示教帧入口", () => {
  const calibration = ws.data!.progress.steps.find(step => step.id === "calibration")!;
  Object.assign(calibration, { status: "blocked", blockers: [{ message: "拍照点 2 · P2 图 1：人工验证未通过", target: { page: "teach", k: 1 } }] });
  ws.data!.progress.next = { step: "calibration", label: "验证拍照点 2 的像素当量", target: { page: "teach", k: 1 } };
  render(<MemoryRouter><ProgressPage /></MemoryRouter>);
  expect(screen.getByRole("link", { name: "验证拍照点 2 的像素当量" })).toHaveAttribute("href", "/recipe/teach?frame=1");
});

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
function groups(count: number) {
  const doc = ws.data!.workspace.doc;
  const shot = structuredClone(doc.shots[0]);
  const progress = structuredClone(ws.data!.progress.shots[0]);
  doc.shots = Array.from({ length: count * 3 }, (_, k) => ({ ...structuredClone(shot), id: `S${Math.floor(k / 3) + 1}-cam${k % 3 + 1}`, poseId: `S${Math.floor(k / 3) + 1}`, camera: `cam${k % 3 + 1}`, views: [{ view: 1, enabled: k % 3 === 0, path: structuredClone(shot.path), mmPerPx: shot.mmPerPx }] }));
  ws.data!.progress.shots = doc.shots.map((shot, k) => ({ ...structuredClone(progress), k, shotId: shot.id, state: k % 3 === 0 ? "trialPassed" : "unconfigured", total: k % 3 === 0 ? 1 : 0, views: [{ ...structuredClone(progress.views[0]), enabled: k % 3 === 0, state: k % 3 === 0 ? "trialPassed" : "unconfigured" }] }));
  return ws.data!.progress;
}

it("每组只选一图且存在未选相机阻塞 → 按有效组计数并继续未保存组", () => {
  const progress = groups(2);
  progress.shots[0].views[0].state = "taught";
  const teach = progress.steps.find(step => step.id === "teach")!;
  teach.blockers = [1, 2, 4, 5].map(k => ({ message: `拍照点 ${k + 1}：未选检测图`, target: { page: "teach", k } }));
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByLabelText("有效示教组数")).toHaveTextContent("1 / 2 组有效");
  expect(card.getByRole("link", { name: "继续示教" })).toHaveAttribute("href", "/recipe/teach?frame=3");
});

it.each(["unselected", "conflict", "missing", "stale"])("检测图为 %s → 不计有效示教并保留正确下一步", mode => {
  const progress = groups(1);
  const selected = mode === "unselected" ? 0 : mode === "conflict" ? 2 : 1;
  ws.data!.workspace.doc.shots.forEach((shot, k) => { shot.views![0].enabled = k < selected; });
  progress.shots.forEach(shot => { shot.views[0].state = "taught"; });
  if (mode === "missing") progress.shots[0].views = [];
  if (mode === "stale") progress.shots[0].views[0].trialStale = true;
  progress.next = null;
  progress.steps.forEach(step => { step.status = "done"; });
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByLabelText("有效示教组数")).toHaveTextContent("0 / 1 组有效");
  expect(card.getByLabelText("单帧示教状态")).not.toHaveTextContent("完成");
  expect(card.getByRole("link", { name: "继续示教" })).toHaveAttribute("href", "/recipe/teach?frame=0");
});

it("未选相机报告重采且后台指向其他步骤 → 只有主操作指向重新采集，示教行等待采集", () => {
  const progress = groups(3);
  progress.shots[0].views[0].state = "taught";
  progress.shots[7].state = "recapture";
  const capture = progress.steps.find(step => step.id === "capture")!;
  Object.assign(capture, { status: "blocked", blockers: [{ message: "设备会话已变化，需要重新采集", target: { page: "capture" } }] });
  progress.next = { step: "calibration", label: "验证标定", target: { page: "teach", k: 0 } };
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  const next = within(screen.getByRole("region", { name: "下一步" }));
  expect(next.getByRole("link", { name: "前往重新采集" })).toHaveAttribute("href", "/recipe/capture");
  expect(next.getByText("设备会话已变化，需要重新采集")).toBeVisible();
  expect(screen.queryByRole("link", { name: "验证标定" })).not.toBeInTheDocument();
  expect(screen.getAllByRole("link", { name: "前往重新采集" })).toHaveLength(1);
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByLabelText("单帧示教状态")).toHaveTextContent("等待采集");
});

it("所有组有效但后台仍指向未选相机 → 不再催办未选相机且保持独立发布状态", () => {
  const progress = groups(1);
  progress.shots[0].views[0].state = "taught";
  const teach = progress.steps.find(step => step.id === "teach")!;
  teach.status = "blocked";
  teach.blockers = [1, 2].map(k => ({ message: `相机 ${k + 1} 未选检测图`, target: { page: "teach", k } }));
  progress.steps.find(step => step.id === "validation")!.blockers = teach.blockers;
  progress.next = { step: "teach", label: "继续示教拍照点 2", target: { page: "teach", k: 1 } };
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  expect(screen.queryByRole("link", { name: "继续示教拍照点 2" })).not.toBeInTheDocument();
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByLabelText("有效示教组数")).toHaveTextContent("1 / 1 组有效");
  expect(card.getByRole("link", { name: "查看示教" })).toHaveAttribute("href", "/recipe/teach");
  expect(screen.getByLabelText("验证与发布状态")).not.toHaveTextContent("完成");
});

it("整圈采集阻塞而后端仍报工件总览完成 → 依赖采集的步骤显示等待采集，默认参数不受影响", () => {
  const capture = ws.data!.progress.steps.find(step => step.id === "capture")!;
  Object.assign(capture, { status: "blocked", detail: "当前设备与采集时不一致，需整圈重新采集" });
  render(<MemoryRouter><ProgressPage /></MemoryRouter>);
  const navigation = within(screen.getByRole("navigation", { name: "配方配置步骤" }));
  const overview = navigation.getByRole("link", { name: /工件总览/ });
  expect(within(overview).getByText("等待采集")).toBeVisible();
  expect(within(overview).queryByText("完成")).not.toBeInTheDocument();
  for (const name of ["单帧示教状态", "工件总览状态", "验证与发布状态"]) expect(screen.getByLabelText(name)).toHaveTextContent("等待采集");
  expect(screen.getByLabelText("默认参数状态")).not.toHaveTextContent("等待采集");
});

// 段配方：进度按 采集 → 段列表 → 走廊示教 → 多轮标定 → 验证与发布，走廊状态取每台相机的示教任务
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

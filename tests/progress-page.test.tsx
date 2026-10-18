import { render, screen, within } from "@testing-library/react";
import { useSyncExternalStore } from "react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import ProgressPage from "../src/features/workspace/ProgressPage";
import { WorkspaceBar } from "../src/features/workspace/components";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceState } from "./fixtures";

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
  expect(screen.getByText("配方默认参数（可选）")).toBeVisible();
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
  expect(card.queryByText("完成", { exact: true })).not.toBeInTheDocument();
  expect(card.getByRole("link", { name: "继续示教" })).toHaveAttribute("href", "/recipe/teach?frame=0");
});

it("未选相机报告重采且后台指向其他步骤 → 主操作和示教入口均优先重新采集", () => {
  const progress = groups(3);
  progress.shots[0].views[0].state = "taught";
  progress.shots[7].state = "recapture";
  const capture = progress.steps.find(step => step.id === "capture")!;
  Object.assign(capture, { status: "blocked", blockers: [{ message: "设备会话已变化，需要重新采集", target: { page: "capture" } }] });
  progress.next = { step: "calibration", label: "验证标定", target: { page: "teach", k: 0 } };
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByRole("link", { name: "前往重新采集" })).toHaveAttribute("href", "/recipe/capture");
  expect(screen.queryByRole("link", { name: "验证标定" })).not.toBeInTheDocument();
  const recaptureLinks = screen.getAllByRole("link", { name: "前往重新采集" });
  expect(recaptureLinks.length).toBeGreaterThan(1);
  recaptureLinks.forEach(link => expect(link).toHaveAttribute("href", "/recipe/capture"));
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
  const publish = within(screen.getByRole("region", { name: "发布与生效" }));
  expect(publish.getByText("未开始")).toBeVisible();
});

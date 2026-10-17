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

it("进度页按后端进度显示步骤状态、可跳转的阻塞原因与下一步", () => {
  const calibration = ws.data!.progress.steps.find(step => step.id === "calibration")!;
  Object.assign(calibration, { status: "blocked", blockers: [{ message: "拍照点 2 · P2 图 1：人工验证未通过", target: { page: "teach", k: 1 } }] });
  ws.data!.progress.next = { step: "calibration", label: "验证拍照点 2 的像素当量", target: { page: "teach", k: 1 } };
  render(<MemoryRouter><ProgressPage /></MemoryRouter>);
  expect(screen.getByRole("link", { name: "拍照点 2 · P2 图 1：人工验证未通过" })).toHaveAttribute("href", "/recipe/teach?frame=1");
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

it("每组只选一图且存在相机级重复诊断 → 按组汇总待办，完成组默认隐藏", async () => {
  const progress = groups(2);
  progress.shots[0].views[0].state = "taught";
  const teach = progress.steps.find(step => step.id === "teach")!;
  teach.blockers = [1, 2, 4, 5].map(k => ({ message: `拍照点 ${k + 1}：未选检测图`, target: { page: "teach", k } }));
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByRole("progressbar", { name: "单帧示教完成进度" })).toHaveAttribute("value", "1");
  expect(card.getByRole("progressbar")).toHaveAttribute("max", "2");
  expect(card.getByRole("link", { name: "拍照点 2：未选检测图" })).not.toBeVisible();
  expect(card.getByRole("link", { name: "继续示教" })).toHaveAttribute("href", "/recipe/teach?frame=3");
  expect(within(screen.getByRole("table", { name: "逐拍照组状态" })).getAllByRole("row")).toHaveLength(2);
  await userEvent.click(screen.getByRole("button", { name: /^全部/ }));
  expect(within(screen.getByRole("table", { name: "逐拍照组状态" })).getAllByRole("row")).toHaveLength(3);
  await userEvent.click(screen.getByText("原始诊断", { exact: true }));
  expect(card.getByRole("link", { name: "拍照点 2：未选检测图" })).toBeVisible();
});

it.each([0, 2])("组内选择 %i 张检测图 → 保留选图待办且不能计为完成", async selected => {
  const progress = groups(1);
  ws.data!.workspace.doc.shots.forEach((shot, k) => { shot.views![0].enabled = k < selected; });
  progress.shots.forEach(shot => { shot.views[0].state = "taught"; });
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  expect(screen.getByRole("progressbar", { name: "单帧示教完成进度" })).toHaveAttribute("value", "0");
  const table = within(screen.getByRole("table", { name: "逐拍照组状态" }));
  expect(table.getByText(selected ? "需重新选图" : "待选图")).toBeVisible();
  expect(table.getByRole("link", { name: "S1 · 选择检测图" })).toHaveAttribute("href", "/recipe/teach?frame=0");
  await userEvent.click(screen.getByRole("button", { name: selected ? /^异常/ : /^待选图/ }));
  expect(screen.getByRole("link", { name: "S1 · 选择检测图" })).toBeVisible();
});

it("未选相机报告重采 → 异常优先展示，主操作与分组操作均前往整圈采集", async () => {
  const progress = groups(3);
  progress.shots[0].views[0].state = "taught";
  progress.shots[7].state = "recapture";
  const capture = progress.steps.find(step => step.id === "capture")!;
  Object.assign(capture, { status: "blocked", blockers: [{ message: "设备会话已变化，需要重新采集", target: { page: "capture" } }] });
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  const card = within(screen.getByRole("region", { name: "单帧示教进度" }));
  expect(card.getByText("S3 · cam2 需整圈重新采集")).toBeVisible();
  expect(card.getByRole("link", { name: "前往重新采集" })).toHaveAttribute("href", "/recipe/capture");
  const rows = within(screen.getByRole("table", { name: "逐拍照组状态" })).getAllByRole("row");
  expect(within(rows[1]).getByText("S3")).toBeVisible();
  expect(within(rows[1]).getByRole("link", { name: "S3 · 前往重采" })).toHaveAttribute("href", "/recipe/capture");
  expect(screen.getByRole("link", { name: "设备会话已变化，需要重新采集" })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: /^异常/ }));
  expect(within(screen.getByRole("table", { name: "逐拍照组状态" })).getAllByRole("row")).toHaveLength(2);
});

it.each(["missing", "stale"])("选中图进度为 %s → 不计完成，保留正确的下一步", mode => {
  const progress = groups(1);
  progress.shots[0].views[0].state = "taught";
  if (mode === "missing") progress.shots[0].views = [];
  else progress.shots[0].views[0].trialStale = true;
  progress.next = null;
  progress.steps.forEach(step => { step.status = "done"; });
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  expect(screen.queryByText("已全部完成")).not.toBeInTheDocument();
  expect(screen.getByRole("progressbar", { name: "单帧示教完成进度" })).toHaveAttribute("value", "0");
  const table = within(screen.getByRole("table", { name: "逐拍照组状态" }));
  expect(table.getAllByText(mode === "missing" ? "状态待确认" : "待试测").length).toBeGreaterThan(0);
  expect(table.getByRole("link", { name: mode === "missing" ? "S1 · 查看" : "S1 · 继续示教" })).toHaveAttribute("href", "/recipe/teach?frame=0");
});

it("所有组已完成但后台仍指向未选相机 → 不再催办未选相机，原始阻塞仍可查看", async () => {
  const progress = groups(1);
  progress.shots[0].views[0].state = "taught";
  const teach = progress.steps.find(step => step.id === "teach")!;
  teach.status = "blocked";
  teach.blockers = [1, 2].map(k => ({ message: `相机 ${k + 1} 未选检测图`, target: { page: "teach", k } }));
  progress.steps.find(step => step.id === "validation")!.blockers = teach.blockers;
  progress.next = { step: "teach", label: "继续示教拍照点 2", target: { page: "teach", k: 1 } };
  render(<MemoryRouter><ProgressPage/></MemoryRouter>);
  expect(screen.queryByRole("link", { name: "继续示教拍照点 2" })).not.toBeInTheDocument();
  expect(screen.queryByText("已全部完成")).not.toBeInTheDocument();
  expect(screen.getByText("所有拍照组已完成示教。")).toBeVisible();
  const validation = within(screen.getByRole("region", { name: "验证" }));
  expect(validation.getByRole("link", { name: "相机 2 未选检测图" })).not.toBeVisible();
  await userEvent.click(validation.getByText("查看 2 项阻塞原因"));
  expect(validation.getByRole("link", { name: "相机 2 未选检测图" })).toBeVisible();
  expect(validation.getByRole("link", { name: "相机 2 未选检测图" })).toHaveAttribute("href", "/recipe/teach?frame=1");
});

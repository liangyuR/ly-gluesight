import { render, screen } from "@testing-library/react";
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

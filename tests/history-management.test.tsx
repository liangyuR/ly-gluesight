import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { SnackbarProvider } from "qing-steel";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import { WorkspaceProvider } from "../src/features/workspace/context";
import type { HistoryDetailState, PartDetail } from "../src/features/history/types";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { burstPartDetail, burstPartSummary, deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const listeners = new Map<string, Set<(event: TauriEvent<unknown>) => void>>();
const project = { schema: 1, id: "line-a", name: "A 线", description: "", createdAt: 0, createdBy: "", copiedFrom: null };
let detail: PartDetail;
type Neighbors = { older: number | null; newer: number | null };
let overview: (id: number) => Promise<PartDetail>;
let neighbors: (id: number) => Promise<Neighbors>;
let setTags: (args: { ids: number[]; tags: string[] }) => Promise<string[]>;
let deleteRecords: () => Promise<{ deleted: number; warnings: string[] }>;

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach(handler => handler({ event, id: 1, payload }));
}

function show(state?: HistoryDetailState) {
  return render(<SnackbarProvider><WorkspaceProvider><MemoryRouter initialEntries={[{ pathname: "/history/1", state }]}><Routes>
    <Route path="/history/:id" element={<HistoryDetailPage />} />
    <Route path="/history" element={<p>历史列表页</p>} />
    <Route path="/recipe/calibration" element={<p>试测页</p>} />
  </Routes></MemoryRouter></WorkspaceProvider></SnackbarProvider>);
}

beforeEach(() => {
  listeners.clear();
  detail = burstPartDetail();
  detail.burst = { ...detail.burst!, frames: [], frameVerdicts: [] };
  overview = async () => detail;
  neighbors = async () => ({ older: null, newer: null });
  setTags = async ({ tags }) => tags;
  deleteRecords = async () => ({ deleted: 1, warnings: [] });
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    const name = String(event);
    const handlers = listeners.get(name) ?? new Set();
    const callback = handler as (event: TauriEvent<unknown>) => void;
    handlers.add(callback); listeners.set(name, handlers);
    return () => { handlers.delete(callback); };
  });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "history_overview": return overview((args as { id: number }).id);
      case "history_neighbors": return neighbors((args as { id: number }).id);
      case "history_set_tags": return setTags(args as { ids: number[]; tags: string[] });
      case "history_delete": return deleteRecords();
      case "recipe_list": return { recipes: [], errors: [] };
      case "workspace_list": case "camera_rig_config": case "workspace_comparison_summaries": return [];
      case "project_current": return project;
      case "project_list": return [{ id: project.id, meta: project, current: true, error: null }];
      case "workspace_history_compare_options": {
        const unavailable = { available: false, reason: "本件没有可用配方" };
        return { candidate: null, choices: [], original: unavailable, rules: unavailable, raw: unavailable };
      }
      default: throw new Error(`Unexpected native command: ${command}`);
    }
  });
});

it.each([
  { kind: "分段", hasBurst: true },
  { kind: "普通", hasBurst: false },
])("$kind 详情打开复检件 → 沿用历史筛选并能返回试测", async ({ hasBurst }) => {
  if (!hasBurst) detail.burst = null;
  detail.retests = [2];
  overview = async id => id === 1 ? detail : { ...detail, summary: burstPartSummary(id), retests: [] };
  const query = { tag: "待复核" };
  show({ query, returnTo: { path: "/recipe/calibration", label: "返回试测" } });

  const retest = await screen.findByRole("link", { name: "#2" });
  expect(retest).toHaveAttribute("href", "/history/2");
  await userEvent.click(retest);
  expect(await screen.findByText("SN SN-102")).toBeVisible();
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("history_neighbors", { id: 2, query }));
  await userEvent.click(screen.getByRole("button", { name: "返回试测" }));
  expect(screen.getByText("试测页")).toBeVisible();
});

it("邻件失效且刷新期间有新件入库 → 按钮和快捷键只使用最新邻件", async () => {
  const stale = deferred<Neighbors>();
  const latest = deferred<Neighbors>();
  const readNeighbors = vi.fn(async (id: number): Promise<Neighbors> => ({ older: id === 3 ? 1 : null, newer: null }));
  readNeighbors.mockResolvedValueOnce({ older: null, newer: 2 }).mockReturnValueOnce(stale.promise).mockReturnValueOnce(latest.promise);
  neighbors = readNeighbors;
  overview = async id => ({ ...detail, summary: burstPartSummary(id) });
  show();
  const nextButton = () => screen.getByRole("button", { name: "后一件" });
  await waitFor(() => expect(nextButton()).toBeEnabled());

  act(() => emit("history://changed", [2]));
  await waitFor(() => expect(readNeighbors).toHaveBeenCalledTimes(2));
  expect(nextButton()).toBeDisabled();
  fireEvent.keyDown(window, { key: "]" });
  expect(screen.getByText("SN SN-101")).toBeVisible();

  act(() => emit("history://inserted", 3));
  expect(readNeighbors).toHaveBeenCalledTimes(2);
  await act(async () => { stale.resolve({ older: null, newer: 2 }); });
  await waitFor(() => expect(readNeighbors).toHaveBeenCalledTimes(3));
  expect(nextButton()).toBeDisabled();

  await act(async () => { latest.resolve({ older: null, newer: 3 }); });
  expect(nextButton()).toBeEnabled();
  fireEvent.keyDown(window, { key: "]" });
  expect(await screen.findByText("SN SN-103")).toBeVisible();
});

it("Tag 保存期间交付与录制状态更新 → 补读完整详情且旧请求不能覆盖", async () => {
  detail.summary.delivery.state = "pending";
  detail.recording = { ...detail.recording, state: "pending", available: false };
  const old = { ...detail, summary: { ...detail.summary, tags: [] } };
  show();
  await screen.findByRole("button", { name: "编辑 Tag" });
  const late = deferred<PartDetail>();
  overview = () => late.promise;
  act(() => emit("history://updated", detail.summary.cycleId));
  await waitFor(() => expect(screen.getByText("等待提交")).toBeVisible());
  overview = async () => detail;
  setTags = async ({ tags }) => {
    detail = {
      ...detail,
      summary: { ...detail.summary, tags, delivery: { state: "submitted", updatedAt: 2, message: "结果已写入" } },
      recording: { ...detail.recording, state: "complete", available: true },
    };
    emit("history://updated", detail.summary.cycleId);
    return tags;
  };
  await userEvent.click(screen.getByRole("button", { name: "编辑 Tag" }));
  await userEvent.click(within(screen.getByRole("dialog", { name: "编辑 Tag" })).getByRole("button", { name: "故障件" }));
  await userEvent.click(screen.getByRole("button", { name: "保存 Tag" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(await screen.findByText("已回写 PLC · 结果已写入")).toBeVisible();
  expect(screen.getByText("完整落盘 · 已核验落盘")).toBeVisible();
  await act(async () => { late.resolve(old); });
  expect(screen.getByText("故障件")).toBeVisible();
  expect(screen.queryByText("未标记")).not.toBeInTheDocument();
  expect(screen.getByText("已回写 PLC · 结果已写入")).toBeVisible();
});

it("删除失败可重试 → 变更事件先到仍展示原图保留提示，再返回原入口", async () => {
  detail.burst = null;
  let firstAttempt = true;
  deleteRecords = async () => {
    if (firstAttempt) { firstAttempt = false; throw new Error("原图仍在落盘"); }
    overview = async () => { throw new Error("记录已删除"); };
    emit("history://changed", [1]);
    await new Promise(resolve => setTimeout(resolve, 0));
    return { deleted: 1, warnings: ["共享原图已保留"] };
  };
  show({ returnTo: { path: "/recipe/calibration", label: "返回试测" } });
  await userEvent.click(await screen.findByRole("button", { name: "删除历史件" }));
  await userEvent.click(screen.getByRole("button", { name: "确认删除 1 件" }));
  expect(await screen.findByText("原图仍在落盘")).toBeVisible();
  expect(screen.queryByText("试测页")).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "确认删除 1 件" }));
  expect(await screen.findByText("共享原图已保留")).toBeVisible();
  expect(screen.queryByRole("button", { name: "确认删除 1 件" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "完成" }));
  expect(screen.getByText("试测页")).toBeVisible();
});

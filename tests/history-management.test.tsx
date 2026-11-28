import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { SnackbarProvider } from "qing-steel";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
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

function TrialDestination() {
  const location = useLocation();
  const recordId = (location.state as { focusRecordId?: number } | null)?.focusRecordId;
  return <><p>试测页</p>{recordId != null && <p>试测件 #{recordId}</p>}</>;
}

function show(state?: HistoryDetailState) {
  return render(<SnackbarProvider><WorkspaceProvider><MemoryRouter initialEntries={[{ pathname: "/history/1", state }]}><Routes>
    <Route path="/history/:id" element={<HistoryDetailPage />} />
    <Route path="/history" element={<p>历史列表页</p>} />
    <Route path="/recipe/calibration" element={<TrialDestination />} />
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

it("历史详情仍在读取 → 可以带着原上下文返回试测", async () => {
  const pending = deferred<PartDetail>();
  overview = () => pending.promise;
  show({ returnTo: { path: "/recipe/calibration", label: "返回试测", state: { focusRecordId: 7 } } });

  await userEvent.click(screen.getByRole("button", { name: "返回试测" }));
  expect(screen.getByText("试测件 #7")).toBeVisible();
  await act(async () => pending.resolve(detail));
  expect(screen.getByText("试测件 #7")).toBeVisible();
});

it("历史详情首次读取失败 → 原地重试打开同一件", async () => {
  const pending = deferred<PartDetail>();
  const readOverview = vi.fn<(id: number) => Promise<PartDetail>>()
    .mockRejectedValueOnce(new Error("暂时无法读取记录"))
    .mockReturnValueOnce(pending.promise);
  overview = readOverview;
  const readNeighbors = vi.fn(neighbors);
  neighbors = readNeighbors;
  show();
  expect(await screen.findByText("暂时无法读取记录")).toBeVisible();

  await userEvent.click(screen.getByRole("button", { name: "重试" }));
  expect(screen.getByText("正在读取历史详情…")).toHaveAttribute("role", "status");
  expect(screen.queryByText("暂时无法读取记录")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "重试" })).not.toBeInTheDocument();
  await act(async () => pending.resolve(detail));
  expect(await screen.findByText("SN SN-101")).toBeVisible();
  expect(readOverview.mock.calls).toEqual([[1], [1]]);
  expect(readNeighbors).toHaveBeenCalledOnce();
});

it.each([
  { description: "同件更新只补读一次", currentUpdated: true },
  { description: "外件更新不触发补读", currentUpdated: false },
])("等待原生订阅后首读 → $description", async ({ currentUpdated }) => {
  const registerUpdated = deferred<void>();
  const registerChanged = deferred<void>();
  const listenImmediately = vi.mocked(listen).getMockImplementation()!;
  vi.mocked(listen).mockImplementation(async (event, handler, options) => {
    if (event === "history://updated") await registerUpdated.promise;
    if (event === "history://changed") await registerChanged.promise;
    return listenImmediately(event, handler, options);
  });
  detail.summary.delivery.state = "pending";
  detail.recording = { ...detail.recording, state: "pending", available: false };
  const updated: PartDetail = {
    ...detail,
    summary: { ...detail.summary, delivery: { state: "submitted", updatedAt: 2, message: "首读期间已写入" } },
    recording: { ...detail.recording, state: "complete", available: true },
  };
  const first = deferred<PartDetail>();
  const refreshed = deferred<PartDetail>();
  const readOverview = vi.fn<(id: number) => Promise<PartDetail>>()
    .mockReturnValueOnce(first.promise).mockReturnValueOnce(refreshed.promise).mockResolvedValue(updated);
  overview = readOverview;
  show();

  expect(readOverview).not.toHaveBeenCalled();
  await act(async () => registerUpdated.resolve(undefined));
  expect(readOverview).not.toHaveBeenCalled();
  await act(async () => registerChanged.resolve(undefined));
  await waitFor(() => expect(readOverview).toHaveBeenCalledExactlyOnceWith(1));
  act(() => {
    emit("history://updated", "other-cycle");
    if (currentUpdated) {
      emit("history://updated", detail.summary.cycleId);
      emit("history://updated", detail.summary.cycleId);
    }
  });
  expect(readOverview).toHaveBeenCalledTimes(1);

  await act(async () => first.resolve(detail));
  expect(await screen.findByText("等待提交")).toBeVisible();
  if (!currentUpdated) {
    expect(readOverview).toHaveBeenCalledExactlyOnceWith(1);
    return;
  }
  await waitFor(() => expect(readOverview.mock.calls).toEqual([[1], [1]]));
  await act(async () => refreshed.resolve(updated));
  expect(await screen.findByText("已回写 PLC · 首读期间已写入")).toBeVisible();
  expect(screen.getByText("完整落盘 · 已核验落盘")).toBeVisible();
  expect(readOverview.mock.calls).toEqual([[1], [1]]);
});

it("已显示的历史详情连续收到同件通知 → 进行中的刷新结束后只补读一次", async () => {
  const refresh = deferred<PartDetail>();
  const latest = deferred<PartDetail>();
  const updated: PartDetail = {
    ...detail,
    summary: { ...detail.summary, delivery: { state: "acknowledged", updatedAt: 3, message: "现场已确认" } },
  };
  const readOverview = vi.fn<(id: number) => Promise<PartDetail>>()
    .mockResolvedValueOnce(detail).mockReturnValueOnce(refresh.promise).mockReturnValueOnce(latest.promise).mockResolvedValue(updated);
  overview = readOverview;
  show();
  await screen.findByRole("button", { name: "编辑 Tag" });
  act(() => emit("history://updated", detail.summary.cycleId));
  await waitFor(() => expect(readOverview).toHaveBeenCalledTimes(2));
  act(() => {
    emit("history://updated", detail.summary.cycleId);
    emit("history://updated", "other-cycle");
    emit("history://updated", detail.summary.cycleId);
  });
  expect(readOverview).toHaveBeenCalledTimes(2);

  await act(async () => refresh.resolve(detail));
  await waitFor(() => expect(readOverview.mock.calls).toEqual([[1], [1], [1]]));
  await act(async () => latest.resolve(updated));
  expect(await screen.findByText("PLC 已确认 · 现场已确认")).toBeVisible();
  expect(readOverview.mock.calls).toEqual([[1], [1], [1]]);
});

it("旧件仍在刷新时切到后件 → 旧响应和已排的补读不带到新件", async () => {
  const refresh = deferred<PartDetail>();
  const next = { ...detail, summary: burstPartSummary(2) };
  const readOverview = vi.fn<(id: number) => Promise<PartDetail>>()
    .mockResolvedValue(next).mockResolvedValueOnce(detail).mockReturnValueOnce(refresh.promise);
  overview = readOverview;
  neighbors = async id => ({ older: null, newer: id === 1 ? 2 : null });
  show();

  const nextButton = await screen.findByRole("button", { name: "后一件" });
  await waitFor(() => expect(nextButton).toBeEnabled());
  act(() => {
    emit("history://updated", detail.summary.cycleId);
    emit("history://updated", detail.summary.cycleId);
  });
  expect(readOverview.mock.calls).toEqual([[1], [1]]);
  await userEvent.click(nextButton);
  expect(await screen.findByText("SN SN-102")).toBeVisible();
  await act(async () => refresh.resolve(detail));

  expect(screen.getByText("SN SN-102")).toBeVisible();
  expect(screen.queryByText("SN SN-101")).not.toBeInTheDocument();
  expect(readOverview.mock.calls).toEqual([[1], [1], [2]]);
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

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { SnackbarProvider } from "qing-steel";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import { WorkspaceProvider } from "../src/features/workspace/context";
import type { HistoryDetailState, PartDetail } from "../src/features/history/types";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { burstPartDetail, deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const listeners = new Map<string, Set<(event: TauriEvent<unknown>) => void>>();
const project = { schema: 1, id: "line-a", name: "A 线", description: "", createdAt: 0, createdBy: "", copiedFrom: null };
let detail: PartDetail;
let overview: () => Promise<PartDetail>;
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
      case "history_overview": return overview();
      case "history_neighbors": return { older: null, newer: null };
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
      case "history_timeline": return { cycleId: detail.summary.cycleId, requestSeq: null, start: null, end: null, stages: [], entries: [], truncated: false, notes: [] };
      default: throw new Error(`Unexpected native command: ${command}`);
    }
  });
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
      summary: { ...detail.summary, tags, delivery: { state: "acknowledged", updatedAt: 2, message: "结果已接收" } },
      recording: { ...detail.recording, state: "complete", available: true },
    };
    emit("history://updated", detail.summary.cycleId);
    return tags;
  };
  await userEvent.click(screen.getByRole("button", { name: "编辑 Tag" }));
  await userEvent.click(within(screen.getByRole("dialog", { name: "编辑 Tag" })).getByRole("button", { name: "故障件" }));
  await userEvent.click(screen.getByRole("button", { name: "保存 Tag" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(await screen.findByText("PLC 已确认 · 结果已接收")).toBeVisible();
  expect(screen.getByText("完整落盘 · 已核验落盘")).toBeVisible();
  await act(async () => { late.resolve(old); });
  expect(screen.getByText("故障件")).toBeVisible();
  expect(screen.queryByText("未标记")).not.toBeInTheDocument();
  expect(screen.getByText("PLC 已确认 · 结果已接收")).toBeVisible();
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

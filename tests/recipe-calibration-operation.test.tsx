import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SnackbarProvider } from "qing-steel";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { expect, it, vi } from "vitest";
import CalibrationPage from "../src/features/workspace/CalibrationPage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import type { OperationProgress } from "../src/features/workspace/operation";
import { inRecipe } from "../src/features/workspace/RecipeRoute";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred, pathSegment } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const CalibrationRoute = inRecipe(CalibrationPage);

it("取消测量示教件仅显示取消状态并解锁，重试实际失败仍有明确提示", async () => {
  const recipe = burstRecipe({ id: "A", partCalib: null, segments: [pathSegment(1, "straight", { expectedFrames: 12 })] });
  const view = burstWorkspaceView(recipe);
  view.plan.cameraTeaching = view.plan.cameraTeaching.map(item => ({ ...item, ready: true }));
  view.plan.calibration = {
    roundId: null, measured: false, expectedTotal: 12,
    segments: [{ index: 1, kind: "straight", nExp: 12, xRef: null, xRefPending: null, hints: [] }],
  };
  const pending = deferred<WorkspaceView>();
  let taskId = "";
  let receiveProgress!: (event: { payload: OperationProgress }) => void;
  vi.mocked(listen).mockImplementation(async (event, callback) => {
    if (event === "operation://progress") receiveProgress = callback as typeof receiveProgress;
    return () => {};
  });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "recipe_list": return { recipes: [burstSummary(recipe)], errors: [], unavailable: [] };
      case "workspace_list": return [view.workspace];
      case "workspace_get": return view;
      case "camera_rig_config": return [];
      case "workspace_history_candidates": return { total: 0, items: [] };
      case "operation_status": return null;
      case "operation_cancel": return true;
      case "workspace_measure_teaching": {
        if (taskId) throw new Error("示教原图无法读取");
        taskId = (args as { taskId: string }).taskId;
        return pending.promise;
      }
      default: throw new Error(`意外的桌面调用：${command}`);
    }
  });
  const listReads = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "workspace_list").length;
  const user = userEvent.setup();
  render(<MemoryRouter initialEntries={["/recipe/A/calibration"]}><SnackbarProvider><WorkspaceProvider>
    <Routes><Route path="/recipe/:recipeId/calibration" element={<CalibrationRoute />} /></Routes>
  </WorkspaceProvider></SnackbarProvider></MemoryRouter>);

  const start = await screen.findByRole("button", { name: "测量示教件" });
  const initialListReads = listReads();
  await user.click(start);
  await waitFor(() => expect(taskId).not.toBe(""));
  act(() => receiveProgress({ payload: {
    taskId, stage: "measuring", completed: 1, total: 12, failed: 0, skipped: 0, elapsedMs: 150,
  } }));
  const progress = screen.getByRole("status", { name: "测量示教件进度" });
  expect(start).toBeDisabled();
  await user.click(within(progress).getByRole("button", { name: "取消" }));
  expect(invoke).toHaveBeenCalledWith("operation_cancel", { taskId });

  await act(async () => pending.reject("OPERATION_CANCELLED: 任务已取消，原结果未更改"));
  expect(progress).toHaveTextContent("已取消，原结果保留");
  expect(start).toBeEnabled();
  expect(screen.getByText("还没测量")).toBeVisible();
  expect(screen.queryByText("操作未完成")).not.toBeInTheDocument();
  expect(screen.queryByText("示教件已按当前走廊测量")).not.toBeInTheDocument();
  expect(listReads()).toBe(initialListReads);

  await user.click(start);
  expect(await screen.findByText("操作未完成")).toBeVisible();
  expect(progress).toHaveTextContent("示教原图无法读取");
  expect(start).toBeEnabled();
});

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import ProgressPage from "../src/features/workspace/ProgressPage";
import TeachingPiecePage from "../src/features/workspace/TeachingPiecePage";
import { SnackbarProvider } from "qing-steel";
import { WorkspaceProvider } from "../src/features/workspace/context";
import type { RecipeDoc } from "../src/features/cycle/types";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstCameras, burstRecipe, burstWorkspaceView, pathSegment, progressV2 } from "./fixtures";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

/** 后端新建模板（schema 8）：三台相机、一个待标定的直线段，走廊都还没示教。 */
const template = (): RecipeDoc => ({
  ...burstRecipe({
    id: "NEW", name: "新配方", version: 1, productCode: 1, planVersion: 0, segments: [pathSegment(1, "straight")], partCalib: null,
    inspect: burstCameras.map(camera => ({ camera, mmPerPx: null, calib: null, detect: null })),
  }),
});

/** workspace_create 的结果：刚建的段配方候选，还没有示教件，进度停在“选示教件”。 */
function createdView(doc: RecipeDoc): WorkspaceView {
  const view = burstWorkspaceView(doc as ReturnType<typeof burstRecipe>);
  view.workspace.revision = 1;
  const plan = view.plan!;
  plan.cameraTeaching = plan.cameras.map(camera => ({ camera, mmPerPx: null, ready: false, nozzle: null, segments: [] }));
  plan.calibration = null;
  plan.pieces = [];
  view.workspace.teachingPiece = null;
  view.workspace.calibration = { rounds: [] };
  const waiting = { detail: "还没有示教件" };
  plan.progress = {
    steps: progressV2().steps.map(step => ({ ...step, status: "notStarted", ...(step.id === "capture" ? waiting : {}) })),
    next: { step: "capture", label: "选示教件", target: { page: "capture" } },
  };
  return view;
}

describe("创建配方流程", () => {
  it("在配方库填写新候选，选中创建结果并继续进入该候选的示教件", async () => {
    let created: WorkspaceView | null = null;
    let preferences: Record<string, unknown> = {};
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      switch (command) {
        case "storage_preferences_get": return { ...preferences };
        case "storage_preferences_update": {
          preferences = { ...preferences, ...(args as { patch: Record<string, unknown> }).patch };
          return { ...preferences };
        }
        case "recipe_list": return { recipes: [], errors: [] };
        case "recipe_template": return structuredClone(template());
        case "workspace_list": return created ? [structuredClone(created.workspace)] : [];
        case "workspace_create":
          created = createdView(structuredClone((args as { doc: RecipeDoc }).doc));
          return structuredClone(created);
        case "workspace_get":
          if (created && (args as { id: string }).id === created.workspace.doc.id) return structuredClone(created);
          throw new Error("候选不存在");
        case "camera_rig_config":
        case "camera_rig_status":
        case "workspace_history_candidates":
        case "cycle_logs": return [];
        case "cycle_snapshot": return null;
        case "plc_get_status": return { state: "disconnected", message: "PLC 未连接" };
        default: throw new Error(`未声明的桌面调用：${command}`);
      }
    });

    const user = userEvent.setup();
    render(<MemoryRouter initialEntries={["/recipe"]}><SnackbarProvider><WorkspaceProvider><Routes>
      <Route path="/recipe" element={<LibraryPage />} />
      <Route path="/recipe/progress" element={<ProgressPage />} />
      <Route path="/recipe/capture" element={<TeachingPiecePage />} />
    </Routes></WorkspaceProvider></SnackbarProvider></MemoryRouter>);

    await user.click(screen.getByRole("button", { name: "新建配方" }));
    const dialog = within(await screen.findByRole("dialog", { name: "建立候选配方" }));
    await user.clear(dialog.getByRole("textbox", { name: "配方编号" }));
    await user.type(dialog.getByRole("textbox", { name: "配方编号" }), "SHELL-2026");
    await user.clear(dialog.getByRole("textbox", { name: "配方名称" }));
    await user.type(dialog.getByRole("textbox", { name: "配方名称" }), " 新壳体配方 ");
    await user.clear(dialog.getByRole("spinbutton", { name: "型号" }));
    await user.type(dialog.getByRole("spinbutton", { name: "型号" }), "27");
    await user.click(dialog.getByRole("button", { name: "创建候选" }));

    expect(await screen.findByRole("heading", { name: "配方进度" })).toBeVisible();
    expect(invoke).toHaveBeenCalledWith("workspace_create", {
      doc: { ...template(), id: "SHELL-2026", name: "新壳体配方", productCode: 27, teachingId: null },
    });
    expect(invoke).toHaveBeenCalledWith("workspace_get", { id: "SHELL-2026" });
    expect(screen.getByRole("combobox", { name: "当前配方" })).toHaveValue("SHELL-2026");
    // 新建的段配方按段模型的步骤显示：示教件 → 段列表 → 走廊示教 → 多轮标定 → 验证与发布
    expect(within(screen.getByRole("region", { name: "示教件进度" })).getByText("还没有示教件")).toBeVisible();
    const steps = within(screen.getByRole("navigation", { name: "配方配置步骤" }));
    for (const name of [/段列表/, /走廊示教/, /多轮标定/]) expect(steps.getByRole("link", { name })).toBeVisible();
    expect(steps.queryByRole("link", { name: /单帧示教/ })).not.toBeInTheDocument();
    await waitFor(() => expect(preferences.lastWorkspace).toBe("SHELL-2026"));

    await user.click(screen.getByRole("link", { name: "选示教件" }));
    expect(await screen.findByRole("heading", { name: "选示教件" })).toBeVisible();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("workspace_history_candidates", { id: "SHELL-2026" }));
    expect(screen.getByRole("combobox", { name: "当前配方" })).toHaveValue("SHELL-2026");
    expect(screen.getByRole("combobox", { name: "当前配方" })).toHaveDisplayValue("SHELL-2026 · 新壳体配方");
  }, 20_000);
});

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import ProgressPage from "../src/features/workspace/ProgressPage";
import CapturePage from "../src/features/workspace/CapturePage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import type { RecipeDoc } from "../src/features/cycle/types";
import type { WorkspaceView } from "../src/features/workspace/types";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

describe("创建配方流程", () => {
  it("在配方库填写新候选，选中创建结果并继续进入该候选的整圈采集", async () => {
    const template: RecipeDoc = {
      id: "NEW", name: "新配方", version: 1, productCode: 1, triggerMode: "fly", schemaVersion: 6,
      spacing: 1, filterWindow: 3, detect: { searchMm: 4, polarity: "dark", widthRange: [1, 6] },
      limits: { position: null, width: null, maxGapLen: .5, minPresent: .8 }, shots: [], cameras: {},
    };
    let created: WorkspaceView | null = null;
    let preferences: Record<string, unknown> = { legacyImported: true };
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      switch (command) {
        case "storage_preferences_get": return { ...preferences };
        case "storage_preferences_update": {
          preferences = { ...preferences, ...(args as { patch: Record<string, unknown> }).patch };
          return { ...preferences };
        }
        case "recipe_list": return { recipes: [], errors: [] };
        case "recipe_template": return structuredClone(template);
        case "workspace_list": return created ? [structuredClone(created.workspace)] : [];
        case "workspace_create": {
          const doc = structuredClone((args as { doc: RecipeDoc }).doc);
          created = {
            workspace: {
              doc, baseRevision: null, revision: 1, frames: [],
              overview: { background: null, positions: [], saved: false },
              samples: [], sampleBank: [], validation: null, pending: null, publish: { state: "draft" }, updatedAt: 1,
            },
            layout: { ...doc, revisionId: "candidate-1", segments: [], points: { x: [], y: [], seg: [], k: [] } },
            productionVersion: null, productionPlanVersion: null, planChanged: true, coverage: 0,
            progress: { steps: [{ id: "capture", label: "整圈采集", optional: false, status: "notStarted", detail: "等待设备就绪后整圈采集并采用", blockers: [] }],
              shots: [], next: { step: "capture", label: "开始整圈采集", target: { page: "capture" } } },
          };
          return structuredClone(created);
        }
        case "workspace_get":
          if (created && (args as { id: string }).id === created.workspace.doc.id) return structuredClone(created);
          throw new Error("候选不存在");
        case "camera_rig_config":
        case "camera_rig_status":
        case "recipe_capture_list":
        case "cycle_logs": return [];
        case "recipe_capture_get":
        case "cycle_snapshot": return null;
        case "plc_get_status": return { state: "disconnected", message: "PLC 未连接" };
        default: throw new Error(`未声明的桌面调用：${command}`);
      }
    });

    const user = userEvent.setup();
    render(<MemoryRouter initialEntries={["/recipe"]}><WorkspaceProvider><Routes>
      <Route path="/recipe" element={<LibraryPage />} />
      <Route path="/recipe/progress" element={<ProgressPage />} />
      <Route path="/recipe/capture" element={<CapturePage />} />
    </Routes></WorkspaceProvider></MemoryRouter>);

    await user.click(screen.getByRole("button", { name: "新建配方" }));
    const dialog = within(await screen.findByRole("dialog", { name: "建立候选配方" }));
    await user.clear(dialog.getByRole("textbox", { name: "配方编号" }));
    await user.type(dialog.getByRole("textbox", { name: "配方编号" }), "SHELL-2026");
    await user.clear(dialog.getByRole("textbox", { name: "配方名称" }));
    await user.type(dialog.getByRole("textbox", { name: "配方名称" }), " 新壳体配方 ");
    await user.clear(dialog.getByRole("spinbutton", { name: "产品代码" }));
    await user.type(dialog.getByRole("spinbutton", { name: "产品代码" }), "27");
    await user.click(dialog.getByRole("button", { name: "创建候选" }));

    expect(await screen.findByRole("heading", { name: "配方创建进度" })).toBeVisible();
    expect(invoke).toHaveBeenCalledWith("workspace_create", {
      doc: { ...template, id: "SHELL-2026", name: "新壳体配方", productCode: 27, teachingId: null },
    });
    expect(invoke).toHaveBeenCalledWith("workspace_get", { id: "SHELL-2026" });
    expect(screen.getByRole("combobox", { name: "当前配方" })).toHaveValue("SHELL-2026");
    expect(screen.getByText("等待设备就绪后整圈采集并采用")).toBeVisible();
    await waitFor(() => expect(preferences.lastWorkspace).toBe("SHELL-2026"));

    await user.click(screen.getByRole("link", { name: "开始整圈采集" }));
    expect(await screen.findByRole("heading", { name: "设备准备与整圈采集" })).toBeVisible();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("recipe_capture_list", { recipeId: "SHELL-2026" }));
    expect(screen.getByRole("combobox", { name: "当前配方" })).toHaveValue("SHELL-2026");
    expect(screen.getByRole("combobox", { name: "当前配方" })).toHaveDisplayValue("SHELL-2026 · 新壳体配方");
  });
});

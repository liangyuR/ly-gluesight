import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import { SnackbarProvider } from "qing-steel";
import { WorkspaceProvider } from "../src/features/workspace/context";
import SegmentsPage from "../src/features/workspace/SegmentsPage";
import TeachingPage from "../src/features/workspace/TeachingPage";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { burstRecipe, burstSummary, burstWorkspaceView, captureRoundV2, siteStatus, snapshot } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.setConfig({ testTimeout: 20_000 });

let view: ReturnType<typeof burstWorkspaceView>;
beforeEach(() => {
  vi.stubGlobal("PointerEvent", MouseEvent);
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:navigation");
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  view = burstWorkspaceView(burstRecipe({ id: "A", segments: [
    ...burstRecipe().segments.map(segment => ({ ...segment, expectedFrames: 4 })),
  ] }));
  view.workspace.captureReview = { rounds: { "round-1": { groupCount: 12, cuts: [4, 8], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
  view.navigation = { map: { format: 1, image: { id: "map-image", width: 100, height: 100, mime: "image/png" }, points: [[0, 0.5], [1, 0.5]], closed: false, captureStart: 0, captureEnd: 1, cuts: [0.4, 0.7], confirmed: true, structureRevision: 1 }, imageAvailable: true, error: null, structureRevision: 1, published: null };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "storage_preferences_get": case "storage_preferences_update": return {};
      case "recipe_list": return { recipes: [burstSummary(view.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] };
      case "workspace_list": return [structuredClone(view.workspace)];
      case "workspace_get": return structuredClone(view);
      case "workspace_navigation_image": return [1, 2, 3];
      case "camera_rig_config": case "camera_rig_status": case "cycle_logs": return [];
      case "cycle_snapshot": return snapshot();
      case "site_status": return siteStatus();
      case "vision_calib_info": return null;
      case "plc_get_status": return { state: "connected" };
      case "workspace_publish_checks": return [{ name: "示教", passed: true, detail: "已完成" }];
      case "history_query": return { items: [], total: 0 };
      case "recipe_capture_get": return captureRoundV2({ recipeId: "A" });
      case "recipe_capture_round_thumbs": {
        const bytes = new Uint8Array(24); new DataView(bytes.buffer).setUint32(0, 12, true); return Array.from(bytes);
      }
      case "workspace_image": case "recipe_capture_frame_image": throw new Error("原图不可用");
      case "workspace_navigation_save": {
        const input = args as { revision: number; map: typeof view.navigation.map; confirm: boolean };
        if (input.revision !== view.workspace.revision) throw new Error("候选修订已变化");
        view.navigation!.map = { ...input.map!, confirmed: input.confirm };
        view.workspace.revision++;
        return structuredClone(view);
      }
      default: throw new Error(`未声明的桌面调用 ${command}`);
    }
  });
});

function show(path: string) {
  render(<MemoryRouter initialEntries={[path]}><SnackbarProvider><WorkspaceProvider><Routes>
    <Route path="/recipe/segments" element={<SegmentsPage />} />
    <Route path="/recipe/teach" element={<TeachingPage />} />
    <Route path="/recipe/validation" element={<ValidationPage />} />
  </Routes></WorkspaceProvider></SnackbarProvider></MemoryRouter>);
  return userEvent.setup();
}

it("地图跨段点击 → 浏览点击对应的原始帧组，未确认选帧前不写入示教数据", async () => {
  const user = show("/recipe/teach?camera=CAM-1&segment=1");
  const canvas = await screen.findByRole("img", { name: "工件图与有向胶路" });
  await waitFor(() => expect(canvas.querySelector("image")).toHaveAttribute("href", "blob:navigation"));
  vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({ width: 400, height: 400, x: 0, y: 0, left: 0, top: 0, right: 400, bottom: 400, toJSON() {} });
  await user.click(within(screen.getByRole("group", { name: "地图段选择" })).getByRole("button", { name: "段 2 · 弯道" }));
  fireEvent.pointerDown(canvas, { clientX: 164, clientY: 200, button: 0 });
  await waitFor(() => expect(screen.getByRole("button", { name: "帧组 5 · CAM-1" })).toHaveAttribute("aria-pressed", "true"));
  expect(screen.getByRole("button", { name: /用此帧示教 CAM-1 · 段 2/ })).toBeVisible();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "workspace_teach_segment_corridor" || command === "workspace_save_doc")).toBe(false);
});

it("编辑地图后离页 → 草稿保留并阻止发布，回到分段保存后恢复可发布状态", async () => {
  const user = show("/recipe/segments");
  await user.click(await screen.findByRole("button", { name: "编辑地图" }));
  await user.click(screen.getByText("精确调整与更多操作", { exact: true }));
  fireEvent.change(screen.getByLabelText("段 1 后分界（路径 %）"), { target: { value: "45" } });
  const boundarySummary = await screen.findByRole("status", { name: "分段草稿" });
  fireEvent.keyDown(screen.getByLabelText("帧组 1 三台相机原图"), { key: "B" });
  expect(boundarySummary).toHaveTextContent("边界 4 / 8");
  expect(boundarySummary).not.toHaveTextContent("边界 1 /");
  await user.click(within(screen.getByRole("navigation", { name: "配方配置步骤" })).getByRole("link", { name: /发布/ }));
  expect(await screen.findByRole("button", { name: "发布生产配方" })).toBeDisabled();
  await user.click(within(screen.getByRole("navigation", { name: "配方配置步骤" })).getByRole("link", { name: /段列表/ }));
  expect(await screen.findByLabelText("段 1 后分界（路径 %）")).toHaveValue(45);
  await user.click(screen.getByRole("button", { name: "确认空间对应并保存" }));
  await waitFor(() => expect(screen.queryByLabelText("段 1 后分界（路径 %）")).toBeNull());
  expect(view.navigation!.map!.cuts).toEqual([0.45, 0.7]);
  expect(view.navigation!.map!.confirmed).toBe(true);
  await user.click(within(screen.getByRole("navigation", { name: "配方配置步骤" })).getByRole("link", { name: /发布/ }));
  await waitFor(() => expect(screen.getByRole("button", { name: "发布生产配方" })).toBeEnabled());
});

it("示教页修改视图并保存 → 重开恢复朝向和底色，空间确认及胶路保持不变", async () => {
  const user = show("/recipe/teach?camera=CAM-1&segment=1");
  await screen.findByRole("img", { name: "工件图与有向胶路" });
  const before = structuredClone(view.navigation!.map!);
  await user.click(screen.getByText("视图设置", { exact: true }));
  await user.click(screen.getByRole("button", { name: "向右旋转90度" }));
  await user.click(screen.getByRole("button", { name: "水平翻转" }));
  fireEvent.change(document.querySelector('select[aria-label="透明图片底色"]')!, { target: { value: "light" } });
  expect(screen.getByRole("button", { name: "保存视图" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "保存视图" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "保存视图" })).toBeNull());
  expect(view.navigation!.map).toEqual({ ...before, view: { rotation: 90, flipX: true, flipY: false, background: "light" } });
  cleanup();
  show("/recipe/segments");
  await user.click(await screen.findByText("视图设置", { exact: true }));
  expect(screen.getByLabelText("工件视图旋转角度")).toHaveValue(90);
  expect(screen.getByRole("button", { name: "水平翻转" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("combobox", { name: "透明图片底色" })).toHaveTextContent("浅色");
  expect(screen.queryByRole("button", { name: "保存视图" })).toBeNull();
});

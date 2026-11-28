import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable, desktopCall } from "../src/lib/desktop";
import { subscribe } from "../src/features/plc/api";
import type { PackageImportTarget, WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView } from "./fixtures";
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn(), desktopCall: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn() }));

/** 本机候选 A：导入前是修订 3、名称“现场候选”，导入后后端返回包里的候选（修订 4、名称“回传候选”）。 */
function candidate(revision: number, name: string, instanceId = 41): WorkspaceView {
  const view = burstWorkspaceView(burstRecipe({ id: "A" }));
  return { ...view, workspace: { ...view.workspace, revision, instanceId, doc: { ...view.workspace.doc, name } } };
}
let current = candidate(3, "现场候选");
const path = "C:/r.zip";
const changedNotice = "工件配方包或当前候选已变化，请重新选择配方包";
let manifestSha256 = "a".repeat(64);
const currentTarget = (): PackageImportTarget => ({
  manifestSha256, candidate: { revision: current.workspace.revision, instanceId: current.workspace.instanceId },
});

let workspace: ReturnType<typeof useWorkspace>;
function Shown() {
  workspace = useWorkspace();
  const { data } = workspace;
  return <output aria-label="当前候选">{data ? `${data.workspace.doc.name} r${data.workspace.revision}` : ""}</output>;
}

beforeEach(() => {
  current = candidate(3, "现场候选");
  manifestSha256 = "a".repeat(64);
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockImplementation(async () => ({ recipes: [burstSummary(current.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] }));
  vi.mocked(recipeApi.preview).mockImplementation(async () => current.layout);
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  vi.mocked(subscribe).mockImplementation(() => () => {});
  vi.mocked(desktopCall).mockImplementation(async (command, args) => {
    switch (command) {
      case "workspace_list": return [current.workspace];
      case "workspace_get": return current;
      case "operation_status": return null;
      case "workspace_package_pick":
        return {
          path, bytes: 2048, recipeId: "A", name: "回传候选", productCode: 1, sourceProjectName: "开发",
          exportedAt: 0, appVersion: "0.1.0", teachingPiece: null, teachingImages: false, productionVersion: 2,
          candidate: { revision: current.workspace.revision, updatedAt: 0, pending: false },
          target: currentTarget(), warnings: [], blocker: null,
        };
      case "workspace_package_import": {
        const target = args?.target as PackageImportTarget | undefined;
        const expected = currentTarget();
        if (target?.manifestSha256 !== expected.manifestSha256 ||
          target.candidate?.instanceId !== expected.candidate?.instanceId ||
          target.candidate?.revision !== expected.candidate?.revision) throw new Error(changedNotice);
        current = candidate(4, "回传候选", current.workspace.instanceId + 1);
        return { id: "A", teachingImages: false, notes: [] };
      }
      default: throw new Error(`未声明的桌面调用：${command}`);
    }
  });
});

describe("工件配方导入", () => {
  it("预览后包和同编号候选变化 → 原确认显示失败，重新选包后导入并更新工作台", async () => {
    render(<SnackbarProvider><WorkspaceProvider><MemoryRouter initialEntries={["/recipe"]}><LibraryPage /><Shown /></MemoryRouter></WorkspaceProvider></SnackbarProvider>);
    await act(() => workspace.select("A"));
    await waitFor(() => expect(screen.getByLabelText("当前候选")).toHaveTextContent("现场候选 r3"));
    const user = userEvent.setup();
    const original = currentTarget();
    await user.click(screen.getByRole("button", { name: "导入工件配方" }));
    const dialog = within(await screen.findByRole("dialog", { name: "导入工件配方" }));

    current = candidate(3, "重建的现场候选", current.workspace.instanceId + 1);
    manifestSha256 = "b".repeat(64);
    const replacement = currentTarget();
    await act(() => workspace.reloadList());
    await user.click(dialog.getByRole("button", { name: "导入并替换候选" }));
    const status = within(await dialog.findByRole("status", { name: "导入工件配方进度" }));
    expect(await status.findByRole("alert")).toHaveTextContent(changedNotice);
    expect(screen.getByLabelText("当前候选")).toHaveTextContent("现场候选 r3");
    await user.click(dialog.getByRole("button", { name: "取消" }));

    await user.click(screen.getByRole("button", { name: "导入工件配方" }));
    await user.click(await screen.findByRole("button", { name: "导入并替换候选" }));
    await waitFor(() => expect(screen.getByLabelText("当前候选")).toHaveTextContent("回传候选 r4"));
    expect(vi.mocked(desktopCall).mock.calls.filter(([command]) => command === "workspace_package_import").map(([, args]) => args)).toEqual([
      { path, target: original, taskId: expect.any(String) },
      { path, target: replacement, taskId: expect.any(String) },
    ]);
  });
});

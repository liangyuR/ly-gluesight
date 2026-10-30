import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import type { NavigationMapData } from "../src/features/workspace/navigation/types";
import { burstRecipe, burstSummary, burstWorkspaceView } from "./fixtures";
vi.mock("../src/lib/storage", () => ({ storageApi: { preferences: async () => ({}), updatePreferences: vi.fn(async () => ({})) } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDoc: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn(() => () => {}) }));
const view = (id: string) => burstWorkspaceView(burstRecipe({ id }));
let workspace: ReturnType<typeof useWorkspace>;
function Probe() { workspace = useWorkspace(); return <output aria-label="页面地址">{useLocation().pathname}</output>; }
beforeEach(() => {
  const a = view("A"), b = view("B");
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [burstSummary(a.workspace.doc as ReturnType<typeof burstRecipe>), burstSummary(b.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] });
  vi.mocked(workspaceApi.list).mockResolvedValue([a.workspace, b.workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async id => view(id));
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
});
/** 在工作台改了导航图没保存，再从“配方库”退回配方库页 */
async function leaveWithUnsavedMap() {
  render(<SnackbarProvider><WorkspaceProvider><MemoryRouter initialEntries={["/recipe"]}><LibraryPage /><Probe /></MemoryRouter></WorkspaceProvider></SnackbarProvider>);
  await waitFor(() => expect(workspace.data?.workspace.doc.id).toBe("A"));
  act(() => workspace.setNavigationDraft({ map: { image: { id: "img" } } as NavigationMapData, revision: workspace.data!.workspace.revision }));
  expect(workspace.navigationDirty).toBe(true);
}
const resume = (id: string) => userEvent.click(within(screen.getByText(id).closest(".wp-recipe-card") as HTMLElement).getByRole("button", { name: "继续配置" }));

describe("配方库：带着未保存修改退出后继续配置", () => {
  it("继续配置当前配方 → 回到修改所在的分段页，未保存的导航图草稿保留", async () => {
    await leaveWithUnsavedMap();
    await resume("A");
    await waitFor(() => expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/segments"));
    expect(workspace.navigationDirty).toBe(true);
  });

  it("继续配置其他配方 → 提示未保存，放弃修改后打开该配方", async () => {
    await leaveWithUnsavedMap();
    await resume("B");
    const dialog = await screen.findByRole("dialog", { name: "配方 A 有未保存的修改" });
    await userEvent.click(within(dialog).getByRole("button", { name: "放弃修改并打开 B" }));
    await waitFor(() => expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/progress"));
    expect(workspace.data?.workspace.doc.id).toBe("B");
    expect(workspace.navigationDirty).toBe(false);
  });
});

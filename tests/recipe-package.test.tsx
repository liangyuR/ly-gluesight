import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import { subscribe } from "../src/features/plc/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView } from "./fixtures";
vi.mock("../src/lib/storage", () => ({ storageApi: { preferences: async () => ({ lastWorkspace: "A" }), updatePreferences: vi.fn(async () => ({})) } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDoc: vi.fn(), packagePick: vi.fn(), packageImport: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn() }));

/** 本机候选 A：导入前是修订 3、名称“现场候选”，导入后后端返回包里的候选（修订 4、名称“回传候选”）。 */
function candidate(revision: number, name: string): WorkspaceView {
  const view = burstWorkspaceView(burstRecipe({ id: "A" }));
  return { ...view, workspace: { ...view.workspace, revision, doc: { ...view.workspace.doc, name } } };
}
let current = candidate(3, "现场候选");

function Shown() {
  const { data } = useWorkspace();
  return <output aria-label="当前候选">{data ? `${data.workspace.doc.name} r${data.workspace.revision}` : ""}</output>;
}

beforeEach(() => {
  current = candidate(3, "现场候选");
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockImplementation(async () => ({ recipes: [burstSummary(current.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] }));
  vi.mocked(recipeApi.preview).mockImplementation(async () => current.layout);
  vi.mocked(workspaceApi.list).mockImplementation(async () => [current.workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async () => current);
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  vi.mocked(subscribe).mockImplementation(() => () => {});
  vi.mocked(workspaceApi.packagePick).mockResolvedValue({ path: "C:/r.zip", bytes: 2048, recipeId: "A", name: "回传候选", productCode: 1, sourceProjectName: "开发",
    exportedAt: 0, appVersion: "0.1.0", teachingPiece: null, productionVersion: 2, candidate: { revision: 3, updatedAt: 0, pending: false }, blocker: null });
  vi.mocked(workspaceApi.packageImport).mockImplementation(async () => {
    current = candidate(4, "回传候选");
    return { id: "A", notes: [] };
  });
});

describe("配方回传包导入", () => {
  it("替换了当前打开的候选：导入后工作台持有包里的新候选，不停在旧修订上", async () => {
    render(<SnackbarProvider><WorkspaceProvider><MemoryRouter initialEntries={["/recipe"]}><LibraryPage /><Shown /></MemoryRouter></WorkspaceProvider></SnackbarProvider>);
    await waitFor(() => expect(screen.getByLabelText("当前候选")).toHaveTextContent("现场候选 r3"));
    await userEvent.click(screen.getByRole("button", { name: "导入回传包" }));
    await userEvent.click(await screen.findByRole("button", { name: "导入并替换候选" }));
    await waitFor(() => expect(screen.getByLabelText("当前候选")).toHaveTextContent("回传候选 r4"));
  });
});

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useNavigate } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { inRecipe } from "../src/features/workspace/RecipeRoute";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { WorkspaceEmpty } from "../src/features/workspace/components";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import { burstRecipe, burstSummary, burstWorkspaceView } from "./fixtures";
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDoc: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn(() => () => {}) }));

const view = (id: string) => burstWorkspaceView(burstRecipe({ id }));
let workspace: ReturnType<typeof useWorkspace>;
/** 配方步骤页：显示工作区打开的配方，并能按地址换到配方 B；没读到配方时同真实页面一样显示空状态 */
const Page = inRecipe(function Page() {
  workspace = useWorkspace();
  const navigate = useNavigate();
  if (!workspace.data) return <WorkspaceEmpty />;
  return <><output aria-label="打开的配方">{workspace.data.workspace.doc.id}</output>
    <button onClick={() => navigate("/recipe/B/progress")}>打开配方 B</button></>;
});
const show = (path: string) => render(<SnackbarProvider><WorkspaceProvider><MemoryRouter initialEntries={[path]}><Routes>
  <Route path="/recipe/:recipeId/progress" element={<Page />} />
</Routes></MemoryRouter></WorkspaceProvider></SnackbarProvider>);

beforeEach(() => {
  const a = view("A"), b = view("B");
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [burstSummary(a.workspace.doc as ReturnType<typeof burstRecipe>), burstSummary(b.workspace.doc as ReturnType<typeof burstRecipe>)], errors: [], unavailable: [] });
  vi.mocked(workspaceApi.list).mockResolvedValue([a.workspace, b.workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async id => view(id));
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
});

describe("配方页按地址打开配方", () => {

  // 防的回归：读取配方临时失败后地址里的编号已等于打开的编号，入口不再重读，只能先打开别的配方或重启
  it("读取失败后可以就地重试", async () => {
    vi.mocked(workspaceApi.get).mockRejectedValueOnce(new Error("候选文件暂时读不到"));
    show("/recipe/A/progress");
    expect(await screen.findByText("候选文件暂时读不到")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "重试读取 A" }));
    await waitFor(() => expect(screen.getByLabelText("打开的配方")).toHaveTextContent("A"));
  });

  // 防的回归：按地址换配方时丢掉另一个配方未保存的修改
  it("另一个配方有未保存的修改时不换，提示先回去保存或撤销", async () => {
    show("/recipe/A/progress");
    await waitFor(() => expect(screen.getByLabelText("打开的配方")).toHaveTextContent("A"));
    act(() => workspace.setDoc({ ...workspace.doc!, name: "改了没保存" }));
    await userEvent.click(screen.getByRole("button", { name: "打开配方 B" }));
    expect(await screen.findByText("配方 A 有未保存的修改")).toBeVisible();
    expect(screen.getByRole("link", { name: "回到 A" })).toHaveAttribute("href", "/recipe/A/progress");
    expect(workspaceApi.get).not.toHaveBeenCalledWith("B");
  });
});

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { desktopAvailable } from "../src/lib/desktop";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { create: vi.fn(), remove: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { template: vi.fn(), doc: vi.fn(), removeUnavailable: vi.fn() } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
let ws: ReturnType<typeof workspaceState>;
function Location() { return <output aria-label="页面地址">{useLocation().pathname}</output>; }
const show = () => render(<MemoryRouter initialEntries={["/recipe"]}><LibraryPage /><Location /></MemoryRouter>);
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.template).mockResolvedValue({ ...workspaceView("NEW").workspace.doc, name: "模板" });
  vi.mocked(workspaceApi.create).mockResolvedValue(workspaceView("NEW"));
  vi.mocked(workspaceApi.remove).mockResolvedValue(undefined);
  vi.mocked(ws.select).mockImplementation(async id=>workspaceView(id));
  vi.mocked(recipeApi.doc).mockResolvedValue(workspaceView().workspace.doc);
});

describe("配方库操作", () => {
  it("旧格式配方 → 可见但不可使用，确认删除后从库中消失", async()=>{
    ws.list=[];ws.drafts=[];
    ws.unavailable=[{fileName:"OLD.json",id:"OLD",name:"旧配方",schemaVersion:5,currentSchemaVersion:6,reason:"文件格式不兼容"}];
    vi.mocked(recipeApi.removeUnavailable).mockResolvedValue(undefined);
    vi.mocked(ws.reloadList).mockImplementation(async()=>{ws.unavailable=[];});
    show();
    expect(screen.getByText("旧配方")).toBeVisible();
    expect(screen.getByText("不可用")).toBeVisible();
    expect(screen.queryByRole("button",{name:"继续配置"})).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"升级（暂未实现）"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"删除文件"}));
    expect(screen.getByRole("dialog")).toHaveTextContent("OLD.json");
    await userEvent.click(screen.getByRole("button",{name:"确认删除文件"}));
    await waitFor(()=>expect(screen.queryByText("旧配方")).not.toBeInTheDocument());
    expect(recipeApi.removeUnavailable).toHaveBeenCalledWith("OLD.json");
    expect(workspaceApi.remove).not.toHaveBeenCalled();
    expect(screen.getByText("尚未加载配方")).toBeVisible();
  });
  it.each([
    ["整件已标定", {}, "3 段 · ΣN_exp 12 ± 3", "shotCount 12"],
    ["整件待标定", { partCalib: null }, "3 段 · ΣN_exp 待标定", "shotCount 待标定"],
  ])("段配方%s → 卡片显示段数与 ΣN_exp ± Σtol，不再显示拍照点数", (_, over, scale, plc) => {
    const doc = burstRecipe(over);
    ws = workspaceState(burstWorkspaceView(doc));
    ws.list = [{ ...burstSummary(doc), triggerCount: burstSummary(doc).expectedTotal }]; ws.drafts = [ws.data!.workspace];
    show();
    const card = screen.getByRole("heading", { name: "段配方 S" }).closest<HTMLElement>(".wp-recipe-card")!;
    expect(card).toHaveTextContent(scale);
    expect(card).toHaveTextContent(plc);
    expect(card).not.toHaveTextContent("个拍照点");
  });
  it("创建已成功但选用失败，重试仅打开已有候选，避免二次创建",async()=>{
    vi.mocked(ws.select).mockResolvedValueOnce(null);show();await userEvent.click(screen.getByRole("button",{name:"新建配方"}));await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button",{name:"创建候选"}));expect(await screen.findByText(/候选已创建，选用未完成/)).toBeVisible();
    expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe");expect(screen.getByRole("textbox",{name:"配方编号"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"打开已创建候选"}));expect(workspaceApi.create).toHaveBeenCalledTimes(1);
    await waitFor(()=>expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/progress"));
  });

  it("删除先确认，确认后禁止重复删除，刷新失败只重试刷新",async()=>{
    const removing=deferred<void>();vi.mocked(workspaceApi.remove).mockReturnValueOnce(removing.promise);vi.mocked(ws.reloadList).mockRejectedValueOnce(new Error("刷新失败"));
    show();await userEvent.click(screen.getByRole("button",{name:"删除配方 A"}));
    expect(workspaceApi.remove).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button",{name:"取消"}));expect(workspaceApi.remove).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button",{name:"删除配方 A"}));fireEvent.click(screen.getByRole("button",{name:"删除 A"}));fireEvent.click(screen.getByRole("button",{name:"删除 A"}));
    expect(workspaceApi.remove).toHaveBeenCalledTimes(1);expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();
    await userEvent.keyboard("{Escape}");expect(screen.getByRole("dialog")).toBeVisible();await act(async()=>removing.resolve());
    expect(screen.getByText("刷新失败")).toBeVisible();await userEvent.click(screen.getByRole("button",{name:"刷新配方库"}));
    expect(workspaceApi.remove).toHaveBeenCalledTimes(1);await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

});

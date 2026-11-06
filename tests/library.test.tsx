import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { desktopAvailable } from "../src/lib/desktop";
import { burstRecipe, burstWorkspaceView, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { create: vi.fn(), remove: vi.fn(), packagePick: vi.fn(), packageImport: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { template: vi.fn(), doc: vi.fn(), removeUnavailable: vi.fn() } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
let ws: ReturnType<typeof workspaceState>;
const view = (id = "A") => burstWorkspaceView(burstRecipe({ id }));
function Location() { return <output aria-label="页面地址">{useLocation().pathname}</output>; }
const show = () => render(<MemoryRouter initialEntries={["/recipe"]}><LibraryPage /><Location /></MemoryRouter>);
beforeEach(() => {
  ws = workspaceState(view()); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.template).mockResolvedValue({ ...view("NEW").workspace.doc, name: "模板" });
  vi.mocked(workspaceApi.create).mockResolvedValue(view("NEW"));
  vi.mocked(workspaceApi.remove).mockResolvedValue(undefined);
  vi.mocked(ws.select).mockImplementation(async id=>view(id));
  vi.mocked(recipeApi.doc).mockResolvedValue(view().workspace.doc);
});

describe("配方库操作", () => {
  it("创建已成功但选用失败，重试仅打开已有候选，避免二次创建",async()=>{
    vi.mocked(ws.select).mockResolvedValueOnce(null);show();await userEvent.click(screen.getByRole("button",{name:"新建配方"}));await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button",{name:"创建候选"}));expect(await screen.findByText(/候选已创建，选用未完成/)).toBeVisible();
    expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe");expect(screen.getByRole("textbox",{name:"配方编号"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"打开已创建候选"}));expect(workspaceApi.create).toHaveBeenCalledTimes(1);
    await waitFor(()=>expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/progress"));
  });
  it("回传包替换了当前打开的候选：导入后重新加载这一候选，不留旧修订在界面上",async()=>{
    ws.selectedId="A";
    vi.mocked(workspaceApi.packagePick).mockResolvedValue({path:"C:/r.zip",bytes:2048,recipeId:"A",name:"A",productCode:1,sourceProjectName:"开发",exportedAt:0,appVersion:"0.1.0",
      teachingPiece:null,productionVersion:2,candidate:{revision:9,updatedAt:0,pending:false},blocker:null});
    vi.mocked(workspaceApi.packageImport).mockResolvedValue({id:"A",notes:["示教件原图已从本机历史记录补回（SN 1）"]});
    show();await userEvent.click(screen.getByRole("button",{name:"导入回传包"}));
    await userEvent.click(await screen.findByRole("button",{name:"导入并替换候选"}));
    expect(await screen.findByText("示教件原图已从本机历史记录补回（SN 1）")).toBeVisible();
    expect(workspaceApi.packageImport).toHaveBeenCalledWith("C:/r.zip");
    expect(ws.reloadList).toHaveBeenCalled();expect(ws.select).toHaveBeenCalledWith("A");
  });
});

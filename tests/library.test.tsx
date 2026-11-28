import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { desktopAvailable } from "../src/lib/desktop";
import { burstRecipe, burstSummary, burstWorkspaceView, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { create: vi.fn(), get: vi.fn(), remove: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { template: vi.fn(), doc: vi.fn(), removeUnavailable: vi.fn() } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
let ws: ReturnType<typeof workspaceState>;
const view = (id = "A") => burstWorkspaceView(burstRecipe({ id }));
function Location() { return <output aria-label="页面地址">{useLocation().pathname}</output>; }
const library = () => <MemoryRouter initialEntries={["/recipe"]}><LibraryPage /><Location /></MemoryRouter>;
const show = () => render(library());
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
  // 防的回归：候选已创建、刷新配方列表失败后重试，又建了第二个候选
  it("创建已成功但刷新列表失败，重试仅打开已有候选，避免二次创建",async()=>{
    vi.mocked(ws.reloadList).mockRejectedValueOnce(new Error("配方列表读取失败"));show();await userEvent.click(screen.getByRole("button",{name:"新建配方"}));await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button",{name:"创建候选"}));expect(await screen.findByText(/配方列表读取失败/)).toBeVisible();
    expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe");expect(screen.getByRole("textbox",{name:"配方编号"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"打开已创建候选"}));expect(workspaceApi.create).toHaveBeenCalledTimes(1);
    await waitFor(()=>expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/NEW/progress"));
  });
});

it("生产与候选型号不同 → 可以按任一型号或卡片编号、名称找到配方", async () => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ id: "L-42", name: "螺旋配方", productCode: 5678 })));
  ws.list = [
    burstSummary(burstRecipe({ id: "L-42", name: "已发布螺旋配方", productCode: 1234 })),
    burstSummary(burstRecipe({ id: "B", name: "直线配方", productCode: 9000 })),
  ];
  show();
  expect(screen.getByText("PLC 型号 1234")).toBeVisible();
  const search = screen.getByRole("textbox", { name: "搜索配方" });
  const user = userEvent.setup();

  for (const query of ["1234", "5678", " l-42 ", "螺旋配方"]) {
    await user.clear(search);
    await user.type(search, query);
    expect(screen.getByRole("heading", { name: "螺旋配方" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "直线配方" })).not.toBeInTheDocument();
  }
});

it("已加载候选比列表新 → 按最新修订删除，刷新失败时只重试刷新", async () => {
  ws.drafts = [structuredClone(ws.drafts[0]), view("B").workspace];
  const current = ws.data!.workspace;
  current.doc.name = "最新候选 A";
  current.revision++;
  vi.mocked(ws.reloadList).mockRejectedValueOnce(new Error("配方列表读取失败"));
  show();
  expect(screen.getByRole("heading", { name: "最新候选 A" })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "更多操作 A" }));
  await userEvent.click(screen.getByRole("menuitem", { name: "删除配方" }));
  const dialog = await screen.findByRole("dialog", { name: "删除配方" });
  expect(workspaceApi.remove).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole("button", { name: "删除 A" }));
  expect(await within(dialog).findByText("配方列表读取失败")).toBeVisible();
  await userEvent.click(within(dialog).getByRole("button", { name: "刷新配方库" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(workspaceApi.remove).toHaveBeenCalledExactlyOnceWith("A", {
    candidate: { instanceId: current.instanceId, revision: current.revision },
    productionRevisionId: ws.list[0].revisionId,
  });
});

it("确认后出现同编号同修订的新候选 → 保留原说明并要求重新确认", async () => {
  const loaded = ws.data!.workspace;
  const legacy = { ...loaded, flow: 1, instanceId: loaded.instanceId + 1 };
  ws.drafts = [legacy];
  const page = show();
  await userEvent.click(screen.getByRole("button", { name: "更多操作 A" }));
  await userEvent.click(screen.getByRole("menuitem", { name: "删除配方" }));
  const dialog = within(await screen.findByRole("dialog", { name: "删除配方" }));
  expect(dialog.getByText(/同编号的生产配方照常运行/)).toBeVisible();

  const replacement = { ...legacy, flow: 2, instanceId: legacy.instanceId + 1 };
  ws.drafts = [replacement];
  page.rerender(library());
  expect(dialog.getByText(/同编号的生产配方照常运行/)).toBeVisible();
  expect(dialog.getByText("配方已更新")).toBeVisible();
  expect(dialog.getByRole("button", { name: "删除 A" })).toBeDisabled();
  expect(workspaceApi.remove).not.toHaveBeenCalled();

  await userEvent.click(dialog.getByRole("button", { name: "取消" }));
  await userEvent.click(screen.getByRole("button", { name: "更多操作 A" }));
  await userEvent.click(screen.getByRole("menuitem", { name: "删除配方" }));
  const reopened = within(await screen.findByRole("dialog", { name: "删除配方" }));
  expect(reopened.getByText(/生产配置、候选及示教资料/)).toBeVisible();
  await userEvent.click(reopened.getByRole("button", { name: "删除 A" }));
  expect(workspaceApi.remove).toHaveBeenCalledExactlyOnceWith("A", {
    candidate: { instanceId: replacement.instanceId, revision: replacement.revision },
    productionRevisionId: ws.list[0].revisionId,
  });
});

it("列表只剩生产配方 → 按生产修订删除，不使用上次打开的候选", async () => {
  const production = burstSummary(burstRecipe({ id: "B" }));
  ws.list = [...ws.list, production];
  ws.data = view("B");
  ws.selectedId = "B";
  show();
  await userEvent.click(screen.getByRole("button", { name: "更多操作 B" }));
  await userEvent.click(screen.getByRole("menuitem", { name: "删除配方" }));
  const dialog = within(await screen.findByRole("dialog", { name: "删除配方" }));
  expect(dialog.getByText(/将删除此生产配方及示教资料/)).toBeVisible();
  await userEvent.click(dialog.getByRole("button", { name: "删除 B" }));
  expect(workspaceApi.remove).toHaveBeenCalledExactlyOnceWith("B", {
    candidate: null,
    productionRevisionId: production.revisionId,
  });
  expect(workspaceApi.get).not.toHaveBeenCalled();
  expect(ws.select).not.toHaveBeenCalled();
});

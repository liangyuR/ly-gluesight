import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LibraryPage from "../src/features/workspace/LibraryPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { desktopAvailable } from "../src/lib/desktop";
import { deferred, summary, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { create: vi.fn(), remove: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { template: vi.fn(), doc: vi.fn() } }));
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
  it("合并生产与候选且去重，候选名称用于搜索", async () => {
    ws.drafts[0].doc.name = "候选名称";
    const b = workspaceView("B"); ws.list.push(summary(b)); ws.drafts.push(b.workspace);
    show(); expect(screen.getAllByRole("button", { name: "配置候选" })).toHaveLength(2);
    await userEvent.type(screen.getByRole("textbox", { name: "搜索配方" }), "候选名称");
    expect(screen.getAllByRole("button", { name: "配置候选" })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "复制配方 A" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "复制配方 B" })).not.toBeInTheDocument();
    await userEvent.clear(screen.getByRole("textbox", { name: "搜索配方" }));
    expect(screen.getByRole("button", { name: "复制配方 B" })).toBeVisible();
    expect(screen.queryByRole("combobox", { name: "配方工况筛选" })).not.toBeInTheDocument();
  });

  it("复制产生独立候选，避开已有编号与产品代码", async () => {
    const copy = workspaceView("A-COPY"); copy.workspace.doc.productCode = 2;
    ws.drafts.push(copy.workspace); show();
    await userEvent.click(screen.getByRole("button", { name: "复制配方 A" }));
    const dialog = await screen.findByRole("dialog", { name: "建立候选配方" });
    expect(within(dialog).getByRole("textbox", { name: "配方编号" })).toHaveValue("A-COPY-2");
    expect(within(dialog).getByRole("spinbutton", { name: "产品代码" })).toHaveValue(3);
    await userEvent.click(within(dialog).getByRole("button", { name: "创建候选" }));
    expect(workspaceApi.create).toHaveBeenCalledWith(expect.objectContaining({ id: "A-COPY-2", version: 1, teachingHash: null, productCode: 3 }));
    expect(ws.drafts[0].doc.id).toBe("A");
    await waitFor(() => expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/geometry"));
  });

  it("创建失败显示错误并保留表单，空编号不能提交", async () => {
    vi.mocked(workspaceApi.create).mockRejectedValueOnce(new Error("编号已存在")); show();
    await userEvent.click(screen.getByRole("button", { name: "新建飞拍" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "创建候选" }));
    expect(await screen.findByText("Error: 编号已存在")).toBeVisible();
    expect(screen.getByRole("dialog")).toBeVisible(); expect(ws.select).not.toHaveBeenCalled();
    await userEvent.clear(within(dialog).getByRole("textbox", { name: "配方编号" }));
    expect(within(dialog).getByRole("button", { name: "创建候选" })).toBeDisabled();
  });

  it("删除先确认，取消不删除，最后一个删除后清空选择", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "删除配方 A" }));
    expect(workspaceApi.remove).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    await userEvent.click(screen.getByRole("button", { name: "删除配方 A" }));
    await userEvent.click(screen.getByRole("button", { name: "删除 A" }));
    expect(workspaceApi.remove).toHaveBeenCalledWith("A");
    await waitFor(() => expect(ws.clearSelection).toHaveBeenCalledTimes(1));
  });

  it("删除失败保留弹窗和当前选择", async () => {
    vi.mocked(workspaceApi.remove).mockRejectedValueOnce(new Error("正在生产")); show();
    await userEvent.click(screen.getByRole("button", { name: "删除配方 A" }));
    await userEvent.click(screen.getByRole("button", { name: "删除 A" }));
    expect(await screen.findByText("Error: 正在生产")).toBeVisible();
    expect(ws.clearSelection).not.toHaveBeenCalled(); expect(screen.getByRole("dialog")).toBeVisible();
  });

  it("浏览器查看模式禁止新建配方", () => {
    vi.mocked(desktopAvailable).mockReturnValue(false); show();
    expect(screen.getByRole("button", { name: "新建飞拍" })).toBeDisabled();
    expect(screen.getByRole("button",{name:"复制配方 A"})).toBeDisabled();expect(screen.getByRole("button",{name:"删除配方 A"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"配置候选"})).toBeDisabled();
  });

  it("搜索编号忽略大小写及外围空白，产品代码可搜索并有空结果提示",async()=>{
    show();fireEvent.change(screen.getByRole("textbox",{name:"搜索配方"}),{target:{value:"  a  "}});
    expect(screen.getByRole("button",{name:"复制配方 A"})).toBeVisible();
    fireEvent.change(screen.getByRole("textbox",{name:"搜索配方"}),{target:{value:"1"}});expect(screen.getByRole("button",{name:"复制配方 A"})).toBeVisible();
    fireEvent.change(screen.getByRole("textbox",{name:"搜索配方"}),{target:{value:"不存在"}});expect(screen.getByRole("heading",{name:"没有匹配的配方"})).toBeVisible();
  });

  it("空库说明加载状态",async()=>{
    ws.list=[];ws.drafts=[];show();expect(screen.getByRole("heading",{name:"尚未加载配方"})).toBeVisible();
    expect(screen.queryByRole("button",{name:"配置候选"})).not.toBeInTheDocument();
  });

  it("选择失败留在库中，重试只有确实选择成功才跳转",async()=>{
    vi.mocked(ws.select).mockResolvedValueOnce(null);show();await userEvent.click(screen.getByRole("button",{name:"配置候选"}));
    expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe");
    await userEvent.click(screen.getByRole("button",{name:"配置候选"}));await waitFor(()=>expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/geometry"));
  });

  it("选择未完成不能重复提交，选择被更新或页面已离开时不跳转",async()=>{
    const pending=deferred<ReturnType<typeof workspaceView>|null>();vi.mocked(ws.select).mockReturnValueOnce(pending.promise);
    const page=show();fireEvent.click(screen.getByRole("button",{name:"配置候选"}));fireEvent.click(screen.getByRole("button",{name:"配置候选"}));
    expect(ws.select).toHaveBeenCalledTimes(1);expect(screen.getByRole("button",{name:"复制配方 A"})).toBeDisabled();
    await act(async()=>pending.resolve(null));expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe");
    const abandoned=deferred<ReturnType<typeof workspaceView>|null>();vi.mocked(ws.select).mockReturnValueOnce(abandoned.promise);
    await userEvent.click(screen.getByRole("button",{name:"配置候选"}));page.unmount();await act(async()=>abandoned.resolve(workspaceView()));
  });

  it("新建使用飞拍模板；取消不创建且准备失败可重试",async()=>{
    vi.mocked(recipeApi.template).mockRejectedValueOnce(new Error("模板读取失败")).mockResolvedValueOnce(workspaceView("NEW").workspace.doc);
    show();await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));
    await waitFor(()=>expect(ws.setError).toHaveBeenCalledWith("Error: 模板读取失败"));expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));expect(recipeApi.template).toHaveBeenLastCalledWith();
    await screen.findByRole("dialog");await userEvent.click(screen.getByRole("button",{name:"取消"}));expect(workspaceApi.create).not.toHaveBeenCalled();
  });

  it("仅生产配方复制读取保存文档，候选副本优先使用草稿且互不改写",async()=>{
    ws.drafts=[];show();await userEvent.click(screen.getByRole("button",{name:"复制配方 A"}));
    expect(recipeApi.doc).toHaveBeenCalledWith("A");await screen.findByRole("dialog");
    expect(screen.getByRole("textbox",{name:"配方名称"})).toHaveValue("工件 A（副本）");
    await userEvent.click(screen.getByRole("button",{name:"取消"}));
  });

  it.each(["","A","a","has space","../other","中文编号"])("新建拒绝无效或大小写重复编号 %j",async id=>{
    show();await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));await screen.findByRole("dialog");
    fireEvent.change(screen.getByRole("textbox",{name:"配方编号"}),{target:{value:id}});
    expect(screen.getByRole("button",{name:"创建候选"})).toBeDisabled();expect(screen.getByRole("alert")).toBeVisible();expect(workspaceApi.create).not.toHaveBeenCalled();
  });

  it.each(["0","-1","1","2.5","65536",""])("新建拒绝无效/重复产品代码 %j",async code=>{
    show();await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));await screen.findByRole("dialog");
    fireEvent.change(screen.getByRole("spinbutton",{name:"产品代码"}),{target:{value:code}});
    expect(screen.getByRole("button",{name:"创建候选"})).toBeDisabled();expect(screen.getByRole("alert")).toBeVisible();expect(workspaceApi.create).not.toHaveBeenCalled();
  });

  it("空白名称禁用提交，填写合法编号代码和名称后准确创建并去除外围空白",async()=>{
    show();await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));await screen.findByRole("dialog");
    fireEvent.change(screen.getByRole("textbox",{name:"配方名称"}),{target:{value:"   "}});expect(screen.getByRole("button",{name:"创建候选"})).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox",{name:"配方名称"}),{target:{value:"  新工件  "}});
    fireEvent.change(screen.getByRole("textbox",{name:"配方编号"}),{target:{value:"PART-NEW_1"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"产品代码"}),{target:{value:"65535"}});
    await userEvent.click(screen.getByRole("button",{name:"创建候选"}));
    expect(workspaceApi.create).toHaveBeenCalledWith(expect.objectContaining({id:"PART-NEW_1",name:"新工件",productCode:65535}));
  });

  it("复制编号避开大小写相同的旧副本",async()=>{
    const old=workspaceView("a-copy");old.workspace.doc.productCode=2;ws.drafts.push(old.workspace);
    show();await userEvent.click(screen.getByRole("button",{name:"复制配方 A"}));await screen.findByRole("dialog");
    expect(screen.getByRole("textbox",{name:"配方编号"})).toHaveValue("A-COPY-2");expect(screen.getByRole("spinbutton",{name:"产品代码"})).toHaveValue(3);
  });

  it("模板读取和创建保存等待期间都不能重复提交，输入/取消/关闭受保护",async()=>{
    const preparing=deferred<ReturnType<typeof workspaceView>["workspace"]["doc"]>(),saving=deferred<ReturnType<typeof workspaceView>>();
    vi.mocked(recipeApi.template).mockReturnValueOnce(preparing.promise);vi.mocked(workspaceApi.create).mockReturnValueOnce(saving.promise);
    show();fireEvent.click(screen.getByRole("button",{name:"新建飞拍"}));fireEvent.click(screen.getByRole("button",{name:"新建飞拍"}));expect(recipeApi.template).toHaveBeenCalledTimes(1);
    await act(async()=>preparing.resolve(workspaceView("NEW").workspace.doc));
    fireEvent.click(screen.getByRole("button",{name:"创建候选"}));fireEvent.click(screen.getByRole("button",{name:"创建中…"}));
    expect(workspaceApi.create).toHaveBeenCalledTimes(1);expect(screen.getByRole("textbox",{name:"配方编号"})).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"产品代码"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();await userEvent.keyboard("{Escape}");await userEvent.click(screen.getByRole("button",{name:"关闭"}));
    expect(screen.getByRole("dialog")).toBeVisible();await act(async()=>saving.resolve(workspaceView("NEW")));
  });

  it("创建已成功但选用失败，重试仅打开已有候选，避免二次创建",async()=>{
    vi.mocked(ws.select).mockResolvedValueOnce(null);show();await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button",{name:"创建候选"}));expect(await screen.findByText(/候选已创建，选用未完成/)).toBeVisible();
    expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe");expect(screen.getByRole("textbox",{name:"配方编号"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"打开已创建候选"}));expect(workspaceApi.create).toHaveBeenCalledTimes(1);
    await waitFor(()=>expect(screen.getByLabelText("页面地址")).toHaveTextContent("/recipe/geometry"));
  });

  it("创建已成功但列表刷新失败，重试也不会重复创建",async()=>{
    vi.mocked(ws.reloadList).mockRejectedValueOnce(new Error("列表读取失败"));show();await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button",{name:"创建候选"}));expect(await screen.findByText("Error: 列表读取失败")).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"打开已创建候选"}));expect(workspaceApi.create).toHaveBeenCalledTimes(1);
  });

  it("删除非当前配方不改变当前选择；删除当前项才选用剩余项",async()=>{
    const b=workspaceView("B");b.workspace.doc.productCode=2;ws.drafts.push(b.workspace);ws.list.push(summary(b));show();
    await userEvent.click(screen.getByRole("button",{name:"删除配方 B"}));await userEvent.click(screen.getByRole("button",{name:"删除 B"}));
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());expect(ws.select).not.toHaveBeenCalled();expect(ws.clearSelection).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button",{name:"删除配方 A"}));await userEvent.click(screen.getByRole("button",{name:"删除 A"}));
    await waitFor(()=>expect(ws.select).toHaveBeenCalledWith("B"));
  });

  it("删除等待期间不能重复操作；后台删除成功刷新失败后只重试刷新",async()=>{
    const removing=deferred<void>();vi.mocked(workspaceApi.remove).mockReturnValueOnce(removing.promise);vi.mocked(ws.reloadList).mockRejectedValueOnce(new Error("刷新失败"));
    show();await userEvent.click(screen.getByRole("button",{name:"删除配方 A"}));fireEvent.click(screen.getByRole("button",{name:"删除 A"}));fireEvent.click(screen.getByRole("button",{name:"删除 A"}));
    expect(workspaceApi.remove).toHaveBeenCalledTimes(1);expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();
    await userEvent.keyboard("{Escape}");expect(screen.getByRole("dialog")).toBeVisible();await act(async()=>removing.resolve());
    expect(screen.getByText("Error: 刷新失败")).toBeVisible();await userEvent.click(screen.getByRole("button",{name:"刷新配方库"}));
    expect(workspaceApi.remove).toHaveBeenCalledTimes(1);await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("待生效版本禁止删除；准备结果晚到或失败不会打开已离开的窗口",async()=>{
    ws.drafts[0].pending={doc:ws.doc!,revision:7,baseHash:"old",frames:[],overview:ws.data!.workspace.overview,validation:ws.data!.workspace.validation!};
    const pending=deferred<ReturnType<typeof workspaceView>["workspace"]["doc"]>();vi.mocked(recipeApi.template).mockReturnValueOnce(pending.promise);
    const page=show();expect(screen.getByRole("button",{name:"删除配方 A"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"新建飞拍"}));page.unmount();await act(async()=>pending.reject(new Error("离开后的模板错误")));
    expect(ws.setError).not.toHaveBeenCalled();
  });
});

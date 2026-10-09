import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi, useCycle } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera";
import type { FrameParams, WorkspaceView } from "../src/features/workspace/types";
import { deferred, snapshot, summary, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn(),recipeApi:{list:vi.fn(),preview:vi.fn()} }));
vi.mock("../src/features/camera",()=>({cameraApi:{rigConfig:vi.fn()}}));
vi.mock("../src/features/plc",()=>({subscribe:vi.fn(()=>()=>{})}));
vi.mock("../src/lib/desktop",()=>({desktopAvailable:()=>true}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  get:vi.fn(),list:vi.fn(),image: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(), capture: vi.fn(), saveParams: vi.fn(), restoreTeach: vi.fn(),
} }));

let ws: ReturnType<typeof workspaceState>;
function show(path = "/recipe/teach") {
  return render(<MemoryRouter initialEntries={[path]}><TeachingPage /></MemoryRouter>);
}
async function showProvider(){
  const real=await vi.importActual<typeof import("../src/features/workspace/context")>("../src/features/workspace/context");
  vi.mocked(useWorkspace).mockImplementation(real.useWorkspace);
  const page=render(<MemoryRouter><real.WorkspaceProvider><TeachingPage/></real.WorkspaceProvider></MemoryRouter>);
  await screen.findByRole("spinbutton",{name:"平移 X"});return page;
}
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.get).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.list).mockResolvedValue([ws.data!.workspace]);
  vi.mocked(recipeApi.list).mockResolvedValue({recipes:[summary(ws.data!)],errors:[]});vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  for (const method of [workspaceApi.trial, workspaceApi.saveTeach, workspaceApi.capture, workspaceApi.saveParams, workspaceApi.restoreTeach]) {
    vi.mocked(method).mockResolvedValue(ws.data!);
  }
});

describe("真实单帧示教页面", () => {
  it("链接指定帧，切换帧后调用携带对应图像、参数和修订号", async () => {
    show("/recipe/teach?frame=1");
    expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "试测当前帧" }));
    expect(workspaceApi.trial).toHaveBeenCalledWith("A", 7, 1, "A-image-1", ws.data!.workspace.frames[1].params);
    await userEvent.click(screen.getByRole("button", { name: "选择帧 k1" }));
    await userEvent.click(screen.getByRole("button", { name: "保存本帧示教" }));
    expect(workspaceApi.saveTeach).toHaveBeenCalledWith("A", 7, 0, "A-image-0", ws.data!.workspace.frames[0].params);
  });

  it.each(["no-image", "failed-trial", "stale-image", "saved", "dirty", "busy"])("状态 %s 阻止保存", async condition => {
    const frame = ws.data!.workspace.frames[0];
    if (condition === "no-image") frame.image = null;
    if (condition === "failed-trial") frame.trial!.passed = false;
    if (condition === "stale-image") frame.trial!.imageId = "old-image";
    if (condition === "saved") frame.saved = true;
    if (condition === "dirty") ws.dirty = true;
    if (condition === "busy") ws.busy = true;
    show();
    const save = screen.getByRole("button", { name: "保存本帧示教" });
    expect(save).toBeDisabled(); await userEvent.click(save);
    expect(workspaceApi.saveTeach).not.toHaveBeenCalled();
  });

  it("修改参数后旧试测不能用于保存，重测使用当前参数草稿", async () => {
    const page = show();
    fireEvent.change(screen.getByRole("spinbutton", { name: "搜索余量" }), { target: { value: "5" } });
    const params = { ...ws.data!.workspace.frames[0].params, searchMm: 5 };
    expect(ws.setFrameParams).toHaveBeenCalledWith(0, params);
    ws.frameDrafts = { 0: params }; ws.frameDirty = true;
    page.rerender(<MemoryRouter><TeachingPage /></MemoryRouter>);
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    expect(screen.getByText("参数已变更，现有试测不用于保存或发布。")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "试测当前帧" }));
    expect(workspaceApi.trial).toHaveBeenCalledWith("A", 7, 0, "A-image-0", params);
  });

  it("模板小于 16 像素不能试测", () => {
    ws.data!.workspace.frames[0].params.rect[2] = 15; show();
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
  });

  it.each(["ACQUIRE", "DRAIN", "JUDGE", "REPORT"] as const)("生产 %s 阶段不能取样", phase => {
    vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(phase), logs: [], measured: [] }); show();
    expect(screen.getByRole("button", { name: "取新样本" })).toBeDisabled();
    expect(screen.getByText("工件正在检测，结束后可取示教样本。")).toBeVisible();
  });

  it("空闲可以取样，成功保存后前进一帧且不越过末帧", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "取新样本" }));
    expect(workspaceApi.capture).toHaveBeenCalledWith("A", 7, 0);
    await userEvent.click(screen.getByRole("button", { name: "保存并示教下一帧" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true"));
    await userEvent.click(screen.getByRole("button", { name: "保存并示教下一帧" }));
    expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true");
  });

  it("保存失败停在原帧", async () => {
    vi.mocked(ws.act).mockResolvedValueOnce(null); show();
    await userEvent.click(screen.getByRole("button", { name: "保存并示教下一帧" }));
    expect(screen.getByRole("button", { name: "选择帧 k1" })).toHaveAttribute("aria-pressed", "true");
  });

  it("随动配方引导到相机标定，无法执行飞拍示教", () => {
    ws.doc!.mode = "follow"; show();
    expect(screen.getByRole("link", { name: "打开随动相机标定" })).toHaveAttribute("href", "/camera/follow");
    expect(screen.queryByRole("button", { name: "保存本帧示教" })).not.toBeInTheDocument();
  });
  it.each([["not-a-frame",1],["-1",1],["0.5",1],["Infinity",1],["99",2]])("路由帧 %s 回到有效帧 k%s",(query,expected)=>{
    show("/recipe/teach?frame="+query);expect(screen.getByRole("button",{name:"选择帧 k"+expected})).toHaveAttribute("aria-pressed","true");
    expect(screen.queryByText("当前胶路没有拍照点")).toBeNull();
  });
  it("没有拍照帧时引导回规划并禁用示教操作",()=>{
    ws.data!.workspace.frames=[];show();expect(screen.getByText("当前胶路没有拍照点")).toBeVisible();
    expect(screen.getByRole("link",{name:"胶路与拍照规划"})).toHaveAttribute("href","/recipe/geometry");
    expect(screen.queryByRole("button",{name:"试测当前帧"})).toBeNull();
  });
  it.each([
    ["搜索余量","searchMm",5],["最低灰度对比","minContrast",40],["最低定位分数","minScore",.7],
    ["像素当量","mmPerPx",.2],["平移 X","dx",3],["平移 Y","dy",-4],["旋转角度","deg",12.5],
  ] as const)("参数 %s 按当前帧保存到草稿",(label,key,value)=>{
    show("/recipe/teach?frame=1");fireEvent.change(screen.getByRole("spinbutton",{name:label}),{target:{value:String(value)}});
    expect(ws.setFrameParams).toHaveBeenCalledWith(1,{...ws.data!.workspace.frames[1].params,[key]:value});
  });
  it.each([
    ["mmPerPx",0],["searchMm",0],["minContrast",-1],["minContrast",256],["minScore",1.1],
    ["dx",NaN],["dy",Infinity],["deg",-Infinity],
  ] as const)("无效参数 %s=%s 不能试测或保存参数草稿",(key,value)=>{
    ws.frameDrafts={0:{...ws.data!.workspace.frames[0].params,[key]:value}};show();
    for(const name of ["试测当前帧","保存本帧示教","保存参数草稿"])expect(screen.getByRole("button",{name})).toBeDisabled();
    expect(screen.getByText("像素当量、搜索余量、对比度或定位分数无效")).toBeVisible();
  });
  it.each([[-1,10,32,32],[10.5,10,32,32],[80,10,32,32],[10,45,32,32]])("模板 %j 必须是图像内的整数像素范围",(x,y,w,h)=>{
    ws.data!.workspace.frames[0].params.rect=[x,y,w,h];show();
    expect(screen.getByRole("button",{name:"试测当前帧"})).toBeDisabled();
    expect(screen.getByText("请在冻结图像内框选至少 16×16 px 的定位模板")).toBeVisible();
  });
  it("图像的几何标记改变后旧试测不能保存",()=>{
    ws.data!.workspace.frames[0].trial!.geometryTag="old-geometry";show();
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"保存并示教下一帧"})).toBeDisabled();
    expect(screen.getByText("试测已过期")).toBeVisible();expect(screen.getByText("现有试测已过期，请重新试测当前帧。")).toBeVisible();
  });
  it("已保存帧直接前进，不重复保存或修改候选修订",async()=>{
    ws.data!.workspace.frames[0].saved=true;show();await userEvent.click(screen.getByRole("button",{name:"保存并示教下一帧"}));
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toHaveAttribute("aria-pressed","true");expect(workspaceApi.saveTeach).not.toHaveBeenCalled();
  });
  it.each(["trial","saveTeach","capture","saveParams","restoreTeach"] as const)("%s 等待锁定当前帧、拒绝重复提交，失败后可重试",async method=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi[method]).mockReturnValueOnce(request.promise);
    const labels={trial:"试测当前帧",saveTeach:"保存本帧示教",capture:"取新样本",saveParams:"保存参数草稿",restoreTeach:"恢复原始示教"};
    if(method==="saveParams")ws.frameDrafts={0:{...ws.data!.workspace.frames[0].params,searchMm:5}};
    if(method==="restoreTeach")ws.data!.workspace.frames[0].backup=structuredClone(ws.data!.workspace.frames[0]);
    show();const button=screen.getByRole("button",{name:labels[method]});fireEvent.click(button);fireEvent.click(button);
    expect(workspaceApi[method]).toHaveBeenCalledTimes(1);expect(button).toBeDisabled();
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"像素当量"})).toBeDisabled();
    await act(async()=>request.reject(new Error("本帧命令失败")));expect(ws.setError).toHaveBeenCalledWith("Error: 本帧命令失败");
    expect(button).toBeEnabled();await userEvent.click(button);expect(workspaceApi[method]).toHaveBeenCalledTimes(2);
  });
  it.each(["recipe","revision","unmount"])("保存并下一帧在 %s 改变后丢弃旧完成",async change=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi.saveTeach).mockReturnValueOnce(request.promise);
    const old=ws.data!,page=show();await userEvent.click(screen.getByRole("button",{name:"保存并示教下一帧"}));
    if(change==="unmount")page.unmount();else{
      ws=workspaceState(workspaceView(change==="recipe"?"B":"A"));if(change==="revision")ws.data!.workspace.revision=8;
      page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);
    }
    await act(async()=>request.resolve(old));if(change!=="unmount")expect(screen.getByRole("button",{name:"选择帧 k1"})).toHaveAttribute("aria-pressed","true");
  });
  it("保存参数与恢复备份携带当前帧修订，曝光增益未记录时不显示零值",async()=>{
    const frame=ws.data!.workspace.frames[1];frame.backup=structuredClone(frame);frame.image!.exposureUs=null;frame.image!.gainDb=null;
    const params:FrameParams={...frame.params,dx:2};ws.frameDrafts={1:params};show("/recipe/teach?frame=1");
    expect(screen.getAllByText("未记录")).toHaveLength(2);await userEvent.click(screen.getByRole("button",{name:"保存参数草稿"}));
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A",7,1,params);await userEvent.click(screen.getByRole("button",{name:"恢复原始示教"}));
    expect(workspaceApi.restoreTeach).toHaveBeenCalledWith("A",7,1);
  });
  it("示教视图可以缩放与关闭叠加，切换帧后展示对应冻结图和几何",async()=>{
    vi.mocked(workspaceApi.image).mockImplementation(async(_id,imageId)=>({url:"data:image/png;base64,"+imageId,width:100,height:60}));
    show();const first=await screen.findByRole("img",{name:"冻结图像 · A-image-0"});expect(first.querySelectorAll("polyline")).toHaveLength(1);
    await userEvent.click(screen.getByRole("button",{name:"放大原图"}));expect(first.querySelector("g")).toHaveAttribute("transform",expect.stringContaining("scale(1.25)"));
    await userEvent.click(screen.getByRole("button",{name:"测量叠加"}));expect(first.querySelectorAll("polyline")).toHaveLength(0);expect(first.querySelector("rect")).toBeNull();
    await userEvent.click(screen.getByRole("button",{name:"测量叠加"}));await userEvent.click(screen.getByRole("button",{name:"适应窗口"}));
    expect(first.querySelector("g")).toHaveAttribute("transform",expect.stringContaining("scale(1)"));await userEvent.click(screen.getByRole("button",{name:"选择帧 k2"}));
    const next=await screen.findByRole("img",{name:"冻结图像 · A-image-1"});await waitFor(()=>expect(next.querySelector("image")).toHaveAttribute("href","data:image/png;base64,A-image-1"));
    expect(next.querySelectorAll("polyline")).toHaveLength(1);expect(screen.getByRole("button",{name:"测量叠加"})).toHaveAttribute("aria-pressed","true");
  });
  it("框选模板只更新当前帧草稿，尚不足 16 像素的草稿可保存而不能试测",async()=>{
    vi.stubGlobal("DOMPoint",class {constructor(public x:number,public y:number){}matrixTransform(){return this;}});
    vi.stubGlobal("PointerEvent",MouseEvent);Object.defineProperty(SVGElement.prototype,"getScreenCTM",{configurable:true,value:()=>({inverse:()=>({})})});
    Object.defineProperty(SVGElement.prototype,"setPointerCapture",{configurable:true,value:vi.fn()});
    const page=show("/recipe/teach?frame=1"),svg=await screen.findByRole("img",{name:"冻结图像 · A-image-1"});
    fireEvent.pointerDown(svg,{clientX:10,clientY:10});fireEvent.pointerMove(svg,{clientX:20,clientY:20});fireEvent.pointerUp(svg);
    const params:FrameParams={...ws.data!.workspace.frames[1].params,rect:[10,10,10,10]};expect(ws.setFrameParams).toHaveBeenLastCalledWith(1,params);
    ws.frameDrafts={1:params};page.rerender(<MemoryRouter initialEntries={["/recipe/teach?frame=1"]}><TeachingPage/></MemoryRouter>);
    expect(screen.getByRole("button",{name:"试测当前帧"})).toBeDisabled();expect(screen.getByRole("button",{name:"保存参数草稿"})).toBeEnabled();
    await userEvent.click(screen.getByRole("button",{name:"保存参数草稿"}));expect(workspaceApi.saveParams).toHaveBeenCalledWith("A",7,1,params);
  });
  it("不能将负数或非整数模板保存为参数草稿",()=>{
    ws.frameDrafts={0:{...ws.data!.workspace.frames[0].params,rect:[-.5,10,32,32]}};show();
    expect(screen.getByRole("button",{name:"保存参数草稿"})).toBeDisabled();
  });
  it("保存命令接受新修订后仍能前进到下一帧",async()=>{
    const page=show(),next=workspaceView();next.workspace.revision=8;next.workspace.frames[0].saved=true;
    vi.mocked(ws.act).mockImplementationOnce(async request=>{
      await request();ws.data=next;ws.doc=next.workspace.doc;page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);return next;
    });
    await userEvent.click(screen.getByRole("button",{name:"保存并示教下一帧"}));
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toHaveAttribute("aria-pressed","true");
  });
  it("真实 Provider 恢复新修订后对齐本帧旧草稿，显示备份参数并清除待试测提示",async()=>{
    const frame=ws.data!.workspace.frames[0];frame.params.dx=2;frame.backup=structuredClone(frame);frame.backup.params.dx=0;
    frame.backup.image!.id="A-original-0";
    const restored=structuredClone(ws.data!);restored.workspace.revision=8;restored.workspace.frames[0]=structuredClone(frame.backup);
    restored.workspace.frames[0].backup=null;restored.workspace.frames[0].trial=null;restored.workspace.frames[0].saved=false;
    vi.mocked(workspaceApi.restoreTeach).mockResolvedValueOnce(restored);
    const real=await vi.importActual<typeof import("../src/features/workspace/context")>("../src/features/workspace/context");
    vi.mocked(useWorkspace).mockImplementation(real.useWorkspace);
    render(<MemoryRouter><real.WorkspaceProvider><TeachingPage/></real.WorkspaceProvider></MemoryRouter>);
    const dx=await screen.findByRole("spinbutton",{name:"平移 X"});fireEvent.change(dx,{target:{value:"8"}});
    expect(screen.getByText("本帧参数有未保存的修改")).toBeVisible();
    const refreshing=deferred<Awaited<ReturnType<typeof recipeApi.list>>>();vi.mocked(recipeApi.list).mockReturnValueOnce(refreshing.promise);
    fireEvent.click(screen.getByRole("button",{name:"恢复原始示教"}));await screen.findByText("修订 8");
    expect(dx).toHaveValue(8);expect(screen.getByText("本帧参数有未保存的修改")).toBeVisible();
    await act(async()=>refreshing.resolve({recipes:[summary(restored)],errors:[]}));
    await waitFor(()=>expect(dx).toHaveValue(0));expect(screen.queryByText("本帧参数有未保存的修改")).toBeNull();
    expect(screen.queryByRole("button",{name:"恢复原始示教"})).toBeNull();expect(workspaceApi.restoreTeach).toHaveBeenCalledWith("A",7,0);
    expect(await screen.findByRole("img",{name:"冻结图像 · A-original-0"})).toBeVisible();
  });
  it.each(["failure","recipe","revision","unmount"])("恢复 %s 时不将旧响应参数写回草稿",async change=>{
    const frame=ws.data!.workspace.frames[0];frame.backup=structuredClone(frame);ws.frameDrafts={0:{...frame.params,dx:8}};
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi.restoreTeach).mockReturnValueOnce(request.promise);
    const setter=ws.setFrameParams,old=structuredClone(ws.data!),page=show();fireEvent.click(screen.getByRole("button",{name:"恢复原始示教"}));
    if(change==="unmount")page.unmount();else if(change!=="failure"){
      ws=workspaceState(workspaceView(change==="recipe"?"B":"A"));if(change==="revision")ws.data!.workspace.revision=9;
      page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);
    }
    await act(async()=>change==="failure"?request.reject(new Error("恢复失败")):request.resolve(old));
    expect(setter).not.toHaveBeenCalled();expect(ws.setFrameParams).not.toHaveBeenCalled();
    if(change==="failure")expect(ws.frameDrafts[0].dx).toBe(8);
  });
  it.each([
    ["平移 X","dx",-4],["平移 Y","dy",-3],["旋转角度","deg",-12.5],
  ] as const)("键盘逐字输入 %s 的负值并保存真实 Provider 草稿",async(label,key,value)=>{
    await showProvider();const user=userEvent.setup(),input=screen.getByRole("spinbutton",{name:label});
    await user.clear(input);expect(input).toHaveValue(null);expect(screen.getByRole("button",{name:"保存参数草稿"})).toBeDisabled();
    await user.type(input,"-");expect(screen.getByRole("button",{name:"试测当前帧"})).toBeDisabled();
    await user.type(input,String(Math.abs(value)));expect(input).toHaveValue(value);
    const params={...ws.data!.workspace.frames[0].params,[key]:value};
    const saved=workspaceView();saved.workspace.revision=8;saved.workspace.frames[0].params=params;saved.workspace.frames[0].trial=null;
    vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(saved);
    await user.click(screen.getByRole("button",{name:"保存参数草稿"}));expect(workspaceApi.saveParams).toHaveBeenCalledWith("A",7,0,params);
    await screen.findByText("修订 8");expect(input).toHaveValue(value);expect(screen.queryByText("本帧参数有未保存的修改")).toBeNull();
  });
  it.each([
    ["搜索余量","searchMm"],["最低灰度对比","minContrast"],["最低定位分数","minScore"],["像素当量","mmPerPx"],
    ["平移 X","dx"],["平移 Y","dy"],["旋转角度","deg"],
  ] as const)("键盘清空 %s 保持空值，禁止试测或保存，填回后恢复",async(label,key)=>{
    await showProvider();const user=userEvent.setup(),input=screen.getByRole("spinbutton",{name:label});await user.clear(input);
    expect(input).toHaveValue(null);for(const name of ["试测当前帧","保存参数草稿","保存本帧示教"])expect(screen.getByRole("button",{name})).toBeDisabled();
    await user.click(screen.getByRole("button",{name:"保存参数草稿"}));expect(workspaceApi.saveParams).not.toHaveBeenCalled();
    await user.type(input,String(ws.data!.workspace.frames[0].params[key]));expect(input).toHaveValue(ws.data!.workspace.frames[0].params[key]);
    expect(screen.getByRole("button",{name:"试测当前帧"})).toBeEnabled();expect(screen.queryByText("本帧参数无效")).toBeNull();
  });
});

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi, useCycle } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera";
import type { ShotTeach, WorkspaceView } from "../src/features/workspace/types";
import { deferred, snapshot, summary, tricamWorkspaceView, twoLines, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn(),recipeApi:{list:vi.fn(),preview:vi.fn()} }));
vi.mock("../src/features/camera",()=>({cameraApi:{rigConfig:vi.fn()}}));
vi.mock("../src/features/plc",()=>({subscribe:vi.fn(()=>()=>{})}));
vi.mock("../src/lib/desktop",()=>({desktopAvailable:()=>true}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  progress:vi.fn(async()=>workspaceView()),saveDraft:vi.fn(async()=>workspaceView()),setViews:vi.fn(async()=>workspaceView()),extractCenterline:vi.fn(),checkCalibration:vi.fn(),get:vi.fn(),list:vi.fn(),image: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(), capture: vi.fn(), selectView: vi.fn(), saveParams: vi.fn(), restoreTeach: vi.fn(),
} }));

const PENDING = "核心库缺少示教胶路/标定算子：glue.taught_path，请选择兼容的核心库";
let ws: ReturnType<typeof workspaceState>;
function show(path = "/recipe/teach") {
  return render(<MemoryRouter initialEntries={[path]}><TeachingPage /></MemoryRouter>);
}
async function showProvider(path = "/recipe/teach"){
  const real=await vi.importActual<typeof import("../src/features/workspace/context")>("../src/features/workspace/context");
  vi.mocked(useWorkspace).mockImplementation(real.useWorkspace);
  const page=render(<MemoryRouter initialEntries={[path]}><real.WorkspaceProvider><TeachingPage/></real.WorkspaceProvider></MemoryRouter>);
  await screen.findByRole("spinbutton",{name:"像素当量"});return page;
}
/** 图像坐标 = 屏幕坐标：jsdom 没有布局，用恒等变换。 */
function identityImage() {
  vi.stubGlobal("DOMPoint",class {constructor(public x:number,public y:number){}matrixTransform(){return this;}});
  vi.stubGlobal("PointerEvent",MouseEvent);
  Object.defineProperty(SVGElement.prototype,"getScreenCTM",{configurable:true,value:()=>({inverse:()=>({})})});
  Object.defineProperty(SVGElement.prototype,"setPointerCapture",{configurable:true,value:vi.fn()});
}
const saved = (view: WorkspaceView, k: number, teach: ShotTeach, revision = 8) => {
  const next = structuredClone(view); next.workspace.revision = revision;
  next.workspace.doc.shots[k] = { ...next.workspace.doc.shots[k], path: teach.path, mmPerPx: teach.mmPerPx ?? undefined };
  if (teach.detect) next.workspace.doc.shots[k].detect = teach.detect; else delete next.workspace.doc.shots[k].detect;
  next.workspace.frames[k].trial = null; next.workspace.frames[k].saved = false;
  return next;
};
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.get).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.list).mockResolvedValue([ws.data!.workspace]);
  vi.mocked(recipeApi.list).mockResolvedValue({recipes:[summary(ws.data!)],errors:[]});vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  for (const method of [workspaceApi.trial, workspaceApi.saveTeach, workspaceApi.capture, workspaceApi.selectView, workspaceApi.saveParams, workspaceApi.restoreTeach]) {
    vi.mocked(method).mockResolvedValue(ws.data!);
  }
});

describe("单帧示教：试测与保存", () => {
  it.each([false,true])("标定记录复用=%s 必须由操作员明确选择", async reuse => {
    vi.mocked(workspaceApi.checkCalibration).mockResolvedValue(ws.data!);
    show();
    const checkbox=screen.getByRole("checkbox",{name:"已确认同设备同图的其他拍照点成像几何适用，复用此次代表点验证"});
    expect(checkbox).not.toBeChecked();
    await userEvent.type(screen.getByRole("spinbutton",{name:"实测尺寸"}),"10");
    if(reuse) await userEvent.click(checkbox);
    await userEvent.click(screen.getByRole("button",{name:"记录人工标定验证"}));
    expect(workspaceApi.checkCalibration).toHaveBeenCalledWith("A",7,0,10,10,reuse);
  });

  it("更换拍照点后代表点复用确认重置",async()=>{
    show();
    const name="已确认同设备同图的其他拍照点成像几何适用，复用此次代表点验证";
    await userEvent.click(screen.getByRole("checkbox",{name}));
    await userEvent.type(screen.getByRole("spinbutton",{name:"实测尺寸"}),"10");
    expect(screen.getByRole("button",{name:"记录人工标定验证"})).toBeEnabled();
    await userEvent.click(screen.getByRole("button",{name:"选择帧 k2"}));
    expect(screen.getByRole("checkbox",{name})).not.toBeChecked();
    expect(screen.getByRole("spinbutton",{name:"实测尺寸"})).toHaveValue(null);
    expect(screen.getByRole("button",{name:"记录人工标定验证"})).toBeDisabled();
  });
  it("重建后的结构化几何字段相同且对象键顺序不同时仍可保存本帧", async () => {
    const frame = ws.data!.workspace.frames[0];
    frame.image!.geometryTag = ["P1", { camera: "cam1", view: 1, pose: { id: "P1", axis: [1, 2] } }];
    frame.trial!.geometryTag = ["P1", { pose: { axis: [1, 2], id: "P1" }, view: 1, camera: "cam1" }];
    show();
    expect(screen.queryByText("试测已过期")).toBeNull();
    const save = screen.getByRole("button", { name: "保存本帧示教" });
    expect(save).toBeEnabled();
    await userEvent.click(save);
    expect(workspaceApi.saveTeach).toHaveBeenCalledWith("A", 7, 0, "A-image-0");
  });

  it.each([
    ["视角改变", ["P1", { camera: "cam1", view: 2, pose: { id: "P1", axis: [1, 2] } }]],
    ["字段缺失", ["P1", { camera: "cam1", view: 1 }]],
    ["数组顺序改变", ["P1", { camera: "cam1", view: 1, pose: { id: "P1", axis: [2, 1] } }]],
  ])("结构化几何%s时阻止保存本帧", async (_name, changed) => {
    const frame = ws.data!.workspace.frames[0];
    frame.image!.geometryTag = ["P1", { camera: "cam1", view: 1, pose: { id: "P1", axis: [1, 2] } }];
    frame.trial!.geometryTag = changed;
    show();
    expect(screen.getByText("试测已过期")).toBeVisible();
    const save = screen.getByRole("button", { name: "保存本帧示教" });
    expect(save).toBeDisabled();
    await userEvent.click(save);
    expect(workspaceApi.saveTeach).not.toHaveBeenCalled();
  });

  it("链接指定帧；试测与保存本帧只带帧号、图像和修订号，不再带参数", async () => {
    show("/recipe/teach?frame=1");
    expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("heading", { name: "P2 · CAM-1 · 视角 1" })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "试测当前帧" }));
    expect(workspaceApi.trial).toHaveBeenCalledWith("A", 7, 1, "A-image-1");
    expect(vi.mocked(workspaceApi.trial).mock.lastCall).toHaveLength(4);
    await userEvent.click(screen.getByRole("button", { name: "选择帧 k1" }));
    await userEvent.click(screen.getByRole("button", { name: "保存本帧示教" }));
    expect(workspaceApi.saveTeach).toHaveBeenCalledWith("A", 7, 0, "A-image-0");
    expect(vi.mocked(workspaceApi.saveTeach).mock.lastCall).toHaveLength(4);
  });

  it("核心库不兼容时如实显示失败原因，不能保存", () => {
    const trial = ws.data!.workspace.frames[0].trial!;
    Object.assign(trial, { passed: false, score: 0, coverage: 0, reason: PENDING, measurement: null });
    show();
    expect(screen.getByText(PENDING)).toBeVisible();
    expect(screen.getAllByText("试测异常")[0]).toBeVisible(); expect(screen.getByText("执行异常")).toBeVisible();
    expect(screen.queryByText("通过")).toBeNull();
    for (const name of ["保存本帧示教", "保存并示教下一幅"]) expect(screen.getByRole("button", { name })).toBeDisabled();
    // 没有逐站结果时不显示 0 分、0% 这类假数值
    const stats = screen.getByText("量成比例").closest(".wp-rule-stat")!;
    expect(within(stats as HTMLElement).getAllByText("—")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeEnabled();
  });

  it("其他原因的试测失败标为试测异常", () => {
    Object.assign(ws.data!.workspace.frames[0].trial!, { passed: false, reason: "中线上 40% 的站没量成" });
    show(); expect(screen.getByText("中线上 40% 的站没量成")).toBeVisible();
    expect(screen.queryByText("图像测量暂不可用")).toBeNull();
    expect(screen.getByText("中线上 40% 的站没量成").closest(".wp-notice")).toHaveTextContent("试测异常");
  });

  it.each(["no-image", "failed-trial", "stale-image", "saved", "dirty", "busy", "skip"])("状态 %s 阻止保存", async condition => {
    const frame = ws.data!.workspace.frames[0];
    if (condition === "no-image") frame.image = null;
    if (condition === "failed-trial") frame.trial!.passed = false;
    if (condition === "stale-image") frame.trial!.imageId = "old-image";
    if (condition === "saved") frame.saved = true;
    if (condition === "dirty") ws.dirty = true;
    if (condition === "busy") ws.busy = true;
    if (condition === "skip") { ws.data!.workspace.doc.shots[0].skip = true; frame.trial = null; }
    show();
    const save = screen.getByRole("button", { name: "保存本帧示教" });
    expect(save).toBeDisabled(); await userEvent.click(save);
    expect(workspaceApi.saveTeach).not.toHaveBeenCalled();
  });

  it("不检的拍照点不用示教：不能点中线、不能试测", async () => {
    identityImage(); ws.data!.workspace.doc.shots[0].skip = true; show();
    expect(screen.getByText("这个拍照点设为不检：只要求这一帧到达，不量不判，不需要示教中线。")).toBeVisible();
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    expect(screen.getByRole("spinbutton", { name: "像素当量" })).toBeDisabled();
    const svg = await screen.findByRole("img", { name: "冻结图像 · A-image-0" });
    fireEvent.pointerDown(svg, { clientX: 50, clientY: 30 }); expect(ws.setFrameDraft).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("中线点 1")).toBeNull();
    expect(within(screen.getByRole("button", { name: "选择帧 k1" })).getByText("不检")).toBeVisible();
  });

  it("中线草稿未保存时不能试测；没有中线时提示先点出中线", () => {
    ws.frameDrafts = { 0: { path: [[10, 10], [30, 10]], mmPerPx: .1 } }; ws.frameDirty = true;
    const page = show();
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    expect(screen.getByText("中线或参数已改，先保存中线再试测；现有试测不用于保存或发布。")).toBeVisible();
    for (const name of ["保存本帧示教", "保存并示教下一幅"]) expect(screen.getByRole("button", { name })).toBeDisabled();
    ws.frameDrafts = {}; ws.data!.workspace.doc.shots[0].path = []; ws.data!.workspace.frames[0].trial = null;
    page.rerender(<MemoryRouter><TeachingPage /></MemoryRouter>);
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    expect(screen.getByText("在原图上从胶嘴一侧往外依次点出胶路中线，填好像素当量后保存中线。")).toBeVisible();
    expect(within(screen.getByRole("button", { name: "选择帧 k1" })).getByText("待点中线")).toBeVisible();
  });

  it.each(["ACQUIRE", "DRAIN", "JUDGE", "REPORT"] as const)("生产 %s 阶段不能取样", phase => {
    vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(phase), logs: [], measured: [] }); show();
    expect(screen.queryByRole("button", { name: "取新样本" })).toBeNull();
    expect(screen.getByText("工件正在检测，当前可编辑已保存图像；重采需等待工件结束。")).toBeVisible();
  });

  it("空闲可以取样，成功保存后前进一帧且不越过末帧", async () => {
    show(); expect(screen.getByRole("link", { name: "整圈重新采集" })).toHaveAttribute("href", "/recipe/capture");
    expect(workspaceApi.capture).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "保存并示教下一幅" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true"));
    await userEvent.click(screen.getByRole("button", { name: "保存并示教下一幅" }));
    expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true");
  });

  it("保存失败停在原帧", async () => {
    vi.mocked(ws.act).mockResolvedValueOnce(null); show();
    await userEvent.click(screen.getByRole("button", { name: "保存并示教下一幅" }));
    expect(screen.getByRole("button", { name: "选择帧 k1" })).toHaveAttribute("aria-pressed", "true");
  });

  it.each([["not-a-frame",1],["-1",1],["0.5",1],["Infinity",1],["99",2]])("路由帧 %s 回到有效帧 k%s",(query,expected)=>{
    show("/recipe/teach?frame="+query);expect(screen.getByRole("button",{name:"选择帧 k"+expected})).toHaveAttribute("aria-pressed","true");
    expect(screen.queryByText("当前配方没有拍照点")).toBeNull();
  });
  it("没有拍照帧时引导回规划并禁用示教操作",()=>{
    ws.data!.workspace.frames=[];show();expect(screen.getByText("当前配方没有拍照点")).toBeVisible();
    expect(screen.getByRole("link",{name:"整圈采集"})).toHaveAttribute("href","/recipe/capture");
    expect(screen.queryByRole("button",{name:"试测当前帧"})).toBeNull();
  });
  it("图像的标记改变后旧试测不能保存",()=>{
    ws.data!.workspace.frames[0].trial!.geometryTag="old-geometry";show();
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"保存并示教下一幅"})).toBeDisabled();
    expect(screen.getByText("试测已过期")).toBeVisible();expect(screen.getByText("现有试测已过期，请重新试测当前帧。")).toBeVisible();
  });
  it("已保存帧直接前进，不重复保存或修改候选修订",async()=>{
    ws.data!.workspace.frames[0].saved=true;show();await userEvent.click(screen.getByRole("button",{name:"保存并示教下一幅"}));
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toHaveAttribute("aria-pressed","true");expect(workspaceApi.saveTeach).not.toHaveBeenCalled();
  });
  it.each(["trial","saveTeach","saveParams"] as const)("%s 等待锁定当前帧、拒绝重复提交，失败后可重试",async method=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi[method]).mockReturnValueOnce(request.promise);
    const labels={trial:"试测当前帧",saveTeach:"保存本帧示教",capture:"取新样本",saveParams:"保存中线",restoreTeach:"恢复原始示教"};
    if(method==="saveParams")ws.frameDrafts={0:{path:twoLines[0],mmPerPx:.2}};
    show();const button=screen.getByRole("button",{name:labels[method]});fireEvent.click(button);fireEvent.click(button);
    expect(workspaceApi[method]).toHaveBeenCalledTimes(1);expect(button).toBeDisabled();
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"像素当量"})).toBeDisabled();
    await act(async()=>request.reject(new Error("本帧命令失败")));expect(ws.setError).toHaveBeenCalledWith("Error: 本帧命令失败");
    expect(button).toBeEnabled();await userEvent.click(button);expect(workspaceApi[method]).toHaveBeenCalledTimes(2);
  });
  it.each(["recipe","revision","unmount"])("保存并下一帧在 %s 改变后丢弃旧完成",async change=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi.saveTeach).mockReturnValueOnce(request.promise);
    const old=ws.data!,page=show();await userEvent.click(screen.getByRole("button",{name:"保存并示教下一幅"}));
    if(change==="unmount")page.unmount();else{
      ws=workspaceState(workspaceView(change==="recipe"?"B":"A"));if(change==="revision")ws.data!.workspace.revision=8;
      page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);
    }
    await act(async()=>request.resolve(old));if(change!=="unmount")expect(screen.getByRole("button",{name:"选择帧 k1"})).toHaveAttribute("aria-pressed","true");
  });
  it("恢复备份携带当前帧修订，不改中线草稿；曝光增益未记录时不显示零值",async()=>{
    const frame=ws.data!.workspace.frames[1];frame.backup=structuredClone(frame);frame.image!.exposureUs=null;frame.image!.gainDb=null;
    show("/recipe/teach?frame=1");expect(screen.getAllByText("未记录")).toHaveLength(2);
    expect(screen.queryByRole("button",{name:"恢复原始示教"})).toBeNull();
    expect(workspaceApi.restoreTeach).not.toHaveBeenCalled();expect(ws.setFrameDraft).not.toHaveBeenCalled();
  });
  it("保存命令接受新修订后仍能前进到下一帧",async()=>{
    const page=show(),next=workspaceView();next.workspace.revision=8;next.workspace.frames[0].saved=true;
    vi.mocked(ws.act).mockImplementationOnce(async request=>{
      await request();ws.data=next;ws.doc=next.workspace.doc;page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);return next;
    });
    await userEvent.click(screen.getByRole("button",{name:"保存并示教下一幅"}));
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toHaveAttribute("aria-pressed","true");
  });
});

describe("单帧示教：同次采集选择检测视角",()=>{
  const selected=(view:WorkspaceView,value:number)=>{
    const next=structuredClone(view),frame=next.workspace.frames[0],shot=next.workspace.doc.shots[0];
    shot.view=value;shot.path=[];delete shot.mmPerPx;
    frame.image=frame.views.find(image=>image.view===value)!;frame.trial=null;frame.saved=false;
    next.workspace.revision++;next.workspace.validation=null;next.layout.shots=structuredClone(next.workspace.doc.shots);
    next.layout.segments=next.layout.segments.filter(segment=>segment.shot!==0);
    return next;
  };
  beforeEach(()=>{
    ws=workspaceState(tricamWorkspaceView());
    vi.mocked(workspaceApi.get).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.list).mockResolvedValue([ws.data!.workspace]);
    vi.mocked(workspaceApi.image).mockImplementation(async(_id,imageId)=>({url:"data:image/png;base64,"+imageId,width:100,height:60}));
  });
  it("同时显示三幅冻结图，查看图 3 后保留全部视角并清除当前中线与试测",async()=>{
    const initial=ws.data!,next=selected(initial,3);vi.mocked(workspaceApi.selectView).mockResolvedValueOnce(next);
    await showProvider();expect(screen.getAllByRole("button",{name:/查看图/})).toHaveLength(3);
    for(const view of [1,2,3])expect(await screen.findByRole("img",{name:`图 ${view}`})).toHaveAttribute("src",`data:image/png;base64,A-image-0${view===1?"":`-v${view}`}`);
    expect(screen.getByRole("button",{name:"查看图 1"})).toHaveAttribute("aria-pressed","true");
    await userEvent.click(screen.getByRole("button",{name:"查看图 3"}));
    expect(workspaceApi.selectView).toHaveBeenCalledExactlyOnceWith("A",7,0,3);
    expect(screen.getByRole("heading",{name:"P1 · CAM-1 · 视角 3"})).toBeVisible();
    expect(screen.getByRole("button",{name:"查看图 3"})).toHaveAttribute("aria-pressed","true");
    expect(screen.getAllByRole("button",{name:/查看图/})).toHaveLength(3);
    const image=await screen.findByRole("img",{name:"冻结图像 · A-image-0-v3"});expect(image.querySelector("polyline")).toBeNull();
    expect(screen.getByRole("spinbutton",{name:"像素当量"})).toHaveValue(null);
    expect(screen.getByRole("button",{name:"试测当前帧"})).toBeDisabled();expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
    expect(within(screen.getByRole("button",{name:"选择帧 k2"})).getByText("试测通过")).toBeVisible();
    expect(workspaceApi.capture).not.toHaveBeenCalled();expect(next.workspace.frames[0].views.map(image=>image.capturedAt)).toEqual([1,1,1]);
  });
  it.each(["dirty","busy","editing","other-draft"])("%s 时不能切换冻结视角",async condition=>{
    if(condition==="dirty")ws.dirty=true;if(condition==="busy")ws.busy=true;
    if(condition==="editing")ws.frameDrafts={0:{path:twoLines[0],mmPerPx:.2}};
    if(condition==="other-draft"){ws.frameDirty=true;ws.frameDrafts={1:{path:twoLines[1],mmPerPx:.2}};}
    show();const button=screen.getByRole("button",{name:"查看图 2"});expect(button).toBeDisabled();
    await userEvent.click(button);expect(workspaceApi.selectView).not.toHaveBeenCalled();
  });
  it("切换等待时锁定帧和中线并阻止重复切换，失败可重试",async()=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi.selectView).mockReturnValueOnce(request.promise);
    show();const button=screen.getByRole("button",{name:"查看图 2"});fireEvent.click(button);fireEvent.click(button);
    expect(workspaceApi.selectView).toHaveBeenCalledTimes(1);expect(button).toBeDisabled();
    expect(screen.getByRole("button",{name:"查看图 3"})).toBeDisabled();expect(screen.getByRole("button",{name:"选择帧 k2"})).toBeDisabled();
    expect(screen.getByRole("spinbutton",{name:"像素当量"})).toBeDisabled();
    await act(async()=>request.reject(new Error("视角图像已失效")));
    expect(ws.setError).toHaveBeenCalledWith("Error: 视角图像已失效");expect(button).toBeEnabled();
    await userEvent.click(button);expect(workspaceApi.selectView).toHaveBeenCalledTimes(2);
  });
  it.each(["resolve","reject"])("视角已改变后旧切换 %s 不覆盖新图或解锁新请求",async outcome=>{
    const old=deferred<WorkspaceView>(),current=deferred<WorkspaceView>(),original=ws.data!,oldError=ws.setError;
    vi.mocked(workspaceApi.selectView).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const page=show();await userEvent.click(screen.getByRole("button",{name:"查看图 2"}));
    ws=workspaceState(selected(original,3));page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);
    await userEvent.click(screen.getByRole("button",{name:"查看图 2"}));
    await act(async()=>{if(outcome==="resolve")old.resolve(selected(original,2));else old.reject(new Error("旧视角错误"));});
    expect(screen.getByRole("button",{name:"查看图 3"})).toHaveAttribute("aria-pressed","true");
    expect(screen.getByRole("button",{name:"查看图 2"})).toBeDisabled();expect(oldError).not.toHaveBeenCalled();expect(ws.setError).not.toHaveBeenCalled();
    await act(async()=>current.resolve(ws.data!));expect(screen.getByRole("button",{name:"查看图 2"})).toBeEnabled();
  });
  it("选中视角 2 后，视角 1 的迟到原图不覆盖主图",async()=>{
    const old=deferred<{url:string;width:number;height:number}>(),initial=ws.data!;
    vi.mocked(workspaceApi.image).mockImplementation((_id,imageId)=>imageId==="A-image-0"?old.promise:Promise.resolve({url:"data:image/png;base64,"+imageId,width:100,height:60}));
    vi.mocked(workspaceApi.selectView).mockResolvedValueOnce(selected(initial,2));await showProvider();
    await userEvent.click(screen.getByRole("button",{name:"查看图 2"}));const image=await screen.findByRole("img",{name:"冻结图像 · A-image-0-v2"});
    expect(image.querySelector("image")).toHaveAttribute("href","data:image/png;base64,A-image-0-v2");
    await act(async()=>old.resolve({url:"data:image/png;base64,old-view-1",width:100,height:60}));
    expect(image.querySelector("image")).toHaveAttribute("href","data:image/png;base64,A-image-0-v2");
  });
});

describe("单帧示教：在冻结原图上点出中线", () => {
  it("原图上叠加这个拍照点已保存的中线与各站（图像像素），可缩放、可关闭叠加，换帧显示对应原图",async()=>{
    vi.mocked(workspaceApi.image).mockImplementation(async(_id,imageId)=>({url:"data:image/png;base64,"+imageId,width:100,height:60}));
    show();const first=await screen.findByRole("img",{name:"冻结图像 · A-image-0"});
    expect(first.querySelector("polyline")).toHaveAttribute("points","10,10 20,10");
    expect(Array.from(first.querySelectorAll("circle:not([data-vertex])")).map(c=>[c.getAttribute("cx"),c.getAttribute("cy")])).toEqual([["10","10"],["20","10"]]);
    expect(within(first).getByLabelText("中线点 1")).toHaveAttribute("stroke","var(--ok)");
    expect(screen.getByText("中线 2 点 · 1.0 mm · 2 站")).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"放大原图"}));expect(first.querySelector("g")).toHaveAttribute("transform",expect.stringContaining("scale(1.25)"));
    await userEvent.click(screen.getByRole("button",{name:"中线叠加"}));expect(first.querySelectorAll("polyline")).toHaveLength(0);
    await userEvent.click(screen.getByRole("button",{name:"中线叠加"}));await userEvent.click(screen.getByRole("button",{name:"适应窗口"}));
    expect(first.querySelector("g")).toHaveAttribute("transform",expect.stringContaining("scale(1)"));await userEvent.click(screen.getByRole("button",{name:"选择帧 k2"}));
    const next=await screen.findByRole("img",{name:"冻结图像 · A-image-1"});await waitFor(()=>expect(next.querySelector("image")).toHaveAttribute("href","data:image/png;base64,A-image-1"));
    expect(next.querySelector("polyline")).toHaveAttribute("points","30,10 40,10");
  });

  it("点图加点、拖点调整、删除末点 / 选中点、清空，草稿只记在当前帧", async () => {
    identityImage(); await showProvider("/recipe/teach?frame=1");
    const svg = await screen.findByRole("img", { name: "冻结图像 · A-image-1" });
    fireEvent.pointerDown(svg, { clientX: 55, clientY: 20 }); fireEvent.pointerUp(svg);
    expect(screen.getByLabelText("中线点 3")).toHaveAttribute("cx", "55");
    expect(screen.getAllByText("中线待保存")[0]).toBeVisible(); expect(screen.getByText("示教中线有未保存的修改")).toBeVisible();
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    // 拖动已有的点
    fireEvent.pointerDown(screen.getByLabelText("中线点 1"), { clientX: 30, clientY: 10 });
    fireEvent.pointerMove(svg, { clientX: 32.26, clientY: 14 }); fireEvent.pointerUp(svg);
    expect(screen.getByLabelText("中线点 1")).toHaveAttribute("cx", "32.3"); expect(screen.getByLabelText("中线点 1")).toHaveAttribute("cy", "14");
    expect(screen.getByLabelText("中线点 1")).toHaveAttribute("fill", "var(--accent)");
    await userEvent.click(screen.getByRole("button", { name: "删除选中点" }));
    expect(screen.queryByLabelText("中线点 3")).toBeNull(); expect(screen.getByLabelText("中线点 1")).toHaveAttribute("cx", "40");
    await userEvent.click(screen.getByRole("button", { name: "删除末点" }));
    expect(screen.getByLabelText("中线点 1")).toBeInTheDocument(); expect(screen.queryByLabelText("中线点 2")).toBeNull();
    expect(screen.getByText("中线至少两个点、长度不能为零")).toBeVisible(); expect(screen.getByRole("button", { name: "保存中线" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "清空中线" }));
    expect(screen.queryByLabelText("中线点 1")).toBeNull(); expect(screen.getByRole("button", { name: "删除末点" })).toBeDisabled();
    // 另一帧没有草稿
    await userEvent.click(screen.getByRole("button", { name: "选择帧 k1" }));
    expect(within(screen.getByRole("button", { name: "选择帧 k1" })).getByText("试测通过")).toBeVisible();
    expect(within(screen.getByRole("button", { name: "选择帧 k2" })).getByText("草稿待保存")).toBeVisible();
    // 清空后保存：中线为空即这个拍照点回到未示教
    await userEvent.click(screen.getByRole("button", { name: "选择帧 k2" }));
    const empty = saved(ws.data!, 1, { path: [], mmPerPx: .1 }); vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(empty);
    await userEvent.click(screen.getByRole("button", { name: "保存中线" }));
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 7, 1, { path: [], mmPerPx: .1 });
  });

  it("保存中线调用 workspace_save_params：中线、像素当量；不本图自定义参数（取消可恢复配方默认）时不发送 detect；保存后草稿清掉、需重新试测", async () => {
    identityImage(); await showProvider();
    const svg = await screen.findByRole("img", { name: "冻结图像 · A-image-0" });
    fireEvent.pointerDown(svg, { clientX: 30.04, clientY: 18 }); fireEvent.pointerUp(svg);
    fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: "0.112" } });
    const teach: ShotTeach = { path: [[10, 10], [20, 10], [30, 18]], mmPerPx: .112 };
    const next = saved(ws.data!, 0, teach); vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(next);
    await userEvent.click(screen.getByRole("button", { name: "保存中线" }));
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 7, 0, teach);
    expect(Object.keys(vi.mocked(workspaceApi.saveParams).mock.lastCall![3])).toEqual(["path", "mmPerPx"]);
    await screen.findByText("修订 8");
    expect(screen.queryByText("示教中线有未保存的修改")).toBeNull(); expect(screen.getByText("中线已保存到候选配方，请重新试测")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存中线" })).toBeDisabled();
    expect(screen.getByText("中线已保存，可以试测当前帧。")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "试测当前帧" }));
    expect(workspaceApi.trial).toHaveBeenCalledWith("A", 8, 0, "A-image-0");
  });

  it("本图自定义参数（取消可恢复配方默认）：从配方默认值起编辑并随中线保存；取消勾选回到默认、不再算修改", async () => {
    await showProvider();
    expect(screen.getByText("用配方的检测参数：搜索半宽 4 mm · 暗胶条 · 胶宽 1–6 mm")).toBeVisible();
    await userEvent.click(screen.getByRole("checkbox", { name: "本图自定义参数（取消可恢复配方默认）" }));
    expect(screen.getByRole("spinbutton", { name: "搜索半宽" })).toHaveValue(4);
    fireEvent.change(screen.getByRole("spinbutton", { name: "搜索半宽" }), { target: { value: "6" } });
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "极性" }), "light");
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽下限" }), { target: { value: "2" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽上限" }), { target: { value: "12" } });
    expect(screen.getByText("胶宽上限要小于搜索宽度 12.0 mm")).toBeVisible(); expect(screen.getByRole("button", { name: "保存中线" })).toBeDisabled();
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽上限" }), { target: { value: "8" } });
    const teach: ShotTeach = { path: twoLines[0], mmPerPx: .1, detect: { searchMm: 6, polarity: "light", widthRange: [2, 8] } };
    vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(saved(ws.data!, 0, teach));
    await userEvent.click(screen.getByRole("button", { name: "保存中线" }));
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 7, 0, teach);
    await screen.findByText("修订 8"); expect(screen.getByRole("checkbox", { name: "本图自定义参数（取消可恢复配方默认）" })).toBeChecked();
    await userEvent.click(screen.getByRole("checkbox", { name: "本图自定义参数（取消可恢复配方默认）" }));
    expect(screen.getByRole("button", { name: "保存中线" })).toBeEnabled();
    await userEvent.click(screen.getByRole("checkbox", { name: "本图自定义参数（取消可恢复配方默认）" }));
    fireEvent.change(screen.getByRole("spinbutton", { name: "搜索半宽" }), { target: { value: "6" } });
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "极性" }), "light");
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽下限" }), { target: { value: "2" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽上限" }), { target: { value: "8" } });
    expect(screen.getByRole("button", { name: "保存中线" })).toBeDisabled(); expect(screen.queryByText("示教中线有未保存的修改")).toBeNull();
  });

  it("像素当量从候选配方预填；还没有时为空，点了中线也不能保存，取样后补上的值并入草稿", async () => {
    identityImage(); const view = ws.data!; delete view.workspace.doc.shots[0].mmPerPx; view.workspace.doc.shots[0].path = [];
    await showProvider();
    expect(screen.getByRole("spinbutton", { name: "像素当量" })).toHaveValue(null);
    const svg = await screen.findByRole("img", { name: "冻结图像 · A-image-0" });
    fireEvent.pointerDown(svg, { clientX: 10, clientY: 10 }); fireEvent.pointerUp(svg);
    fireEvent.pointerDown(svg, { clientX: 60, clientY: 10 }); fireEvent.pointerUp(svg);
    expect(screen.getByText("像素当量需在 0–10 mm/px 之间")).toBeVisible(); expect(screen.getByRole("button", { name: "保存中线" })).toBeDisabled();
    expect(screen.queryByRole("button",{name:"取新样本"})).toBeNull();
    expect(screen.getByRole("link",{name:"整圈重新采集"})).toHaveAttribute("href","/recipe/capture");
  });

  it.each([["0", "像素当量需在 0–10 mm/px 之间"], ["", "像素当量需在 0–10 mm/px 之间"], ["11", "像素当量需在 0–10 mm/px 之间"]])("像素当量 %j 无效时不能保存中线", async (value, message) => {
    await showProvider(); const input = screen.getByRole("spinbutton", { name: "像素当量" });
    fireEvent.change(input, { target: { value } });
    expect(screen.getByText(message)).toBeVisible(); expect(screen.getByRole("button", { name: "保存中线" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "保存中线" })); expect(workspaceApi.saveParams).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "0.1" } }); expect(screen.queryByText(message)).toBeNull();
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeEnabled();
  });

  it("中线保存失败保留草稿，可以重试", async () => {
    identityImage(); vi.mocked(workspaceApi.saveParams).mockRejectedValueOnce(new Error("拍照点 P1 的中线只有 0.5 mm"));
    await showProvider(); const svg = await screen.findByRole("img", { name: "冻结图像 · A-image-0" });
    fireEvent.pointerDown(svg, { clientX: 25, clientY: 10 }); fireEvent.pointerUp(svg);
    await userEvent.click(screen.getByRole("button", { name: "保存中线" }));
    expect(await screen.findByText("Error: 拍照点 P1 的中线只有 0.5 mm")).toBeVisible();
    expect(screen.getByLabelText("中线点 3")).toBeInTheDocument(); expect(screen.getByRole("button", { name: "保存中线" })).toBeEnabled();
  });
});

describe("多图配方示教", () => {
  it("图 1 和图 3 可同时选用，切换编辑图不会取消勾选", async () => {
    ws=workspaceState(tricamWorkspaceView());
    const shot=ws.doc!.shots[0]; shot.views=[1,2,3].map(view=>({view,enabled:view===1,path:[],mmPerPx:.1}));
    show(); await userEvent.click(screen.getByRole("checkbox",{name:"检测图 3"}));
    expect(workspaceApi.setViews).toHaveBeenCalledWith("A",7,0,[1,3],false);
    await userEvent.click(screen.getByRole("button",{name:"查看图 2"}));
    expect(workspaceApi.selectView).toHaveBeenCalledWith("A",7,0,2);
    expect(workspaceApi.setViews).toHaveBeenCalledTimes(1);
  });
  it("有效 NG 允许完成示教，执行异常仍禁止完成", () => {
    ws.data!.workspace.frames[0].trial!.verdict="NG_GAP";
    const page=show();
    expect(screen.getByText("有效 · NG_GAP")).toBeVisible();
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeEnabled();
    ws.data!.workspace.frames[0].trial!.passed=false;
    page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
  });
  it("框选区域调用真实提取接口，返回路径只进入草稿", async () => {
    identityImage(); vi.mocked(workspaceApi.extractCenterline).mockResolvedValue({path:[[10,20],[40,20]],message:"已提取"});
    show(); const image=await screen.findByRole("img",{name:"冻结图像 · A-image-0"});
    await userEvent.click(screen.getByRole("button",{name:"框选胶路区域"}));
    fireEvent.pointerDown(image,{clientX:5,clientY:10}); fireEvent.pointerMove(image,{clientX:45,clientY:30}); fireEvent.pointerUp(image);
    await userEvent.click(screen.getByRole("button",{name:"自动提取中线"}));
    expect(workspaceApi.extractCenterline).toHaveBeenCalledWith("A",7,0,"A-image-0",[5,10,40,20]);
    expect(ws.setFrameDraft).toHaveBeenCalledWith(0,expect.objectContaining({path:[[10,20],[40,20]]}));
    expect(workspaceApi.saveTeach).not.toHaveBeenCalled();
  });
  it("恢复上次拍照点并记录当前编辑位置", () => {
    ws.data!.workspace.lastPosition={k:1,view:1};show();
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toHaveAttribute("aria-pressed","true");
    expect(ws.rememberPosition).toHaveBeenCalledWith(1,1);
  });
});

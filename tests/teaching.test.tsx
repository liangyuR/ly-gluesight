import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi, useCycle } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera";
import { snapshot, summary, tricamWorkspaceView, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn(),recipeApi:{list:vi.fn(),preview:vi.fn()} }));
vi.mock("../src/features/camera",()=>({cameraApi:{rigConfig:vi.fn()}}));
vi.mock("../src/features/plc",()=>({subscribe:vi.fn(()=>()=>{})}));
vi.mock("../src/lib/desktop",()=>({desktopAvailable:()=>true}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  progress:vi.fn(async()=>workspaceView()),saveDraft:vi.fn(async()=>workspaceView()),setViews:vi.fn(async()=>workspaceView()),extractCenterline:vi.fn(),checkCalibration:vi.fn(),get:vi.fn(),list:vi.fn(),image: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(), capture: vi.fn(), selectView: vi.fn(), saveParams: vi.fn(), restoreTeach: vi.fn(),
} }));

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

  it("图像的标记改变后旧试测不能保存",()=>{
    ws.data!.workspace.frames[0].trial!.geometryTag="old-geometry";show();
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"保存并示教下一幅"})).toBeDisabled();
    expect(screen.getByText("试测已过期")).toBeVisible();expect(screen.getByText("现有试测已过期，请重新试测当前帧。")).toBeVisible();
  });

});

describe("单帧示教：在冻结原图上点出中线", () => {
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

});

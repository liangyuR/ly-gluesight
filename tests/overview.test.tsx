import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import OverviewPage, { defaultPositions, WorkpieceOverview } from "../src/features/workspace/OverviewPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { deferred, shotList, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveOverview: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
let decoded: HTMLImageElement[],autoDecode: boolean;
beforeEach(() => {
  decoded=[];autoDecode=true;
  vi.stubGlobal("Image",class {
    naturalWidth=800;naturalHeight=400;onload:(()=>void)|null=null;onerror:(()=>void)|null=null;
    set src(_value:string){decoded.push(this as unknown as HTMLImageElement);if(autoDecode)queueMicrotask(()=>this.onload?.());}
  });
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => null });
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.saveOverview).mockResolvedValue(ws.data!);
});
const show = () => render(<MemoryRouter><OverviewPage/></MemoryRouter>);
const save = async () => userEvent.click(screen.getByRole("button", { name: "保存总览" }));
describe("工件总览操作", () => {
  it("选中帧显示对应原图（叠加示教中线）与相机、胶条、示教状态，键盘选帧可用", async () => {
    ws.data!.layout.shots[1] = { ...ws.data!.layout.shots[1], camera: "CAM-2", view: 1, bead: "J2", poseId: "A7" };
    show(); await userEvent.click(screen.getByRole("button", { name: "总览选择帧 k2" }));
    expect(screen.getByRole("button", { name: "总览选择帧 k2" })).toHaveAttribute("aria-pressed", "true");
    expect(within(screen.getByRole("button", { name: "总览选择帧 k2" })).getByText("P2 · CAM-2 · 视角 1")).toBeInTheDocument();
    expect(screen.getByText("P2 · CAM-2 · 视角 1", { selector: ".wp-kv strong" })).toBeVisible();
    for (const text of ["J2", "A7", "已示教 2 点 · 1.0 mm", "100%"]) expect(screen.getByText(text, { selector: ".wp-kv strong" })).toBeVisible();
    expect(screen.queryByText(/物理中心|视野/)).toBeNull();
    expect(workspaceApi.image).toHaveBeenCalledWith("A", "A-image-1");
    const image = await screen.findByRole("img", { name: "A-image-1" });
    expect(image.querySelector("polyline")).toHaveAttribute("points", "30,10 40,10");
    fireEvent.keyDown(screen.getByRole("button", { name: "总览选择帧 k1" }), { key: "Enter" });
    expect(screen.getByRole("button", { name: "总览选择帧 k1" })).toHaveAttribute("aria-pressed", "true");
  });
  it("方向键及 Shift 微调显示框，边界截断，保存不改物理拍照点", async () => {
    ws.data!.workspace.overview.positions[0] = [.99, .01]; const physical = structuredClone(ws.doc!.shots); show();
    const target = screen.getByRole("button", { name: "总览选择帧 k1" });
    fireEvent.keyDown(target, { key: "ArrowRight", shiftKey: true }); fireEvent.keyDown(target, { key: "ArrowUp", shiftKey: true });
    expect(screen.getByText("总览待保存")).toBeVisible(); await save();
    expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A", 7, expect.objectContaining({ positions: [[1, 0], [.75, .5]], saved: false }));
    expect(ws.doc!.shots).toEqual(physical);
  });
  it("自动布置恢复物理规划对应位置，丢失位置时生成默认布置", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "自动布置" })); await save();
    expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A", 7, expect.objectContaining({ positions: defaultPositions(ws.data!.layout) }));
  });
  it.each(["oversized", "format","empty"])("%s 背景被拒绝，原布置不改", async kind => {
    show(); const file = new File([kind==="empty"?"":"png"], kind === "format" ? "bg.svg" : "bg.png", { type: kind === "format" ? "image/svg+xml" : "image/png" });
    if (kind === "oversized") Object.defineProperty(file, "size", { value: 1_000_001 });
    fireEvent.change(screen.getByLabelText("导入总览图"), { target: { files: [file] } });
    expect(ws.setError).toHaveBeenCalledWith(expect.stringContaining("1 MB")); await save();
    expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A", 7, expect.objectContaining({ background: null }));
  });
  it("有效背景导入、保存与移除；读取中禁止提前保存", async () => {
    show(); fireEvent.change(screen.getByLabelText("导入总览图"), { target: { files: [new File(["png"], "bg.png", { type: "image/png" })] } });
    expect(screen.getByRole("button", { name: "保存总览" })).toBeDisabled();
    await screen.findByRole("button", { name: "移除背景" }); await save();
    expect(workspaceApi.saveOverview).toHaveBeenLastCalledWith("A", 7, expect.objectContaining({ background: expect.stringMatching(/^data:image\/png;base64,/), saved: false }));
    await userEvent.click(screen.getByRole("button", { name: "移除背景" })); await save();
    expect(workspaceApi.saveOverview).toHaveBeenLastCalledWith("A", 7, expect.objectContaining({ background: null }));
  });
  it("旧配方背景读取完成后不覆盖新配方，读取失败显示原因", async () => {
    let reader: FileReader | undefined;
    vi.spyOn(FileReader.prototype, "readAsDataURL").mockImplementation(function (this: FileReader) { reader = this; });
    const page = show(); fireEvent.change(screen.getByLabelText("导入总览图"), { target: { files: [new File(["png"], "bg.png", { type: "image/png" })] } });
    ws = workspaceState(workspaceView("B")); page.rerender(<MemoryRouter><OverviewPage/></MemoryRouter>);
    Object.defineProperty(reader!, "result", { value: "data:image/png;base64,old" }); act(() => reader!.dispatchEvent(new Event("load")));
    expect(screen.queryByRole("button", { name: "移除背景" })).toBeNull();
    fireEvent.change(screen.getByLabelText("导入总览图"), { target: { files: [new File(["png"], "bg.png", { type: "image/png" })] } });
    act(() => reader!.dispatchEvent(new Event("error"))); await waitFor(() => expect(ws.setError).toHaveBeenCalledWith("总览图读取失败"));
  });
  it.each(["busy", "dirty"])("%s 状态禁止保存错误总览", condition => {
    if (condition === "busy") ws.busy = true; if (condition === "dirty") ws.dirty = true;
    show(); expect(screen.getByRole("button", { name: "保存总览" })).toBeDisabled();
    expect(screen.getByLabelText("导入总览图")).toBeDisabled();expect(screen.getByRole("button",{name:"自动布置"})).toBeDisabled();
  });
  it("背景解码完成前保持等待，方向键不会修改读取前布置",async()=>{
    autoDecode=false;show();fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[new File(["png"],"bg.png",{type:"image/png"})]}});
    await waitFor(()=>expect(decoded).toHaveLength(1));fireEvent.keyDown(screen.getByRole("button",{name:"总览选择帧 k1"}),{key:"ArrowRight"});
    expect(screen.getByRole("button",{name:"保存总览"})).toBeDisabled();expect(screen.getByRole("button",{name:"自动布置"})).toBeDisabled();
    act(()=>decoded[0].onload?.(new Event("load")));await save();
    expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A",7,expect.objectContaining({positions:[[.25,.5],[.75,.5]],background:expect.any(String)}));
  });
  it.each(["error","zero-size"])("背景解码 %s 不替换已有背景并允许重新导入",async failure=>{
    autoDecode=false;ws.data!.workspace.overview.background="data:image/png;base64,original";show();
    fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[new File(["broken"],"bad.png",{type:"image/png"})]}});await waitFor(()=>expect(decoded).toHaveLength(1));
    act(()=>{if(failure==="zero-size"){Object.defineProperty(decoded[0],"naturalWidth",{value:0});decoded[0].onload?.(new Event("load"));}else decoded[0].onerror?.(new Event("error"));});
    expect(ws.setError).toHaveBeenCalledWith("总览背景不是可读取的图像");expect(screen.getByLabelText("导入总览图")).toBeEnabled();await save();
    expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A",7,expect.objectContaining({background:"data:image/png;base64,original"}));
  });
  it.each(["abort","throw","invalid-result"])("背景读取 %s 解除等待，保留重试入口",async failure=>{
    let reader:FileReader|undefined;vi.spyOn(FileReader.prototype,"readAsDataURL").mockImplementation(function(this:FileReader){reader=this;if(failure==="throw")throw new Error("读取失败");});
    show();const file=new File(["png"],"bg.png",{type:"image/png"});fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[file]}});
    if(failure!=="throw")act(()=>{if(failure==="invalid-result"){Object.defineProperty(reader!,"result",{value:null});reader!.dispatchEvent(new Event("load"));}else reader!.dispatchEvent(new Event("abort"));});
    expect(ws.setError).toHaveBeenCalledWith(failure==="abort"?"总览图读取已取消":failure==="throw"?"总览图读取失败":"总览背景不是可读取的图像");
    expect(screen.getByRole("button",{name:"保存总览"})).toBeEnabled();expect(screen.getByLabelText("导入总览图")).toBeEnabled();expect(decoded).toHaveLength(0);
  });
  it.each(["recipe","revision","unmount"])("背景解码完成在 %s 切换后不覆盖当前总览",async change=>{
    autoDecode=false;const page=show();fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[new File(["png"],"bg.png",{type:"image/png"})]}});
    await waitFor(()=>expect(decoded).toHaveLength(1));
    if(change==="unmount")page.unmount();else{
      ws=workspaceState(workspaceView(change==="recipe"?"B":"A"));if(change==="revision")ws.data!.workspace.revision=8;
      page.rerender(<MemoryRouter><OverviewPage/></MemoryRouter>);
    }
    act(()=>decoded[0].onload?.(new Event("load")));expect(screen.queryByRole("button",{name:"移除背景"})).toBeNull();expect(ws.setError).not.toHaveBeenCalled();
  });
  it("背景文件可以反复选择同一文件，失败旧读取不解除新读取",async()=>{
    const readers:FileReader[]=[];vi.spyOn(FileReader.prototype,"readAsDataURL").mockImplementation(function(this:FileReader){readers.push(this);});
    const page=show(),file=new File(["png"],"bg.png",{type:"image/png"});fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[file]}});
    ws=workspaceState(workspaceView("B"));page.rerender(<MemoryRouter><OverviewPage/></MemoryRouter>);fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[file]}});
    act(()=>readers[0].dispatchEvent(new Event("error")));expect(ws.setError).not.toHaveBeenCalled();expect(screen.getByLabelText("导入总览图")).toBeDisabled();
    Object.defineProperty(readers[1],"result",{value:"data:image/png;base64,new"});await act(async()=>readers[1].dispatchEvent(new Event("load")));
    expect(screen.getByLabelText("导入总览图")).toBeEnabled();expect(screen.getByLabelText("导入总览图")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("导入总览图"),{target:{files:[file]}});expect(readers).toHaveLength(3);
  });
  it("保存等待锁定编辑、重复提交被拒绝，失败后可重新保存",async()=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi.saveOverview).mockReturnValueOnce(request.promise);show();
    const button=screen.getByRole("button",{name:"保存总览"});fireEvent.click(button);fireEvent.click(button);expect(workspaceApi.saveOverview).toHaveBeenCalledTimes(1);
    expect(button).toBeDisabled();expect(screen.getByRole("button",{name:"自动布置"})).toBeDisabled();expect(screen.getByLabelText("导入总览图")).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("button",{name:"总览选择帧 k1"}),{key:"ArrowRight"});await act(async()=>request.reject(new Error("总览保存失败")));
    expect(ws.setError).toHaveBeenCalledWith("Error: 总览保存失败");expect(button).toBeEnabled();await userEvent.click(button);
    expect(workspaceApi.saveOverview).toHaveBeenLastCalledWith("A",7,expect.objectContaining({positions:[[.25,.5],[.75,.5]]}));
  });
  it("旧保存失败不影响新配方保存的等待和提示",async()=>{
    const old=deferred<WorkspaceView>(),next=deferred<WorkspaceView>();vi.mocked(workspaceApi.saveOverview).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const page=show();fireEvent.click(screen.getByRole("button",{name:"保存总览"}));ws=workspaceState(workspaceView("B"));page.rerender(<MemoryRouter><OverviewPage/></MemoryRouter>);
    fireEvent.click(screen.getByRole("button",{name:"保存总览"}));await act(async()=>old.reject(new Error("旧保存错误")));
    expect(ws.setError).not.toHaveBeenCalled();expect(screen.getByRole("button",{name:"保存总览"})).toBeDisabled();await act(async()=>next.resolve(ws.data!));
    expect(screen.getByRole("button",{name:"保存总览"})).toBeEnabled();expect(workspaceApi.saveOverview).toHaveBeenCalledTimes(2);
  });
  it("布局缺少保存位置时标记待保存，保存成功刷新后显示已保存",async()=>{
    ws.data!.workspace.overview.positions=[];const page=show();expect(screen.getByText("总览待保存")).toBeVisible();await save();
    const overview=vi.mocked(workspaceApi.saveOverview).mock.calls[0][2];expect(overview.positions).toEqual(defaultPositions(ws.data!.layout));
    ws.data!.workspace.overview={...overview,saved:true};ws.data!.workspace.revision++;
    page.rerender(<MemoryRouter><OverviewPage/></MemoryRouter>);expect(screen.getByText("总览已保存")).toBeVisible();
  });
  it("总览拖动按画布坐标归一化并截断边界，释放或取消后停止移动",async()=>{
    vi.stubGlobal("DOMPoint",class {constructor(public x:number,public y:number){}matrixTransform(){return this;}});
    Object.defineProperty(SVGElement.prototype,"getScreenCTM",{configurable:true,value:()=>({inverse:()=>({})})});
    Object.defineProperty(SVGElement.prototype,"setPointerCapture",{configurable:true,value:vi.fn()});
    vi.stubGlobal("PointerEvent",MouseEvent);show();const target=screen.getByRole("button",{name:"总览选择帧 k1"});
    fireEvent.pointerDown(target,{clientX:270,clientY:189,pointerId:1});fireEvent.pointerMove(target,{clientX:530,clientY:304,pointerId:1});
    fireEvent.pointerUp(target,{pointerId:1});fireEvent.pointerMove(target,{clientX:0,clientY:0,pointerId:1});await save();
    expect(workspaceApi.saveOverview).toHaveBeenLastCalledWith("A",7,expect.objectContaining({positions:[[.75,1],[.75,.5]]}));
    fireEvent.pointerDown(target,{clientX:530,clientY:304,pointerId:1});fireEvent.pointerMove(target,{clientX:2000,clientY:-1000,pointerId:1});fireEvent.pointerCancel(target,{pointerId:1});
    fireEvent.pointerMove(target,{clientX:0,clientY:1000,pointerId:1});await save();expect(workspaceApi.saveOverview).toHaveBeenLastCalledWith("A",7,expect.objectContaining({positions:[[1,0],[.75,.5]]}));
  });
  it("没有可用坐标矩阵时点击只选帧，空格和所有方向键仍正确工作",async()=>{
    show();const target=screen.getByRole("button",{name:"总览选择帧 k2"});fireEvent.pointerDown(target,{clientX:0,clientY:0});fireEvent.pointerMove(target,{clientX:500,clientY:500});
    fireEvent.keyDown(target,{key:" "});expect(target).toHaveAttribute("aria-pressed","true");
    fireEvent.keyDown(target,{key:"ArrowLeft"});fireEvent.keyDown(target,{key:"ArrowDown"});await save();
    expect(workspaceApi.saveOverview).toHaveBeenCalledWith("A",7,expect.objectContaining({positions:[[.25,.5],[.74,.51]]}));
  });
  it("默认显示位置按计划顺序排成均匀网格，不依赖工件坐标", () => {
    const layout = workspaceView().layout;
    expect(defaultPositions(layout)).toEqual([[.25, .5], [.75, .5]]);
    expect(defaultPositions({ shots: shotList([[]]) })).toEqual([[.5, .5]]);
    const five = defaultPositions({ shots: shotList([[], [], [], [], []]) });
    expect(five).toEqual([[.125, .25], [.375, .25], [.625, .25], [.875, .25], [.125, .75]]);
    expect(defaultPositions({ shots: [] })).toEqual([]);
    const many = defaultPositions({ shots: shotList(Array.from({ length: 64 }, () => [])) });
    expect(many.every(([x, y]) => x > 0 && x < 1 && y > 0 && y < 1)).toBe(true); expect(new Set(many.map(p => p.join())).size).toBe(64);
  });
  it("有测量状态时各帧框按拍照点结论着色，不检与未示教虚线", () => {
    const layout = workspaceView().layout;
    layout.shots.push({ ...layout.shots[0], id: "P3", poseId: "P3", skip: true }, { id: "P4", poseId: "P4", camera: "CAM-1", view: 1, bead: "J1", skip: false, path: [] });
    render(<svg><WorkpieceOverview layout={layout} overview={null} vis={["ok", "ok", "gap", "ok"]} /></svg>);
    const frame = (k: number) => screen.getByRole("button", { name: `总览选择帧 k${k}` });
    expect(frame(2)).toHaveAttribute("data-state", "gap"); expect(frame(2).querySelector("rect")).toHaveAttribute("stroke", "var(--ng)");
    expect(within(frame(2)).getByText("断胶")).toBeInTheDocument();
    expect(frame(3).querySelector("rect")).toHaveAttribute("stroke-dasharray", "5 4"); expect(within(frame(3)).getByText("不检")).toBeInTheDocument();
    expect(frame(4).querySelector("rect")).toHaveAttribute("stroke-dasharray", "5 4"); expect(within(frame(4)).getByText("未示教")).toBeInTheDocument();
    expect(frame(1).querySelector("rect")).toHaveAttribute("stroke", "var(--accent)");
    expect(screen.queryByText(/NaN/)).toBeNull();
  });
});

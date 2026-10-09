import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import FollowCalibPanel from "../src/features/camera/components/FollowCalibPanel";
import { cameraApi, defaultCameraConfig } from "../src/features/camera/api";
import type { CameraConfig } from "../src/features/camera/types";
import type { StationView } from "../src/features/workspace/StationCapture";
import { deferred, workspaceView } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("../src/features/camera/api", async importOriginal => {
  const actual = await importOriginal<typeof import("../src/features/camera/api")>();
  return { ...actual, cameraApi: { ...actual.cameraApi, saveConfig: vi.fn() } };
});
let config: CameraConfig;
const sample: StationView = { metadata: { ...workspaceView().workspace.frames[0].image!, id: "station-1", size: [1000, 800] },
  image: { url: "data:image/png;base64,AA==", width: 1000, height: 800 } };
const onSaved = vi.fn();
const points = [1, 2].map(l => ({ l, offset: .1, width: 2, st: 0, px: [20, 30] as [number, number] }));
const show = (frozen: StationView | null = sample) => render(<FollowCalibPanel cam={1} config={config} frame={undefined} sample={frozen} onSaved={onSaved} />);
const change = (label: string, value: number) => fireEvent.change(screen.getByRole("spinbutton", { name: label }), { target: { value: String(value) } });
function expectFiniteDrawing(container: HTMLElement) {
  for (const element of container.querySelectorAll(".calib-view svg circle,.calib-view svg line")) {
    for (const name of ["cx", "cy", "r", "x1", "y1", "x2", "y2"]) {
      const value = element.getAttribute(name);
      if (value !== null) expect(Number.isFinite(Number(value)), `${name}=${value}`).toBe(true);
    }
  }
}
function coordinates(container: HTMLElement) {
  const svg = container.querySelector(".calib-view svg")!;
  Object.defineProperty(svg, "getScreenCTM", { value: () => ({ inverse: () => ({}) }) });
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  return svg;
}
beforeEach(() => {
  config = { ...defaultCameraConfig, id: "CAM-2", acquisition: "freeRun", follow: {
    nozzle: [500, 700], angleDeg: 0, mirror: false, mmPerPx: .1, maskPx: 30, imageSize: [1000, 800],
  } };
  onSaved.mockReset();
  vi.mocked(cameraApi.saveConfig).mockResolvedValue([]);
  vi.mocked(invoke).mockResolvedValue({ points, directionDeg: -90 });
});

describe("随动标定的基本操作", () => {
  it("没有冻结图像时禁用保存和试测", () => {
    show(null);
    for (const name of ["保存标定", "自动找方向", "在当前帧试测"]) expect(screen.getByRole("button", { name })).toBeDisabled();
  });

  it("在冻结图像上点胶嘴、设走向并点方向，镜像结果随参数换算", async () => {
    const page = show(); const svg = coordinates(page.container);
    fireEvent.click(svg, { clientX: 250, clientY: 600 });
    expect(screen.getByRole("spinbutton", { name: "胶嘴 x（px）" })).toHaveValue(250);
    expect(screen.getByRole("spinbutton", { name: "胶嘴 y（px）" })).toHaveValue(600);
    change("标定时走向（°）", 0);
    await userEvent.click(screen.getByRole("button", { name: "点胶条方向" }));
    fireEvent.click(svg, { clientX: 250, clientY: 500 });
    expect(screen.getByRole("spinbutton", { name: "图像方位（°）" })).toHaveValue(90);
    await userEvent.click(screen.getByRole("checkbox", { name: "图像相对工件坐标镜像" }));
    fireEvent.click(svg, { clientX: 250, clientY: 500 });
    expect(screen.getByRole("spinbutton", { name: "图像方位（°）" })).toHaveValue(-90);
  });

  it("点完比例尺后修改实际距离会重新计算像素当量", async () => {
    const page = show(); const svg = coordinates(page.container);
    await userEvent.click(screen.getByRole("button", { name: "量比例" }));
    fireEvent.click(svg, { clientX: 100, clientY: 200 });
    fireEvent.click(svg, { clientX: 200, clientY: 200 });
    expect(screen.getByRole("spinbutton", { name: "像素当量（mm/px）" })).toHaveValue(.1);
    change("量比例：实际距离（mm）", 20);
    expect(screen.getByRole("spinbutton", { name: "像素当量（mm/px）" })).toHaveValue(.2);
    await userEvent.click(screen.getByRole("button", { name: "保存标定" }));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(1, expect.objectContaining({ follow: expect.objectContaining({ mmPerPx: .2 }) }));
  });

  it("两点距离过近不会改变像素当量，第三个点重新开始量比例", async () => {
    const page = show(); const svg = coordinates(page.container);
    await userEvent.click(screen.getByRole("button", { name: "量比例" }));
    fireEvent.click(svg, { clientX: 100, clientY: 100 }); fireEvent.click(svg, { clientX: 101, clientY: 101 });
    expect(screen.getByRole("spinbutton", { name: "像素当量（mm/px）" })).toHaveValue(.1);
    fireEvent.click(svg, { clientX: 200, clientY: 200 }); fireEvent.click(svg, { clientX: 250, clientY: 200 });
    expect(screen.getByRole("spinbutton", { name: "像素当量（mm/px）" })).toHaveValue(.2);
  });

  it("默认三目使用当前冻结图像尺寸与相机方位", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "默认三目" }));
    await userEvent.click(screen.getByRole("button", { name: "保存标定" }));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(1, expect.objectContaining({ follow: expect.objectContaining({
      nozzle: [500, 704], angleDeg: 120, imageSize: [1000, 800], mmPerPx: .05,
    }) }));
  });

  it("自动找方向更新方位并保留本次试测统计", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "自动找方向" }));
    expect(invoke).toHaveBeenCalledWith("teach_follow_probe", { request: expect.objectContaining({ cam: 1, imageId: "station-1", directionDeg: null }) });
    await waitFor(() => expect(screen.getByRole("spinbutton", { name: "图像方位（°）" })).toHaveValue(90));
    expect(screen.getByText(/测到 2\/2 点/)).toBeVisible();
  });

  it("手动试测绑定冻结图像与当前参数，改参数后撤销旧统计", async () => {
    show(); change("名义胶宽（mm）", 3);
    await userEvent.click(screen.getByRole("button", { name: "在当前帧试测" }));
    expect(invoke).toHaveBeenCalledWith("teach_follow_probe", { request: expect.objectContaining({ imageId: "station-1", beadWidth: 3, directionDeg: -180 }) });
    expect(await screen.findByText(/测到 2\/2 点/)).toBeVisible();
    change("搜索半宽（mm）", 5); expect(screen.queryByText(/测到 2\/2 点/)).toBeNull();
  });

  it("旧样本的试测结果晚到不会覆盖新样本", async () => {
    const request = deferred<{ points: typeof points; directionDeg: number }>(); vi.mocked(invoke).mockReturnValue(request.promise);
    const page = show(); await userEvent.click(screen.getByRole("button", { name: "在当前帧试测" }));
    page.rerender(<FollowCalibPanel cam={1} config={config} frame={undefined} sample={{ ...sample, metadata: { ...sample.metadata, id: "station-2" } }} onSaved={onSaved} />);
    await act(async () => request.resolve({ points, directionDeg: -90 }));
    expect(screen.queryByText(/测到 2\/2 点/)).toBeNull();
    expect(screen.getByRole("button", { name: "在当前帧试测" })).toBeEnabled();
  });

  it("保存等待期间锁定参数并拒绝重复保存，失败可以重试", async () => {
    const request = deferred<string[]>(); vi.mocked(cameraApi.saveConfig).mockReturnValueOnce(request.promise);
    show(); const save = screen.getByRole("button", { name: "保存标定" });
    fireEvent.click(save); fireEvent.click(save);
    expect(cameraApi.saveConfig).toHaveBeenCalledTimes(1);
    expect(save).toBeDisabled(); expect(screen.getByRole("spinbutton", { name: "像素当量（mm/px）" })).toBeDisabled();
    await act(async () => request.reject(new Error("保存失败")));
    expect(await screen.findByText("Error: 保存失败")).toBeVisible(); expect(save).toBeEnabled();
    await userEvent.click(save); expect(await screen.findByText("随动标定已保存")).toBeVisible();
  });

  it.each([0, -1])("像素当量 %s 禁止保存和试测", value => {
    show(); change("像素当量（mm/px）", value);
    expect(screen.getByRole("button", { name: "保存标定" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "在当前帧试测" })).toBeDisabled();
    expect(screen.getByText(/像素当量必须大于 0/)).toBeVisible();
  });
  it.each([0,2])("工位 %s 的默认三目标定保存对应方位",async cam=>{
    render(<FollowCalibPanel cam={cam} config={config} frame={undefined} sample={sample} onSaved={onSaved}/>);
    await userEvent.click(screen.getByRole("button",{name:"默认三目"}));await userEvent.click(screen.getByRole("button",{name:"保存标定"}));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(cam,expect.objectContaining({follow:expect.objectContaining({angleDeg:120*cam,nozzle:[500,704],mirror:false,maskPx:60})}));
  });
  it("首次冻结图像到达时按真实尺寸生成未标定工位的默认胶嘴",async()=>{
    config.follow=null;const page=show(null);page.rerender(<FollowCalibPanel cam={1} config={config} frame={undefined} sample={sample} onSaved={onSaved}/>);
    expect(screen.getByRole("spinbutton",{name:"胶嘴 x（px）"})).toHaveValue(500);expect(screen.getByRole("spinbutton",{name:"胶嘴 y（px）"})).toHaveValue(704);
    await userEvent.click(screen.getByRole("button",{name:"保存标定"}));expect(cameraApi.saveConfig).toHaveBeenCalledWith(1,expect.objectContaining({follow:expect.objectContaining({imageSize:[1000,800]})}));
  });
  it("所有标定字段、镜像和走向使用同一冻结样本参与试测，保存保留相机配置",async()=>{
    show();change("胶嘴 x（px）",400);change("胶嘴 y（px）",600);change("遮挡半径（px）",40);change("像素当量（mm/px）",.2);
    change("图像方位（°）",30);change("标定时走向（°）",90);await userEvent.click(screen.getByRole("checkbox",{name:"图像相对工件坐标镜像"}));
    change("名义胶宽（mm）",3);change("搜索半宽（mm）",6);change("窗口近端（mm）",2);change("窗口远端（mm）",20);
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"胶条极性"}),"light");await userEvent.click(screen.getByRole("button",{name:"在当前帧试测"}));
    const request=vi.mocked(invoke).mock.calls[0][1] as {request:{directionDeg:number;calib:unknown}};
    expect(request.request.directionDeg).toBeCloseTo(-60);expect(request.request).toMatchObject({cam:1,imageId:"station-1",polarity:"light",beadWidth:3,searchMm:6,nearMm:2,farMm:20,
      calib:{nozzle:[400,600],angleDeg:30,mirror:true,mmPerPx:.2,maskPx:40,imageSize:[1000,800]}});
    await userEvent.click(screen.getByRole("button",{name:"保存标定"}));expect(cameraApi.saveConfig).toHaveBeenCalledWith(1,{...config,follow:request.request.calib});
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({id:"CAM-2",follow:request.request.calib}));
  });
  it.each(["dark","light","any"])("胶条极性 %s 传递到真实试测命令",async polarity=>{
    show();await userEvent.selectOptions(screen.getByRole("combobox",{name:"胶条极性"}),polarity);await userEvent.click(screen.getByRole("button",{name:"在当前帧试测"}));
    expect(invoke).toHaveBeenCalledWith("teach_follow_probe",{request:expect.objectContaining({polarity})});
  });
  it.each([.001,5,6])("像素当量 %s 与后端范围一致，拒绝临界值",value=>{
    show();change("像素当量（mm/px）",value);expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"自动找方向"})).toBeDisabled();expect(screen.getByRole("alert")).toHaveTextContent("0.001–5 mm/px");
  });
  it.each([["胶嘴 x（px）",-1],["胶嘴 x（px）",1000],["胶嘴 y（px）",800],["遮挡半径（px）",-1]] as const)("无效标定 %s=%s 阻止保存与试测",(label,value)=>{
    show();change(label,value);for(const name of ["保存标定","自动找方向","在当前帧试测"])expect(screen.getByRole("button",{name})).toBeDisabled();
    expect(screen.getByRole("alert")).toBeVisible();expect(cameraApi.saveConfig).not.toHaveBeenCalled();expect(invoke).not.toHaveBeenCalled();
  });
  it("过小冻结图像不会提交标定",()=>{
    show({...sample,image:{...sample.image,width:16,height:16}});expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("冻结图像尺寸需大于 16 px");
  });
  it.each([["名义胶宽（mm）",0],["搜索半宽（mm）",0],["窗口近端（mm）",-1],["窗口远端（mm）",3]] as const)("无效试测 %s=%s 不能调用卡尺",(label,value)=>{
    show();change(label,value);expect(screen.getByRole("button",{name:"在当前帧试测"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"自动找方向"})).toBeDisabled();expect(screen.getByRole("button",{name:"保存标定"})).toBeEnabled();
    expect(screen.getByRole("alert")).toBeVisible();expect(invoke).not.toHaveBeenCalled();
  });
  it.each([0,-5])("比例尺实际距离 %s 不沿用旧像素当量，修正后恢复",async distance=>{
    const page=show(),svg=coordinates(page.container);await userEvent.click(screen.getByRole("button",{name:"量比例"}));
    fireEvent.click(svg,{clientX:100,clientY:100});fireEvent.click(svg,{clientX:200,clientY:100});change("量比例：实际距离（mm）",distance);
    expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();expect(screen.getByRole("alert")).toHaveTextContent("比例尺实际距离必须大于 0");
    change("量比例：实际距离（mm）",20);expect(screen.getByRole("button",{name:"保存标定"})).toBeEnabled();
    expect(screen.getByRole("spinbutton",{name:"像素当量（mm/px）"})).toHaveValue(.2);
  });
  it("默认三目清除无效比例尺，两点与新样本互不串用",async()=>{
    const page=show(),svg=coordinates(page.container);await userEvent.click(screen.getByRole("button",{name:"量比例"}));
    fireEvent.click(svg,{clientX:100,clientY:100});fireEvent.click(svg,{clientX:101,clientY:100});expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"默认三目"}));expect(screen.queryByRole("alert")).toBeNull();expect(svg.querySelectorAll('circle[r="5"]')).toHaveLength(0);
    await userEvent.click(screen.getByRole("button",{name:"量比例"}));fireEvent.click(svg,{clientX:100,clientY:100});expect(svg.querySelectorAll('circle[r="5"]')).toHaveLength(1);
    page.rerender(<FollowCalibPanel cam={1} config={config} frame={undefined} sample={{...sample,metadata:{...sample.metadata,id:"station-2"}}} onSaved={onSaved}/>);
    expect(svg.querySelectorAll('circle[r="5"]')).toHaveLength(0);
  });
  it("图像外点击和胶嘴中心的方向点击不会改变有效标定",async()=>{
    const page=show(),svg=coordinates(page.container);fireEvent.click(svg,{clientX:-1,clientY:200});fireEvent.click(svg,{clientX:1000,clientY:200});
    expect(screen.getByRole("spinbutton",{name:"胶嘴 x（px）"})).toHaveValue(500);await userEvent.click(screen.getByRole("button",{name:"点胶条方向"}));
    fireEvent.click(svg,{clientX:500,clientY:700});expect(screen.getByRole("spinbutton",{name:"图像方位（°）"})).toHaveValue(0);
  });
  it("边缘小数坐标取整后仍保留在图像最后一个像素内",()=>{
    const page=show(),svg=coordinates(page.container);fireEvent.click(svg,{clientX:999.9,clientY:799.9});
    expect(screen.getByRole("spinbutton",{name:"胶嘴 x（px）"})).toHaveValue(999);expect(screen.getByRole("spinbutton",{name:"胶嘴 y（px）"})).toHaveValue(799);
    expect(screen.getByRole("button",{name:"保存标定"})).toBeEnabled();
  });
  it("试测中锁定参数和图像工具，重复点击不重复发起，失败可重试",async()=>{
    const request=deferred<{points:typeof points;directionDeg:number}>();vi.mocked(invoke).mockReturnValueOnce(request.promise);
    const page=show(),svg=coordinates(page.container),button=screen.getByRole("button",{name:"在当前帧试测"});fireEvent.click(button);fireEvent.click(button);
    expect(invoke).toHaveBeenCalledTimes(1);expect(screen.getByRole("spinbutton",{name:"标定时走向（°）"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"点胶嘴"})).toBeDisabled();fireEvent.click(svg,{clientX:50,clientY:50});
    expect(screen.getByRole("spinbutton",{name:"胶嘴 x（px）"})).toHaveValue(500);
    await act(async()=>request.reject(new Error("卡尺试测失败")));expect(screen.getByText("Error: 卡尺试测失败")).toBeVisible();expect(button).toBeEnabled();
    await userEvent.click(button);expect(await screen.findByText(/测到 2\/2 点/)).toBeVisible();expect(screen.queryByText("Error: 卡尺试测失败")).toBeNull();
  });
  it.each(["success","failure"])("旧试测 %s 完成不会解除新样本试测的等待",async outcome=>{
    const old=deferred<{points:typeof points;directionDeg:number}>(),next=deferred<{points:typeof points;directionDeg:number}>();
    vi.mocked(invoke).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);const page=show();
    fireEvent.click(screen.getByRole("button",{name:"在当前帧试测"}));page.rerender(<FollowCalibPanel cam={1} config={config} frame={undefined} sample={{...sample,metadata:{...sample.metadata,id:"station-2"}}} onSaved={onSaved}/>);
    fireEvent.click(screen.getByRole("button",{name:"在当前帧试测"}));await act(async()=>outcome==="success"?old.resolve({points,directionDeg:-90}):old.reject(new Error("旧错误")));
    expect(screen.getByRole("button",{name:"在当前帧试测"})).toBeDisabled();expect(screen.queryByText(/测到 2\/2 点/)).toBeNull();expect(screen.queryByText("Error: 旧错误")).toBeNull();
    await act(async()=>next.resolve({points,directionDeg:-90}));expect(screen.getByText(/测到 2\/2 点/)).toBeVisible();expect(invoke).toHaveBeenCalledTimes(2);
  });
  it.each(["camera","sample","config","unmount"])("标定保存晚到时 %s 改变后不回调旧配置",async scope=>{
    const request=deferred<string[]>();vi.mocked(cameraApi.saveConfig).mockReturnValueOnce(request.promise);const page=show();
    fireEvent.click(screen.getByRole("button",{name:"保存标定"}));
    if(scope==="unmount")page.unmount();else{
      const next=scope==="camera"?{...config,id:"CAM-3",follow:{...config.follow!,nozzle:[100,200] as [number,number],angleDeg:240}}:scope==="config"?{...config,exposureUs:80}:config;
      const frozen=scope==="sample"?{...sample,metadata:{...sample.metadata,id:"station-2"}}:sample;
      page.rerender(<FollowCalibPanel cam={scope==="camera"?2:1} config={next} frame={undefined} sample={frozen} onSaved={onSaved}/>);
      if(scope==="camera")expect(screen.getByRole("spinbutton",{name:"胶嘴 x（px）"})).toHaveValue(100);
      expect(screen.getByRole("button",{name:"保存标定"})).toBeEnabled();
    }
    await act(async()=>request.resolve([]));expect(onSaved).not.toHaveBeenCalled();expect(screen.queryByText("随动标定已保存")).toBeNull();
  });
  it("旧标定保存失败不会给新工位报错或解除新保存",async()=>{
    const old=deferred<string[]>(),next=deferred<string[]>();vi.mocked(cameraApi.saveConfig).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const page=show();fireEvent.click(screen.getByRole("button",{name:"保存标定"}));const nextConfig={...config,id:"CAM-3"};
    page.rerender(<FollowCalibPanel cam={2} config={nextConfig} frame={undefined} sample={sample} onSaved={onSaved}/>);fireEvent.click(screen.getByRole("button",{name:"保存标定"}));
    await act(async()=>old.reject(new Error("旧标定错误")));expect(screen.queryByText("Error: 旧标定错误")).toBeNull();expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();
    await act(async()=>next.resolve([]));expect(onSaved).toHaveBeenCalledExactlyOnceWith(nextConfig);expect(screen.getByText("随动标定已保存")).toBeVisible();
  });
  it("没有测到有效胶条时明确显示失败统计",async()=>{
    vi.mocked(invoke).mockResolvedValueOnce({points:[{...points[0],st:1,width:null,offset:null}],directionDeg:0});show();
    await userEvent.click(screen.getByRole("button",{name:"在当前帧试测"}));const notice=await screen.findByText(/测到 0\/1 点/);expect(notice).toHaveClass("error");
  });
  it("键盘清空并逐字输入负方位，未完成负号不提交，完成后保存正确角度",async()=>{
    const error=vi.spyOn(console,"error"),page=show();const user=userEvent.setup(),input=screen.getByRole("spinbutton",{name:"图像方位（°）"});await user.clear(input);
    expect(input).toHaveValue(null);expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();await user.type(input,"-");
    expectFiniteDrawing(page.container);expect(error).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"在当前帧试测"})).toBeDisabled();await user.click(screen.getByRole("button",{name:"保存标定"}));
    expect(cameraApi.saveConfig).not.toHaveBeenCalled();await user.type(input,"12.5");expect(input).toHaveValue(-12.5);
    await user.click(screen.getByRole("button",{name:"保存标定"}));expect(cameraApi.saveConfig).toHaveBeenCalledWith(1,expect.objectContaining({follow:expect.objectContaining({angleDeg:-12.5})}));
  });
  it("负走向通过真实键盘输入后按当前方向参与试测",async()=>{
    const error=vi.spyOn(console,"error"),page=show();const user=userEvent.setup(),input=screen.getByRole("spinbutton",{name:"标定时走向（°）"});await user.clear(input);
    expect(input).toHaveValue(null);expectFiniteDrawing(page.container);expect(error).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"在当前帧试测"})).toBeDisabled();await user.type(input,"-90");
    expect(input).toHaveValue(-90);await user.click(screen.getByRole("button",{name:"在当前帧试测"}));
    expect(invoke).toHaveBeenCalledWith("teach_follow_probe",{request:expect.objectContaining({directionDeg:expect.closeTo(90)})});
  });
  it.each([
    ["胶嘴 x（px）",500],["胶嘴 y（px）",700],["遮挡半径（px）",30],["像素当量（mm/px）",.1],["图像方位（°）",0],
  ] as const)("键盘清空标定 %s 不保存零值，重新填写后恢复",async(label,value)=>{
    const error=vi.spyOn(console,"error"),page=show();const user=userEvent.setup(),input=screen.getByRole("spinbutton",{name:label});await user.clear(input);expect(input).toHaveValue(null);
    expectFiniteDrawing(page.container);expect(error).not.toHaveBeenCalled();
    for(const name of ["保存标定","自动找方向","在当前帧试测"])expect(screen.getByRole("button",{name})).toBeDisabled();
    await user.click(screen.getByRole("button",{name:"保存标定"}));expect(cameraApi.saveConfig).not.toHaveBeenCalled();
    await user.type(input,String(value));expect(input).toHaveValue(value);expect(screen.getByRole("button",{name:"保存标定"})).toBeEnabled();
  });
  it.each([["名义胶宽（mm）",2],["搜索半宽（mm）",4],["窗口近端（mm）",3],["窗口远端（mm）",18]] as const)("键盘清空试测 %s 不发送零值，重新填写后恢复",async(label,value)=>{
    const error=vi.spyOn(console,"error"),page=show();const user=userEvent.setup(),input=screen.getByRole("spinbutton",{name:label});await user.clear(input);expect(input).toHaveValue(null);
    expectFiniteDrawing(page.container);expect(error).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"在当前帧试测"})).toBeDisabled();await user.click(screen.getByRole("button",{name:"在当前帧试测"}));expect(invoke).not.toHaveBeenCalled();
    await user.type(input,String(value));expect(screen.getByRole("button",{name:"在当前帧试测"})).toBeEnabled();
  });
  it("键盘清空比例尺长度后不沿用旧比例保存，填写有效长度恢复",async()=>{
    const page=show(),svg=coordinates(page.container),user=userEvent.setup();await user.click(screen.getByRole("button",{name:"量比例"}));
    fireEvent.click(svg,{clientX:100,clientY:100});fireEvent.click(svg,{clientX:200,clientY:100});const input=screen.getByRole("spinbutton",{name:"量比例：实际距离（mm）"});
    await user.clear(input);expect(input).toHaveValue(null);expect(screen.getByRole("button",{name:"保存标定"})).toBeDisabled();
    await user.type(input,"20");expect(screen.getByRole("spinbutton",{name:"像素当量（mm/px）"})).toHaveValue(.2);expect(screen.getByRole("button",{name:"保存标定"})).toBeEnabled();
  });
});

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CameraPage, { FlyshotCalibrationPage } from "../src/pages/CameraPage";
import { cameraApi, defaultCameraConfig } from "../src/features/camera/api";
import { recipeApi } from "../src/features/cycle";
import { workspaceApi } from "../src/features/workspace/api";
import type { CameraConfig, CameraStatus, DeviceSummary } from "../src/features/camera/types";
import type { Snapshot } from "../src/features/cycle/types";
import { deferred, shotList, snapshot, summary, workspaceView, twoLines } from "./fixtures";

const hooks=vi.hoisted(()=>({phase:"IDLE",statuses:[] as CameraStatus[],desktop:true}));
vi.mock("../src/lib/desktop",()=>({desktopAvailable:()=>hooks.desktop}));
vi.mock("../src/features/camera/api",async importOriginal=>{
  const actual=await importOriginal<typeof import("../src/features/camera/api")>();
  return {...actual,cameraApi:{...actual.cameraApi,rigConfig:vi.fn(),add:vi.fn(),remove:vi.fn(),listDevices:vi.fn(),records:vi.fn(),saveConfig:vi.fn(),softTrigger:vi.fn()},
    usePreviewCanvas:()=>({img:null,canvas:{current:null}})};
});
vi.mock("../src/features/camera",async importOriginal=>{
  const actual=await importOriginal<typeof import("../src/features/camera")>();
  return {...actual,useRigStatus:()=>({statuses:hooks.statuses,lastFrame:{}}),
    CalibPanel:({cam}:{cam:number})=><div>飞拍标定工位 {cam}</div>,FeasibilityCalc:()=>null,DryRunPanel:()=>null};
});
vi.mock("../src/features/cycle",()=>({recipeApi:{list:vi.fn()},useCycle:()=>({snapshot:snapshot(hooks.phase as Snapshot["phase"])}),SimControls:()=>null}));
vi.mock("../src/features/workspace/api",()=>({workspaceApi:{list:vi.fn()}}));
vi.mock("../src/features/workspace/StationCapture",()=>({default:({cam}:{cam:number})=><div>取样工位 {cam}</div>}));

let configs:CameraConfig[];
const status=(config:CameraConfig,cam:number):CameraStatus=>({cam,id:config.id,name:config.name,source:config.source,acquisition:config.acquisition,ready:true,message:"",device:null,sdkVersion:null,frames:0,fps:0,maxFps:null,lostPackets:0,droppedFrames:0,warnings:[]});
beforeEach(()=>{
  configs=[{...defaultCameraConfig,id:"CAM-1",name:"相机 1"},{...defaultCameraConfig,id:"CAM-2",name:"相机 2",source:"replay",replayDir:"D:/images"}];
  hooks.phase="IDLE";hooks.desktop=true;hooks.statuses=configs.map(status);
  vi.mocked(cameraApi.rigConfig).mockImplementation(async()=>configs);
  vi.mocked(cameraApi.add).mockResolvedValue(2);vi.mocked(cameraApi.remove).mockResolvedValue(undefined);
  vi.mocked(cameraApi.records).mockResolvedValue({root:"D:/records",items:[]});vi.mocked(cameraApi.listDevices).mockResolvedValue([]);
  vi.mocked(cameraApi.saveConfig).mockResolvedValue([]);vi.mocked(cameraApi.softTrigger).mockResolvedValue(undefined);
  vi.mocked(recipeApi.list).mockResolvedValue({recipes:[],errors:[]});vi.mocked(workspaceApi.list).mockResolvedValue([]);
  vi.spyOn(window,"confirm").mockReturnValue(true);
});
const show=()=>render(<MemoryRouter><CameraPage/></MemoryRouter>);
const loaded=()=>screen.findByRole("button",{name:"相机 1 CAM-1"});

describe("设备与采集页基础操作",()=>{
  it("三目设备标定明确绑定视角 1，其他视角提示独立标定或手动像素当量",async()=>{
    configs[0].viewCount=3;render(<MemoryRouter><FlyshotCalibrationPage/></MemoryRouter>);
    expect(await screen.findByText("工位标定使用视角 1")).toBeVisible();
    expect(screen.getByText("工位标定当前使用视角 1；其他视角需独立标定引用或手动像素当量。")).toBeVisible();
    expect(screen.getByText("取样工位 0")).toBeVisible();expect(screen.getByText("飞拍标定工位 0")).toBeVisible();
  });
  it("初始加载失败保留重试入口，成功后显示相机与操作",async()=>{
    vi.mocked(cameraApi.rigConfig).mockRejectedValueOnce(new Error("设备配置读取失败"));
    show();expect(await screen.findByText("Error: 设备配置读取失败")).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"刷新配置"}));await loaded();
    expect(screen.queryByText("Error: 设备配置读取失败")).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"添加相机"})).toBeEnabled();
  });
  it("切换相机显示各自参数，导航不额外保存或刷新设备",async()=>{
    show();await loaded();
    await userEvent.click(screen.getByRole("button",{name:"相机 2 CAM-2"}));
    expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("相机 2");
    expect(screen.getByRole("textbox",{name:"图片目录"})).toHaveValue("D:/images");
    expect(screen.getByRole("button",{name:"下一张"})).toBeEnabled();
    expect(cameraApi.saveConfig).not.toHaveBeenCalled();expect(cameraApi.rigConfig).toHaveBeenCalledTimes(1);
  });
  it("配置加载未完成时不能添加，相机组为空时使用默认配置建立首台相机",async()=>{
    const request=deferred<CameraConfig[]>();vi.mocked(cameraApi.rigConfig).mockReturnValueOnce(request.promise);
    show();expect(screen.getByRole("button",{name:"添加相机"})).toBeDisabled();await userEvent.click(screen.getByRole("button",{name:"添加相机"}));expect(cameraApi.add).not.toHaveBeenCalled();
    await act(async()=>request.resolve([]));
    vi.mocked(cameraApi.add).mockImplementationOnce(async()=>{configs=[{...defaultCameraConfig,id:"CAM-1",name:"相机 1"}];return 0;});
    await userEvent.click(screen.getByRole("button",{name:"添加相机"}));
    expect(cameraApi.add).toHaveBeenCalledWith({...defaultCameraConfig,name:"相机 1",serial:""});expect(await loaded()).toBeVisible();
  });
  it("刷新后相机顺序变化仍按固定编号保持选择，保存绑定新索引",async()=>{
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"相机 2 CAM-2"}));
    configs=[configs[1],configs[0]];await userEvent.click(screen.getByRole("button",{name:"刷新配置"}));
    await waitFor(()=>expect(screen.getByRole("button",{name:"刷新配置"})).toBeEnabled());
    expect(screen.getByRole("button",{name:"相机 2 CAM-2"})).toHaveClass("active");expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("相机 2");
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));expect(cameraApi.saveConfig).toHaveBeenCalledWith(0,expect.objectContaining({id:"CAM-2"}));
  });
  it("参数写入中同步锁定页头增删、切换与刷新，失败才恢复操作",async()=>{
    const request=deferred<string[]>();vi.mocked(cameraApi.saveConfig).mockReturnValue(request.promise);
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    for(const name of ["添加相机","移除当前","刷新配置","相机 2 CAM-2"])expect(screen.getByRole("button",{name})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"移除当前"}));expect(cameraApi.remove).not.toHaveBeenCalled();
    await act(async()=>request.reject(new Error("参数保存失败")));
    expect(screen.getByText("Error: 参数保存失败")).toBeVisible();for(const name of ["添加相机","移除当前","刷新配置","相机 2 CAM-2"])expect(screen.getByRole("button",{name})).toBeEnabled();
  });
  it("添加继承采集配置但清空序列号，等待时防止重复并选中新相机",async()=>{
    configs[1]={...configs[1],serial:"OLD"};
    const request=deferred<number>();vi.mocked(cameraApi.add).mockReturnValue(request.promise);
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"添加相机"}));
    expect(cameraApi.add).toHaveBeenCalledWith(expect.objectContaining({name:"相机 3",source:"replay",serial:"",replayDir:"D:/images"}));
    expect(screen.getByRole("button",{name:"添加中…"})).toBeDisabled();expect(screen.getByRole("button",{name:"相机 2 CAM-2"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button",{name:"添加中…"}));expect(cameraApi.add).toHaveBeenCalledTimes(1);
    configs=[...configs,{...configs[1],id:"CAM-3",name:"相机 3",serial:""}];
    await act(async()=>request.resolve(2));
    await waitFor(()=>expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("相机 3"));
  });
  it("添加失败显示原因，保留当前相机并允许重试",async()=>{
    vi.mocked(cameraApi.add).mockRejectedValueOnce(new Error("配置文件只读")).mockImplementationOnce(async()=>{configs=[...configs,{...configs[0],id:"CAM-3",name:"相机 3"}];return 2;});
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"添加相机"}));
    expect(await screen.findByText("Error: 配置文件只读")).toBeVisible();expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("相机 1");
    await userEvent.click(screen.getByRole("button",{name:"添加相机"}));
    await waitFor(()=>expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("相机 3"));
  });
  it("添加使用不重复名称，页面卸载后不继续刷新或设置选中相机",async()=>{
    configs[1]={...configs[1],name:"相机 3"};const request=deferred<number>();vi.mocked(cameraApi.add).mockReturnValue(request.promise);
    const page=show();await loaded();await userEvent.click(screen.getByRole("button",{name:"添加相机"}));
    expect(cameraApi.add).toHaveBeenCalledWith(expect.objectContaining({name:"相机 4"}));page.unmount();
    await act(async()=>request.resolve(2));expect(cameraApi.rigConfig).toHaveBeenCalledTimes(1);
  });
  it("移除需要确认，取消保持配置，确认后切回剩余相机",async()=>{
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"相机 2 CAM-2"}));
    vi.mocked(window.confirm).mockReturnValueOnce(false);
    await userEvent.click(screen.getByRole("button",{name:"移除当前"}));expect(cameraApi.remove).not.toHaveBeenCalled();
    vi.mocked(cameraApi.remove).mockImplementationOnce(async()=>{configs=[configs[0]];});
    await userEvent.click(screen.getByRole("button",{name:"移除当前"}));
    expect(cameraApi.remove).toHaveBeenCalledWith(1);
    await waitFor(()=>expect(screen.queryByRole("button",{name:"相机 2 CAM-2"})).not.toBeInTheDocument());
    expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("相机 1");expect(screen.queryByRole("button",{name:"移除当前"})).not.toBeInTheDocument();
  });
  it("移除请求未完成时锁定选择与重复提交，后端拒绝后保留设备",async()=>{
    const request=deferred<void>();vi.mocked(cameraApi.remove).mockReturnValue(request.promise);
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"移除当前"}));
    expect(screen.getByRole("button",{name:"移除中…"})).toBeDisabled();fireEvent.click(screen.getByRole("button",{name:"移除中…"}));
    await act(async()=>request.reject(new Error("配方刚刚引用了 CAM-1")));
    expect(screen.getByText("Error: 配方刚刚引用了 CAM-1")).toBeVisible();expect(await loaded()).toBeVisible();expect(cameraApi.remove).toHaveBeenCalledTimes(1);
  });
  it.each(["production","candidate","mixed","pending"] as const)("保护 %s 配方引用的相机并指出引用来源",async kind=>{
    const draft=workspaceView().workspace;
    if(kind==="production")vi.mocked(recipeApi.list).mockResolvedValue({recipes:[summary()],errors:[]});
    else if(kind==="candidate")vi.mocked(workspaceApi.list).mockResolvedValue([draft]);
    // 只有第二个拍照点用当前相机，也算引用
    else if(kind==="mixed")vi.mocked(workspaceApi.list).mockResolvedValue([{...draft,doc:{...draft.doc,shots:[{...draft.doc.shots[0],camera:"CAM-2"},draft.doc.shots[1]]}}]);
    else vi.mocked(workspaceApi.list).mockResolvedValue([{...draft,doc:{...draft.doc,shots:shotList(twoLines,"CAM-2")},pending:{doc:draft.doc,bundleHash:"bundle-v1",baseHash:null,revision:7,frames:draft.frames,overview:draft.overview,validation:draft.validation!}}]);
    show();await loaded();expect(screen.getByText("当前相机被配方引用")).toBeVisible();
    expect(screen.getByRole("button",{name:"移除当前"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"移除当前"}));expect(window.confirm).not.toHaveBeenCalled();expect(cameraApi.remove).not.toHaveBeenCalled();
    expect(screen.getByText(/先在 .*工件 A/)).toBeVisible();
  });
  it("引用列表读取失败禁止移除，刷新成功再解锁",async()=>{
    vi.mocked(workspaceApi.list).mockRejectedValueOnce(new Error("候选库不可读"));
    show();await loaded();expect(screen.getByText(/相机引用读取失败.*候选库不可读/)).toBeVisible();
    expect(screen.getByRole("button",{name:"移除当前"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"刷新配置"}));
    await waitFor(()=>expect(screen.getByRole("button",{name:"移除当前"})).toBeEnabled());
  });
  it("生产库存在解析错误时同样不能按空引用列表移除",async()=>{
    vi.mocked(recipeApi.list).mockResolvedValue({recipes:[],errors:["broken.json 无法解析"]});
    show();await loaded();expect(screen.getByText(/broken.json 无法解析/)).toBeVisible();expect(screen.getByRole("button",{name:"移除当前"})).toBeDisabled();
  });
  it("检测期间禁止配置、添加、移除与触发，故障复位阶段允许配置",async()=>{
    hooks.phase="ACQUIRE";const page=show();await loaded();
    expect(screen.getByRole("button",{name:"添加相机"})).toBeDisabled();expect(screen.getByRole("textbox",{name:"名称"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"相机 2 CAM-2"}));expect(screen.getByRole("button",{name:"下一张"})).toBeDisabled();
    hooks.phase="FAULT";page.rerender(<MemoryRouter><CameraPage/></MemoryRouter>);
    expect(screen.getByRole("textbox",{name:"名称"})).toBeEnabled();
  });
  it("最多八台相机，浏览器预览中不能执行设备操作",async()=>{
    configs=Array.from({length:8},(_,i)=>({...defaultCameraConfig,id:`CAM-${i+1}`,name:`相机 ${i+1}`}));
    const page=show();await loaded();expect(screen.getByRole("button",{name:"添加相机"})).toBeDisabled();expect(screen.getByRole("button",{name:"移除当前"})).toBeEnabled();
    hooks.desktop=false;page.rerender(<MemoryRouter><CameraPage/></MemoryRouter>);
    expect(screen.getByRole("button",{name:"添加相机"})).toBeDisabled();expect(screen.getByRole("button",{name:"移除当前"})).toBeDisabled();expect(screen.getByRole("button",{name:"模拟相机"})).toBeDisabled();
  });
  it("标定页面只列触发采集工位，没有适合工位时提供设备入口",async()=>{
    configs[1]={...configs[1],acquisition:"freeRun"};
    const page=render(<MemoryRouter><FlyshotCalibrationPage/></MemoryRouter>);
    expect(await screen.findByText("飞拍标定工位 0")).toBeVisible();expect(screen.queryByRole("button",{name:"相机 2 CAM-2"})).not.toBeInTheDocument();
    configs=[];page.unmount();render(<MemoryRouter><FlyshotCalibrationPage/></MemoryRouter>);
    expect(await screen.findByText("没有触发采集相机")).toBeVisible();expect(screen.getByRole("link",{name:"设备与采集"})).toHaveAttribute("href","/camera");
  });
  it("自动固定序列号触发新读取，迟到旧配置不会改回相机参数",async()=>{
    configs[0]={...configs[0],source:"mvs",serial:""};const page=show();await loaded();
    const old=deferred<CameraConfig[]>();vi.mocked(cameraApi.rigConfig).mockReturnValueOnce(old.promise).mockResolvedValueOnce([{...configs[0],serial:"AUTO",name:"最新配置"},configs[1]]);
    const device:DeviceSummary={serial:"AUTO",model:"MVS",transport:"GigE",ip:null,userName:""};
    hooks.statuses=[{...status(configs[0],0),device},status(configs[1],1)];
    // 重渲染状态相当于设备连接事件。
    page.rerender(<MemoryRouter><CameraPage/></MemoryRouter>);
    await waitFor(()=>expect(cameraApi.rigConfig).toHaveBeenCalledTimes(2));
    hooks.statuses=[status(configs[0],0),status(configs[1],1)];page.rerender(<MemoryRouter><CameraPage/></MemoryRouter>);
    hooks.statuses=[{...status(configs[0],0),device},status(configs[1],1)];page.rerender(<MemoryRouter><CameraPage/></MemoryRouter>);
    await screen.findByRole("button",{name:"最新配置 CAM-1"});
    await act(async()=>old.resolve([{...configs[0],name:"过期配置"},configs[1]]));
    expect(screen.queryByRole("button",{name:"过期配置 CAM-1"})).not.toBeInTheDocument();page.unmount();
  });
});

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CameraPage from "../src/pages/CameraPage";
import { cameraApi, defaultCameraConfig } from "../src/features/camera/api";
import { recipeApi } from "../src/features/cycle";
import { workspaceApi } from "../src/features/workspace/api";
import type { CameraConfig, CameraStatus, DeviceSummary } from "../src/features/camera/types";
import type { Snapshot } from "../src/features/cycle/types";
import { deferred, shotList, snapshot, summary, workspaceView, twoLines } from "./fixtures";

const hooks=vi.hoisted(()=>({phase:"IDLE",statuses:[] as CameraStatus[]}));
vi.mock("../src/lib/desktop",()=>({desktopAvailable:()=>true}));
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
  hooks.phase="IDLE";hooks.statuses=configs.map(status);
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

  it("参数写入中同步锁定页头增删、切换与刷新，失败才恢复操作",async()=>{
    const request=deferred<string[]>();vi.mocked(cameraApi.saveConfig).mockReturnValue(request.promise);
    show();await loaded();await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    for(const name of ["添加相机","移除当前","刷新配置","相机 2 CAM-2"])expect(screen.getByRole("button",{name})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"移除当前"}));expect(cameraApi.remove).not.toHaveBeenCalled();
    await act(async()=>request.reject(new Error("参数保存失败")));
    expect(screen.getByText("Error: 参数保存失败")).toBeVisible();for(const name of ["添加相机","移除当前","刷新配置","相机 2 CAM-2"])expect(screen.getByRole("button",{name})).toBeEnabled();
  });

  it.each(["production","candidate","mixed","pending"] as const)("保护 %s 配方引用的相机并指出引用来源",async kind=>{
    const draft=workspaceView().workspace;
    if(kind==="production")vi.mocked(recipeApi.list).mockResolvedValue({recipes:[summary()],errors:[]});
    else if(kind==="candidate")vi.mocked(workspaceApi.list).mockResolvedValue([draft]);
    // 只有第二个拍照点用当前相机，也算引用
    else if(kind==="mixed")vi.mocked(workspaceApi.list).mockResolvedValue([{...draft,doc:{...draft.doc,shots:[{...draft.doc.shots[0],camera:"CAM-2"},draft.doc.shots[1]]}}]);
    else vi.mocked(workspaceApi.list).mockResolvedValue([{...draft,doc:{...draft.doc,shots:shotList(twoLines,"CAM-2")},pending:{doc:draft.doc,bundleId:"bundle-v1",baseRevision:null,revision:7,frames:draft.frames,overview:draft.overview,validation:draft.validation!}}]);
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

  it("检测期间禁止配置、添加、移除与触发，故障复位阶段允许配置",async()=>{
    hooks.phase="ACQUIRE";const page=show();await loaded();
    expect(screen.getByRole("button",{name:"添加相机"})).toBeDisabled();expect(screen.getByRole("textbox",{name:"名称"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"相机 2 CAM-2"}));expect(screen.getByRole("button",{name:"下一张"})).toBeDisabled();
    hooks.phase="FAULT";page.rerender(<MemoryRouter><CameraPage/></MemoryRouter>);
    expect(screen.getByRole("textbox",{name:"名称"})).toBeEnabled();
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

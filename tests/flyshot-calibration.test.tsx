import {fireEvent,render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import {beforeEach,expect,it,vi} from "vitest";
import {invoke} from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import FlyshotCalibrationPage from "../src/pages/FlyshotCalibrationPage";
import FeasibilityCalc from "../src/features/camera/components/FeasibilityCalc";
import {cameraApi,defaultCameraConfig,useRigStatus} from "../src/features/camera/api";
import {useCycle,useSiteStatus} from "../src/features/cycle/api";
import type {CameraStatus} from "../src/features/camera/types";
import {workspaceApi} from "../src/features/workspace/api";
import type {FrozenImage} from "../src/features/workspace/types";
import {snapshot,siteStatus} from "./fixtures";
const desktop=vi.hoisted(()=>({on:false}));
vi.mock("@tauri-apps/api/core",()=>({invoke:vi.fn(),isTauri:()=>desktop.on}));
vi.mock("../src/features/camera/api",async original=>({...await original<object>(),cameraApi:{rigConfig:vi.fn()},useRigStatus:vi.fn()}));
vi.mock("../src/features/cycle/api",async original=>({...await original<object>(),useCycle:vi.fn(),useSiteStatus:vi.fn()}));
const status=(frames:number):CameraStatus=>({cam:0,id:"CAM-1",name:"相机 1",source:"mvs",ready:true,released:false,message:"",device:null,sdkVersion:null,frames,fps:0,configuredFrameRate:10, maxFps:5.6,lostPackets:0,droppedFrames:0,warnings:[]});
beforeEach(()=>{
 localStorage.clear();
 desktop.on=false;
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"}]);
 vi.mocked(useRigStatus).mockReturnValue({statuses:[status(1)],lastFrame:{}} as ReturnType<typeof useRigStatus>);
 vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[],measured:[]});
 vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
});

it("相机状态轮询多次刷新 → 飞拍模拟只渲染一个面板",async()=>{
 const page=()=><SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>;
 const view=render(page());
 await screen.findByRole("heading",{name:"飞拍模拟"});
 for(let i=2;i<7;i++){vi.mocked(useRigStatus).mockReturnValue({statuses:[status(i)],lastFrame:{}} as ReturnType<typeof useRigStatus>);view.rerender(page());}
 expect(screen.getAllByRole("heading",{name:"飞拍模拟"})).toHaveLength(1);
});

it("速度和拍照点间距未填 → 不给出满足结论并列出缺项",()=>{
 render(<FeasibilityCalc sources={{exposure:{value:60,from:"来自配方"},fps:{value:5.6,from:"来自相机最高帧率"},mmPerPx:{value:0.04,from:"来自工位标定"}}} imageSize={{width:1280,height:1024}}/>);
 expect(screen.getByRole("status")).toHaveTextContent("还缺：速度、最小拍照点间距");
 expect(screen.queryByText("满足")).toBeNull();
});

it("有图像尺寸和当量 → 视野取所选运动方向的像素数乘当量",async()=>{
 render(<FeasibilityCalc sources={{mmPerPx:{value:0.05,from:"来自工位标定"}}} imageSize={{width:1280,height:1024}}/>);
 const fov=screen.getByRole("spinbutton",{name:/视野沿运动方向/});
 expect(fov).toHaveValue(64);
 fireEvent.change(screen.getByRole("combobox",{name:"运动方向"}).closest(".qs-select")!.querySelector("select")!,{target:{value:"height"}});
 expect(fov).toHaveValue(51.2);
});

it("手填像素当量后配方曝光更新 → 保留手填当量并重新核算",async()=>{
 const view=render(<FeasibilityCalc sources={{exposure:{value:60,from:"来自配方"},fps:{value:10,from:"来自相机"}}} imageSize={{width:1280,height:1024}}/>);
 await userEvent.type(screen.getByRole("spinbutton",{name:/速度/}),"100");
 await userEvent.type(screen.getByRole("spinbutton",{name:/最小拍照点间距/}),"30");
 const scale=screen.getByRole("spinbutton",{name:/^像素当量/});
 await userEvent.type(scale,"0.08");
 view.rerender(<FeasibilityCalc sources={{exposure:{value:80,from:"来自配方"},fps:{value:10,from:"来自相机"}}} imageSize={{width:1280,height:1024}}/>);
 expect(scale).toHaveValue(0.08);
 expect(screen.getByRole("spinbutton",{name:/视野沿运动方向/})).toHaveValue(102.4);
 expect(screen.getByText(/0.008 mm = 0.10 px/)).toBeInTheDocument();
 expect(screen.getByRole("status")).toHaveTextContent("按当前数值满足飞拍条件");
});

it("三目相机（旧配方）→ 可选图 2，取样与标定都带 view=2",async()=>{
 desktop.on=true;
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1",viewCount:3}]);
 vi.mocked(invoke).mockImplementation(async command=>command==="vision_calibrate"?{mmPerPx:0.05,rms:0.02,pattern:[11,8],square:5,ts:1}:null);
 const capture=vi.spyOn(workspaceApi,"stationCapture").mockResolvedValue({id:"img-2",source:"live",capturedAt:1,size:[1280,1024],camera:"CAM-1",view:2} as FrozenImage);
 vi.spyOn(workspaceApi,"stationImage").mockResolvedValue({url:"data:,",width:1280,height:1024});
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 await userEvent.selectOptions(await screen.findByRole("combobox",{name:"标定图像"}),"2");
 await userEvent.click(screen.getByRole("button",{name:"拍照"}));
 const calibrate=await screen.findByRole("button",{name:"标定"});
 await vi.waitFor(()=>expect(calibrate).toBeEnabled());
 await userEvent.click(calibrate);
 await vi.waitFor(()=>expect(screen.getByRole("spinbutton",{name:/^像素当量/})).toHaveValue(0.05));
 expect(capture).toHaveBeenCalledWith(0,2);
 expect(invoke).toHaveBeenCalledWith("vision_calibrate",expect.objectContaining({cam:0,view:2,imageId:"img-2"}));
});

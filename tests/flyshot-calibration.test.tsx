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
vi.mock("../src/features/camera/api",async original=>({...await original<object>(),cameraApi:{rigConfig:vi.fn(),calibInfo:vi.fn()},useRigStatus:vi.fn()}));
vi.mock("../src/features/cycle/api",async original=>({...await original<object>(),useCycle:vi.fn(),useSiteStatus:vi.fn()}));
const status=(frames:number):CameraStatus=>({cam:0,id:"CAM-1",name:"相机 1",source:"mvs",ready:true,released:false,message:"",device:null,sdkVersion:null,frames,fps:0,configuredFrameRate:10, maxFps:5.6,lostPackets:0,droppedFrames:0,warnings:[],imaging:{exposureUs:60,gainDb:6}});
beforeEach(()=>{
 localStorage.clear();
 desktop.on=false;
 vi.mocked(invoke).mockImplementation(async command=>command==="storage_preferences_get"?{}:null);
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"}]);
 vi.mocked(cameraApi.calibInfo).mockResolvedValue(null);
 vi.mocked(useRigStatus).mockReturnValue({statuses:[status(1)],lastFrame:{}} as ReturnType<typeof useRigStatus>);
 vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[]});
 vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
});

it("速度和拍照点间距未填 → 不给出满足结论并列出缺项",()=>{
 render(<FeasibilityCalc sources={{exposure:{value:60,from:"来自相机页保存的曝光"},fps:{value:5.6,from:"来自相机最高帧率"},mmPerPx:{value:0.04,from:"来自工位标定"}}} imageSize={{width:1280,height:1024}}/>);
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

it("手填像素当量后相机曝光更新 → 保留手填当量并重新核算",async()=>{
 const view=render(<FeasibilityCalc sources={{exposure:{value:60,from:"来自相机页保存的曝光"},fps:{value:10,from:"来自相机"}}} imageSize={{width:1280,height:1024}}/>);
 await userEvent.type(screen.getByRole("spinbutton",{name:/速度/}),"100");
 await userEvent.type(screen.getByRole("spinbutton",{name:/最小拍照点间距/}),"30");
 const scale=screen.getByRole("spinbutton",{name:/^像素当量/});
 await userEvent.type(scale,"0.08");
 view.rerender(<FeasibilityCalc sources={{exposure:{value:80,from:"来自相机页保存的曝光"},fps:{value:10,from:"来自相机"}}} imageSize={{width:1280,height:1024}}/>);
 expect(scale).toHaveValue(0.08);
 expect(screen.getByRole("spinbutton",{name:/视野沿运动方向/})).toHaveValue(102.4);
 expect(screen.getByText(/0.008 mm = 0.10 px/)).toBeInTheDocument();
 expect(screen.getByRole("status")).toHaveTextContent("按当前数值满足飞拍条件");
});

it("单视角相机完成标定 → 按冻结图像更新像素当量与飞拍视野，相机条改为已标定并计入标定数",async()=>{
 desktop.on=true;
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"}]);
 let calibrated=false;
 vi.mocked(invoke).mockImplementation(async command=>{if(command==="storage_preferences_get")return {};if(command!=="vision_calibrate")return null;calibrated=true;return {mmPerPx:0.05,rms:0.02,pattern:[11,8],square:5,ts:1};});
 vi.mocked(cameraApi.calibInfo).mockImplementation(async()=>calibrated?{mmPerPx:0.05}:null);
 const capture=vi.spyOn(workspaceApi,"stationCapture").mockResolvedValue({id:"calibration-image",source:"live",capturedAt:1,size:[1280,1024],camera:"CAM-1",view:1} as FrozenImage);
 vi.spyOn(workspaceApi,"stationImage").mockResolvedValue({url:"data:,",width:1280,height:1024});
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 await screen.findByRole("heading",{name:"标定板参数"});
 expect(await screen.findByText("未标定",{selector:".cam-calib"})).toBeVisible();
 await userEvent.click(screen.getByRole("button",{name:"拍照"}));
 const calibrate=await screen.findByRole("button",{name:"标定"});
 await vi.waitFor(()=>expect(calibrate).toBeEnabled());
 await userEvent.click(calibrate);
 await vi.waitFor(()=>expect(screen.getByRole("spinbutton",{name:/^像素当量/})).toHaveValue(0.05));
 expect(screen.getByRole("spinbutton",{name:/视野沿运动方向/})).toHaveValue(64);
 expect(screen.getByText(/^RMS 0.0200 mm · 0.0500 mm\/px/)).toBeVisible();
 expect(await screen.findByText("已标定 · 0.0500 mm/px")).toBeVisible();
 expect(screen.getByText("标定 1 / 1")).toBeVisible();
 expect(capture).toHaveBeenCalledWith(0,1);
 expect(invoke).toHaveBeenCalledWith("vision_calibrate",expect.objectContaining({cam:0,view:1,imageId:"calibration-image"}));
});

it("相机未就绪 → 禁止拍照但可导入离线标定图",async()=>{
 desktop.on=true;
 vi.mocked(useRigStatus).mockReturnValue({statuses:[{...status(0),ready:false,message:"相机未连接"}],lastFrame:{}} as ReturnType<typeof useRigStatus>);
 vi.spyOn(workspaceApi,"stationImport").mockResolvedValue({id:"imported-image",source:"import",capturedAt:1,size:[1280,1024],camera:"CAM-1",view:1} as FrozenImage);
 vi.spyOn(workspaceApi,"stationImage").mockResolvedValue({url:"data:,",width:1280,height:1024});
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 await screen.findByRole("heading",{name:"标定板参数"});
 expect(screen.getByRole("button",{name:"拍照"})).toBeDisabled();
 expect(screen.getByRole("button",{name:"导入离线图"})).toBeEnabled();
 const file=new File(["image"],"calibration.png",{type:"image/png"});
 file.arrayBuffer=async()=>new TextEncoder().encode("image").buffer;
 await userEvent.upload(screen.getByLabelText("导入离线图",{selector:"input"}),file);
 expect(await screen.findByText(/CAM-1 · 离线原图/)).toBeVisible();
});

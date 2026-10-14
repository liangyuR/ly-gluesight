import {render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import {beforeEach,expect,it,vi} from "vitest";
import FlyshotCalibrationPage from "../src/pages/FlyshotCalibrationPage";
import FeasibilityCalc from "../src/features/camera/components/FeasibilityCalc";
import {cameraApi,defaultCameraConfig,useRigStatus} from "../src/features/camera/api";
import {useCycle,useSiteStatus} from "../src/features/cycle/api";
import type {CameraStatus} from "../src/features/camera/types";
import {snapshot,siteStatus} from "./fixtures";
vi.mock("../src/features/camera/api",async original=>({...await original<object>(),cameraApi:{rigConfig:vi.fn()},useRigStatus:vi.fn()}));
vi.mock("../src/features/cycle/api",async original=>({...await original<object>(),useCycle:vi.fn(),useSiteStatus:vi.fn()}));
const status=(frames:number):CameraStatus=>({cam:0,id:"CAM-1",name:"相机 1",source:"mvs",ready:true,released:false,message:"",device:null,sdkVersion:null,frames,fps:0,maxFps:5.6,lostPackets:0,droppedFrames:0,warnings:[]});
beforeEach(()=>{
 localStorage.clear();
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"}]);
 vi.mocked(useRigStatus).mockReturnValue({statuses:[status(1)],lastFrame:{}} as ReturnType<typeof useRigStatus>);
 vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[],measured:[]});
 vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
});

it("相机状态轮询多次刷新 → 飞拍可行性只渲染一个面板",async()=>{
 const page=()=><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter>;
 const view=render(page());
 await screen.findByRole("heading",{name:"飞拍可行性"});
 for(let i=2;i<7;i++){vi.mocked(useRigStatus).mockReturnValue({statuses:[status(i)],lastFrame:{}} as ReturnType<typeof useRigStatus>);view.rerender(page());}
 expect(screen.getAllByRole("heading",{name:"飞拍可行性"})).toHaveLength(1);
});

it("速度和拍照点间距未填 → 不给出满足结论并列出缺项",()=>{
 render(<FeasibilityCalc sources={{exposure:{value:60,from:"来自相机参数"},fps:{value:5.6,from:"来自相机最高帧率"},mmPerPx:{value:0.04,from:"来自工位标定"}}} imageSize={{width:1280,height:1024}}/>);
 expect(screen.getByRole("status")).toHaveTextContent("还缺：速度、最小拍照点间距");
 expect(screen.queryByText("满足")).toBeNull();
});

it("有图像尺寸和当量 → 视野取所选运动方向的像素数乘当量",async()=>{
 render(<FeasibilityCalc sources={{mmPerPx:{value:0.05,from:"来自工位标定"}}} imageSize={{width:1280,height:1024}}/>);
 const fov=screen.getByRole("spinbutton",{name:/视野沿运动方向/});
 expect(fov).toHaveValue(64);
 await userEvent.selectOptions(screen.getByRole("combobox",{name:"运动方向"}),"height");
 expect(fov).toHaveValue(51.2);
});

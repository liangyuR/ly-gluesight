import {act,render,screen} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import {beforeEach,expect,it,vi} from "vitest";
import {invoke} from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import FlyshotCalibrationPage from "../src/pages/FlyshotCalibrationPage";
import {cameraApi,defaultCameraConfig,useRigStatus} from "../src/features/camera/api";
import {useProductionBusy} from "../src/features/cycle/control";
import type {CameraStatus} from "../src/features/camera/types";
import type {FrozenImage} from "../src/features/workspace/types";
import { deferred } from "./fixtures";
const desktop=vi.hoisted(()=>({on:false}));
vi.mock("@tauri-apps/api/core",()=>({invoke:vi.fn(),isTauri:()=>desktop.on}));
vi.mock("../src/features/camera/api",async importOriginal=>{
 const original=await importOriginal<typeof import("../src/features/camera/api")>();
 return {...original,cameraApi:{...original.cameraApi,rigConfig:vi.fn()},useRigStatus:vi.fn()};
});
vi.mock("../src/features/cycle/control",()=>({useProductionBusy:vi.fn()}));
const status=(frames:number):CameraStatus=>({cam:0,id:"CAM-1",name:"相机 1",source:"mvs",ready:true,released:false,message:"",device:null,sdkVersion:null,frames,fps:0,configuredFrameRate:10, maxFps:5.6,lostPackets:0,droppedFrames:0,warnings:[],imaging:{exposureUs:60,gainDb:6}});
const frozen=(camera:string,view:number,id:string,source="calib"):FrozenImage=>({
 id,camera,view,source,capturedAt:1,size:[1,1],cameraTag:null,calibTag:null,geometryTag:null,exposureUs:null,gainDb:null,historyId:null,
});
const frame=new ArrayBuffer(21);
const header=new DataView(frame);
for(let field=0;field<5;field++)header.setUint32(field*4,1,true);

beforeEach(()=>{
 localStorage.clear();
 desktop.on=false;
 vi.mocked(invoke).mockReset().mockResolvedValue(null);
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"}]);
 vi.mocked(useRigStatus).mockReturnValue({statuses:[status(1)],lastFrame:{}} as ReturnType<typeof useRigStatus>);
 vi.mocked(useProductionBusy).mockReturnValue(false);
 vi.spyOn(HTMLCanvasElement.prototype,"getContext").mockReturnValue(null);
});

it("单视角相机完成标定 → 显示新标定结果，相机条改为已标定并计入标定数",async()=>{
 desktop.on=true;
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"}]);
 let calibrated=false;
 const calibration={mmPerPx:0.05,rms:0.02,pattern:[11,8],square:5,ts:1};
 vi.mocked(invoke).mockImplementation(async command=>{
  if(command==="workspace_station_capture")return frozen("CAM-1",1,"calibration-image","live");
  if(command==="workspace_station_image")return frame;
  if(command==="vision_calibrate"){calibrated=true;return calibration;}
  if(command==="vision_calib_info")return calibrated?calibration:null;
  return null;
 });
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 await screen.findByRole("heading",{name:"标定板参数"});
 expect(await screen.findByText("未标定",{selector:".cam-calib"})).toBeVisible();
 await userEvent.click(screen.getByRole("button",{name:"拍照"}));
 const calibrate=await screen.findByRole("button",{name:"标定"});
 await vi.waitFor(()=>expect(calibrate).toBeEnabled());
 await userEvent.click(calibrate);
 expect(await screen.findByText(/^RMS 0.0200 mm · 0.0500 mm\/px/)).toBeVisible();
 expect(await screen.findByText("已标定 · 0.0500 mm/px")).toBeVisible();
 expect(screen.getByText("标定 1 / 1")).toBeVisible();
 const target={cam:0,id:"CAM-1",view:1};
 expect(invoke).toHaveBeenCalledWith("vision_calib_info",target);
 expect(invoke).toHaveBeenCalledWith("workspace_station_restore",target);
 expect(invoke).toHaveBeenCalledWith("workspace_station_capture",target);
 expect(invoke).toHaveBeenCalledWith("workspace_station_image",{...target,imageId:"calibration-image"});
 expect(invoke).toHaveBeenCalledWith("vision_calibrate",expect.objectContaining({...target,imageId:"calibration-image"}));
});

it("相机未就绪 → 禁止拍照但可导入离线标定图",async()=>{
 desktop.on=true;
 vi.mocked(useRigStatus).mockReturnValue({statuses:[{...status(0),ready:false,message:"相机未连接"}],lastFrame:{}} as ReturnType<typeof useRigStatus>);
 vi.mocked(invoke).mockImplementation(async command=>{
  if(command==="workspace_station_import")return frozen("CAM-1",1,"imported-image","import");
  if(command==="workspace_station_image")return frame;
  return null;
 });
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 await screen.findByRole("heading",{name:"标定板参数"});
 expect(screen.getByRole("button",{name:"拍照"})).toBeDisabled();
 await vi.waitFor(()=>expect(screen.getByLabelText("导入离线图",{selector:"input"})).toBeEnabled());
 const file=new File(["image"],"calibration.png",{type:"image/png"});
 file.arrayBuffer=async()=>new TextEncoder().encode("image").buffer;
 await userEvent.upload(screen.getByLabelText("导入离线图",{selector:"input"}),file);
 expect(await screen.findByText(/CAM-1 · 离线原图/)).toBeVisible();
 expect(invoke).toHaveBeenCalledWith("workspace_station_import",expect.objectContaining({cam:0,id:"CAM-1",view:1}));
});

it("进入标定页且已有标定原图 → 自动显示原图并叠画角点；重新拍照后角点消失",async()=>{
 desktop.on=true;
 const calibrated={mmPerPx:0.05,rms:0.02,pattern:[11,8],square:5,ts:1,sampleId:"calib-sample",corners:[[10,10],[20,10],[30,10]]};
 vi.mocked(invoke).mockImplementation(async command=>{
  if(command==="vision_calib_info")return calibrated;
  if(command==="workspace_station_restore")return frozen("CAM-1",1,"calib-sample");
  if(command==="workspace_station_image")return frame;
  if(command==="workspace_station_capture")return frozen("CAM-1",1,"new-image","live");
  return null;
 });
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 expect(await screen.findByText(/CAM-1 · 标定原图/)).toBeVisible();
 await vi.waitFor(()=>expect(screen.getByLabelText("标定角点").querySelectorAll("circle")).toHaveLength(3));
 await userEvent.click(screen.getByRole("button",{name:"拍照"}));
 await screen.findByText(/^CAM-1 · (?!标定原图)/);
 expect(screen.queryByLabelText("标定角点")).toBeNull();
});

it("已显示一台相机的样本时切换相机 → 载入新相机的标定原图",async()=>{
 desktop.on=true;
 vi.mocked(cameraApi.rigConfig).mockResolvedValue([{...defaultCameraConfig,id:"CAM-1",name:"相机 1",serial:"SN1"},{...defaultCameraConfig,id:"CAM-2",name:"相机 2",serial:"SN2"}]);
 vi.mocked(invoke).mockImplementation(async(command,args)=>{
  if(command==="workspace_station_image")return frame;
  if(command!=="workspace_station_restore")return null;
  const {id,view}=args as {id:string;view:number};
  return frozen(id,view,"calib-"+id);
 });
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 expect(await screen.findByText(/CAM-1 · 标定原图/)).toBeVisible();
 await userEvent.click(screen.getByText("相机 2"));
 expect(await screen.findByText(/CAM-2 · 标定原图/)).toBeVisible();
});

it("标定原图载入未完成 → 拍照与导入不可用，载入结束后恢复",async()=>{
 desktop.on=true;
 let finish:(value:null)=>void=()=>{};
 vi.mocked(invoke).mockImplementation(async command=>command==="workspace_station_restore"?new Promise(resolve=>{finish=resolve;}):null);
 render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage/></MemoryRouter></SnackbarProvider>);
 await screen.findByRole("heading",{name:"标定板参数"});
 await vi.waitFor(()=>expect(invoke).toHaveBeenCalledWith("workspace_station_restore",expect.anything()));
 expect(screen.getByRole("button",{name:"拍照"})).toBeDisabled();
 expect(screen.getByLabelText("导入离线图",{selector:"input"})).toBeDisabled();
 finish(null);
 await vi.waitFor(()=>expect(screen.getByRole("button",{name:"拍照"})).toBeEnabled());
 expect(screen.getByLabelText("导入离线图",{selector:"input"})).toBeEnabled();
});

it("三目相机图 2 回查期间切换相机 → 新相机只恢复图 1，旧请求不再读图", async () => {
  desktop.on = true;
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([
    { ...defaultCameraConfig, id: "CAM-1", name: "相机 1", serial: "SN1", viewCount: 3 },
    { ...defaultCameraConfig, id: "CAM-2", name: "相机 2", serial: "SN2", viewCount: 3 },
  ]);
  vi.mocked(useRigStatus).mockReturnValue({ statuses: [status(1), { ...status(1), id: "CAM-2", cam: 1, name: "相机 2" }], lastFrame: {} });
  const previous = deferred<FrozenImage>();
  const ipc = vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "workspace_station_image") return frame;
    if (command !== "workspace_station_restore") return null;
    const { cam, view } = args as { cam: number; view: number };
    if (cam === 0 && view === 2) return previous.promise;
    if (cam === 1 && view === 1) return frozen("CAM-2", 1, "cam2-view1");
    return null;
  });
  const user = userEvent.setup();
  render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage /></MemoryRouter></SnackbarProvider>);
  const view = () => screen.getByRole("combobox", { name: "标定图像" });
  await user.selectOptions(await screen.findByRole("combobox", { name: "标定图像" }), "2");
  await vi.waitFor(() => expect(ipc).toHaveBeenCalledWith("workspace_station_restore", { cam: 0, id: "CAM-1", view: 2 }));
  await user.click(screen.getByText("相机 2"));
  expect(await screen.findByRole("img", { name: "cam2-view1" })).toBeVisible();
  expect(view()).toHaveValue("1");

  await act(async () => previous.resolve(frozen("CAM-1", 2, "cam1-view2")));
  expect(screen.getByRole("img", { name: "cam2-view1" })).toBeVisible();
  expect(screen.queryByRole("img", { name: "cam1-view2" })).not.toBeInTheDocument();
  expect(ipc.mock.calls.filter(([command, args]) => command === "workspace_station_restore" && (args as { cam?: number })?.cam === 1))
    .toEqual([["workspace_station_restore", { cam: 1, id: "CAM-2", view: 1 }]]);
  expect(ipc.mock.calls.filter(([command]) => command === "workspace_station_image"))
    .toEqual([["workspace_station_image", { cam: 1, id: "CAM-2", view: 1, imageId: "cam2-view1" }]]);
});

it.each([
  { mismatch: "相机", camera: "CAM-2", view: 1 },
  { mismatch: "视图", camera: "CAM-1", view: 2 },
])("当前拍照返回的$mismatch 不匹配 → 不读取这张原图，也不能标定", async ({ camera, view }) => {
  desktop.on = true;
  const ipc = vi.mocked(invoke).mockImplementation(async command => {
    if (command === "workspace_station_capture") return frozen(camera, view, "other-frame", "live");
    if (command === "workspace_station_image") return frame;
    return null;
  });
  render(<SnackbarProvider><MemoryRouter><FlyshotCalibrationPage /></MemoryRouter></SnackbarProvider>);
  const capture = () => screen.getByRole("button", { name: "拍照" });
  await screen.findByRole("heading", { name: "标定板参数" });
  await vi.waitFor(() => expect(capture()).toBeEnabled());
  await userEvent.click(capture());
  await vi.waitFor(() => expect(capture()).toBeEnabled());

  expect(ipc).toHaveBeenCalledWith("workspace_station_capture", { cam: 0, id: "CAM-1", view: 1 });
  expect(ipc.mock.calls.some(([command]) => command === "workspace_station_image")).toBe(false);
  expect(screen.queryByRole("img", { name: "other-frame" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "标定" })).toBeDisabled();
});

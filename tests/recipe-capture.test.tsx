import {render,screen,waitFor,within} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import {beforeEach,expect,it,vi} from "vitest";
import CapturePage from "../src/features/workspace/CapturePage";
import {workspaceApi} from "../src/features/workspace/api";
import {useWorkspace} from "../src/features/workspace/context";
import {cameraApi} from "../src/features/camera/api";
import {plcApi} from "../src/features/plc/api";
import {useCycle} from "../src/features/cycle/api";
import type {CaptureRound} from "../src/features/workspace/types";
import type {CameraConfig} from "../src/features/camera/types";
import {storageApi} from "../src/lib/storage";
import {snapshot,workspaceState} from "./fixtures";
vi.mock("../src/features/workspace/context",()=>({useWorkspace:vi.fn()}));
vi.mock("../src/features/workspace/api",()=>({workspaceApi:{setCameraImaging:vi.fn(),captureGet:vi.fn(),captureList:vi.fn(),captureStart:vi.fn(),captureStop:vi.fn(),adoptCapture:vi.fn(),captureSample:vi.fn(),captureImage:vi.fn()}}));
vi.mock("../src/features/camera/api",()=>({cameraApi:{rigStatus:vi.fn(),calibInfo:vi.fn(async()=>null)}}));
vi.mock("../src/lib/storage",()=>({storageApi:{preferences:vi.fn(async()=>({}))}}));
vi.mock("../src/features/plc/api",()=>({plcApi:{getStatus:vi.fn()}}));
vi.mock("../src/features/cycle/api",()=>({useCycle:vi.fn(),useSiteStatus:()=>null}));
let ws:ReturnType<typeof workspaceState>;
const round=(state:CaptureRound["state"]="complete"):CaptureRound=>({roundId:"r1",recipeId:"A",cameras:[{cameraId:"CAM-1",deviceSession:7,receivedCount:20}],plannedCount:20,state,createdAt:1,endedAt:2,error:null,simulated:true,frames:[]});
const shot=(i:number,cameras:string[]=["CAM-1"],width=1280)=>({shotId:"s"+i,ordinal:i,cameras:cameras.map(cameraId=>({cameraId,frameCounter:i,triggerCounter:i,originalPath:"",views:[{view:1,path:"",width,height:1024}]}))});
const device=(id:string,name:string):CameraConfig=>({id,name,source:"sim",viewCount:1,serial:id,triggerSource:"Line0",triggerActivation:"RisingEdge",triggerDelayUs:0,debouncerUs:0,strobe:false,chunk:true});
const show=(url="/recipe/capture")=>render(<MemoryRouter initialEntries={[url]}><CapturePage/></MemoryRouter>);
beforeEach(()=>{
 ws=workspaceState();ws.cameras=[{id:"CAM-1",name:"拼接设备",source:"sim",viewCount:3,serial:"DEVICE",triggerSource:"Line0",triggerActivation:"RisingEdge",triggerDelayUs:0,debouncerUs:0,strobe:false,chunk:true}];
 vi.mocked(useWorkspace).mockImplementation(()=>ws);vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[],measured:[]});
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"connected",message:"PLC已连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(null);vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
 vi.mocked(workspaceApi.adoptCapture).mockResolvedValue(ws.data!);
});

it("两台相机各改曝光与增益后点应用 → 按相机一次写入候选并标出下发失败的相机",async()=>{
 ws.cameras=[device("CAM-1","相机1"),device("CAM-2","相机2")];
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true},{id:"CAM-2",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(workspaceApi.setCameraImaging).mockResolvedValue({view:ws.data!,applyErrors:{"CAM-2":"相机2未连接，不能下发曝光与增益"}});show();
 const exposure1=await screen.findByRole("spinbutton",{name:"相机1 曝光时间（µs）"}),gain2=screen.getByRole("spinbutton",{name:"相机2 增益（dB）"}),apply=screen.getByRole("button",{name:"应用"});
 expect(exposure1).toHaveValue(60);expect(apply).toBeDisabled();
 await userEvent.clear(exposure1);await userEvent.type(exposure1,"120");await userEvent.clear(gain2);await userEvent.type(gain2,"2.5");
 await userEvent.click(apply);
 expect(workspaceApi.setCameraImaging).toHaveBeenCalledWith("A",7,{"CAM-1":{exposureUs:120,gainDb:6},"CAM-2":{exposureUs:60,gainDb:2.5}});
 expect(await screen.findByText("相机2 已写入候选配方，但没能下发到相机")).toBeVisible();
 expect(screen.queryByText("相机1 已写入候选配方，但没能下发到相机")).toBeNull();
});
it("三台相机中一台未就绪 → 仍默认勾选全部，开始接收禁用并点名未就绪相机",async()=>{
 ws.cameras=[device("CAM-1","相机1"),device("CAM-2","相机2"),device("CAM-3","相机3")];
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true},{id:"CAM-2",ready:false},{id:"CAM-3",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);show();
 expect(await screen.findByText("采集相机未就绪：相机2。")).toBeVisible();
 const group=screen.getByRole("group",{name:"采集相机"});
 for(const name of [/相机1/,/相机2/,/相机3/]) expect(within(group).getByRole("checkbox",{name})).toBeChecked();
 const start=screen.getByRole("button",{name:"开始接收"});
 expect(start).toBeDisabled();await userEvent.click(start);expect(workspaceApi.captureStart).not.toHaveBeenCalled();
});
it("从配置设备返回 → 保留手动取消的相机，新增相机自动勾选",async()=>{
 ws.cameras=[device("CAM-1","相机1"),device("CAM-2","相机2"),device("CAM-3","相机3")];
 vi.mocked(cameraApi.rigStatus).mockResolvedValue(["CAM-1","CAM-2","CAM-3"].map(id=>({id,ready:true})) as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(workspaceApi.captureStart).mockResolvedValue(round("waitingStart"));
 const captureReturn={recipeId:"A",search:"",cameraIds:["CAM-1"],excludedCameraIds:["CAM-2"],drain:1500,previewK:0};
 render(<MemoryRouter initialEntries={[{pathname:"/recipe/capture",state:{captureReturn}}]}><CapturePage/></MemoryRouter>);
 const group=await screen.findByRole("group",{name:"采集相机"});
 expect(within(group).getByRole("checkbox",{name:/相机2/})).not.toBeChecked();expect(within(group).getByRole("checkbox",{name:/相机3/})).toBeChecked();
 const start=screen.getByRole("button",{name:"开始接收"});
 await waitFor(()=>expect(start).toBeEnabled());await userEvent.click(start);
 expect(workspaceApi.captureStart).toHaveBeenCalledWith("A",["CAM-1","CAM-3"],1500);
});
it("多相机轮次切换相机 → 按相机读取同一拍照点的原图",async()=>{
 ws.cameras=[device("CAM-1","相机1"),device("CAM-2","相机2")];
 vi.mocked(workspaceApi.captureImage).mockResolvedValue({url:"data:image/png;base64,",width:1280,height:1024});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round(),cameras:[{cameraId:"CAM-1",deviceSession:7,receivedCount:1},{cameraId:"CAM-2",deviceSession:8,receivedCount:1}],plannedCount:1,frames:[shot(1,["CAM-1","CAM-2"])]});show();
 await screen.findByRole("img",{name:"拍照点 S001 · 相机1 · 完整拼接原图"});
 await userEvent.click(within(screen.getByRole("group",{name:"查看相机"})).getByRole("button",{name:"相机2"}));
 expect(await screen.findByRole("img",{name:"拍照点 S001 · 相机2 · 完整拼接原图"})).toBeVisible();
 expect(workspaceApi.captureImage).toHaveBeenLastCalledWith("r1",0,"CAM-2");
});
it("异常轮保留数量和原因，不能采用",async()=>{
 vi.mocked(workspaceApi.captureImage).mockReturnValue(new Promise(()=>{}));
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round("failed"),frames:Array.from({length:19},(_,i)=>shot(i+1)),error:"缺少一帧"});show();
 expect(await screen.findByText("缺少一帧")).toBeVisible();expect(screen.getByText("已接收 19 / 20 张")).toBeVisible();expect(screen.queryByRole("button",{name:"采用本轮图像"})).toBeNull();
});
it("示教采集轮次不允许同时作为独立验证样本",async()=>{
 ws.data!.workspace.captureId="r1";vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show("/recipe/capture?purpose=validation");
 const button=await screen.findByRole("button",{name:"保存为独立验证样本"});
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();expect(screen.getByText("需要独立于示教的采集轮次")).toBeVisible();
 await userEvent.click(button);expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});

it("工件进行中禁止采用完整采集轮次",async()=>{
 ws.data!.workspace.captureId="teaching-round";
 vi.mocked(useCycle).mockReturnValue({snapshot:snapshot("ACQUIRE"),logs:[],measured:[]});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show();
 const button=await screen.findByRole("button",{name:"采用本轮图像"});
 expect(button).toBeDisabled();await userEvent.click(button);expect(workspaceApi.adoptCapture).not.toHaveBeenCalled();expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});

it("个别拍照点尺寸与其余不同 → 汇总标出不一致且仅该点显示实际尺寸",async()=>{
 vi.mocked(workspaceApi.captureImage).mockReturnValue(new Promise(()=>{}));
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round(),frames:[shot(1),shot(2),shot(3,["CAM-1"],640)]});show();
 expect(await screen.findByText("尺寸不一致 · 多数为 1280×1024")).toBeVisible();
 expect(screen.getByRole("button",{name:/^S003/})).toHaveTextContent("640×1024");
 expect(screen.getByRole("button",{name:/^S001/})).not.toHaveTextContent("×");
});
it("切换拍照点且新图未读回 → 保留当前图像不回到读取占位",async()=>{
 vi.mocked(workspaceApi.captureImage).mockResolvedValueOnce({url:"data:image/png;base64,",width:1280,height:1024}).mockReturnValue(new Promise(()=>{}));
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round(),frames:[shot(1),shot(2)]});show();
 await screen.findByRole("img",{name:"拍照点 S001 · 完整拼接原图"});
 await userEvent.click(screen.getByRole("button",{name:/^S002/}));
 expect(screen.getByRole("img",{name:"拍照点 S001 · 完整拼接原图 · 正在读取 S002"})).toBeVisible();
 expect(screen.queryByText("正在读取原图…")).toBeNull();
});
it("采集完成后设备准备收起 → 收起栏仍可直接整圈重采",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());vi.mocked(workspaceApi.captureStart).mockResolvedValue(round("waitingStart"));show();
 const bar=await screen.findByRole("region",{name:"设备准备"});
 expect(screen.queryByRole("group",{name:"采集相机"})).toBeNull();
 await userEvent.click(within(bar).getByRole("button",{name:"整圈重采"}));
 expect(workspaceApi.captureStart).toHaveBeenCalledWith("A",["CAM-1"],1500);
});
it("PLC 未连接 →开始接收禁用并给出 PLC 通讯入口",async()=>{
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"disconnected",message:"PLC 未连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);show();
 expect(await screen.findByText(/PLC 未连接：整圈采集依赖/)).toBeVisible();
 expect(screen.getByRole("button",{name:"开始接收"})).toBeDisabled();
 expect(screen.getByRole("link",{name:"前往 PLC 通讯"})).toHaveAttribute("href","/plc");
});
it("PLC 重连中且后端带连接地址 → 检查卡只显示连接中，不展示重连详情",async()=>{
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"connecting",message:"正在连接 S7 192.168.0.10:102 (机架 0 插槽 1)"} as Awaited<ReturnType<typeof plcApi.getStatus>>);show();
 const checks=within(await screen.findByRole("list",{name:"开始前检查"}));
 expect(await checks.findByText("连接中")).toBeVisible();
 expect(screen.queryByText(/192\.168\.0\.10/)).toBeNull();
 expect(screen.getByRole("button",{name:"开始接收"})).toBeDisabled();
});
it("飞拍速度超过按曝光、标定当量、帧率和间距算出的上限 → 检查卡标出超速与最高速度",async()=>{
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",cam:0,ready:true,maxFps:50}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(cameraApi.calibInfo).mockResolvedValue({mmPerPx:.1});
 vi.mocked(storageApi.preferences).mockResolvedValue({flyshotSpeedMmS:1000,flyshotSpacingMm:130});show();
 const checks=within(await screen.findByRole("list",{name:"开始前检查"}));
 expect(await checks.findByText("超速 · 最高 833 mm/s")).toBeVisible();
});

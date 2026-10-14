import {render,screen,within} from "@testing-library/react";
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
import {snapshot,workspaceState} from "./fixtures";
vi.mock("../src/features/workspace/context",()=>({useWorkspace:vi.fn()}));
vi.mock("../src/features/workspace/api",()=>({workspaceApi:{captureGet:vi.fn(),captureList:vi.fn(),captureStart:vi.fn(),captureStop:vi.fn(),adoptCapture:vi.fn(),captureSample:vi.fn(),captureImage:vi.fn()}}));
vi.mock("../src/features/camera/api",()=>({cameraApi:{rigStatus:vi.fn()}}));
vi.mock("../src/features/plc/api",()=>({plcApi:{getStatus:vi.fn()}}));
vi.mock("../src/features/cycle/api",()=>({useCycle:vi.fn(),useSiteStatus:()=>null}));
let ws:ReturnType<typeof workspaceState>;
const round=(state:CaptureRound["state"]="complete"):CaptureRound=>({roundId:"r1",recipeId:"A",cameraId:"CAM-1",deviceSession:7,plannedCount:20,receivedCount:20,state,createdAt:1,endedAt:2,error:null,simulated:true,frames:[]});
const show=(url="/recipe/capture")=>render(<MemoryRouter initialEntries={[url]}><CapturePage/></MemoryRouter>);
beforeEach(()=>{
 ws=workspaceState();ws.cameras=[{id:"CAM-1",name:"拼接设备",source:"sim",viewCount:3,serial:"DEVICE",triggerSource:"Line0",triggerActivation:"RisingEdge",triggerDelayUs:0,debouncerUs:0,exposureUs:60,gainDb:0,strobe:false,chunk:true}];
 vi.mocked(useWorkspace).mockImplementation(()=>ws);vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[],measured:[]});
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"connected",message:"PLC已连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(null);vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
 vi.mocked(workspaceApi.adoptCapture).mockResolvedValue(ws.data!);
});

it("异常轮保留数量和原因，不能采用",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round("failed"),receivedCount:19,error:"缺少一帧"});show();
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
 const frame=(i:number,width:number)=>({shotId:"s"+i,ordinal:i,frameCounter:i,triggerCounter:i,originalPath:"",views:[{view:1,path:"",width,height:1024}]});
 vi.mocked(workspaceApi.captureImage).mockReturnValue(new Promise(()=>{}));
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round(),frames:[frame(1,1280),frame(2,1280),frame(3,640)]});show();
 expect(await screen.findByText("尺寸不一致 · 多数为 1280×1024")).toBeVisible();
 expect(screen.getByRole("button",{name:/^03/})).toHaveTextContent("640×1024");
 expect(screen.getByRole("button",{name:/^01/})).not.toHaveTextContent("×");
});
it("切换拍照点且新图未读回 → 保留当前图像不回到读取占位",async()=>{
 const frame=(i:number)=>({shotId:"s"+i,ordinal:i,frameCounter:i,triggerCounter:i,originalPath:"",views:[{view:1,path:"",width:1280,height:1024}]});
 vi.mocked(workspaceApi.captureImage).mockResolvedValueOnce({url:"data:image/png;base64,",width:1280,height:1024}).mockReturnValue(new Promise(()=>{}));
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round(),frames:[frame(1),frame(2)]});show();
 await screen.findByRole("img",{name:"拍照点 01 · 完整拼接原图"});
 await userEvent.click(screen.getByRole("button",{name:/^02/}));
 expect(screen.getByRole("img",{name:"拍照点 01 · 完整拼接原图 · 正在读取 02"})).toBeVisible();
 expect(screen.queryByText("正在读取原图…")).toBeNull();
});
it("采集完成后设备准备收起 → 收起栏仍可直接整圈重采",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());vi.mocked(workspaceApi.captureStart).mockResolvedValue(round("waitingStart"));show();
 const bar=await screen.findByRole("region",{name:"设备准备"});
 expect(screen.queryByRole("combobox",{name:"采集 device"})).toBeNull();
 await userEvent.click(within(bar).getByRole("button",{name:"开始接收 / 整圈重采"}));
 expect(workspaceApi.captureStart).toHaveBeenCalledWith("A","CAM-1",1500);
});
it("PLC 未连接 →开始接收禁用并给出 PLC 通讯入口",async()=>{
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"disconnected",message:"PLC 未连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);show();
 expect(await screen.findByText(/PLC 未连接：整圈采集依赖/)).toBeVisible();
 expect(screen.getByRole("button",{name:"开始接收 / 整圈重采"})).toBeDisabled();
 expect(screen.getByRole("link",{name:"前往 PLC 通讯"})).toHaveAttribute("href","/plc");
});

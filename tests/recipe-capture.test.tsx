import {render,screen} from "@testing-library/react";
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
vi.mock("../src/features/cycle/api",()=>({useCycle:vi.fn()}));
let ws:ReturnType<typeof workspaceState>;
const round=(state:CaptureRound["state"]="complete"):CaptureRound=>({roundId:"r1",recipeId:"A",cameraId:"CAM-1",deviceSession:7,plannedCount:20,receivedCount:20,state,createdAt:1,endedAt:2,error:null,simulated:true,frames:[]});
const show=(url="/recipe/capture")=>render(<MemoryRouter initialEntries={[url]}><CapturePage/></MemoryRouter>);
beforeEach(()=>{
 ws=workspaceState();ws.cameras=[{id:"CAM-1",name:"拼接设备",source:"sim",viewCount:3,serial:"DEVICE",acquisition:"triggered",fps:20,triggerSource:"Line0",triggerActivation:"RisingEdge",triggerDelayUs:0,debouncerUs:0,exposureUs:60,gainDb:0,strobe:false,chunk:true,replayDir:"",replayChannel:0}];
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
it("重采带入示教必须人工确认轨迹顺序",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show();const adopt=await screen.findByRole("button",{name:"采用本轮图像"});
 await userEvent.click(screen.getByRole("checkbox",{name:"带入旧中线与参数作为草稿"}));expect(adopt).toBeDisabled();
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认轨迹和触发顺序保持一致，拍照点对应正确"}));await userEvent.click(adopt);
 expect(workspaceApi.adoptCapture).toHaveBeenCalledWith("A",7,"r1",true,true);
});
it.each([null,"r1"])("示教来源 %s 不允许当前轮次作为独立验证样本",async captureId=>{
 ws.data!.workspace.captureId=captureId;vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show("/recipe/capture?purpose=validation");
 const button=await screen.findByRole("button",{name:"保存为独立正常验证样本"});
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();expect(screen.getByText("需要独立于示教的采集轮次")).toBeVisible();
 await userEvent.click(button);expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});

it.each(["/recipe/capture","/recipe/capture?purpose=validation"])("%s 工件进行中禁止采用完整采集轮次",async url=>{
 ws.data!.workspace.captureId="teaching-round";
 vi.mocked(useCycle).mockReturnValue({snapshot:snapshot("ACQUIRE"),logs:[],measured:[]});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show(url);
 const button=await screen.findByRole("button",{name:url.includes("validation")?"保存为独立正常验证样本":"采用本轮图像"});
 if(url.includes("validation"))await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();await userEvent.click(button);expect(workspaceApi.adoptCapture).not.toHaveBeenCalled();expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});

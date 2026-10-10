import {render,screen,waitFor} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {Link,MemoryRouter,Route,Routes,useLocation} from "react-router-dom";
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
 vi.mocked(workspaceApi.captureStart).mockImplementation(async()=>{const value=round("waitingStart");vi.mocked(workspaceApi.captureGet).mockResolvedValue(value);return value;});vi.mocked(workspaceApi.adoptCapture).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.captureSample).mockResolvedValue(ws.data!);
});
it("单个设备开始接收，收到计划数量仍等PLC结束而不允许采用",async()=>{
 show();const button=screen.getByRole("button",{name:"开始接收 / 整圈重采"});await waitFor(()=>expect(button).toBeEnabled());await userEvent.click(button);
 expect(workspaceApi.captureStart).toHaveBeenCalledWith("A","CAM-1",20,1500);expect(screen.getByText("等待现场启动")).toBeVisible();expect(screen.queryByRole("button",{name:"采用本轮图像"})).toBeNull();
});
it.each([199,200,30000,30001])("在途等待 %s ms 按后端范围限制",async milliseconds=>{
 show();const button=screen.getByRole("button",{name:"开始接收 / 整圈重采"});await waitFor(()=>expect(button).toBeEnabled());
 const field=screen.getByRole("spinbutton",{name:"在途图像等待时间"});await userEvent.clear(field);await userEvent.type(field,String(milliseconds));
 if(milliseconds<200||milliseconds>30000) {expect(button).toBeDisabled();await userEvent.click(button);expect(workspaceApi.captureStart).not.toHaveBeenCalled();}
 else {expect(button).toBeEnabled();await userEvent.click(button);expect(workspaceApi.captureStart).toHaveBeenCalledWith("A","CAM-1",20,milliseconds);}
});
it("异常轮保留数量和原因，不能采用",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue({...round("failed"),receivedCount:19,error:"缺少一帧"});show();
 expect(await screen.findByText("缺少一帧")).toBeVisible();expect(screen.getByText("实收 19 / 配置计划 20")).toBeVisible();expect(screen.queryByRole("button",{name:"采用本轮图像"})).toBeNull();
});
it("重采带入示教必须人工确认轨迹顺序",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show();const adopt=await screen.findByRole("button",{name:"采用本轮图像"});
 await userEvent.click(screen.getByRole("checkbox",{name:"带入旧中线与参数作为草稿"}));expect(adopt).toBeDisabled();
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认轨迹和触发顺序保持一致，拍照点对应正确"}));await userEvent.click(adopt);
 expect(workspaceApi.adoptCapture).toHaveBeenCalledWith("A",7,"r1",true,true);
});
it("独立实拍正常样本加入验证库，不替换示教图",async()=>{
 ws.data!.workspace.captureId="teaching-round";
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show("/recipe/capture?purpose=validation");
 const save=await screen.findByRole("button",{name:"保存为独立正常验证样本"});expect(save).toBeDisabled();
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));await userEvent.click(save);
 expect(workspaceApi.captureSample).toHaveBeenCalledWith("A",7,"r1","OK",true);expect(workspaceApi.adoptCapture).not.toHaveBeenCalled();
});
it.each([null,"r1"])("示教来源 %s 不允许当前轮次作为独立验证样本",async captureId=>{
 ws.data!.workspace.captureId=captureId;vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show("/recipe/capture?purpose=validation");
 const button=await screen.findByRole("button",{name:"保存为独立正常验证样本"});
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();expect(screen.getByText("需要独立于示教的采集轮次")).toBeVisible();
 await userEvent.click(button);expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});
it.each(["/recipe/capture","/recipe/capture?purpose=validation"])("%s 冻结候选时禁止重采和采用",async url=>{
 ws.data!.workspace.pending={doc:ws.doc!,bundleId:"bundle-v2",revision:7,baseRevision:"old",frames:[],overview:ws.data!.workspace.overview,validation:ws.data!.workspace.validation!};
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show(url);
 const button=await screen.findByRole("button",{name:url.includes("validation")?"保存为独立正常验证样本":"采用本轮图像"});
 expect(button).toBeDisabled();expect(screen.getByRole("button",{name:"开始接收 / 整圈重采"})).toBeDisabled();
 await userEvent.click(button);expect(workspaceApi.adoptCapture).not.toHaveBeenCalled();expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});
it.each(["/recipe/capture","/recipe/capture?purpose=validation"])("%s 工件进行中禁止采用完整采集轮次",async url=>{
 ws.data!.workspace.captureId="teaching-round";
 vi.mocked(useCycle).mockReturnValue({snapshot:snapshot("ACQUIRE"),logs:[],measured:[]});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show(url);
 const button=await screen.findByRole("button",{name:url.includes("validation")?"保存为独立正常验证样本":"采用本轮图像"});
 if(url.includes("validation"))await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();await userEvent.click(button);expect(workspaceApi.adoptCapture).not.toHaveBeenCalled();expect(workspaceApi.captureSample).not.toHaveBeenCalled();
});

it.each(["/recipe/capture","/recipe/capture?purpose=validation"])("%s 配置设备返回后保留采集参数和历史轮次",async url=>{
 ws.cameras.push({...ws.cameras[0],id:"CAM-2",name:"第二设备"});
 const historical={...round("failed"),roundId:"history-1"};
 vi.mocked(workspaceApi.captureList).mockResolvedValue([historical]);
 function DevicePage(){const location=useLocation();return <Link to={url} state={location.state}>返回采集</Link>;}
 render(<MemoryRouter initialEntries={[url]}><Routes><Route path="/recipe/capture" element={<CapturePage/>}/><Route path="/camera" element={<DevicePage/>}/></Routes></MemoryRouter>);
 await screen.findByRole("option",{name:/采集异常/});
 await userEvent.selectOptions(screen.getByRole("combobox",{name:"采集 device"}),"CAM-2");
 const planned=screen.getByRole("spinbutton",{name:"PLC 计划触发次数"}),drain=screen.getByRole("spinbutton",{name:"在途图像等待时间"});
 await userEvent.clear(planned);await userEvent.type(planned,"32");await userEvent.clear(drain);await userEvent.type(drain,"2600");
 await userEvent.selectOptions(screen.getByRole("combobox",{name:"采集历史"}),"history-1");
 await userEvent.click(screen.getByRole("link",{name:"配置设备"}));await userEvent.click(screen.getByRole("link",{name:"返回采集"}));
 expect(screen.getByRole("combobox",{name:"采集 device"})).toHaveValue("CAM-2");
 expect(screen.getByRole("spinbutton",{name:"PLC 计划触发次数"})).toHaveValue(32);expect(screen.getByRole("spinbutton",{name:"在途图像等待时间"})).toHaveValue(2600);
 expect(screen.getByRole("combobox",{name:"采集历史"})).toHaveValue("history-1");expect(ws.doc!.id).toBe("A");
 expect(screen.getByRole("heading",{name:url.includes("validation")?"实拍独立验证样本":"设备准备与整圈采集"})).toBeVisible();
 expect(workspaceApi.captureStart).not.toHaveBeenCalled();
});

it("返回时原设备已移除则选用仍存在的设备",async()=>{
 render(<MemoryRouter initialEntries={[{pathname:"/recipe/capture",state:{captureReturn:{recipeId:"A",search:"",cameraId:"removed",planned:32,drain:2600,previewK:0}}}]}><CapturePage/></MemoryRouter>);
 expect(screen.getByRole("combobox",{name:"采集 device"})).toHaveValue("CAM-1");
 const start=screen.getByRole("button",{name:"开始接收 / 整圈重采"});await waitFor(()=>expect(start).toBeEnabled());await userEvent.click(start);
 expect(workspaceApi.captureStart).toHaveBeenCalledWith("A","CAM-1",32,2600);
});

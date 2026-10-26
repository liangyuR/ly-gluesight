import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import CapturePage from "../src/features/workspace/CapturePage";
import {workspaceApi} from "../src/features/workspace/api";
import {useWorkspace} from "../src/features/workspace/context";
import {cameraApi} from "../src/features/camera/api";
import {plcApi} from "../src/features/plc/api";
import {useCycle,useSiteStatus,cycleApi} from "../src/features/cycle/api";
import type {CameraConfig} from "../src/features/camera/types";
import {burstRecipe,burstWorkspaceView,captureRoundV2,snapshot,workspaceState} from "./fixtures";
vi.mock("../src/features/workspace/context",()=>({useWorkspace:vi.fn()}));
vi.mock("../src/features/workspace/api",()=>({workspaceApi:{setCameraImaging:vi.fn(),captureGet:vi.fn(),capturePlcStatus:vi.fn(),captureList:vi.fn(),captureStart:vi.fn(),captureStop:vi.fn(),captureSample:vi.fn(),captureImage:vi.fn(),captureFrameImage:vi.fn(),calibrationAddRound:vi.fn(),calibrationRemoveRound:vi.fn()}}));
vi.mock("../src/features/camera/api",()=>({cameraApi:{rigStatus:vi.fn(),calibInfo:vi.fn(async()=>null)}}));
vi.mock("../src/lib/storage",()=>({storageApi:{preferences:vi.fn(async()=>({}))}}));
vi.mock("../src/features/plc/api",()=>({plcApi:{getStatus:vi.fn()}}));
vi.mock("../src/features/cycle/api",()=>({useCycle:vi.fn(),useSiteStatus:vi.fn(),cycleApi:{offlineCapture:vi.fn()}}));
let ws:ReturnType<typeof workspaceState>;
const picture={url:"data:image/png;base64,",width:1280,height:1024};

const device=(id:string,name:string):CameraConfig=>({id,name,source:"sim",viewCount:1,serial:id,triggerSource:"Line0",triggerActivation:"LevelHigh",frameRate:10,triggerDelayUs:0,debouncerUs:0,strobe:false,chunk:true});
const triple=()=>[device("CAM-1","相机1"),device("CAM-2","相机2"),device("CAM-3","相机3")];
const show=(url="/recipe/capture")=>render(<MemoryRouter initialEntries={[url]}><CapturePage/></MemoryRouter>);
/** 采集结果里的汇总标签（chip）的值 */
const chip=(label:string)=>screen.getByText(label,{selector:".wp-capture-chip small"}).nextElementSibling;
/** 一轮还没加入标定、也没示教过的新轮次（夹具里 round-1…round-10 已在标定轮次里） */
const freshRound=()=>captureRoundV2({recipeId:"A",roundId:"round-new"});
/** 段配方候选（三台相机、完整的 V2 轮次由各用例给） */
function useBurst(){
 ws=workspaceState(burstWorkspaceView(burstRecipe({id:"A"})));ws.cameras=triple();
 vi.mocked(useWorkspace).mockImplementation(()=>ws);
 vi.mocked(cameraApi.rigStatus).mockResolvedValue(["CAM-1","CAM-2","CAM-3"].map(id=>({id,ready:true})) as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
}
beforeEach(()=>{
 useBurst();
 vi.mocked(useSiteStatus).mockReturnValue(null);
 vi.mocked(workspaceApi.capturePlcStatus).mockResolvedValue({available:true,busy:false,roundId:null,phase:"idle",message:"示教接口就绪"});
 vi.mocked(useWorkspace).mockImplementation(()=>ws);vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[],measured:[]});
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"connected",message:"PLC已连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(null);vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
 vi.mocked(workspaceApi.captureImage).mockResolvedValue(picture);vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
 vi.mocked(workspaceApi.calibrationAddRound).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.calibrationRemoveRound).mockResolvedValue(ws.data!);
});

it("请求已发布且图片接收已准备 → 离线送圈可启动 PLC 接受与采集",async()=>{
 vi.mocked(useSiteStatus).mockReturnValue({mode:"offline",source:"synthetic",last:null,measurement:"simulated",port:null,running:false,continuous:false,parts:0,message:"",heartbeatStopped:false,sampleSet:null,nextPart:null,recent:[]});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A",state:"receiving"}));
 vi.mocked(workspaceApi.capturePlcStatus).mockResolvedValue({available:true,busy:true,roundId:1,phase:"requested",message:"等待 PLC 接受"});
 vi.mocked(cycleApi.offlineCapture).mockResolvedValue(undefined);
 show();
 const send=await screen.findByRole("button",{name:"离线产线送一圈"});
 await waitFor(()=>expect(send).toBeEnabled());
 await userEvent.click(send);
 expect(cycleApi.offlineCapture).toHaveBeenCalledWith(null);
});

it("三台相机中一台未就绪 → 仍默认勾选全部，开始整圈采集禁用并点名未就绪相机",async()=>{
 ws.cameras=triple();
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true},{id:"CAM-2",ready:false},{id:"CAM-3",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);show();
 expect(await screen.findByText("采集相机未就绪：相机2。")).toBeVisible();
 const group=screen.getByRole("group",{name:"采集相机"});
 for(const name of [/相机1/,/相机2/,/相机3/]) expect(within(group).getByRole("checkbox",{name})).toBeChecked();
 const start=screen.getByRole("button",{name:"开始整圈采集"});
 expect(start).toBeDisabled();
});

// 防的回归：采集页退回“按 PLC 计划数核对”——进度按 receivedCount/plannedCount 画、缺帧整轮判异常（规格 §5：示教轮次不核对；§3：缺帧只告警）
it("新轮次有缺帧和丢帧告警 → 显示帧组数、各相机帧数、不完整帧组与告警，PLC 预期总数只作参考，不出现数量核对",async()=>{
 useBurst();
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A",deviceTriggers:0,lossAlarms:[{camera:0,count:12,reason:{type:"cameras",other:1,otherCount:9}}]}));show();
 await screen.findByRole("heading",{name:"采集结果"});
 expect(chip("帧组")).toHaveTextContent("12 个");
 expect(chip("相机1")).toHaveTextContent("12 帧");
 expect(chip("相机2")).toHaveTextContent("11 帧");
 expect(chip("相机3")).toHaveTextContent("12 帧");
 expect(chip("不完整帧组")).toHaveTextContent("1 个");
 expect(chip("PLC 预期总数（仅供参考）")).toHaveTextContent("12");
 expect(screen.queryByText("PLC 累计触发（仅供参考）")).toBeNull();

 expect(within(screen.getByRole("group",{name:"不完整帧组"})).getByRole("button",{name:/^5/})).toHaveTextContent("缺 相机2");
 expect(screen.queryByText("数量核对")).toBeNull();expect(screen.queryByText(/ \/ 12 张/)).toBeNull();
 expect(screen.queryByText("本轮异常，需要重新采集")).toBeNull();
});
it("异常轮保留原因，不能用于示教或标定",async()=>{
 useBurst();
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A",state:"failed",error:"相机 CAM-2 断连",failure:{kind:"camera",cameraId:"CAM-2"}}));show();
 expect(await screen.findByText("相机 CAM-2 断连")).toBeVisible();
 for(const name of ["用于走廊示教","加入标定","加入标定（标准件）"]) expect(screen.queryByRole("button",{name})).toBeNull();
 expect(screen.queryByRole("link",{name:/用于走廊示教/})).toBeNull();
});

it("采集页整理已屏蔽轮次并切换预览 → 保留原帧组总数，旧图加载期间不能标记新边界", async () => {
 ws.data!.workspace.captureReview={rounds:{"round-1":{groupCount:12,cuts:[4,8],status:"draft",excludedGroups:[{seq:1,reason:"反光"}],segmentSignature:null,captureSignature:null,templateVersion:null}},template:null};
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A"}));
 vi.mocked(workspaceApi.captureFrameImage).mockImplementation(async (_round,_camera,seq)=>seq===1 ? picture : new Promise(()=>{}));
 show();
 expect(await screen.findByRole("heading",{name:"轮次整理"})).toBeVisible();
 expect(chip("帧组")).toHaveTextContent("12 个");
 expect(screen.getByText("原始 12 组 · 屏蔽 1 组 · 未屏蔽 11 组")).toBeVisible();
 expect(screen.getByText("当前帧组 1 已屏蔽")).toBeVisible();
 const mark=screen.getByRole("button",{name:"以当前帧结束第 1 段"});
 await waitFor(()=>expect(mark).toBeEnabled());
 await userEvent.click(screen.getByRole("button",{name:"下一帧组"}));
 expect(screen.getByRole("spinbutton",{name:"帧组序号"})).toHaveValue(2);
 expect(mark).toBeDisabled();
 expect(screen.getByRole("spinbutton",{name:"第 1 段结束帧组"})).toHaveValue(4);
});

it.each([
 ["段配方：本轮已有相机用作示教帧",()=>{ ws.data!.plan!.cameraTeaching[0].segments[0].frame.source="capture:round-1"; },true],
 ["段配方：本轮已加入多轮标定",()=>{ ws.data!.plan!.calibration!.rounds[0].roundId="round-1"; },true],
])("示教或标定用过的轮次不允许同时作为独立验证样本 · %s",async(_,setup,burst)=>{
 if(burst) useBurst(); setup();
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A"}));show("/recipe/capture?purpose=validation");
 const button=await screen.findByRole("button",{name:"保存为独立验证样本"});
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();expect(screen.getByText("需要独立于示教的采集轮次")).toBeVisible();
});

it("本轮已作为独立验证样本入库 → 不能再用于走廊示教或标定",async()=>{
 useBurst();
 ws.data!.workspace.sampleBank=[{id:"round-new",name:"独立采集",geometryTag:null,expected:"OK",createdAt:1,captureId:"round-new"}];
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(freshRound());show();
 expect(await screen.findByText(/本轮已作为独立验证样本加入样本库/)).toBeVisible();
 for(const name of ["用于走廊示教","加入标定","加入标定（标准件）"]) expect(screen.getByRole("button",{name})).toBeDisabled();
});

it("PLC 未连接 →开始整圈采集禁用并给出 PLC 通讯入口",async()=>{
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"disconnected",message:"PLC 未连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);show();
 expect(await screen.findByText(/PLC 未连接：整圈采集需要/)).toBeVisible();
 expect(screen.getByRole("button",{name:"开始整圈采集"})).toBeDisabled();
 expect(screen.getByRole("link",{name:"前往 PLC 通讯"})).toHaveAttribute("href","/plc");
});

it("Software 相机参与整圈采集 → 阻止启动并要求切换 Line0",async()=>{
 ws.cameras=[{...device("CAM-1","相机1"),source:"mvs",triggerSource:"Software"}];
 show();
 expect(await screen.findByText("请将采集相机切换为 Line0 高电平：相机1。")).toBeVisible();
 expect(screen.getByRole("button",{name:"开始整圈采集"})).toBeDisabled();
});


it.each([
 {phase:"unavailable" as const, busy:false, message:"示教 V1 点位尚未配置"},
 {phase:"releasing" as const, busy:true, message:"原图已保存，等待 PLC 释放本轮"},
])("示教接口 $phase → 即使图片完成也不能启动下一轮", async ({phase,busy,message}) => {
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(freshRound());
 vi.mocked(workspaceApi.capturePlcStatus).mockResolvedValue({available:false,busy,roundId:12,phase,message});
 show();
 expect((await screen.findAllByText(new RegExp(message)))[0]).toBeVisible();
 expect(screen.getByRole("button",{name:"开始整圈采集"})).toBeDisabled();
 if(busy) expect(screen.getByRole("spinbutton",{name:"相机1 曝光时间（µs）"})).toBeDisabled();
});

it("取消采集后原图轮次已失败 → 等待 PLC 确认期间仍不能重新开始", async () => {
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A",state:"receiving"}));
 vi.mocked(workspaceApi.capturePlcStatus).mockResolvedValue({available:false,busy:true,roundId:12,phase:"accepted",message:"PLC 已接受"});
 vi.mocked(workspaceApi.captureStop).mockResolvedValue(captureRoundV2({recipeId:"A",state:"failed",error:"操作员取消"}));
 show();
 await userEvent.click(await screen.findByRole("button",{name:"取消本轮"}));
 expect(await screen.findByRole("button",{name:"等待取消确认"})).toBeDisabled();
 expect(screen.getByRole("button",{name:"开始整圈采集"})).toBeDisabled();
});

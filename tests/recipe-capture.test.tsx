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
import {burstRecipe,burstWorkspaceView,captureRoundV2,snapshot,workspaceState} from "./fixtures";
vi.mock("../src/features/workspace/context",()=>({useWorkspace:vi.fn()}));
vi.mock("../src/features/workspace/api",()=>({workspaceApi:{setCameraImaging:vi.fn(),captureGet:vi.fn(),captureList:vi.fn(),captureStart:vi.fn(),captureStop:vi.fn(),adoptCapture:vi.fn(),captureSample:vi.fn(),captureImage:vi.fn(),captureFrameImage:vi.fn(),calibrationAddRound:vi.fn(),calibrationRemoveRound:vi.fn()}}));
vi.mock("../src/features/camera/api",()=>({cameraApi:{rigStatus:vi.fn(),calibInfo:vi.fn(async()=>null)}}));
vi.mock("../src/lib/storage",()=>({storageApi:{preferences:vi.fn(async()=>({}))}}));
vi.mock("../src/features/plc/api",()=>({plcApi:{getStatus:vi.fn()}}));
vi.mock("../src/features/cycle/api",()=>({useCycle:vi.fn(),useSiteStatus:()=>null}));
let ws:ReturnType<typeof workspaceState>;
const picture={url:"data:image/png;base64,",width:1280,height:1024};
/** 按拍照点采的旧轮次（旧拍照点配方）：每次触发一项 */
const round=(state:CaptureRound["state"]="complete"):CaptureRound=>({roundId:"r1",recipeId:"A",cameras:[{cameraId:"CAM-1",deviceSession:7,receivedCount:20}],plannedCount:20,state,createdAt:1,endedAt:2,error:null,simulated:true,frames:[]});
const shot=(i:number,cameras:string[]=["CAM-1"],width=1280)=>({shotId:"s"+i,ordinal:i,cameras:cameras.map(cameraId=>({cameraId,frameCounter:i,triggerCounter:i,originalPath:"",views:[{view:1,path:"",width,height:1024}]}))});
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
 vi.mocked(useWorkspace).mockImplementation(()=>ws);vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[],measured:[]});
 vi.mocked(plcApi.getStatus).mockResolvedValue({state:"connected",message:"PLC已连接"} as Awaited<ReturnType<typeof plcApi.getStatus>>);
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(null);vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
 vi.mocked(workspaceApi.captureImage).mockResolvedValue(picture);vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
 vi.mocked(workspaceApi.adoptCapture).mockResolvedValue(ws.data!);
 vi.mocked(workspaceApi.calibrationAddRound).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.calibrationRemoveRound).mockResolvedValue(ws.data!);
});

it("两台相机各改曝光与增益后点应用 → 按相机一次写入候选并标出下发失败的相机",async()=>{
 ws.cameras=[device("CAM-1","相机1"),device("CAM-2","相机2")];
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true},{id:"CAM-2",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(workspaceApi.setCameraImaging).mockResolvedValue({view:ws.data!,applyErrors:{"CAM-2":"相机2未连接，不能下发曝光与增益"}});show();
 const exposure1=await screen.findByRole("spinbutton",{name:"相机1 曝光时间（µs）"}),gain2=screen.getByRole("spinbutton",{name:"相机2 增益（dB）"}),apply=screen.getByRole("button",{name:"应用"});
 expect(exposure1).toHaveValue(60);expect(apply).toBeDisabled();
 await userEvent.clear(exposure1);await userEvent.type(exposure1,"120");await userEvent.clear(gain2);await userEvent.type(gain2,"2.5");
 await userEvent.click(apply);
 expect(workspaceApi.setCameraImaging).toHaveBeenCalledWith("A",3,{"CAM-1":{exposureUs:120,gainDb:6},"CAM-2":{exposureUs:60,gainDb:2.5}});
 expect(await screen.findByText(/相机2.*没能下发/)).toBeVisible();
 expect(screen.queryByText(/相机1.*没能下发/)).toBeNull();
});
it("三台相机中一台未就绪 → 仍默认勾选全部，开始接收禁用并点名未就绪相机",async()=>{
 ws.cameras=triple();
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",ready:true},{id:"CAM-2",ready:false},{id:"CAM-3",ready:true}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);show();
 expect(await screen.findByText("采集相机未就绪：相机2。")).toBeVisible();
 const group=screen.getByRole("group",{name:"采集相机"});
 for(const name of [/相机1/,/相机2/,/相机3/]) expect(within(group).getByRole("checkbox",{name})).toBeChecked();
 const start=screen.getByRole("button",{name:"开始接收"});
 expect(start).toBeDisabled();
});
it("从配置设备返回 → 保留手动取消的相机，新增相机自动勾选",async()=>{
 ws.cameras=triple();
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
it.each([
 ["新轮次（按帧号读图）",true,"round-1",1],
])("段配方多相机%s切换相机 → 按相机读取同一帧组的原图",async(_,v2,roundId,seq)=>{
 useBurst();
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(v2 ? captureRoundV2({recipeId:"A"}) : {...round(),cameras:[{cameraId:"CAM-1",deviceSession:7,receivedCount:1},{cameraId:"CAM-2",deviceSession:8,receivedCount:1}],frames:[shot(1,["CAM-1","CAM-2"])]});show();
 await screen.findByRole("img",{name:"帧组 1 · 相机1 · 原图"});
 await userEvent.click(within(screen.getByRole("group",{name:"查看相机"})).getByRole("button",{name:"相机2"}));
 expect(await screen.findByRole("img",{name:"帧组 1 · 相机2 · 原图"})).toBeVisible();
 if(v2) expect(workspaceApi.captureFrameImage).toHaveBeenLastCalledWith(roundId,"CAM-2",seq); else expect(workspaceApi.captureImage).toHaveBeenLastCalledWith(roundId,seq,"CAM-2");
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
 ["段配方：本轮已有相机用作示教帧",()=>{ ws.data!.plan!.cameraTeaching[0].frame!.source="capture:round-1"; },true],
 ["段配方：本轮已加入多轮标定",()=>{ ws.data!.plan!.calibration!.rounds[0].roundId="round-1"; },true],
])("示教或标定用过的轮次不允许同时作为独立验证样本 · %s",async(_,setup,burst)=>{
 if(burst) useBurst(); setup();
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(burst ? captureRoundV2({recipeId:"A"}) : {...round(),roundId:"round-1"});show("/recipe/capture?purpose=validation");
 const button=await screen.findByRole("button",{name:"保存为独立验证样本"});
 await userEvent.click(screen.getByRole("checkbox",{name:"已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致"}));
 expect(button).toBeDisabled();expect(screen.getByText("需要独立于示教的采集轮次")).toBeVisible();
});
it.each([
 ["独立样本模式下本轮独立于示教且未入库 → 主操作是保存为独立验证样本，收起栏采集新一圈样本降为次操作","teach","保存为独立验证样本","采集新一圈样本"],
 ["独立样本模式下本轮就是示教采集轮次 → 主操作是采集新一圈样本","r1","采集新一圈样本","保存为独立验证样本"],
])("%s",async(_,captureId,primary,secondary)=>{
 ws.data!.plan!.calibration!.rounds=[]; ws.data!.plan!.cameraTeaching[0].frame!.source="capture:"+captureId;vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());show("/recipe/capture?purpose=validation");
 await screen.findByRole("region",{name:"设备准备"});
 expect(screen.getByRole("button",{name:primary})).toHaveAttribute("data-variant","filled");
 expect(screen.getByRole("button",{name:secondary})).toHaveAttribute("data-variant","tonal");
});

it("本轮已有相机用作示教帧 → 标明已用于哪几台相机的走廊示教，入口仍是用于走廊示教",async()=>{
 useBurst();ws.data!.plan!.cameraTeaching[0].frame!.source="capture:round-1";
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A"}));show();
 expect(await screen.findByText("已用于 相机1 的走廊示教")).toBeVisible();
 expect(screen.getByRole("link",{name:"用于走廊示教"})).toHaveAttribute("href","/recipe/teach?round=round-1");
});
// 防的回归：加入标定时把普通轮次和标准件轮次弄反（标准件只用于胶偏参考 x_ref，不计入轮数）
it.each([
 ["加入标定",false,"已加入多轮标定"],
 ["加入标定（标准件）",true,"已加入标定（标准件轮次）"],
])("点“%s” → 以对应的轮次类型加入多轮标定",async(label,standard)=>{
 useBurst();
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(freshRound());show();
 await userEvent.click(await screen.findByRole("button",{name:label}));
 expect(workspaceApi.calibrationAddRound).toHaveBeenCalledWith("A",3,"round-new",standard);
});
it("本轮已在标定轮次里 → 显示已加入并只能移出，不能重复加入",async()=>{
 useBurst();ws.data!.plan!.calibration!.rounds[0]={...ws.data!.plan!.calibration!.rounds[0],standard:true};
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A"}));show();
 expect(await screen.findByText("已加入标定（标准件轮次）")).toBeVisible();
 expect(screen.queryByRole("button",{name:"加入标定"})).toBeNull();
 await userEvent.click(screen.getByRole("button",{name:"移出标定"}));
 expect(workspaceApi.calibrationRemoveRound).toHaveBeenCalledWith("A",3,"round-1");
});
it("本轮已作为独立验证样本入库 → 不能再用于走廊示教或标定",async()=>{
 useBurst();
 ws.data!.workspace.sampleBank=[{id:"round-new",name:"独立采集",geometryTag:null,expected:"OK",createdAt:1,captureId:"round-new"}];
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(freshRound());show();
 expect(await screen.findByText(/本轮已作为独立验证样本加入样本库/)).toBeVisible();
 for(const name of ["用于走廊示教","加入标定","加入标定（标准件）"]) expect(screen.getByRole("button",{name})).toBeDisabled();
});
it("工件进行中 → 禁止把本轮加入标定",async()=>{
 useBurst();vi.mocked(useCycle).mockReturnValue({snapshot:snapshot("ACQUIRE"),logs:[],measured:[]});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(freshRound());show();
 expect(await screen.findByRole("button",{name:"加入标定"})).toBeDisabled();
});

it("切换帧组且新图未读回 → 保留当前图像不回到读取占位",async()=>{
 useBurst();
 vi.mocked(workspaceApi.captureFrameImage).mockResolvedValueOnce(picture).mockReturnValue(new Promise(()=>{}));
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({recipeId:"A"}));show();
 await screen.findByRole("img",{name:"帧组 1 · 相机1 · 原图"});
 await userEvent.click(screen.getByRole("button",{name:"下一帧组"}));
 expect(screen.getByRole("img",{name:"帧组 1 · 相机1 · 原图 · 正在读取帧组 2"})).toBeVisible();
 expect(screen.queryByText("正在读取原图…")).toBeNull();
});
it("采集完成后设备准备收起 → 收起栏仍可直接整圈重采",async()=>{
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(round());vi.mocked(workspaceApi.captureStart).mockResolvedValue(round("waitingStart"));show();
 const bar=await screen.findByRole("region",{name:"设备准备"});
 expect(screen.queryByRole("group",{name:"采集相机"})).toBeNull();
 await userEvent.click(within(bar).getByRole("button",{name:"整圈重采"}));
 expect(workspaceApi.captureStart).toHaveBeenCalledWith("A",["CAM-1","CAM-2","CAM-3"],1500);
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
 ws.cameras=[device("CAM-1","相机1")];
 vi.mocked(cameraApi.rigStatus).mockResolvedValue([{id:"CAM-1",cam:0,ready:true,maxFps:50,configuredFrameRate:50}] as Awaited<ReturnType<typeof cameraApi.rigStatus>>);
 vi.mocked(cameraApi.calibInfo).mockResolvedValue({mmPerPx:.1});
 vi.mocked(storageApi.preferences).mockResolvedValue({flyshotSpeedMmS:1000,flyshotSpacingMm:130});show();
 const checks=within(await screen.findByRole("list",{name:"开始前检查"}));
 expect(await checks.findByText("超速 · 最高 833 mm/s")).toBeVisible();
});

it("Software 相机参与整圈采集 → 阻止启动并要求切换 Line0",async()=>{
 ws.cameras=[{...device("CAM-1","相机1"),source:"mvs",triggerSource:"Software"}];
 show();
 expect(await screen.findByText("请将采集相机切换为 Line0 高电平：相机1。")).toBeVisible();
 expect(screen.getByRole("button",{name:"开始接收"})).toBeDisabled();
});

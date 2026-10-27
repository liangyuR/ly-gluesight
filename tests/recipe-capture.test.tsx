import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import CapturePage from "../src/features/workspace/CapturePage";
import {workspaceApi} from "../src/features/workspace/api";
import {useWorkspace} from "../src/features/workspace/context";
import {useCycle} from "../src/features/cycle/api";
import type {CameraConfig} from "../src/features/camera/types";
import {burstRecipe,burstWorkspaceView,captureRoundV2,snapshot,workspaceState} from "./fixtures";
vi.mock("../src/features/workspace/context",()=>({useWorkspace:vi.fn()}));
vi.mock("../src/features/workspace/api",()=>({workspaceApi:{captureGet:vi.fn(),captureList:vi.fn(),captureFrameImage:vi.fn(),calibrationAddRound:vi.fn(),calibrationRemoveRound:vi.fn(),historyCandidates:vi.fn(),historyImport:vi.fn()}}));
vi.mock("../src/features/cycle/api",()=>({useCycle:vi.fn()}));
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
}
beforeEach(()=>{
 useBurst();
 vi.mocked(useWorkspace).mockImplementation(()=>ws);vi.mocked(useCycle).mockReturnValue({snapshot:snapshot(),logs:[]});
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(null);vi.mocked(workspaceApi.captureList).mockResolvedValue([]);
 vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([]);
 vi.mocked(workspaceApi.calibrationAddRound).mockResolvedValue(ws.data!);vi.mocked(workspaceApi.calibrationRemoveRound).mockResolvedValue(ws.data!);
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
 expect(screen.queryByText("本轮异常，不能用于示教或标定")).toBeNull();
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

it("本轮已作为独立验证样本入库 → 不能再用于走廊示教或标定",async()=>{
 useBurst();
 ws.data!.workspace.sampleBank=[{id:"round-new",name:"独立采集",geometryTag:null,expected:"OK",createdAt:1,captureId:"round-new"}];
 vi.mocked(workspaceApi.captureGet).mockResolvedValue(freshRound());show();
 expect(await screen.findByText(/本轮已作为独立验证样本加入样本库/)).toBeVisible();
 for(const name of ["用于走廊示教","加入标定","加入标定（标准件）"]) expect(screen.getByRole("button",{name})).toBeDisabled();
});

// 防的回归（在线投运 5.2、第七节）：不可用的历史件也能勾选导入；标准件标记没随导入提交；导入后没选中新轮次，示教与标定找不到它
it("从历史选用 → 不可用件不能勾选并列出原因，勾选可用件标为标准件后批量导入并选中新轮次",async()=>{
 const row=(recordId:number,sn:string,reasons:string[])=>({recordId,cycleId:"c-"+recordId,ts:1,sn,mode:"maintenanceCollect" as const,groupCount:12,
  cameras:["CAM-1","CAM-2","CAM-3"].map(cameraId=>({cameraId,frames:reasons.length ? 11 : 12})),complete:!reasons.length,reasons,warnings:[],durationMs:null,measuredFps:null,setFps:null,importedRound:null});
 vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([row(1,"SN-1",[]),row(2,"SN-2",["缺帧 1 帧"])]);
 vi.mocked(workspaceApi.historyImport).mockResolvedValue(["capture-9-0"]);
 show();
 expect(await screen.findByText("缺帧 1 帧")).toBeVisible();
 expect(screen.getByRole("checkbox",{name:"选用 SN SN-2"})).toBeDisabled();
 expect(screen.getByRole("checkbox",{name:"SN SN-1 是标准件"})).toBeDisabled();
 await userEvent.click(screen.getByRole("checkbox",{name:"选用 SN SN-1"}));
 await userEvent.click(screen.getByRole("checkbox",{name:"SN SN-1 是标准件"}));
 await userEvent.click(screen.getByRole("button",{name:"导入选中的 1 件"}));
 expect(workspaceApi.historyImport).toHaveBeenCalledWith("A",ws.data!.workspace.revision,[{recordId:1,standard:true}]);
 await waitFor(()=>expect(workspaceApi.captureGet).toHaveBeenLastCalledWith("capture-9-0"));
});

import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import OperationGuidePage from "../src/pages/OperationGuidePage";
import { useRigStatus } from "../src/features/camera/api";
import { usePlcStatus } from "../src/features/plc/api";
import { useWorkspace } from "../src/features/workspace/context";
import type { CameraStatus } from "../src/features/camera/types";
import { workspaceState } from "./fixtures";

vi.mock("../src/features/camera/api",async importOriginal=>({...await importOriginal<typeof import("../src/features/camera/api")>(),useRigStatus:vi.fn()}));
vi.mock("../src/features/plc/api",async importOriginal=>({...await importOriginal<typeof import("../src/features/plc/api")>(),usePlcStatus:vi.fn()}));
vi.mock("../src/features/workspace/context",()=>({useWorkspace:vi.fn()}));
function location(){return <output aria-label="页面地址">{useLocation().pathname}</output>;}
function show(){const Location=location;return render(<MemoryRouter initialEntries={["/guide"]}><OperationGuidePage/><Location/></MemoryRouter>);}
const journeys:[string,[string,string][]][]=[
  ["首次建站",[["设备与采集","/camera"],["PLC 通讯","/plc"],["工位标定","/camera/calibration"],["配方库","/recipe"]]],
  ["日常生产",[["在线检测","/inspect"],["历史记录","/history"]]],
  ["缺陷排查",[["历史记录","/history"],["历史复测","/history/retest"],["单帧示教","/recipe/teach"]]],
  ["配置变更",[["拍照规划","/recipe/geometry"],["单帧示教","/recipe/teach"],["工件总览","/recipe/overview"],["验证与发布","/recipe/validation"]]],
];
beforeEach(()=>{
  vi.mocked(useWorkspace).mockReturnValue(workspaceState());
  vi.mocked(useRigStatus).mockReturnValue({statuses:[],lastFrame:{}});
  vi.mocked(usePlcStatus).mockReturnValue(null);
});
function camera(cam:number,ready:boolean):CameraStatus{return {cam,id:`cam${cam+1}`,name:`相机${cam+1}`,source:"sim",acquisition:"freeRun",ready,message:"test",device:null,sdkVersion:null,
  frames:0,fps:0,maxFps:null,lostPackets:0,droppedFrames:0,warnings:[]};}

describe("操作流程的基本入口",()=>{
  it.each(journeys)("%s 包含全部对应步骤，开始链接进入首步",(title,links)=>{
    show();const panel=within(screen.getByRole("heading",{name:title}).closest("section")!);
    for(const [label,path] of links){const link=panel.getByRole("link",{name:label});expect(link).toHaveAttribute("href",path);fireEvent.click(link);expect(screen.getByLabelText("页面地址")).toHaveTextContent(path);}
    fireEvent.click(panel.getByRole("link",{name:`开始${title}`}));expect(screen.getByLabelText("页面地址")).toHaveTextContent(links[0][1]);
  });

  it.each([["配置采集设备","/camera"],["检查通讯与点位","/plc"],["打开配方库","/recipe"]])("状态卡 %s 跳到 %s",(label,path)=>{
    show();fireEvent.click(screen.getByRole("link",{name:label}));expect(screen.getByLabelText("页面地址")).toHaveTextContent(path);
  });

  it("设备及生产配方状态反映真实 hook 数据，不把零相机当就绪",()=>{
    const page=show();expect(screen.getByText("0/0 就绪")).toHaveClass("warn");expect(screen.getByText("未连接")).toHaveClass("warn");
    vi.mocked(useRigStatus).mockReturnValue({statuses:[camera(0,true),camera(1,false)],lastFrame:{}});
    vi.mocked(usePlcStatus).mockReturnValue({state:"connected",message:"已连接",since:1,lastPoll:1,cycleMs:1,pollCount:1,errorCount:0});
    page.rerender(<MemoryRouter><OperationGuidePage/></MemoryRouter>);
    expect(screen.getByText("1/2 就绪")).toHaveClass("warn");expect(screen.getByText("已连接")).toHaveClass("ok");
    vi.mocked(useRigStatus).mockReturnValue({statuses:[camera(0,true),camera(1,true)],lastFrame:{}});
    page.rerender(<MemoryRouter><OperationGuidePage/></MemoryRouter>);expect(screen.getByText("2/2 就绪")).toHaveClass("ok");
    expect(screen.getByText("1 个生产配方")).toBeVisible();expect(screen.getByText(/1 个候选工作区/)).toBeVisible();
  });
});

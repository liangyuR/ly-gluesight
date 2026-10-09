import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import DryRunPanel from "../src/features/camera/components/DryRunPanel";
import { cameraApi } from "../src/features/camera/api";
import type { DryFrame } from "../src/features/camera/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/camera/api", () => ({ cameraApi: { dryRunStart: vi.fn(), dryRunStop: vi.fn(), dryRunGet: vi.fn() } }));
const frames: DryFrame[] = [
  { cam: 0, tMs: 0, frameCounter: 10, triggerCounter: 20, lostPackets: 0 },
  { cam: 0, tMs: 200, frameCounter: 11, triggerCounter: 21, lostPackets: 0 },
  { cam: 1, tMs: 210, frameCounter: 100, triggerCounter: 100, lostPackets: 0 },
];
beforeEach(() => {
  vi.mocked(cameraApi.dryRunStart).mockResolvedValue(undefined);
  vi.mocked(cameraApi.dryRunStop).mockResolvedValue(structuredClone(frames));
  vi.mocked(cameraApi.dryRunGet).mockResolvedValue([]);
});

describe("触发空跑判定", () => {
  it.each(["good", "missing", "frame-jump", "trigger-mismatch", "trigger-middle", "lost-packets", "too-fast", "duplicate-time", "negative-time", "empty"])("%s 空跑结论与当前相机帧一致", async condition => {
    const data = structuredClone(frames);
    if (condition === "missing") data.splice(1, 1);
    if (condition === "frame-jump") data[1].frameCounter = 12;
    if (condition === "trigger-mismatch") data[1].triggerCounter = 22;
    if (condition === "lost-packets") data[1].lostPackets = 1;
    if (condition === "too-fast") data[1].tMs = 90;
    if(condition==="duplicate-time")data[1].tMs=0;
    if(condition==="negative-time")data[1].tMs=-1;
    if(condition==="empty")data.length=0;
    if(condition==="trigger-middle"){data.splice(2,0,{...data[1],frameCounter:12,triggerCounter:22,tMs:400});data[1].triggerCounter=20;}
    vi.mocked(cameraApi.dryRunStop).mockResolvedValue(data);
    render(<DryRunPanel cam={0} frameMs={100} />);
    fireEvent.change(screen.getByRole("spinbutton", { name: "计划 N" }), { target: { value: condition==="trigger-middle"?"3":"2" } });
    await userEvent.click(screen.getByRole("button", { name: "开始空跑" }));
    expect(screen.getByText("等待触发…")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "结束" }));
    expect(await screen.findByText(condition === "good" ? "通过" : "不通过")).toBeVisible();
    if (condition === "good") expect(screen.getByText("200 ms")).toBeVisible();
    expect(screen.getByRole("button", { name: "开始空跑" })).toBeEnabled();
  });

  it("启动失败显示错误，保留开始按钮以便重试", async () => {
    vi.mocked(cameraApi.dryRunStart).mockRejectedValueOnce(new Error("未连接相机"));
    render(<DryRunPanel cam={0} frameMs={null} />);
    await userEvent.click(screen.getByRole("button", { name: "开始空跑" }));
    expect(await screen.findByText("Error: 未连接相机")).toBeVisible();
    expect(screen.getByRole("button", { name: "开始空跑" })).toBeEnabled();
  });

  it("结束失败保留运行状态以便重试，请求中禁止重复操作", async () => {
    const request = deferred<DryFrame[]>(); vi.mocked(cameraApi.dryRunStop).mockReturnValueOnce(request.promise);
    render(<DryRunPanel cam={0} frameMs={100}/>); await userEvent.click(screen.getByRole("button", { name: "开始空跑" }));
    const stop = screen.getByRole("button", { name: "结束" }); fireEvent.click(stop); fireEvent.click(stop);
    expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1); expect(stop).toBeDisabled();
    await act(async () => request.reject(new Error("停止失败"))); expect(await screen.findByText("Error: 停止失败")).toBeVisible();
    expect(stop).toBeEnabled(); await userEvent.click(stop); expect(await screen.findByRole("button", { name: "开始空跑" })).toBeEnabled();
  });

  it.each(["0","-1","1.5","","9007199254740992"])("计划 %s 无效时禁止开始，修正后恢复",async value=>{
    render(<DryRunPanel cam={0} frameMs={100}/>);const input=screen.getByRole("spinbutton",{name:"计划 N"});fireEvent.change(input,{target:{value}});
    expect(screen.getByRole("button",{name:"开始空跑"})).toBeDisabled();await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));expect(cameraApi.dryRunStart).not.toHaveBeenCalled();
    fireEvent.change(input,{target:{value:"2"}});expect(screen.getByRole("button",{name:"开始空跑"})).toBeEnabled();
  });

  it("启动等待时锁定计划与重复提交，结束之前不给出结论",async()=>{
    const request=deferred<void>();vi.mocked(cameraApi.dryRunStart).mockReturnValueOnce(request.promise);
    render(<DryRunPanel cam={0} frameMs={100}/>);const button=screen.getByRole("button",{name:"开始空跑"});fireEvent.click(button);fireEvent.click(button);
    expect(cameraApi.dryRunStart).toHaveBeenCalledTimes(1);expect(button).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"计划 N"})).toBeDisabled();
    await act(async()=>request.resolve());expect(screen.getByRole("button",{name:"结束"})).toBeEnabled();expect(screen.queryByText("通过")).not.toBeInTheDocument();
  });

  it("当前相机单帧计划可完成，重设计划清掉旧结论再运行",async()=>{
    render(<DryRunPanel cam={1} frameMs={null}/>);fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"1"}});
    await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));await userEvent.click(screen.getByRole("button",{name:"结束"}));expect(await screen.findByText("通过")).toBeVisible();
    expect(screen.getByText(/单帧时间未知/)).toBeVisible();fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"2"}});expect(screen.queryByText("通过")).not.toBeInTheDocument();
  });

  it("少帧与多帧不能靠改计划得到旧的通过结论",async()=>{
    vi.mocked(cameraApi.dryRunStop).mockResolvedValue([...frames,{...frames[1],frameCounter:12,triggerCounter:22,tMs:400}]);
    render(<DryRunPanel cam={0} frameMs={100}/>);fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"2"}});
    await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));await userEvent.click(screen.getByRole("button",{name:"结束"}));expect(await screen.findByText("不通过")).toBeVisible();
    fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"3"}});expect(screen.queryByText("通过")).not.toBeInTheDocument();expect(screen.queryByText("不通过")).not.toBeInTheDocument();
  });

  it("计数读取串行，迟到轮询不能覆盖停止后的最终结果",async()=>{
    vi.useFakeTimers();try{
      const request=deferred<DryFrame[]|null>();vi.mocked(cameraApi.dryRunGet).mockReturnValue(request.promise);
      render(<DryRunPanel cam={0} frameMs={100}/>);fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"2"}});
      await act(async()=>fireEvent.click(screen.getByRole("button",{name:"开始空跑"})));await act(()=>vi.advanceTimersByTimeAsync(1200));expect(cameraApi.dryRunGet).toHaveBeenCalledTimes(1);
      await act(async()=>fireEvent.click(screen.getByRole("button",{name:"结束"})));expect(screen.getByText("通过")).toBeVisible();
      await act(async()=>request.resolve([]));expect(screen.getByText("通过")).toBeVisible();expect(screen.getByText("收到帧")).toHaveTextContent(/收到帧\s*2/);
    }finally{vi.useRealTimers();}
  });

  it("轮询失败后保留运行，下一次成功清除错误并恢复计数",async()=>{
    vi.useFakeTimers();try{
      vi.mocked(cameraApi.dryRunGet).mockRejectedValueOnce(new Error("计数不可读")).mockResolvedValueOnce(frames);
      render(<DryRunPanel cam={0} frameMs={100}/>);await act(async()=>fireEvent.click(screen.getByRole("button",{name:"开始空跑"})));
      await act(()=>vi.advanceTimersByTimeAsync(300));expect(screen.getByText("Error: 计数不可读")).toBeVisible();expect(screen.getByRole("button",{name:"结束"})).toBeEnabled();
      await act(()=>vi.advanceTimersByTimeAsync(300));expect(screen.queryByText("Error: 计数不可读")).not.toBeInTheDocument();expect(screen.getByText("收到帧")).toHaveTextContent(/收到帧\s*2/);
    }finally{vi.useRealTimers();}
  });

  it("后端提前结束不会把最后一次轮询当作最终通过结果",async()=>{
    vi.useFakeTimers();try{
      vi.mocked(cameraApi.dryRunGet).mockResolvedValueOnce(frames).mockResolvedValueOnce(null);
      render(<DryRunPanel cam={0} frameMs={100}/>);fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"2"}});await act(async()=>fireEvent.click(screen.getByRole("button",{name:"开始空跑"})));
      await act(()=>vi.advanceTimersByTimeAsync(600));expect(screen.getByText("空跑已结束，未返回最终帧数据。请重新开始。")).toBeVisible();expect(screen.queryByText("通过")).not.toBeInTheDocument();expect(screen.getByRole("button",{name:"开始空跑"})).toBeEnabled();
    }finally{vi.useRealTimers();}
  });

  it("卸载时停止自己启动的空跑，避免后续检测帧继续被空跑接管",async()=>{
    const page=render(<DryRunPanel cam={0} frameMs={100}/>);await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));page.unmount();await waitFor(()=>expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1));
  });

  it("开始尚未完成就离开页面，成功后仍结束自己的空跑",async()=>{
    const request=deferred<void>();vi.mocked(cameraApi.dryRunStart).mockReturnValue(request.promise);const page=render(<DryRunPanel cam={0} frameMs={100}/>);
    await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));page.unmount();expect(cameraApi.dryRunStop).not.toHaveBeenCalled();
    await act(async()=>request.resolve());expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1);
  });

  it("结束请求处理中卸载不重复停止，失败才做一次离开清理",async()=>{
    const request=deferred<DryFrame[]>();vi.mocked(cameraApi.dryRunStop).mockReturnValueOnce(request.promise);
    const page=render(<DryRunPanel cam={0} frameMs={100}/>);await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));await userEvent.click(screen.getByRole("button",{name:"结束"}));page.unmount();expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1);
    await act(async()=>request.reject(new Error("暂时失败")));expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(2);
  });

  it("切换工位停止旧空跑并清空显示，不将其他相机帧作为本工位结论",async()=>{
    const page=render(<DryRunPanel cam={0} frameMs={100}/>);await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));page.rerender(<DryRunPanel cam={1} frameMs={100}/>);
    await waitFor(()=>expect(screen.getByRole("button",{name:"开始空跑"})).toBeEnabled());expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1);expect(screen.getByText("收到帧")).toHaveTextContent(/收到帧\s*0/);expect(screen.queryByText("通过")).not.toBeInTheDocument();
  });

  it("长空跑使用全部帧判定并限制图表数量，避免产生负柱宽",async()=>{
    const all=Array.from({length:100},(_,i)=>({cam:0,tMs:i*10,frameCounter:i+1,triggerCounter:i+1,lostPackets:0}));vi.mocked(cameraApi.dryRunStop).mockResolvedValue(all);
    render(<DryRunPanel cam={0} frameMs={5}/>);fireEvent.change(screen.getByRole("spinbutton",{name:"计划 N"}),{target:{value:"100"}});await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));await userEvent.click(screen.getByRole("button",{name:"结束"}));
    expect(await screen.findByText("通过")).toBeVisible();expect(screen.getByText(/图表显示最近 64 个间隔/)).toBeVisible();const bars=document.querySelectorAll("svg rect");expect(bars).toHaveLength(64);
    bars.forEach(bar=>expect(Number(bar.getAttribute("width"))).toBeGreaterThan(0));
  });
});

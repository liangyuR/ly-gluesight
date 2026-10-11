import { act, fireEvent, render, screen } from "@testing-library/react";
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
  it("结束失败保留运行状态以便重试，请求中禁止重复操作", async () => {
    const request = deferred<DryFrame[]>(); vi.mocked(cameraApi.dryRunStop).mockReturnValueOnce(request.promise);
    render(<DryRunPanel cam={0} frameMs={100}/>); await userEvent.click(screen.getByRole("button", { name: "开始空跑" }));
    const stop = screen.getByRole("button", { name: "结束" }); fireEvent.click(stop); fireEvent.click(stop);
    expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1); expect(stop).toBeDisabled();
    await act(async () => request.reject(new Error("停止失败"))); expect(await screen.findByText("Error: 停止失败")).toBeVisible();
    expect(stop).toBeEnabled(); await userEvent.click(stop); expect(await screen.findByRole("button", { name: "开始空跑" })).toBeEnabled();
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

  it("开始尚未完成就离开页面，成功后仍结束自己的空跑",async()=>{
    const request=deferred<void>();vi.mocked(cameraApi.dryRunStart).mockReturnValue(request.promise);const page=render(<DryRunPanel cam={0} frameMs={100}/>);
    await userEvent.click(screen.getByRole("button",{name:"开始空跑"}));page.unmount();expect(cameraApi.dryRunStop).not.toHaveBeenCalled();
    await act(async()=>request.resolve());expect(cameraApi.dryRunStop).toHaveBeenCalledTimes(1);
  });

});

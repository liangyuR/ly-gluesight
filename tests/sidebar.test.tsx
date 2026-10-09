import { act, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "../src/layout/Sidebar";
import { getEngineStatus, type EngineStatus } from "../src/lib/api";
import { usePlcStatus } from "../src/features/plc/api";
import { deferred } from "./fixtures";

vi.mock("../src/lib/api",async importOriginal=>({...await importOriginal<typeof import("../src/lib/api")>(),getEngineStatus:vi.fn()}));
vi.mock("../src/features/plc/api",async importOriginal=>({...await importOriginal<typeof import("../src/features/plc/api")>(),usePlcStatus:vi.fn()}));
const engine:EngineStatus={backend:"LyFlow",ready:true,message:"已就绪"};
beforeEach(()=>{vi.mocked(getEngineStatus).mockResolvedValue(engine);vi.mocked(usePlcStatus).mockReturnValue(null);});

describe("侧栏状态刷新",()=>{
  it.each(["resolve","reject"])("旧状态请求 %s 不覆盖最新状态，卸载后停止刷新",async finish=>{
    vi.useFakeTimers();try{
      const old=deferred<EngineStatus>();vi.mocked(getEngineStatus).mockReturnValueOnce(old.promise);
      const page=render(<MemoryRouter><Sidebar/></MemoryRouter>);
      await act(()=>vi.advanceTimersByTimeAsync(3000));expect(screen.getByText("LyFlow · 就绪")).toBeVisible();
      await act(async()=>finish==="resolve"?old.resolve({backend:"Old",ready:false,message:"旧状态"}):old.reject(new Error("旧状态失败")));
      expect(screen.getByText("LyFlow · 就绪")).toBeVisible();expect(screen.queryByText(/Old ·/)).not.toBeInTheDocument();
      page.unmount();const count=vi.mocked(getEngineStatus).mock.calls.length;await act(()=>vi.advanceTimersByTimeAsync(9000));expect(getEngineStatus).toHaveBeenCalledTimes(count);
    }finally{vi.useRealTimers();}
  });

  it("最新连接失败明确回到未连接；PLC 状态点与提示匹配",async()=>{
    vi.useFakeTimers();try{
      vi.mocked(getEngineStatus).mockResolvedValueOnce(engine).mockRejectedValueOnce(new Error("连接失败"));
      vi.mocked(usePlcStatus).mockReturnValue({state:"error",message:"PLC 连接失败",since:1,lastPoll:null,cycleMs:null,pollCount:0,errorCount:1});
      render(<MemoryRouter><Sidebar/></MemoryRouter>);await act(()=>vi.advanceTimersByTimeAsync(0));
      expect(screen.getByText("PLC · 异常").closest(".engine-status")).toHaveAttribute("title","PLC 连接失败");
      expect(screen.getByText("PLC · 异常").parentElement?.querySelector(".dot")).toHaveClass("err");
      await act(()=>vi.advanceTimersByTimeAsync(3000));expect(screen.getByText("后端未连接")).toBeVisible();
    }finally{vi.useRealTimers();}
  });
});

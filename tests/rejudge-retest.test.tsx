import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RejudgeDialog from "../src/features/history/components/RejudgeDialog";

import { historyApi } from "../src/features/history/api";
import type { HistoryQuery, RejudgeResult } from "../src/features/history/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/history/api",()=>({historyApi:{rejudge:vi.fn()}}));
const result:RejudgeResult={total:2,skipped:1,skipReasons:[{id:8,sn:108,ts:1,reason:"测点布局与原始版本不一致"}],limitHit:false,matrix:[{from:"NG_GAP",to:"OK",count:1},{from:"OK",to:"NG_WIDTH",count:1}],
  changes:[{id:7,sn:107,ts:1,from:"NG_GAP",to:"OK",reason:"候选规则合格"}]};
const query:HistoryQuery={recipeId:"A",sn:"123",limit:50};
function element(value:HistoryQuery=query,total=3,onClose=vi.fn()){
  return <MemoryRouter><Routes><Route path="/" element={<RejudgeDialog query={value} total={total} onClose={onClose}/>}/><Route path="/history/7" element={<p>打开工件 7</p>}/></Routes></MemoryRouter>;
}
beforeEach(()=>vi.mocked(historyApi.rejudge).mockResolvedValue(structuredClone(result)));

describe("历史批量重判",()=>{

  it("绝对限颠倒会阻止重判；没有工件也不能运行",()=>{
    const page=render(element());
    fireEvent.change(screen.getByRole("spinbutton",{name:"胶宽绝对限下"}),{target:{value:"5"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"胶宽绝对限上"}),{target:{value:"3"}});
    expect(screen.getByRole("button",{name:"重判当前筛选（3 件）"})).toBeDisabled();
    page.unmount();render(element(query,0));expect(screen.getByRole("button",{name:"重判当前筛选（0 件）"})).toBeDisabled();
  });

  it("等待时锁定版本和参数，重复点击不会重复执行",async()=>{
    const pending=deferred<RejudgeResult>();vi.mocked(historyApi.rejudge).mockReturnValueOnce(pending.promise);
    render(element());fireEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));fireEvent.click(screen.getByRole("button",{name:"重判中…"}));
    expect(historyApi.rejudge).toHaveBeenCalledTimes(1);expect(screen.getByRole("button",{name:"当前配方"})).toBeDisabled();
    for(const input of screen.getAllByRole("spinbutton"))expect(input).toBeDisabled();
    await act(async()=>pending.resolve(result));expect(screen.getByRole("button",{name:"重判当前筛选（3 件）"})).toBeEnabled();
  });

  it.each(["resolve","reject"])("切换筛选后旧请求 %s 不显示旧结果或错误",async finish=>{
    const pending=deferred<RejudgeResult>();vi.mocked(historyApi.rejudge).mockReturnValueOnce(pending.promise);
    const page=render(element());await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));
    page.rerender(element({...query,recipeId:"B"},5));
    await act(async()=>finish==="resolve"?pending.resolve(result):pending.reject(new Error("旧筛选失败")));
    expect(screen.queryByRole("table")).not.toBeInTheDocument();expect(screen.queryByText("Error: 旧筛选失败")).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"重判当前筛选（5 件）"})).toBeEnabled();
  });

  it("失败保留试算参数可以重试，显示上限与跳过件数",async()=>{
    vi.mocked(historyApi.rejudge).mockRejectedValueOnce(new Error("历史库读取失败")).mockResolvedValueOnce({...result,limitHit:true});
    render(element());fireEvent.change(screen.getByRole("spinbutton",{name:"滤波窗口（奇数）"}),{target:{value:"7"}});
    await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));expect(await screen.findByText("Error: 历史库读取失败")).toBeVisible();
    expect(screen.getByRole("spinbutton",{name:"滤波窗口（奇数）"})).toHaveValue(7);
    await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));expect(await screen.findByText(/超过 5000 件/)).toBeVisible();
    expect(screen.queryByText("Error: 历史库读取失败")).not.toBeInTheDocument();
  });

});

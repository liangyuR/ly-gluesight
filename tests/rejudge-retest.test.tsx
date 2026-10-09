import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RejudgeDialog from "../src/features/history/components/RejudgeDialog";
import HistoryRetestPage from "../src/pages/HistoryRetestPage";
import { historyApi } from "../src/features/history/api";
import type { HistoryQuery, RejudgeResult } from "../src/features/history/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/history/api",()=>({historyApi:{rejudge:vi.fn()}}));
const result:RejudgeResult={total:2,skipped:1,limitHit:false,matrix:[{from:"NG_GAP",to:"OK",count:1},{from:"OK",to:"NG_WIDTH",count:1}],
  changes:[{id:7,sn:107,ts:1,from:"NG_GAP",to:"OK",reason:"候选规则合格"}]};
const query:HistoryQuery={recipeId:"A",sn:"123",limit:50};
function element(value:HistoryQuery=query,total=3,onClose=vi.fn()){
  return <MemoryRouter><Routes><Route path="/" element={<RejudgeDialog query={value} total={total} onClose={onClose}/>}/><Route path="/history/7" element={<p>打开工件 7</p>}/></Routes></MemoryRouter>;
}
beforeEach(()=>vi.mocked(historyApi.rejudge).mockResolvedValue(structuredClone(result)));

describe("历史批量重判",()=>{
  it("默认使用记录版本和空覆盖参数，展示变化矩阵且可打开变化工件",async()=>{
    render(element());await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));
    expect(historyApi.rejudge).toHaveBeenCalledWith({query,ids:[],useCurrentRecipe:false,overrides:{line:{},corner:{},width:{}}});
    expect(await screen.findByText(/NG→OK 1 件/)).toBeVisible();expect(screen.getByRole("table")).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:/SN 107/}));expect(await screen.findByText("打开工件 7")).toBeVisible();
  });

  it("切换当前配方、各类参数与断胶/滤波覆盖值准确发送；清空回到不变",async()=>{
    render(element());await userEvent.click(screen.getByRole("button",{name:"当前配方"}));
    fireEvent.change(screen.getByRole("spinbutton",{name:"直边上公差"}),{target:{value:"0.4"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"R 角下公差"}),{target:{value:"0.7"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"胶宽绝对限下"}),{target:{value:"-0.5"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"断胶允许长度（mm）"}),{target:{value:"0.8"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"滤波窗口（奇数）"}),{target:{value:"5"}});
    await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));
    expect(historyApi.rejudge).toHaveBeenLastCalledWith({query,ids:[],useCurrentRecipe:true,overrides:{line:{tolUpper:.4},corner:{tolLower:.7},width:{absMin:-.5},maxGapLen:.8,filterWindow:5}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"直边上公差"}),{target:{value:""}});
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"记录当时的配方版本"}));
    await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));
    expect(historyApi.rejudge).toHaveBeenLastCalledWith(expect.objectContaining({useCurrentRecipe:false,overrides:expect.objectContaining({line:{tolUpper:undefined}})}));
  });

  it.each([["直边上公差","-1"],["R 角下公差","-1"],["胶宽连续超差允许长度","-0.5"],["断胶允许长度（mm）","-1"],["滤波窗口（奇数）","0"],["滤波窗口（奇数）","4"],["滤波窗口（奇数）","33"],["滤波窗口（奇数）","1.5"]])("拒绝 %s=%s，恢复后允许重试",(field,value)=>{
    render(element());fireEvent.change(screen.getByRole("spinbutton",{name:field}),{target:{value}});
    expect(screen.getByRole("alert")).toBeVisible();expect(screen.getByRole("button",{name:"重判当前筛选（3 件）"})).toBeDisabled();
    expect(historyApi.rejudge).not.toHaveBeenCalled();fireEvent.change(screen.getByRole("spinbutton",{name:field}),{target:{value:""}});
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();expect(screen.getByRole("button",{name:"重判当前筛选（3 件）"})).toBeEnabled();
  });

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

  it("可关闭读操作；卸载后晚到失败不产生未处理错误",async()=>{
    const pending=deferred<RejudgeResult>(),close=vi.fn();vi.mocked(historyApi.rejudge).mockReturnValueOnce(pending.promise);
    const page=render(element(query,3,close));await userEvent.click(screen.getByRole("button",{name:"重判当前筛选（3 件）"}));
    await userEvent.click(screen.getAllByRole("button",{name:"关闭"})[1]);
    expect(close).toHaveBeenCalled();page.unmount();await act(async()=>pending.reject(new Error("已退出读取")));
  });
});

describe("历史复测入口",()=>{
  it("说明两种复测方式并通过入口选择历史工件",async()=>{
    render(<MemoryRouter initialEntries={["/history/retest"]}><Routes><Route path="/history/retest" element={<HistoryRetestPage/>}/><Route path="/history" element={<p>历史工件列表</p>}/></Routes></MemoryRouter>);
    expect(screen.getByText("先选择需要排查的工件")).toBeVisible();
    await userEvent.click(screen.getByRole("link",{name:"打开历史记录"}));await waitFor(()=>expect(screen.getByText("历史工件列表")).toBeVisible());
  });
});

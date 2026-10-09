import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import FeasibilityCalc from "../src/features/camera/components/FeasibilityCalc";

const set = (name: string, value: number) => fireEvent.change(screen.getByRole("spinbutton", { name }), { target: { value: String(value) } });
const result = (label: string) => within(screen.getByText(label).parentElement!);
describe("飞拍可行性输入与反馈", () => {
  it("使用实际曝光/帧率，并保留其他用户编辑", () => {
    const page = render(<FeasibilityCalc exposure={60} fps={10}/>); set("速度（mm/s）", 500);
    page.rerender(<FeasibilityCalc exposure={80} fps={20}/>);
    expect(screen.getByRole("spinbutton", { name: "速度（mm/s）" })).toHaveValue(500); expect(screen.getByRole("spinbutton", { name: "曝光（µs）" })).toHaveValue(80);
    expect(screen.getByRole("spinbutton", { name: "帧率（fps）" })).toHaveValue(20);
    expect(result("运动模糊").getByText(/1.00 px/)).toBeVisible(); expect(result("运动模糊").getByText("不满足")).toBeVisible();
  });
  it("改变点间距与帧率会分别反馈触发间隔和重叠不足", () => {
    render(<FeasibilityCalc fps={1}/>); set("最小拍照点间距（mm）", 210);
    expect(result("相邻触发间隔").getByText("不满足")).toBeVisible(); expect(result("相邻视野重叠").getByText("不满足")).toBeVisible();
    set("最小拍照点间距（mm）", 100); set("帧率（fps）", 10);
    expect(result("相邻触发间隔").getByText("满足")).toBeVisible(); expect(result("相邻视野重叠").getByText("满足")).toBeVisible();
  });
  it.each(["像素当量（mm/px）", "曝光（µs）", "帧率（fps）", "速度（mm/s）","最小拍照点间距（mm）","视野沿运动方向（mm）"])("%s 为 0 不能显示可行", label => {
    render(<FeasibilityCalc/>); set(label, 0); expect(screen.getByRole("alert")).toHaveTextContent("必须大于 0");
    expect(screen.queryAllByText("满足")).toHaveLength(0); expect(screen.getAllByText("参数无效")).toHaveLength(4);
  });
  it.each(["速度（mm/s）","曝光（µs）","像素当量（mm/px）","帧率（fps）","最小拍照点间距（mm）","视野沿运动方向（mm）"])("%s 的负值与空值都不可计算，修正恢复",label=>{
    render(<FeasibilityCalc/>);const input=screen.getByRole("spinbutton",{name:label});fireEvent.change(input,{target:{value:"-1"}});expect(screen.getByRole("status")).toHaveTextContent("请修正参数后计算");
    fireEvent.change(input,{target:{value:""}});expect(input).toHaveValue(null);expect(screen.queryAllByText("满足")).toHaveLength(0);
    fireEvent.change(input,{target:{value:"1"}});expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
  it("临界模糊 0.5 px、相邻一帧时间与 20 mm 余量均可满足",()=>{
    render(<FeasibilityCalc/>);set("速度（mm/s）",100);set("曝光（µs）",100);set("像素当量（mm/px）",.02);set("帧率（fps）",10);set("最小拍照点间距（mm）",10);set("视野沿运动方向（mm）",30);
    expect(screen.getAllByText("满足")).toHaveLength(4);expect(screen.getByRole("status")).toHaveTextContent("当前输入满足飞拍条件");
    set("速度（mm/s）",101);expect(result("运动模糊").getByText("不满足")).toBeVisible();expect(result("相邻触发间隔").getByText("不满足")).toBeVisible();expect(result("该配置允许的最高速度").getByText("不满足")).toBeVisible();
    set("速度（mm/s）",100);set("视野沿运动方向（mm）",29.9);expect(result("相邻视野重叠").getByText("不满足")).toBeVisible();expect(screen.getByRole("status")).toHaveTextContent("当前输入存在未满足项");
  });
  it("实际参数 0 不会被当作缺省值忽略，小帧率保留精度",()=>{
    const page=render(<FeasibilityCalc exposure={0} fps={.01}/>);expect(screen.getByRole("spinbutton",{name:"曝光（µs）"})).toHaveValue(0);expect(screen.getByRole("spinbutton",{name:"帧率（fps）"})).toHaveValue(.01);expect(screen.getByRole("alert")).toBeVisible();
    page.rerender(<FeasibilityCalc exposure={10} fps={.01}/>);expect(result("相邻触发间隔").getByText(/单帧时间 100000 ms/)).toBeVisible();expect(result("相邻触发间隔").getByText("不满足")).toBeVisible();
  });
  it("只更新实际帧率不重置手动曝光，只更新曝光也保留手动帧率",()=>{
    const page=render(<FeasibilityCalc exposure={60} fps={10}/>);set("曝光（µs）",20);page.rerender(<FeasibilityCalc exposure={60} fps={20}/>);expect(screen.getByRole("spinbutton",{name:"曝光（µs）"})).toHaveValue(20);
    set("帧率（fps）",15);page.rerender(<FeasibilityCalc exposure={80} fps={20}/>);expect(screen.getByRole("spinbutton",{name:"帧率（fps）"})).toHaveValue(15);expect(screen.getByRole("spinbutton",{name:"曝光（µs）"})).toHaveValue(80);
    page.rerender(<FeasibilityCalc exposure={undefined} fps={null}/>);expect(screen.getByRole("spinbutton",{name:"帧率（fps）"})).toHaveValue(15);
  });
  it("有限输入导致计算溢出时不给出 Infinity 或虚假的满足结论",()=>{
    render(<FeasibilityCalc/>);set("速度（mm/s）",1e308);set("曝光（µs）",1e308);expect(screen.getByRole("alert")).toHaveTextContent("数值过大");expect(screen.queryAllByText("满足")).toHaveLength(0);expect(screen.queryByText(/Infinity/)).not.toBeInTheDocument();
  });
});

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import FeasibilityCalc from "../src/features/camera/components/FeasibilityCalc";

const set = (name: string, value: number) => fireEvent.change(screen.getByRole("spinbutton", { name }), { target: { value: String(value) } });

describe("飞拍可行性输入与反馈", () => {
  it("只更新实际帧率不重置手动曝光，只更新曝光也保留手动帧率",()=>{
    const page=render(<FeasibilityCalc exposure={60} fps={10}/>);set("曝光（µs）",20);page.rerender(<FeasibilityCalc exposure={60} fps={20}/>);expect(screen.getByRole("spinbutton",{name:"曝光（µs）"})).toHaveValue(20);
    set("帧率（fps）",15);page.rerender(<FeasibilityCalc exposure={80} fps={20}/>);expect(screen.getByRole("spinbutton",{name:"帧率（fps）"})).toHaveValue(15);expect(screen.getByRole("spinbutton",{name:"曝光（µs）"})).toHaveValue(80);
    page.rerender(<FeasibilityCalc exposure={undefined} fps={null}/>);expect(screen.getByRole("spinbutton",{name:"帧率（fps）"})).toHaveValue(15);
  });
  it("有限输入导致计算溢出时不给出 Infinity 或虚假的满足结论",()=>{
    render(<FeasibilityCalc/>);set("速度（mm/s）",1e308);set("曝光（µs）",1e308);expect(screen.getByRole("alert")).toHaveTextContent("数值过大");expect(screen.queryAllByText("满足")).toHaveLength(0);expect(screen.queryByText(/Infinity/)).not.toBeInTheDocument();
  });
});

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import CalibPanel from "../src/features/camera/components/CalibPanel";
import { deferred } from "./fixtures";

const host=vi.hoisted(()=>({desktop:true}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => host.desktop, invoke: vi.fn() }));
const info = { path: "calib.json", rms: .0123, mmPerPx: .04, maxError: .02, pattern: [9, 6], square: 4, ts: 1 };
beforeEach(() => { host.desktop=true;vi.mocked(invoke).mockReset().mockResolvedValue(null); });
const ready=()=>waitFor(()=>expect(screen.queryByText("正在读取标定…")).not.toBeInTheDocument());

describe("工位标定", () => {
  it.each(["sim", "no-image"])("%s 不能标定实际工位", async condition => {
    render(<CalibPanel cam={0} isSim={condition === "sim"} imageId={condition === "no-image" ? undefined : "frozen"} />);
    expect(screen.getByRole("button", { name: "用冻结样本标定" })).toBeDisabled();
    await ready();
  });

  it("加载当前相机标定尺寸，提交绑定冻结图像", async () => {
    vi.mocked(invoke).mockResolvedValue(info);
    render(<CalibPanel cam={2} isSim={false} imageId="frozen-2" />);
    await waitFor(() => expect(screen.getByRole("spinbutton", { name: "内角点（列）" })).toHaveValue(9));
    expect(screen.getByRole("spinbutton", { name: "内角点（行）" })).toHaveValue(6);
    fireEvent.change(screen.getByRole("spinbutton", { name: "格长（mm）" }), { target: { value: "5" } });
    await userEvent.click(screen.getByRole("button", { name: "用冻结样本标定" }));
    expect(invoke).toHaveBeenCalledWith("vision_calibrate", { pattern: [9, 6], square: 5, cam: 2, imageId: "frozen-2" });
    expect(await screen.findByText("标定完成：残差 RMS 0.0123 mm，约 0.0400 mm/px")).toBeVisible();
  });

  it("失败保留先前标定并允许重试", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(info).mockRejectedValueOnce(new Error("未识别角点"));
    render(<CalibPanel cam={0} isSim={false} imageId="frozen" />);
    await screen.findByText(/RMS 0.0123 mm/);
    await userEvent.click(screen.getByRole("button", { name: "用冻结样本标定" }));
    expect(await screen.findByText("Error: 未识别角点")).toBeVisible();
    expect(screen.getByText(/RMS 0.0123 mm/)).toBeVisible();
    expect(screen.getByRole("button", { name: "用冻结样本标定" })).toBeEnabled();
  });

  it("当前标定加载期间锁定输入和执行，避免迟到读取覆盖手动设置",async()=>{
    const request=deferred<typeof info|null>();vi.mocked(invoke).mockReturnValue(request.promise);
    render(<CalibPanel cam={2} isSim={false} imageId="frozen-2"/>);
    expect(screen.getByRole("spinbutton",{name:"内角点（列）"})).toBeDisabled();expect(screen.getByRole("button",{name:"用冻结样本标定"})).toBeDisabled();expect(screen.getByRole("button",{name:"刷新标定"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));expect(invoke).toHaveBeenCalledTimes(1);
    await act(async()=>request.resolve(info));expect(screen.getByRole("spinbutton",{name:"内角点（列）"})).toHaveValue(9);expect(screen.getByRole("button",{name:"用冻结样本标定"})).toBeEnabled();
  });

  it("读取失败不当作未标定，刷新后加载已有标定",async()=>{
    vi.mocked(invoke).mockRejectedValueOnce(new Error("工位文件不可读")).mockResolvedValueOnce(info);
    render(<CalibPanel cam={0} isSim={false} imageId="frozen"/>);
    expect(await screen.findByRole("alert")).toHaveTextContent("读取标定失败：Error: 工位文件不可读");expect(screen.getByText("尚未读取标定")).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"刷新标定"}));expect(await screen.findByText(/RMS 0.0123 mm/)).toBeVisible();expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("刷新已有标定失败保留旧结果和编辑值，重试恢复",async()=>{
    vi.mocked(invoke).mockResolvedValueOnce(info).mockRejectedValueOnce(new Error("读锁冲突")).mockResolvedValueOnce(info);
    render(<CalibPanel cam={0} isSim={false} imageId="frozen"/>);await ready();fireEvent.change(screen.getByRole("spinbutton",{name:"格长（mm）"}),{target:{value:"7"}});
    await userEvent.click(screen.getByRole("button",{name:"刷新标定"}));expect(await screen.findByRole("alert")).toHaveTextContent("读锁冲突");expect(screen.getByText(/RMS 0.0123 mm/)).toBeVisible();expect(screen.getByRole("spinbutton",{name:"格长（mm）"})).toHaveValue(7);
    await userEvent.click(screen.getByRole("button",{name:"刷新标定"}));await waitFor(()=>expect(screen.getByRole("spinbutton",{name:"格长（mm）"})).toHaveValue(4));
  });

  it.each([
    ["内角点（列）","1"],["内角点（列）","101"],["内角点（列）","2.5"],
    ["内角点（行）","1"],["内角点（行）","101"],["内角点（行）","2.5"],
    ["格长（mm）","0"],["格长（mm）","-1"],["格长（mm）",""],
  ])("%s = %s 无效时不能执行，修正才恢复",async(label,value)=>{
    render(<CalibPanel cam={0} isSim={false} imageId="frozen"/>);await ready();
    fireEvent.change(screen.getByRole("spinbutton",{name:label}),{target:{value}});
    expect(screen.getByRole("alert")).toHaveTextContent("内角点行列需为 2–100 的整数，格长必须大于 0");expect(screen.getByRole("button",{name:"用冻结样本标定"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.change(screen.getByRole("spinbutton",{name:label}),{target:{value:"3"}});expect(screen.getByRole("button",{name:"用冻结样本标定"})).toBeEnabled();
  });

  it("行列范围两端及小数格长可提交，标定中禁止重复、编辑和刷新",async()=>{
    const request=deferred<typeof info>();vi.mocked(invoke).mockResolvedValueOnce(null).mockReturnValueOnce(request.promise);
    render(<CalibPanel cam={1} isSim={false} imageId="frozen-board"/>);await ready();
    fireEvent.change(screen.getByRole("spinbutton",{name:"内角点（列）"}),{target:{value:"2"}});fireEvent.change(screen.getByRole("spinbutton",{name:"内角点（行）"}),{target:{value:"100"}});fireEvent.change(screen.getByRole("spinbutton",{name:"格长（mm）"}),{target:{value:"0.125"}});
    await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));expect(invoke).toHaveBeenLastCalledWith("vision_calibrate",{cam:1,pattern:[2,100],square:.125,imageId:"frozen-board"});
    expect(screen.getByRole("button",{name:"标定中…"})).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"格长（mm）"})).toBeDisabled();expect(screen.getByRole("button",{name:"刷新标定"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button",{name:"标定中…"}));expect(invoke).toHaveBeenCalledTimes(2);
    await act(async()=>request.resolve(info));expect(screen.getByText("标定完成：残差 RMS 0.0123 mm，约 0.0400 mm/px")).toBeVisible();
  });

  it("失败可以重新执行同一冻结样本，空返回不能伪装标定成功",async()=>{
    vi.mocked(invoke).mockResolvedValueOnce(info).mockResolvedValueOnce(null).mockResolvedValueOnce(info);
    render(<CalibPanel cam={0} isSim={false} imageId="frozen"/>);await ready();
    await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));expect(await screen.findByText("Error: 标定没有返回有效结果")).toBeVisible();expect(screen.queryByText(/标定完成：/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));expect(await screen.findByText(/标定完成：残差 RMS/)).toBeVisible();
  });

  it.each(["success","error"])("旧工位读取 %s 晚到不能改新工位参数与错误",async outcome=>{
    const old=deferred<typeof info|null>();vi.mocked(invoke).mockReturnValueOnce(old.promise).mockResolvedValueOnce({...info,pattern:[7,5],square:3,rms:.02});
    const page=render(<CalibPanel cam={0} isSim={false} imageId="old"/>);page.rerender(<CalibPanel cam={1} isSim={false} imageId="new"/>);await ready();
    await act(async()=>outcome==="success"?old.resolve(info):old.reject(new Error("旧工位读取失败")));
    expect(screen.getByRole("spinbutton",{name:"内角点（列）"})).toHaveValue(7);expect(screen.getByRole("spinbutton",{name:"格长（mm）"})).toHaveValue(3);expect(screen.queryByText(/旧工位读取失败/)).not.toBeInTheDocument();
  });

  it.each(["success","error"])("旧冻结样本标定 %s 不能覆盖新样本或解锁新请求",async outcome=>{
    const old=deferred<typeof info>(),current=deferred<typeof info>();vi.mocked(invoke).mockResolvedValueOnce(info).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const page=render(<CalibPanel cam={0} isSim={false} imageId="old"/>);await ready();await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));
    page.rerender(<CalibPanel cam={0} isSim={false} imageId="new"/>);await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));
    await act(async()=>outcome==="success"?old.resolve({...info,rms:.1}):old.reject(new Error("旧样本无效")));
    expect(screen.getByRole("button",{name:"标定中…"})).toBeDisabled();expect(screen.queryByText(/旧样本无效|标定完成：/)).not.toBeInTheDocument();
    await act(async()=>current.resolve({...info,rms:.001}));expect(screen.getByText("标定完成：残差 RMS 0.0010 mm，约 0.0400 mm/px")).toBeVisible();
  });

  it("页面卸载后不接受迟到标定结果或自动刷新",async()=>{
    const request=deferred<typeof info>();vi.mocked(invoke).mockResolvedValueOnce(info).mockReturnValueOnce(request.promise);
    const page=render(<CalibPanel cam={0} isSim={false} imageId="frozen"/>);await ready();await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));page.unmount();
    await act(async()=>request.resolve(info));expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("浏览器预览不读取或运行标定，模拟工位说明内置标定",()=>{
    host.desktop=false;render(<CalibPanel cam={0} isSim={true} imageId="frozen"/>);
    expect(screen.getByText("使用内置标定（0.08 mm/px）")).toBeVisible();expect(screen.getByRole("button",{name:"刷新标定"})).toBeDisabled();expect(screen.getByRole("button",{name:"用冻结样本标定"})).toBeDisabled();expect(invoke).not.toHaveBeenCalled();
  });
});

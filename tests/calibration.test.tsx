import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import CalibPanel from "../src/features/camera/components/CalibPanel";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
const info = { path: "calib.json", rms: .0123, mmPerPx: .04, maxError: .02, pattern: [9, 6], square: 4, ts: 1 };
beforeEach(() => { vi.mocked(invoke).mockReset().mockResolvedValue(null); });
const ready=()=>waitFor(()=>expect(screen.queryByText("正在读取标定…")).not.toBeInTheDocument());

describe("工位标定", () => {
  it("加载当前相机标定尺寸，提交绑定冻结图像", async () => {
    vi.mocked(invoke).mockResolvedValue(info);
    render(<CalibPanel cam={2} isSim={false} imageId="frozen-2" />);
    await waitFor(() => expect(screen.getByRole("spinbutton", { name: "内角点（列）" })).toHaveValue(9));
    expect(screen.getByRole("spinbutton", { name: "内角点（行）" })).toHaveValue(6);
    fireEvent.change(screen.getByRole("spinbutton", { name: "格长（mm）" }), { target: { value: "5" } });
    await userEvent.click(screen.getByRole("button", { name: "用冻结样本标定" }));
    expect(invoke).toHaveBeenCalledWith("vision_calibrate", { pattern: [9, 6], square: 5, cam: 2, view: 1, imageId: "frozen-2" });
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

  it.each(["success","error"])("旧冻结样本标定 %s 不能覆盖新样本或解锁新请求",async outcome=>{
    const old=deferred<typeof info>(),current=deferred<typeof info>();vi.mocked(invoke).mockResolvedValueOnce(info).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const page=render(<CalibPanel cam={0} isSim={false} imageId="old"/>);await ready();await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));
    page.rerender(<CalibPanel cam={0} isSim={false} imageId="new"/>);await userEvent.click(screen.getByRole("button",{name:"用冻结样本标定"}));
    await act(async()=>outcome==="success"?old.resolve({...info,rms:.1}):old.reject(new Error("旧样本无效")));
    expect(screen.getByRole("button",{name:"标定中…"})).toBeDisabled();expect(screen.queryByText(/旧样本无效|标定完成：/)).not.toBeInTheDocument();
    await act(async()=>current.resolve({...info,rms:.001}));expect(screen.getByText("标定完成：残差 RMS 0.0010 mm，约 0.0400 mm/px")).toBeVisible();
  });

});

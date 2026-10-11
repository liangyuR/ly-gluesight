import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CameraConfigPanel from "../src/features/camera/components/CameraConfigPanel";
import { cameraApi, defaultCameraConfig } from "../src/features/camera/api";
import type { CameraConfig, DeviceSummary } from "../src/features/camera/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/camera/api", async importOriginal => {
  const actual = await importOriginal<typeof import("../src/features/camera/api")>();
  return { ...actual, cameraApi: { ...actual.cameraApi, listDevices: vi.fn(), records: vi.fn(), saveConfig: vi.fn(), pickReplayDir: vi.fn() } };
});
let config: CameraConfig;
beforeEach(() => {
  config = { ...defaultCameraConfig, id: "CAM-1", source: "mvs" };
  vi.mocked(cameraApi.listDevices).mockResolvedValue([
    { serial: "AUTO", model: "相机 1", userName: "", transport: "GigE", ip: "192.168.1.2" },
    { serial: "MANUAL", model: "相机 2", userName: "", transport: "USB3", ip: null },
  ]);
  vi.mocked(cameraApi.records).mockResolvedValue({ root: "D:/records", items: [] });
  vi.mocked(cameraApi.saveConfig).mockResolvedValue([]);
  vi.mocked(cameraApi.pickReplayDir).mockResolvedValue(null);
});
const show = () => render(<CameraConfigPanel cam={0} initial={config} status={null} />);

describe("相机配置表单", () => {
  it("开流前计数默认未知，填写与清空均明确保存，不推断为零",async()=>{
    show();
    const input=screen.getByRole("spinbutton",{name:"开流前触发计数"});
    expect(input).toHaveValue(null);
    expect(screen.getByText(/仅填写台架确认的开流前触发计数/)).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    expect(cameraApi.saveConfig).toHaveBeenLastCalledWith(0,expect.objectContaining({counterAfterOpen:null}));
    fireEvent.change(input,{target:{value:"1038"}});
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    expect(cameraApi.saveConfig).toHaveBeenLastCalledWith(0,expect.objectContaining({counterAfterOpen:1038}));
    fireEvent.change(input,{target:{value:""}});
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    expect(cameraApi.saveConfig).toHaveBeenLastCalledWith(0,expect.objectContaining({counterAfterOpen:null}));
  });

  it("目录窗口打开时禁止重复选择和保存，取消后保留路径并可手动输入", async () => {
    config = { ...config, source: "replay", replayDir: "D:/原目录" };
    const request = deferred<string | null>();
    vi.mocked(cameraApi.pickReplayDir).mockReturnValue(request.promise);
    show();
    await userEvent.click(screen.getByRole("button", { name: "选择目录" }));
    expect(screen.getByRole("button", { name: "选择中…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
    await act(async () => request.resolve(null));
    expect(screen.getByRole("textbox", { name: "图片目录" })).toHaveValue("D:/原目录");
    fireEvent.change(screen.getByRole("textbox", { name: "图片目录" }), { target: { value: "D:/手工目录" } });
    expect(screen.getByRole("textbox", { name: "图片目录" })).toHaveValue("D:/手工目录");
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeEnabled();
  });

  it("目录选择失败保留原路径并允许重试", async () => {
    config = { ...config, source: "replay", replayDir: "D:/原目录" };
    vi.mocked(cameraApi.pickReplayDir).mockRejectedValueOnce(new Error("无法打开目录窗口"));
    show();
    await userEvent.click(screen.getByRole("button", { name: "选择目录" }));
    expect(await screen.findByText(/无法打开目录窗口/)).toBeVisible();
    expect(screen.getByRole("textbox", { name: "图片目录" })).toHaveValue("D:/原目录");
    await userEvent.click(screen.getByRole("button", { name: "选择目录" }));
    expect(screen.queryByText(/无法打开目录窗口/)).not.toBeInTheDocument();
  });

  it("后台更新不覆盖用户尚未保存的序列号", async () => {
    const page = show(); await screen.findByRole("option", { name: /相机 2/ });
    await userEvent.selectOptions(document.getElementById("cam-serial")!, "MANUAL");
    page.rerender(<CameraConfigPanel cam={0} initial={{ ...config, serial: "AUTO" }} status={null} />);
    expect(document.getElementById("cam-serial")).toHaveValue("MANUAL");
  });

  it("保存中禁用按钮，失败解锁并保留输入", async () => {
    const request = deferred<string[]>(); vi.mocked(cameraApi.saveConfig).mockReturnValue(request.promise);
    show(); await userEvent.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(screen.getByRole("button", { name: "写入中…" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "名称" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "回放目录" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "写入中…" }));expect(cameraApi.saveConfig).toHaveBeenCalledTimes(1);
    request.reject(new Error("相机断开"));
    expect(await screen.findByText("Error: 相机断开")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeEnabled();
  });

  it("设备枚举失败可刷新，迟到的旧设备列表和错误不能覆盖新枚举",async()=>{
    const old=deferred<DeviceSummary[]>();vi.mocked(cameraApi.listDevices).mockReturnValueOnce(old.promise).mockResolvedValueOnce([{serial:"NEW",model:"最新设备",userName:"",transport:"USB3",ip:null}]);
    show();await userEvent.click(screen.getByRole("button",{name:"重新枚举"}));
    expect(await screen.findByRole("option",{name:/最新设备/})).toBeVisible();
    await act(async()=>old.reject(new Error("旧枚举失败")));
    expect(screen.queryByText("Error: 旧枚举失败")).not.toBeInTheDocument();
    vi.mocked(cameraApi.listDevices).mockRejectedValueOnce(new Error("设备离线"));
    await userEvent.click(screen.getByRole("button",{name:"重新枚举"}));expect(await screen.findByText("Error: 设备离线")).toBeVisible();
    vi.mocked(cameraApi.listDevices).mockResolvedValueOnce([]);
    await userEvent.click(screen.getByRole("button",{name:"重新枚举"}));await waitFor(()=>expect(screen.queryByText("Error: 设备离线")).not.toBeInTheDocument());
  });

  it("切换图像源忽略旧录制请求，重新切回可加载最新目录",async()=>{
    const old=deferred<{root:string;items:{path:string;name:string;frames:number}[]}>();
    vi.mocked(cameraApi.records).mockReturnValueOnce(old.promise).mockResolvedValueOnce({root:"D:/records",items:[{path:"new",name:"最新录制",frames:3}]});
    show();await userEvent.click(screen.getByRole("button",{name:"回放目录"}));await userEvent.click(screen.getByRole("button",{name:"模拟相机"}));
    await userEvent.click(screen.getByRole("button",{name:"回放目录"}));expect(await screen.findByRole("option",{name:"最新录制 · 3 帧"})).toBeVisible();
    await act(async()=>old.resolve({root:"old",items:[{path:"old",name:"旧录制",frames:4}]}));
    expect(screen.queryByRole("option",{name:"旧录制 · 4 帧"})).not.toBeInTheDocument();expect(screen.getByRole("option",{name:"最新录制 · 3 帧"})).toBeVisible();
  });

});

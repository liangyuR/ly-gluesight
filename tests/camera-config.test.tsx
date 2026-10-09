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
  it("选择回放目录后填入路径，保存时应用所选目录", async () => {
    config = { ...config, source: "replay", replayDir: "D:/原目录" };
    vi.mocked(cameraApi.pickReplayDir).mockResolvedValue("D:/现场图/Glue1");
    show();
    await userEvent.click(screen.getByRole("button", { name: "选择目录" }));
    expect(cameraApi.pickReplayDir).toHaveBeenCalledWith("D:/原目录");
    expect(screen.getByRole("textbox", { name: "图片目录" })).toHaveValue("D:/现场图/Glue1");
    expect(cameraApi.saveConfig).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(0, expect.objectContaining({ replayDir: "D:/现场图/Glue1" }));
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

  it("切换连续采集显示帧率并隐藏触发参数，Software 禁用触发沿", async () => {
    show(); await userEvent.selectOptions(document.getElementById("cam-trigger")!, "Software");
    expect(document.getElementById("cam-activation")).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "连续（仅预览）" }));
    expect(document.getElementById("cam-fps")).toBeVisible(); expect(document.getElementById("cam-trigger")).toBeNull();
  });

  it("后台固定序列号同步到未编辑的表单", async () => {
    const page = show(); await screen.findByRole("option", { name: /相机 1/ });
    page.rerender(<CameraConfigPanel cam={0} initial={{ ...config, serial: "AUTO" }} status={null} />);
    expect(document.getElementById("cam-serial")).toHaveValue("AUTO");
  });

  it("后台更新不覆盖用户尚未保存的序列号", async () => {
    const page = show(); await screen.findByRole("option", { name: /相机 2/ });
    await userEvent.selectOptions(document.getElementById("cam-serial")!, "MANUAL");
    page.rerender(<CameraConfigPanel cam={0} initial={{ ...config, serial: "AUTO" }} status={null} />);
    expect(document.getElementById("cam-serial")).toHaveValue("MANUAL");
  });

  it("保存包含编辑值，显示相机拒绝参数的警告", async () => {
    const onSaved = vi.fn(); vi.mocked(cameraApi.saveConfig).mockResolvedValue(["曝光拒绝"]);
    render(<CameraConfigPanel cam={2} initial={config} status={null} onSaved={onSaved} />);
    fireEvent.change(document.getElementById("cam-exposureUs")!, { target: { value: "80" } });
    await userEvent.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(2, expect.objectContaining({ exposureUs: 80 }));
    expect(await screen.findByText("已应用，1 项参数相机未接受")).toBeVisible();
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ exposureUs: 80 }));
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

  it("回放录制目录与通道一起保存，切换图像源保留其他采集参数",async()=>{
    vi.mocked(cameraApi.records).mockResolvedValue({root:"D:/records",items:[{path:"D:/records/part-1",name:"工件一",frames:8}]});
    show();await userEvent.click(screen.getByRole("button",{name:"回放目录"}));
    expect(screen.getByRole("button",{name:"保存并应用"})).toBeDisabled();
    await screen.findByRole("option",{name:"工件一 · 8 帧"});
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"帧录制"}),"D:/records/part-1");
    fireEvent.change(screen.getByRole("spinbutton",{name:"通道"}),{target:{value:"2"}});
    await userEvent.click(screen.getByRole("button",{name:"连续（仅预览）"}));
    fireEvent.change(screen.getByRole("spinbutton",{name:"帧率（fps）"}),{target:{value:"24"}});
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(0,expect.objectContaining({source:"replay",replayDir:"D:/records/part-1",replayChannel:2,acquisition:"freeRun",fps:24,exposureUs:60}));
    await userEvent.click(screen.getByRole("button",{name:"模拟相机"}));
    expect(screen.queryByRole("textbox",{name:"图片目录"})).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"回放目录"}));expect(screen.getByRole("textbox",{name:"图片目录"})).toHaveValue("D:/records/part-1");
  });
  it("海康设备、触发沿、曝光、频闪与 Chunk 参数按当前编辑保存",async()=>{
    show();await screen.findByRole("option",{name:/相机 2/});
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"相机设备"}),"MANUAL");await userEvent.selectOptions(screen.getByRole("combobox",{name:"触发沿"}),"FallingEdge");
    fireEvent.change(screen.getByRole("textbox",{name:"名称"}),{target:{value:"工位侧视"}});
    for(const [label,value] of [["触发延时（µs）","20"],["输入滤波（µs）","8"],["曝光时间（µs）","125"],["增益（dB）","6.5"]])fireEvent.change(screen.getByRole("spinbutton",{name:label}),{target:{value}});
    await userEvent.click(screen.getByRole("checkbox",{name:"曝光信号（ExposureStartActive）驱动频闪"}));await userEvent.click(screen.getByRole("checkbox",{name:"帧计数、Line0 触发计数、时间戳"}));
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    expect(cameraApi.saveConfig).toHaveBeenCalledWith(0,expect.objectContaining({name:"工位侧视",source:"mvs",serial:"MANUAL",triggerSource:"Line0",triggerActivation:"FallingEdge",triggerDelayUs:20,debouncerUs:8,exposureUs:125,gainDb:6.5,strobe:false,chunk:false}));
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

  it("回放录制列表加载失败显示重试入口且不阻止手工输入目录",async()=>{
    config={...config,source:"replay",replayDir:"D:/custom"};
    vi.mocked(cameraApi.records).mockRejectedValueOnce(new Error("录制库不可读")).mockResolvedValueOnce({root:"D:/records",items:[{path:"D:/records/new",name:"新录制",frames:2}]});
    show();expect(await screen.findByText(/录制库不可读/)).toBeVisible();
    expect(screen.getByRole("button",{name:"保存并应用"})).toBeEnabled();
    await userEvent.click(screen.getByRole("button",{name:"重新读取录制"}));expect(await screen.findByRole("option",{name:"新录制 · 2 帧"})).toBeVisible();
    expect(screen.queryByText(/录制库不可读/)).not.toBeInTheDocument();
  });

  it("切换图像源忽略旧录制请求，重新切回可加载最新目录",async()=>{
    const old=deferred<{root:string;items:{path:string;name:string;frames:number}[]}>();
    vi.mocked(cameraApi.records).mockReturnValueOnce(old.promise).mockResolvedValueOnce({root:"D:/records",items:[{path:"new",name:"最新录制",frames:3}]});
    show();await userEvent.click(screen.getByRole("button",{name:"回放目录"}));await userEvent.click(screen.getByRole("button",{name:"模拟相机"}));
    await userEvent.click(screen.getByRole("button",{name:"回放目录"}));expect(await screen.findByRole("option",{name:"最新录制 · 3 帧"})).toBeVisible();
    await act(async()=>old.resolve({root:"old",items:[{path:"old",name:"旧录制",frames:4}]}));
    expect(screen.queryByRole("option",{name:"旧录制 · 4 帧"})).not.toBeInTheDocument();expect(screen.getByRole("option",{name:"最新录制 · 3 帧"})).toBeVisible();
  });

  it.each([
    ["曝光时间（µs）","0","曝光时间需在 1–1000000 µs 之间"],
    ["增益（dB）","","增益需为有限数"],
    ["触发延时（µs）","-1","触发延时与输入滤波需为非负有限数"],
  ])("%s 不合法时不能保存，修正后恢复",async(label,value,error)=>{
    show();const input=screen.getByRole("spinbutton",{name:label});fireEvent.change(input,{target:{value}});
    expect(screen.getByRole("alert")).toHaveTextContent(error);expect(screen.getByRole("button",{name:"保存并应用"})).toBeDisabled();
    expect(cameraApi.saveConfig).not.toHaveBeenCalled();fireEvent.change(input,{target:{value:"10"}});expect(screen.getByRole("button",{name:"保存并应用"})).toBeEnabled();
  });

  it("回放通道与连续帧率必须合法，空输入不会变成零参数",async()=>{
    config={...config,source:"replay",replayDir:"D:/images",acquisition:"freeRun"};show();
    const channel=screen.getByRole("spinbutton",{name:"通道"}),fps=screen.getByRole("spinbutton",{name:"帧率（fps）"});
    fireEvent.change(channel,{target:{value:"1.5"}});expect(screen.getByRole("alert")).toHaveTextContent("回放通道需为非负整数");
    fireEvent.change(channel,{target:{value:"0"}});fireEvent.change(fps,{target:{value:""}});
    expect(fps).toHaveValue(null);expect(screen.getByRole("button",{name:"保存并应用"})).toBeDisabled();
    fireEvent.change(fps,{target:{value:"501"}});expect(screen.getByRole("alert")).toHaveTextContent("连续采集帧率需在 1–500 fps 之间");
    fireEvent.change(fps,{target:{value:"20"}});expect(screen.getByRole("button",{name:"保存并应用"})).toBeEnabled();
  });
});

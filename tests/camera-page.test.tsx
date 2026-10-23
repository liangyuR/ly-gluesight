import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CameraPage from "../src/pages/CameraPage";
import { defaultCameraConfig } from "../src/features/camera/api";
import type { BandwidthEstimate, CameraConfig, CameraStatus } from "../src/features/camera/types";
import { siteStatus, snapshot } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const device = (serial: string) => ({ serial, model: "MV-CA050", userName: "", transport: "GigE" as const, ip: "192.168.1.10" });
const camera = (id: string, name: string, over: Partial<CameraConfig> = {}): CameraConfig => ({ ...defaultCameraConfig, id, name, serial: id + "-SN", ...over });
const status = (cam: number, config: CameraConfig): CameraStatus => ({ cam, id: config.id, name: config.name, source: "mvs", ready: true, released: false, message: "", device: device(config.serial), sdkVersion: null, frames: 0, fps: 0, maxFps: null, configuredFrameRate: 10, lostPackets: 0, droppedFrames: 0, warnings: [] });

let configs: CameraConfig[];
let released: string[];
let bandwidth: BandwidthEstimate;
beforeEach(() => {
  configs = [camera("cam1", "左相机"), camera("cam2", "右相机")];
  released = [];
  bandwidth = { hz: 10, mbpsTotal: 524.3, perCamera: [262.1, 262.1], budgetMbps: 800, overBudget: false };
  vi.mocked(listen).mockResolvedValue(() => {});
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "camera_rig_config": return configs;
      case "camera_bandwidth": return bandwidth;
      case "camera_rig_status": return configs.map((c, i) => ({ ...status(i, c), ...(released.includes(c.id) ? { ready: false, released: true, device: null } : {}) }));
      case "camera_list_devices": return configs.map(c => device(c.serial));
      case "camera_save_config": return [];
      case "camera_preview": return new ArrayBuffer(0);
      case "recipe_list": return { recipes: [], errors: [] };
      case "site_status": return siteStatus();
      case "cycle_snapshot": return snapshot();
      default: return [];
    }
  });
});

const renderPage = () => render(<MemoryRouter initialEntries={["/camera"]}><Routes><Route path="/camera" element={<CameraPage />} /><Route path="/camera/calibration" element={<p>标定页</p>} /></Routes></MemoryRouter>);
const switcher = () => within(screen.getByRole("group", { name: "切换相机" }));
async function openCamera(label: string) {
  renderPage();
  await userEvent.click(within(await screen.findByRole("article", { name: label })).getByRole("button", { name: /参数详情|进入详情选择设备/ }));
}

describe("相机参数未保存修改", () => {
  it("改名后切换相机并选保存并应用 → 先把修改写入当前相机，再切到目标相机", async () => {
    await openCamera("左相机 cam1");
    const name = await screen.findByRole("textbox", { name: "名称" });
    await userEvent.clear(name);
    await userEvent.type(name, "左相机A");
    await userEvent.click(switcher().getByRole("button", { name: "右相机 cam2" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "保存并应用" }));
    await waitFor(() => expect(switcher().getByRole("button", { name: "右相机 cam2" })).toHaveAttribute("aria-current", "true"));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_save_config", { cam: 0, config: expect.objectContaining({ id: "cam1", name: "左相机A" }) });
  });

  it("有未保存修改时点链接离开并取消 → 停留在相机页且修改保留", async () => {
    await openCamera("左相机 cam1");
    const name = await screen.findByRole("textbox", { name: "名称" });
    await userEvent.type(name, "X");
    await userEvent.click(screen.getByRole("link", { name: /前往飞拍标定/ }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "取消" }));
    expect(screen.queryByText("标定页")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("左相机X");
  });
});

describe("相机参数校验", () => {
  it("海康相机未绑定序列号 → 不能保存并提示选择设备", async () => {
    configs = [camera("cam1", "左相机", { serial: "" })];
    await openCamera("左相机 cam1");
    expect(await screen.findByRole("alert")).toHaveTextContent("请选择要绑定的相机设备");
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
  });

  it("相机已释放给 MVS 时修改参数 → 不能保存并提示先重新连接", async () => {
    released = ["cam1"];
    await openCamera("左相机 cam1");
    expect(await screen.findByRole("button", { name: "重新连接" })).toBeEnabled();
    await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "X");
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
    expect(screen.getByText("已释放给 MVS，重新连接后再保存")).toBeVisible();
  });
});

describe("出图验证", () => {
  it("软触发相机有未保存的参数修改 → 取一帧不可用并提示先保存", async () => {
    configs = [camera("cam1", "左相机", { triggerSource: "Software" })];
    await openCamera("左相机 cam1");
    const test = within(await screen.findByRole("region", { name: "出图验证" }));
    await waitFor(() => expect(test.getByRole("button", { name: "取一帧" })).toBeEnabled());
    await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "A");
    expect(test.getByRole("button", { name: "取一帧" })).toBeDisabled();
    expect(test.getByText("先保存并应用再测试")).toBeVisible();
  });
});

describe("应用到全部相机", () => {
  it("复制触发接线 → 其他已绑定相机写入同样的接线，已释放和未绑定的相机跳过并提示，不中断其余相机", async () => {
    configs = [camera("cam1", "左相机", { triggerDelayUs: 30, debouncerUs: 8, frameRate: 18 }), camera("cam2", "右相机"), camera("cam3", "新相机", { serial: "" }), camera("cam4", "后相机")];
    released = ["cam2"];
    await openCamera("左相机 cam1");
    const batch = within(await screen.findByLabelText("应用到全部相机"));
    await waitFor(() => expect(batch.getByRole("button", { name: "应用到全部相机" })).toBeEnabled());
    await userEvent.click(batch.getByRole("button", { name: "应用到全部相机" }));
    expect(await batch.findByText("跳过：已释放给 MVS，重新连接后再应用")).toBeVisible();
    expect(batch.getByText("跳过：未绑定设备")).toBeVisible();
    const saved = vi.mocked(invoke).mock.calls.filter(([command]) => command === "camera_save_config").map(([, args]) => args as { cam: number; config: CameraConfig });
    expect(saved).toEqual([{ cam: 3, config: expect.objectContaining({ id: "cam4", name: "后相机", serial: "cam4-SN", triggerDelayUs: 30, debouncerUs: 8, frameRate: 18 }) }]);
  });
});

describe("链路带宽", () => {
  it.each([
    ["三台相机 10 fps 合计超过单口预算 → 显示合计与带宽超预算警告", 943.7, true],
    ["三台相机 10 fps 合计未超单口预算 → 只显示合计，不显示警告", 786.4, false],
  ])("%s", async (_, mbpsTotal, overBudget) => {
    configs = [camera("cam1", "左相机"), camera("cam2", "右相机"), camera("cam3", "后相机")];
    bandwidth = { hz: 10, mbpsTotal, perCamera: [mbpsTotal / 3, mbpsTotal / 3, mbpsTotal / 3], budgetMbps: 800, overBudget };
    await openCamera("左相机 cam1");
    const region = await screen.findByRole("region", { name: "链路带宽" });
    await waitFor(() => expect(region).toHaveTextContent(`${Math.round(mbpsTotal)} Mbps`));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_bandwidth", { hz: 10 });
    if (overBudget) expect(within(region).getByText("带宽超预算")).toBeVisible();
    else expect(within(region).queryByText("带宽超预算")).not.toBeInTheDocument();
  });
});

describe("高级参数（调试）", () => {
  it("临时下发曝光与增益 → 只写入当前相机设备，不写配方", async () => {
    await openCamera("左相机 cam1");
    await userEvent.click(await screen.findByText("高级参数（调试）"));
    await userEvent.type(screen.getByRole("spinbutton", { name: "调试曝光（µs）" }), "8000");
    await userEvent.type(screen.getByRole("spinbutton", { name: "调试增益（dB）" }), "6");
    await userEvent.click(screen.getByRole("button", { name: "临时下发到设备" }));
    expect(await screen.findByText(/本次调试已下发/)).toBeVisible();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_set_imaging", { cam: 0, id: "cam1", imaging: { exposureUs: 8000, gainDb: 6 } });
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("workspace_set_camera_imaging", expect.anything());
  });
});

describe("相机组总览", () => {
  it("点相机卡的非按钮区域 → 进入该相机详情", async () => {
    renderPage();
    const card = await screen.findByRole("article", { name: "右相机 cam2" });
    await userEvent.click(within(card).getByText("帧计数"));
    expect(await switcher().findByRole("button", { name: "右相机 cam2" })).toHaveAttribute("aria-current", "true");
  });

  it.each([
    ["双击设备序列号选词", async (card: HTMLElement) => { await userEvent.dblClick(within(card).getByText("cam2-SN")); }],
    ["拖选相机名称后松开", async (card: HTMLElement) => {
      const name = within(card).getByText("右相机");
      const range = document.createRange();
      range.selectNodeContents(name);
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(range);
      fireEvent.click(name);
    }],
  ])("%s → 停留在总览，不进入详情", async (_, select) => {
    renderPage();
    const card = await screen.findByRole("article", { name: "右相机 cam2" });
    await select(card);
    expect(screen.getByRole("article", { name: "右相机 cam2" })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "切换相机" })).not.toBeInTheDocument();
    window.getSelection()!.removeAllRanges();
  });
});


describe("触发方式", () => {
  it("修改 Line0 帧率后保存 → 保留高电平并写入新帧率", async () => {
    await openCamera("左相机 cam1");
    const fps=screen.getByRole("spinbutton",{name:"设定帧率"});
    fireEvent.change(fps,{target:{value:"15"}});
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    await waitFor(()=>expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_save_config",{cam:0,config:expect.objectContaining({frameRate:15,triggerActivation:"LevelHigh"})}));
  });
  it("Software 调试 → 隐藏硬件触发参数且允许单帧取图", async () => {
    configs=[camera("cam1","左相机",{triggerSource:"Software"})];
    await openCamera("左相机 cam1");
    expect(screen.queryByRole("spinbutton",{name:"设定帧率"})).not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton",{name:"输入滤波（µs）"})).not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton",{name:"触发延时（µs）"})).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button",{name:"取一帧"}));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_soft_trigger",{cam:0});
  });
});

it("旧 Software 边沿配置切回 Line0 → 保存为高电平", async()=>{
 Object.defineProperty(HTMLElement.prototype,"scrollIntoView",{value:()=>{},configurable:true});
 configs=[camera("cam1","左相机",{triggerSource:"Software",triggerActivation:"RisingEdge" as CameraConfig["triggerActivation"]})];
 await openCamera("左相机 cam1");
 screen.getByRole("combobox",{name:"触发源"}).focus();
 await userEvent.keyboard("{ArrowDown}");
 fireEvent.click(await screen.findByRole("option",{name:"Line0（高电平连续采集）"}));
 await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
 await waitFor(()=>expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_save_config",{cam:0,config:expect.objectContaining({triggerSource:"Line0",triggerActivation:"LevelHigh"})}));
});

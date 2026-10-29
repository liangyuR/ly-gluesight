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
const status = (cam: number, config: CameraConfig): CameraStatus => ({ cam, id: config.id, name: config.name, source: "mvs", ready: true, released: false, message: "", device: device(config.serial), sdkVersion: null, frames: 0, fps: 0, maxFps: null, configuredFrameRate: 10, lostPackets: 0, droppedFrames: 0, warnings: [], imaging: { exposureUs: 60, gainDb: 6 } });

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

});

describe("相机参数校验", () => {

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
  it("设备因触发参数初始化失败 → 参数面板保留原始故障，Fault 下历史 PLC 锁不阻止释放", async () => {
    const message = "未适配型号不支持所需的高电平门控，TriggerSelector 仅支持 FrameBurstStart";
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "camera_rig_status") return configs.map((c, i) => ({ ...status(i, c), ready: false, device: null, message }));
      if (command === "cycle_snapshot") return { ...snapshot("FAULT"), plcLocked: true };
      return original(command, args, options);
    });
    await openCamera("左相机 cam1");
    const applied = within(screen.getByRole("region", { name: "参数生效情况" }));
    expect(applied.getByText("相机未就绪，参数尚未确认")).toBeVisible();
    expect(applied.getByText(message)).toBeVisible();
    expect(screen.queryByText("相机未连接")).not.toBeInTheDocument();
    const release = screen.getByRole("button", { name: "释放给 MVS" });
    expect(release).toBeEnabled();
    await userEvent.click(release);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_release", { cam: 0 });
  });

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

describe("触发方式", () => {
  it("修改 Line0 帧率后保存 → 保留高电平并写入新帧率", async () => {
    await openCamera("左相机 cam1");
    const fps=screen.getByRole("spinbutton",{name:"设定帧率"});
    fireEvent.change(fps,{target:{value:"15"}});
    await userEvent.click(screen.getByRole("button",{name:"保存并应用"}));
    await waitFor(()=>expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_save_config",{cam:0,config:expect.objectContaining({frameRate:15,triggerActivation:"LevelHigh"})}));
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

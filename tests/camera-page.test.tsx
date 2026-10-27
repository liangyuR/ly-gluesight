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

  it("有未保存修改时点链接离开并取消 → 停留在相机页且修改保留", async () => {
    await openCamera("左相机 cam1");
    const name = await screen.findByRole("textbox", { name: "名称" });
    await userEvent.type(name, "X");
    await userEvent.click(screen.getByRole("link", { name: /前往标定/ }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "取消" }));
    expect(screen.queryByText("标定页")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("左相机X");
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

describe("应用到全部相机", () => {
  it.each([{ warnings: [] }, { warnings: ["不支持 Chunk：设备未提供 ChunkModeActive"] }])("JHEM 门控已验收 → 不再提示待验收且仍显示真实参数问题（$warnings）", async ({ warnings }) => {
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "camera_rig_status") return configs.map((c, i) => ({ ...status(i, c), device: { ...device(c.serial), model: "JHEM131GC-XS" }, warnings }));
      if (command === "camera_save_config") return warnings;
      return original(command, args, options);
    });
    await openCamera("左相机 cam1");
    const applied = within(screen.getByRole("region", { name: "参数生效情况" }));
    expect(applied.queryByText("实机验收提醒")).not.toBeInTheDocument();
    expect(applied.getByText(warnings.length ? "已应用，1 项参数相机未接受" : "已应用，相机全部接受")).toBeVisible();
    if (warnings.length) {
      expect(applied.getByText(/部分参数可能已写入设备，未进行整组回滚/)).toBeVisible();
      expect(screen.getByText(/相机未接受：不支持 Chunk/)).toHaveTextContent("请核对设备实际值");
    }
    expect(screen.queryByText(/保持写入前的值/)).not.toBeInTheDocument();
    const batch = within(screen.getByLabelText("应用到全部相机"));
    await userEvent.click(batch.getByRole("button", { name: "应用到全部相机" }));
    expect(await batch.findByText(warnings.length ? /应用到全部相机：0 台已应用，1 台需处理/ : /应用到全部相机：1 台已应用/)).toBeVisible();
    expect(batch.queryByText(/高电平门控待实机验收/)).not.toBeInTheDocument();
    expect(batch.getByText(warnings.length ? "已应用，1 项参数相机未接受" : "已应用")).toBeVisible();
  });

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

describe("曝光与增益", () => {
  it("改曝光后保存 → 按相机编号写入相机组配置，显示已保存的值", async () => {
    await openCamera("左相机 cam1");
    const region = within(await screen.findByRole("region", { name: "曝光与增益" }));
    const exposure = region.getByRole("spinbutton", { name: "曝光时间（µs）" });
    await waitFor(() => expect(exposure).toHaveValue(60));
    await userEvent.clear(exposure);
    await userEvent.type(exposure, "8000");
    await userEvent.click(region.getByRole("button", { name: "保存曝光与增益" }));
    expect(await region.findByText(/已保存/)).toBeVisible();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_set_imaging", { cam: 0, id: "cam1", imaging: { exposureUs: 8000, gainDb: 6 } });
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

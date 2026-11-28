import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CameraPage from "../src/pages/CameraPage";
import { defaultCameraConfig } from "../src/features/camera/api";
import type { BandwidthEstimate, CameraConfig, CameraStatus } from "../src/features/camera/types";
import type { WorkspaceCatalogEntry } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred, workspaceCatalogEntry } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const device = (serial: string) => ({ serial, model: "MV-CA050", userName: "", transport: "GigE" as const, ip: "192.168.1.10" });
const camera = (id: string, name: string, over: Partial<CameraConfig> = {}): CameraConfig => ({ ...defaultCameraConfig, id, name, serial: id + "-SN", ...over });
const status = (cam: number, config: CameraConfig): CameraStatus => ({ cam, id: config.id, name: config.name, source: "mvs", ready: true, released: false, message: "", device: device(config.serial), sdkVersion: null, frames: 0, fps: 0, maxFps: null, configuredFrameRate: 10, lostPackets: 0, droppedFrames: 0, warnings: [], imaging: { exposureUs: 60, gainDb: 6 } });

let configs: CameraConfig[];
let released: string[];
let bandwidth: BandwidthEstimate;
let workspaceChanged: ((id: string) => void) | undefined;
beforeEach(() => {
  configs = [camera("cam1", "左相机"), camera("cam2", "右相机")];
  released = [];
  bandwidth = { hz: 10, mbpsTotal: 524.3, perCamera: [262.1, 262.1], budgetMbps: 800, overBudget: false };
  workspaceChanged = undefined;
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "workspace://changed") {
      workspaceChanged = id => handler({ event, id: 1, payload: id } as TauriEvent<never>);
      return () => { workspaceChanged = undefined; };
    }
    return () => {};
  });
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "camera_rig_config": return configs;
      case "camera_bandwidth": return bandwidth;
      case "camera_rig_status": return configs.map((c, i) => ({ ...status(i, c), ...(released.includes(c.id) ? { ready: false, released: true, device: null } : {}) }));
      case "camera_list_devices": return configs.map(c => device(c.serial));
      case "camera_save_config": return { saved: true, applied: true, warnings: [], applyError: null };
      case "camera_preview": return new ArrayBuffer(0);
      case "recipe_list": return { recipes: [], errors: [] };
      case "cycle_control_state": return { productionBusy: false, plcLocked: false, plcHandshake: null, maintenance: null };
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

async function notifyWorkspaceChanged(id: string) {
  expect(workspaceChanged).toBeDefined();
  await act(async () => workspaceChanged!(id));
}

it("相机仅被待生效配方引用 → 配方目录仍阻止移除并显示引用来源", async () => {
  const draft = workspaceCatalogEntry(burstWorkspaceView(burstRecipe({ id: "A" })).workspace);
  const catalog = [{ ...draft, cameras: ["cam1"], publish: { state: "waiting" },
    pending: { id: draft.id, name: "待生效配方", productCode: draft.productCode, cameras: ["cam2"] } }];
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) =>
    command === "workspace_catalog" ? Promise.resolve(catalog) : original(command, args, options));
  await openCamera("右相机 cam2");

  expect(await screen.findByText(/被 待发布配方 待生效配方（A） 引用/)).toBeVisible();
  expect(screen.getByRole("button", { name: "移除相机" })).toBeDisabled();
});

it("生产配方生效后解除相机引用 → 当前详情可移除，同时保留编辑中的名称", async () => {
  const old = { ...burstSummary(burstRecipe({ id: "A", name: "工件 A" })), cameras: ["cam2"] };
  let recipes = [old];
  const ipc = vi.mocked(invoke);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args, options) => command === "recipe_list"
    ? Promise.resolve({ recipes, errors: [], unavailable: [] }) : original(command, args, options));
  const rigReads = () => ipc.mock.calls.filter(([command]) => command === "camera_rig_config").length;
  await openCamera("右相机 cam2");
  expect(await screen.findByText(/被 生产配方 工件 A（A） 引用/)).toBeVisible();
  expect(screen.getByRole("button", { name: "移除相机" })).toBeDisabled();
  await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "（待保存）");
  const readsBefore = rigReads();

  recipes = [{ ...old, version: 2, revisionId: "A-v2", cameras: ["cam1"] }];
  await notifyWorkspaceChanged("A");

  expect(await screen.findByText("未被任何配方引用，可移除。")).toBeVisible();
  expect(screen.getByRole("button", { name: "移除相机" })).toBeEnabled();
  expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("右相机（待保存）");
  expect(switcher().getByRole("button", { name: "右相机 cam2" })).toHaveAttribute("aria-current", "true");
  expect(rigReads()).toBe(readsBefore);
});

it("引用查询期间再次收到配方通知 → 旧的无引用结果不能放开移除", async () => {
  const previous = deferred<WorkspaceCatalogEntry[]>(), refreshed = deferred<WorkspaceCatalogEntry[]>();
  const draft = workspaceCatalogEntry(burstWorkspaceView(burstRecipe({ id: "A" })).workspace);
  const pending = { id: draft.id, name: "待生效配方", productCode: draft.productCode, cameras: ["cam2"] };
  const readCatalog = vi.fn<() => Promise<WorkspaceCatalogEntry[]>>().mockResolvedValue([])
    .mockResolvedValueOnce([]).mockReturnValueOnce(previous.promise).mockReturnValueOnce(refreshed.promise);
  const ipc = vi.mocked(invoke);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args, options) =>
    command === "workspace_catalog" ? readCatalog() : original(command, args, options));
  await openCamera("右相机 cam2");
  const remove = () => screen.getByRole("button", { name: "移除相机" });
  await waitFor(() => expect(remove()).toBeEnabled());

  await notifyWorkspaceChanged("A");
  expect(remove()).toBeDisabled();
  await waitFor(() => expect(readCatalog).toHaveBeenCalledTimes(2));
  await notifyWorkspaceChanged("A");
  await act(async () => previous.resolve([]));
  expect(remove()).toBeDisabled();

  await act(async () => refreshed.resolve([{ ...draft, cameras: ["cam1"], publish: { state: "waiting" }, pending }]));
  expect(await screen.findByText(/被 待发布配方 待生效配方（A） 引用/)).toBeVisible();
  expect(remove()).toBeDisabled();
});

it("确认移除时相机索引已变化 → 保留目标，重新确认后才按新索引移除", async () => {
  configs.push(camera("cam3", "第三相机"));
  const ipc = vi.mocked(invoke);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(async (command, args, options) => {
    if (command !== "camera_remove") return original(command, args, options);
    const { cam, id } = args as { cam: number; id: string };
    if (configs[cam]?.id !== id) throw new Error("相机组已变化，请刷新配置后重试移除");
    configs = configs.filter((_, index) => index !== cam);
  });
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(true).mockImplementationOnce(() => {
    configs = configs.filter(({ id }) => id !== "cam1");
    return true;
  });
  const removals = () => ipc.mock.calls.filter(([command]) => command === "camera_remove").map(([, args]) => args);
  const remove = () => screen.getByRole("button", { name: "移除相机" });
  const user = userEvent.setup();

  try {
    await openCamera("右相机 cam2");
    await waitFor(() => expect(remove()).toBeEnabled());
    await user.click(remove());

    expect(await screen.findByText(/相机组已变化/)).toBeVisible();
    await waitFor(() => expect(remove()).toBeEnabled());
    expect(switcher().getByRole("button", { name: "右相机 cam2" })).toHaveAttribute("aria-current", "true");
    expect(switcher().getByRole("button", { name: "第三相机 cam3" })).toBeVisible();
    expect(switcher().queryByRole("button", { name: "左相机 cam1" })).not.toBeInTheDocument();
    expect(removals()).toEqual([{ cam: 1, id: "cam2" }]);
    expect(confirm).toHaveBeenCalledOnce();

    await user.click(remove());
    expect(await screen.findByRole("article", { name: "第三相机 cam3" })).toBeVisible();
    expect(screen.queryByRole("article", { name: "右相机 cam2" })).not.toBeInTheDocument();
    expect(removals()).toEqual([{ cam: 1, id: "cam2" }, { cam: 0, id: "cam2" }]);
    expect(confirm).toHaveBeenCalledTimes(2);
  } finally {
    confirm.mockRestore();
  }
});

describe("相机参数未保存修改", () => {
  it("配置落盘但设备离线 → 显示应用失败、保留当前页面且不再提示未保存", async () => {
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "camera_save_config") {
        const { cam, config } = args as { cam: number; config: CameraConfig };
        configs = configs.map((current, i) => i === cam ? config : current);
        return { saved: true, applied: false, warnings: [], applyError: "设备未连接" };
      }
      return original(command, args, options);
    });
    await openCamera("左相机 cam1");
    await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "A");
    await userEvent.click(switcher().getByRole("button", { name: "右相机 cam2" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "保存并应用" }));
    expect(await screen.findByText(/配置已保存，设备应用失败：设备未连接/)).toBeVisible();
    expect(switcher().getByRole("button", { name: "左相机A cam1" })).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("button", { name: "放弃" })).toBeDisabled();
    await userEvent.click(switcher().getByRole("button", { name: "右相机 cam2" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(switcher().getByRole("button", { name: "右相机 cam2" })).toHaveAttribute("aria-current", "true");
  });

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

  it("保存彩色相机的白平衡后再改曝光 → 重连设定保留三通道原生刻度", async () => {
    const wb = { unit: "deviceInteger", red: 1600, green: 1024, blue: 1230 } as const;
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "camera_rig_status") return configs.map((c, i) => ({ ...status(i, c), observedWhiteBalance: wb }));
      return original(command, args, options);
    });
    await openCamera("左相机 cam1");
    await userEvent.click(screen.getByRole("button", { name: "保存当前白平衡" }));
    expect(await screen.findByText(/重连设定：红 1600 · 绿 1024 · 蓝 1230/)).toBeVisible();
    const region = within(screen.getByRole("region", { name: "曝光与增益" }));
    const exposure = region.getByLabelText("曝光时间（µs）");
    fireEvent.change(exposure, { target: { value: "80" } });
    await userEvent.click(region.getByRole("button", { name: "保存曝光与增益" }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_set_imaging", {
      cam: 0, id: "cam1", imaging: { exposureUs: 80, gainDb: 6, whiteBalance: wb },
    });
    expect(region.getByText(/重连设定：红 1600 · 绿 1024 · 蓝 1230/)).toBeVisible();
  });

  it("相机已释放给 MVS 时修改参数 → 不能保存并提示先重新连接", async () => {
    released = ["cam1"];
    await openCamera("左相机 cam1");
    const reconnect = await screen.findByRole("button", { name: "重新连接" });
    expect(reconnect).toBeEnabled();
    await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "X");
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
    expect(screen.getByText("已释放给 MVS，重新连接后再保存")).toBeVisible();
    await userEvent.click(reconnect);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_reconnect", { cam: 0, id: "cam1" });
  });
});

it("相机总览中重连故障设备 → 按该卡片的相机身份操作，错误留在该卡片", async () => {
  const ipc = vi.mocked(invoke);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(async (command, args, options) => {
    if (command === "camera_rig_status") return configs.map((config, index) => ({
      ...status(index, config), ...(config.id === "cam2" ? { ready: false, message: "设备连接中断" } : {}),
    }));
    if (command === "camera_reconnect") throw new Error("相机仍被 MVS 占用");
    return original(command, args, options);
  });
  renderPage();
  const faulty = within(await screen.findByRole("article", { name: "右相机 cam2" }));
  await userEvent.click(await faulty.findByRole("button", { name: "立即重新连接" }));

  expect(await faulty.findByText(/相机仍被 MVS 占用/)).toBeVisible();
  expect(faulty.getByRole("button", { name: "立即重新连接" })).toBeEnabled();
  expect(within(screen.getByRole("article", { name: "左相机 cam1" })).queryByText(/相机仍被 MVS 占用/)).not.toBeInTheDocument();
  expect(ipc).toHaveBeenCalledWith("camera_reconnect", { cam: 1, id: "cam2" });
});

describe("出图验证", () => {
  it("设备状态来自同一位置的另一台相机 → 详情不允许按该状态释放或重连", async () => {
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "camera_rig_status") return [
        status(0, configs[0]),
        { ...status(1, camera("cam3", "另一台相机")), ready: false, released: true, device: null },
      ];
      return original(command, args, options);
    });
    await openCamera("右相机 cam2");

    const preview = within(screen.getByRole("region", { name: "出图验证" }));
    expect(preview.getByRole("button", { name: "释放给 MVS" })).toBeDisabled();
    expect(preview.queryByRole("button", { name: "重新连接" })).not.toBeInTheDocument();
  });

  it("设备因触发参数初始化失败 → 参数面板保留原始故障，Fault 下历史 PLC 锁不阻止释放", async () => {
    const message = "未适配型号不支持所需的高电平门控，TriggerSelector 仅支持 FrameBurstStart";
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      if (command === "camera_rig_status") return configs.map((c, i) => ({ ...status(i, c), ready: false, device: null, message }));
      if (command === "cycle_control_state") return { productionBusy: false, plcLocked: true, plcHandshake: null, maintenance: null };
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
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_release", { cam: 0, id: "cam1" });
  });

  it("软触发相机有未保存的参数修改 → 取一帧不可用并提示先保存", async () => {
    configs[1] = camera("cam2", "右相机", { triggerSource: "Software" });
    const capture = deferred<void>();
    const ipc = vi.mocked(invoke);
    const original = ipc.getMockImplementation()!;
    ipc.mockImplementation((command, args, options) =>
      command === "camera_soft_trigger" ? capture.promise : original(command, args, options));
    await openCamera("右相机 cam2");
    const preview = within(await screen.findByRole("region", { name: "出图验证" }));
    await waitFor(() => expect(preview.getByRole("button", { name: "取一帧" })).toBeEnabled());
    await userEvent.click(preview.getByRole("button", { name: "取一帧" }));
    expect(preview.getByRole("button", { name: "取图中…" })).toBeDisabled();
    expect(ipc).toHaveBeenCalledWith("camera_soft_trigger", { cam: 1, id: "cam2" });
    await act(async () => capture.resolve(undefined));

    await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "A");
    expect(preview.getByRole("button", { name: "取一帧" })).toBeDisabled();
    expect(preview.getByText("先保存并应用再测试")).toBeVisible();
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

it("切换外部模拟器并选灰度 → 保存匹配输入通道的 TCP 单视角 Line0 配置", async () => {
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { value: () => {}, configurable: true });
  configs = [camera("cam1", "左相机", { triggerSource: "Software" })];
  await openCamera("左相机 cam1");
  screen.getByRole("combobox", { name: "图像源" }).focus();
  await userEvent.keyboard("{ArrowDown}");
  await userEvent.click(await screen.findByRole("option", { name: "外部模拟器" }));
  const save = screen.getByRole("button", { name: "保存并应用" });
  expect(save).toBeDisabled();
  await userEvent.type(screen.getByRole("textbox", { name: "模拟器地址" }), "127.0.0.1:7101");
  screen.getByRole("combobox", { name: "图像模式" }).focus();
  await userEvent.keyboard("{ArrowDown}");
  await userEvent.click(await screen.findByRole("option", { name: "灰度" }));
  await userEvent.click(save);
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_save_config", { cam: 0, config: expect.objectContaining({ source: "simulator", serial: "127.0.0.1:7101", imageMode: "gray", triggerSource: "Line0", triggerActivation: "LevelHigh", viewCount: 1 }) }));
});

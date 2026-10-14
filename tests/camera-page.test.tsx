import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CameraPage from "../src/pages/CameraPage";
import { defaultCameraConfig } from "../src/features/camera/api";
import type { CameraConfig, CameraStatus } from "../src/features/camera/types";
import { siteStatus, snapshot } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const device = (serial: string) => ({ serial, model: "MV-CA050", userName: "", transport: "GigE" as const, ip: "192.168.1.10" });
const camera = (id: string, name: string, over: Partial<CameraConfig> = {}): CameraConfig => ({ ...defaultCameraConfig, id, name, serial: id + "-SN", ...over });
const status = (cam: number, config: CameraConfig): CameraStatus => ({ cam, id: config.id, name: config.name, source: "mvs", acquisition: "triggered", ready: true, message: "", device: device(config.serial), sdkVersion: null, frames: 0, fps: 0, maxFps: null, lostPackets: 0, droppedFrames: 0, warnings: [] });

let configs: CameraConfig[];
beforeEach(() => {
  configs = [camera("cam1", "左相机"), camera("cam2", "右相机")];
  vi.mocked(listen).mockResolvedValue(() => {});
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "camera_rig_config": return configs;
      case "camera_rig_status": return configs.map((c, i) => status(i, c));
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
const list = () => within(screen.getByRole("complementary", { name: "相机列表" }));

describe("相机参数未保存修改", () => {
  it("改名后切换相机并选保存并应用 → 先把修改写入当前相机，再切到目标相机", async () => {
    renderPage();
    const name = await screen.findByRole("textbox", { name: "名称" });
    await userEvent.clear(name);
    await userEvent.type(name, "左相机A");
    await userEvent.click(list().getByRole("button", { name: "右相机 cam2" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "保存并应用" }));
    await waitFor(() => expect(list().getByRole("button", { name: "右相机 cam2" })).toHaveAttribute("aria-current", "true"));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("camera_save_config", { cam: 0, config: expect.objectContaining({ id: "cam1", name: "左相机A" }) });
  });

  it("有未保存修改时点链接离开并取消 → 停留在相机页且修改保留", async () => {
    renderPage();
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
    renderPage();
    expect(await screen.findByRole("alert")).toHaveTextContent("请选择要绑定的相机设备");
    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
  });
});

describe("出图测试", () => {
  it("软触发相机有未保存的参数修改 → 软触发不可用并提示先保存", async () => {
    configs = [camera("cam1", "左相机", { triggerSource: "Software" })];
    renderPage();
    const test = within(await screen.findByRole("region", { name: "出图测试" }));
    await waitFor(() => expect(test.getByRole("button", { name: "软触发一次" })).toBeEnabled());
    await userEvent.type(screen.getByRole("spinbutton", { name: "触发延时（µs）" }), "5");
    expect(test.getByRole("button", { name: "软触发一次" })).toBeDisabled();
    expect(test.getByText("先保存并应用再测试")).toBeVisible();
  });
});

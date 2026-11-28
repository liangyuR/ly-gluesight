import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent } from "@tauri-apps/api/event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import type { CameraStatus } from "../src/features/camera/types";
import type { PlcStatus } from "../src/features/plc/types";
import type { WorkspaceCatalogEntry } from "../src/features/workspace/types";
import OperationGuidePage from "../src/pages/OperationGuidePage";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred, workspaceCatalogEntry } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

type CameraOverview = Pick<CameraStatus, "id" | "name" | "ready" | "released">;
const camera: CameraOverview = { id: "cam1", name: "左相机", ready: true, released: false };
const readCameras = vi.fn<() => Promise<CameraOverview[]>>();
const readPlc = vi.fn<() => Promise<Pick<PlcStatus, "state" | "message">>>();
let recipes: ReturnType<typeof burstSummary>[];
let drafts: WorkspaceCatalogEntry[];
let workspaceChanged: ((id: string) => void) | undefined;

beforeEach(() => {
  const doc = burstRecipe({ id: "A" });
  recipes = [burstSummary(doc)];
  drafts = [workspaceCatalogEntry(burstWorkspaceView(doc).workspace)];
  workspaceChanged = undefined;
  readCameras.mockReset().mockResolvedValue([camera]);
  readPlc.mockReset().mockResolvedValue({ state: "connected", message: "" });
  vi.mocked(listen).mockReset().mockImplementation(async (event, handler) => {
    if (event === "workspace://changed") {
      workspaceChanged = id => handler({ event, id: 1, payload: id } as TauriEvent<never>);
      return () => { workspaceChanged = undefined; };
    }
    return () => {};
  });
  vi.mocked(invoke).mockReset().mockImplementation(async command => {
    switch (command) {
      case "camera_rig_status": return readCameras();
      case "plc_get_status": return readPlc();
      case "recipe_list": return { recipes, errors: [], unavailable: [] };
      case "workspace_catalog": return drafts;
      default: throw new Error(`未声明的桌面调用：${command}`);
    }
  });
});

const show = () => render(<MemoryRouter><OperationGuidePage /></MemoryRouter>);
const card = (name: string) => within(screen.getByRole("region", { name }));

it("配方变更通知 → 更新发布队列，设备状态继续显示且不重新查询", async () => {
  show();
  expect(await card("采集设备").findByText("设备就绪")).toBeVisible();
  expect(await card("PLC 通讯").findByText("已连接")).toBeVisible();
  expect(await card("配方版本").findByText("无待生效版本")).toBeVisible();

  recipes = [...recipes, burstSummary(burstRecipe({ id: "B", productCode: 3 }))];
  const { id, name, productCode, cameras } = drafts[0];
  drafts = [{ ...drafts[0], publish: { state: "waiting" }, pending: { id, name, productCode, cameras } }];
  expect(workspaceChanged).toBeDefined();
  await act(async () => workspaceChanged!("A"));

  expect(await card("配方版本").findByText("1 个待生效 · 0 个需处理")).toBeVisible();
  expect(card("配方版本").getByText("已发布 2 个")).toBeVisible();
  expect(card("采集设备").getByText("设备就绪")).toBeVisible();
  expect(card("PLC 通讯").getByText("已连接")).toBeVisible();
  expect(readCameras).toHaveBeenCalledOnce();
  expect(readPlc).toHaveBeenCalledOnce();
});

it("相机读取未完成时连点手动刷新 → 保留已读状态，只补查一次最新值", async () => {
  const previous = deferred<CameraOverview[]>(), refreshed = deferred<CameraOverview[]>();
  readCameras.mockResolvedValueOnce([camera])
    .mockReturnValueOnce(previous.promise).mockReturnValueOnce(refreshed.promise);
  vi.useFakeTimers();
  try {
    show();
    await act(() => vi.advanceTimersByTimeAsync(0));
    const refresh = screen.getByRole("button", { name: "刷新总览状态" });
    const devices = () => card("采集设备");
    expect(devices().getByText("设备就绪")).toBeVisible();

    fireEvent.click(refresh);
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(readCameras).toHaveBeenCalledTimes(2);
    fireEvent.click(refresh);
    fireEvent.click(refresh);
    expect(readCameras).toHaveBeenCalledTimes(2);
    expect(devices().getByText("设备就绪")).toBeVisible();

    await act(async () => previous.resolve([{ ...camera, name: "过期相机状态", ready: false }]));
    expect(readCameras).toHaveBeenCalledTimes(3);
    expect(devices().queryByText("过期相机状态")).not.toBeInTheDocument();
    expect(devices().getByText("设备就绪")).toBeVisible();

    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(readCameras).toHaveBeenCalledTimes(3);
    await act(async () => refreshed.resolve([{ ...camera, ready: false, released: true }]));
    expect(devices().getByText("已释放")).toBeVisible();
    expect(readCameras).toHaveBeenCalledTimes(3);
  } finally {
    vi.useRealTimers();
  }
});

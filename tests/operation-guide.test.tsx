import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import OperationGuidePage from "../src/pages/OperationGuidePage";

const api = vi.hoisted(() => ({ camera: vi.fn(), plc: vi.fn(), recipes: vi.fn(), drafts: vi.fn(), site: vi.fn() }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: () => true }));
vi.mock("../src/features/camera", () => ({ cameraApi: { rigStatus: api.camera } }));
vi.mock("../src/features/plc", () => ({ plcApi: { getStatus: api.plc }, linkStateLabels: { connected: "已连接" }, subscribe: () => () => {} }));
vi.mock("../src/features/cycle", () => ({ recipeApi: { list: api.recipes }, siteApi: { status: api.site }, siteModeText: () => "真机" }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: api.drafts } }));

beforeEach(() => {
  api.camera.mockResolvedValue([]);
  api.plc.mockResolvedValue({ state: "connected", message: "通讯连接成功" });
  api.recipes.mockResolvedValue({ recipes: [], errors: [] });
  api.drafts.mockResolvedValue([]);
  api.site.mockResolvedValue({ mode: "real" });
});

it("设备未返回且生产库读取失败 → 不伪造零值，独立显示已连接的 PLC", async () => {
  api.camera.mockReturnValue(new Promise(() => {}));
  api.recipes.mockRejectedValue(new Error("配方库不可读"));
  render(<MemoryRouter><OperationGuidePage /></MemoryRouter>);
  const camera = within(screen.getByRole("region", { name: "采集设备" }));
  expect(camera.getByText("读取中…")).toBeVisible();
  expect(camera.queryByText("未配置相机")).not.toBeInTheDocument();
  expect(await screen.findByText("配方库不可读")).toBeVisible();
  expect(screen.queryByText("0 个已发布")).not.toBeInTheDocument();
  expect(screen.getByText("已连接")).toBeVisible();
});

it("设备连接成功但没有已发布配方 → 不宣告生产或流程完成", async () => {
  api.camera.mockResolvedValue([{ id: "CAM-1", name: "相机 1", ready: true, message: "设备已连接" }]);
  render(<MemoryRouter><OperationGuidePage /></MemoryRouter>);
  expect(await screen.findByText("1/1 设备就绪")).toBeVisible();
  expect(await screen.findByText("0 个已发布")).toBeVisible();
  expect(screen.queryByText(/^(可生产|已完成)$/)).not.toBeInTheDocument();
});

it.each(["sampleSet", "synthetic"])("离线来源 %s → 标明模拟范围并提供对应调试入口", async source => {
  api.site.mockResolvedValue({ mode: "offline", source });
  render(<MemoryRouter><OperationGuidePage /></MemoryRouter>);
  const title = source === "sampleSet" ? "候选离线试运行" : "合成流程演练";
  const workflow = within(await screen.findByRole("region", { name: title }));
  expect(screen.queryByRole("region", { name: "日常生产" })).not.toBeInTheDocument();
  expect(screen.getByText(/非真实 PLC/)).toBeVisible();
  expect(workflow.getByRole("link", { name: source === "sampleSet" ? "打开候选试运行" : "打开离线演练" })).toHaveAttribute("href", source === "sampleSet" ? "/inspect" : "/offline");
  if (source === "sampleSet") expect(workflow.getByText(/无需发布.*结果标记为调试/)).toBeVisible();
});

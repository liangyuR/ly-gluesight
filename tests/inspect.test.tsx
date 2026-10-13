import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import InspectPage from "../src/pages/InspectPage";
import { cycleApi, useCycle, useLayout } from "../src/features/cycle/api";
import { workspaceApi } from "../src/features/workspace/api";
import { plcApi } from "../src/features/plc/api";
import { readiness, siteStatus, snapshot, summary, workspaceView } from "./fixtures";
import { cycleMeasurement, cyclePart, cycleResult } from "./cycle-visual-fixtures";
import { plcConfig } from "./plc-fixtures";
import type { LogLine, Measured, Readiness, Recipe, RecipeSummary, SiteStatus, Snapshot } from "../src/features/cycle/types";
import type { PointValue } from "../src/features/plc/types";

vi.mock("../src/features/cycle/api", () => ({
  useCycle: vi.fn(), useRecipes: () => recipeList, useLayout: vi.fn(),
  useSiteStatus: () => site, useReadiness: () => ({ readiness: report, refresh: async () => report }), cycleApi: { selectRecipe: vi.fn(), reset: vi.fn(), offlineRun: vi.fn(), offlineStop: vi.fn() },
}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { runtimeOverview: vi.fn(), liveImage: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getConfig: vi.fn() }, usePlcValues: () => values,
  usePlcStatus: () => ({ state: "connected", since: 1 }), subscribe: () => () => {} }));

let state: Snapshot, layout: Recipe | null, logs: LogLine[], measured: Measured[];
let recipeList: RecipeSummary[];
let site: SiteStatus;
let report: Readiness;
let values: Record<string, PointValue>;
beforeEach(() => {
  state = snapshot(); layout = workspaceView().layout; logs = []; measured = []; values = {};
  recipeList = [summary(), summary(workspaceView("B"))];
  site = siteStatus();
  report = readiness();
  vi.mocked(useCycle).mockImplementation(() => ({ snapshot: state, logs, measured }));
  vi.mocked(useLayout).mockImplementation(() => layout);
  vi.mocked(cycleApi.selectRecipe).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.reset).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.offlineRun).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.offlineStop).mockReset().mockResolvedValue(undefined);
  vi.mocked(workspaceApi.runtimeOverview).mockReset().mockResolvedValue(null);
  vi.mocked(workspaceApi.liveImage).mockReset().mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(plcApi.getConfig).mockReset().mockResolvedValue(plcConfig());
});
const manual = () => screen.getByRole("combobox", { name: "当前检测配方" });

describe("在线检测切型锁定", () => {
  it("切型确认期间开始接收工件，禁止切换并允许取消", async () => {
    const page = render(<InspectPage />);
    await userEvent.selectOptions(manual(), "B");
    state = { ...state, phase: "ACQUIRE" };
    page.rerender(<InspectPage />);
    expect(screen.getByRole("button", { name: "确认切换" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("暂时不能切换配方");
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
  });
});

describe("在线检测离线产线控件", () => {
  it("真机模式 → 不显示离线运行控件", () => {
    render(<InspectPage />);
    expect(screen.queryByRole("button", { name: "运行一件" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "离线配方" })).not.toBeInTheDocument();
  });

  it("离线模式运行一件 → 按所选配方请求正常件单件运行", async () => {
    site = siteStatus({ mode: "offline", source: "synthetic", measurement: "simulated", port: 50102 });
    render(<InspectPage />);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "离线配方" }), "B");
    await userEvent.click(screen.getByRole("button", { name: "运行一件" }));
    expect(cycleApi.offlineRun).toHaveBeenCalledWith("B", "normal", false, null);
  });
});

describe("在线检测开工检查清单", () => {
  it("非可开工 → 清单列出阻塞项原因与去处", () => {
    const ok = readiness().items;
    report = readiness({ ready: false, items: ok.map(i => i.key === "recipe" ? { ...i, state: "blocked", detail: "没有可开工配方（A：未示教）", target: "workspace" } : i) });
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const list = within(screen.getByRole("region", { name: "开工检查清单" }));
    expect(list.getByText("没有可开工配方（A：未示教）")).toBeVisible();
    expect(list.getByRole("link", { name: "去配方工作台" })).toHaveAttribute("href", "/recipe");
  });

  it("在制件期间出现阻塞 → 保留本件检测中并显示下一件异常", () => {
    state = { ...state, phase: "ACQUIRE" };
    report = readiness({ ready: false, items: readiness().items.map(i => i.key === "audit" ? { ...i, state: "blocked", detail: "持久追溯故障" } : i) });
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.getByText("本件检测中／下一件条件异常")).toBeVisible();
    expect(screen.getByText("持久追溯故障")).toBeVisible();
  });
});

describe("在线检测选帧、曲线与快照绑定", () => {
  it("从整件布置选择拍照点 → 返回对应原图且保留整件判定", async () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() };
    render(<InspectPage />);
    const main = within(screen.getByRole("region", { name: "检测主视图" }));
    expect(main.getByRole("button", { name: "原图" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(main.getByRole("button", { name: "整件布置" }));
    fireEvent.click(main.getByRole("button", { name: "总览选择帧 k2" }));
    expect(main.getByRole("button", { name: "原图" })).toHaveAttribute("aria-pressed", "true");
    expect(main.getByText(/P2 · CAM-1 · 视角 1 · k2/)).toBeVisible();
    expect(screen.getByText("测试样本偏移超差")).toBeVisible();
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-1", "revisionId-A", 1));
    await userEvent.click(screen.getByRole("button", { name: "返回当前拍照点" }));
    expect(screen.getByText("跟随当前拍照点")).toBeVisible();
    expect(screen.getByText("测试样本偏移超差")).toBeVisible();
  });

  it("同 SN 重检的新周期清除旧选帧、选点、测量值和结论", async () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() }; measured = [cycleMeasurement()];
    const page = render(<InspectPage />);
    fireEvent.change(screen.getByRole("slider", { name: "位置曲线选点" }), { target: { value: "1" } });
    expect(screen.getByText(/点 2 · P1 · J1 · s=1.00/)).toBeVisible();
    state = { ...state, phase: "FAULT", fault: "新件相机错误", part: { ...cyclePart(1, "cycle-2"), frames: [cyclePart().frames[0], { ...cyclePart().frames[1], status: "waiting" }] } };
    page.rerender(<InspectPage />);
    expect(screen.getByText("暂无检测结果")).toBeVisible();
    expect(screen.queryByText("测试样本偏移超差")).not.toBeInTheDocument();
    expect(screen.queryByText(/点 2 · P1 · J1 · s=1.00/)).not.toBeInTheDocument();
    expect(screen.getByText(/P1 · CAM-1 · 视角 1 · k1/)).toBeVisible();
    expect(screen.getByText("跟随当前拍照点")).toBeVisible();
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-2", "revisionId-A", 0));
  });

  it("布局快照尚未匹配本件时不显示旧测量、旧帧或本件原图", async () => {
    state.part = cyclePart(); state.part.recipeRevision = "new-revisionId"; measured = [cycleMeasurement()];
    render(<InspectPage />);
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看帧 k1" })).toHaveClass("s-waiting");
    expect(workspaceApi.liveImage).not.toHaveBeenCalled();
    expect(screen.getByText("点击曲线或用方向键选择测量点")).toBeVisible();
  });
});

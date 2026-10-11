import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import InspectPage from "../src/pages/InspectPage";
import { cycleApi, useCycle, useLayout } from "../src/features/cycle/api";
import { workspaceApi } from "../src/features/workspace/api";
import { plcApi } from "../src/features/plc/api";
import { deferred, snapshot, summary, workspaceView } from "./fixtures";
import { cycleMeasurement, cyclePart, cycleResult } from "./cycle-visual-fixtures";
import { plcConfig } from "./plc-fixtures";
import type { LogLine, Measured, Recipe, RecipeSummary, Snapshot } from "../src/features/cycle/types";
import type { GrayImage } from "../src/features/workspace/types";
import type { PointValue } from "../src/features/plc/types";

vi.mock("../src/features/cycle/api", () => ({
  useCycle: vi.fn(), useRecipes: () => recipeList, useLayout: vi.fn(),
  useSimStatus: () => null, cycleApi: { selectRecipe: vi.fn(), reset: vi.fn(), simStart: vi.fn(), simStop: vi.fn() },
}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { runtimeOverview: vi.fn(), liveImage: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getConfig: vi.fn() }, usePlcValues: () => values,
  usePlcStatus: () => ({ state: "connected", since: 1 }), subscribe: () => () => {} }));

let state: Snapshot, layout: Recipe | null, logs: LogLine[], measured: Measured[];
let recipeList: RecipeSummary[];
let values: Record<string, PointValue>;
beforeEach(() => {
  state = snapshot(); layout = workspaceView().layout; logs = []; measured = []; values = {};
  recipeList = [summary(), summary(workspaceView("B"))];
  vi.mocked(useCycle).mockImplementation(() => ({ snapshot: state, logs, measured }));
  vi.mocked(useLayout).mockImplementation(() => layout);
  vi.mocked(cycleApi.selectRecipe).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.reset).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.simStart).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.simStop).mockReset().mockResolvedValue(undefined);
  vi.mocked(workspaceApi.runtimeOverview).mockReset().mockResolvedValue(null);
  vi.mocked(workspaceApi.liveImage).mockReset().mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(plcApi.getConfig).mockReset().mockResolvedValue(plcConfig());
});
const manual = () => screen.getByRole("combobox", { name: "当前检测配方" });

describe("在线检测切型与故障恢复", () => {
  it.each(["VALIDATE", "ACQUIRE", "DRAIN", "JUDGE", "REPORT", "RELEASE"] as const)("%s 阶段禁止切换型号", phase => {
    state.phase = phase; render(<InspectPage />); expect(manual()).toBeDisabled();
  });

  it("切换期间显示等待、禁止重复提交和关闭，失败恢复可重试", async () => {
    const request = deferred<void>(); vi.mocked(cycleApi.selectRecipe).mockReturnValueOnce(request.promise);
    render(<InspectPage />);
    await userEvent.selectOptions(manual(), "B");
    const confirm = screen.getByRole("button", { name: "确认切换" });
    fireEvent.click(confirm); fireEvent.click(confirm);
    expect(cycleApi.selectRecipe).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "切换中…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "切换中…" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    expect(manual()).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "关闭" }));
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBeVisible();
    await act(async () => request.reject(new Error("工件已开始")));
    expect(await screen.findByRole("alert")).toHaveTextContent("Error: 工件已开始");
    expect(screen.getByRole("button", { name: "确认切换" })).toBeEnabled();
    await userEvent.click(screen.getByRole("button", { name: "确认切换" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(cycleApi.selectRecipe).toHaveBeenCalledTimes(2);
  });

  it.each(["phase", "source"])("确认前 %s 改变禁止提交，并允许取消", async change => {
    const page = render(<InspectPage />);
    await userEvent.selectOptions(manual(), "B");
    state = change === "phase" ? { ...state, phase: "ACQUIRE" } : { ...state, productSource: "plc" };
    page.rerender(<InspectPage />);
    expect(screen.getByRole("button", { name: "确认切换" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("暂时不能切换配方");
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
  });

  it("故障复位显示等待、仅提交一次，失败可重试", async () => {
    state.phase = "FAULT"; state.fault = "相机未就绪";
    const request = deferred<void>(); vi.mocked(cycleApi.reset).mockReturnValueOnce(request.promise);
    render(<InspectPage />);
    expect(screen.getByText("故障 · 相机未就绪")).toBeVisible();
    const reset = screen.getByRole("button", { name: "复位故障" }); fireEvent.click(reset); fireEvent.click(reset);
    expect(cycleApi.reset).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "复位中…" })).toBeDisabled();
    await act(async () => request.reject(new Error("连接仍未恢复")));
    expect(await screen.findByRole("alert")).toHaveTextContent("连接仍未恢复");
    await userEvent.click(screen.getByRole("button", { name: "复位故障" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(cycleApi.reset).toHaveBeenCalledTimes(2);
  });

});

describe("在线检测选帧、曲线与快照绑定", () => {

  it("同 SN 重检的新周期清除旧选帧、选点、测量值和结论", async () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() }; measured = [cycleMeasurement()];
    const page = render(<InspectPage />);
    fireEvent.change(screen.getByRole("slider", { name: "位置曲线选点" }), { target: { value: "1" } });
    expect(screen.getByText(/点 2 · P1 · J1 · s=1.00/)).toBeVisible();
    state = { ...state, phase: "FAULT", fault: "新件相机错误", part: { ...cyclePart(1, "cycle-2"), frames: [cyclePart().frames[0], { ...cyclePart().frames[1], status: "waiting" }] } };
    page.rerender(<InspectPage />);
    expect(screen.getByText("等待工件")).toBeVisible();
    expect(screen.queryByText("测试样本偏移超差")).not.toBeInTheDocument();
    expect(screen.queryByText(/点 2 · P1 · J1 · s=1.00/)).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "选中帧 k1" })).toBeVisible();
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-2", "revisionId-A", 0));
  });

  it.each(["resolve", "reject"] as const)("同 SN 重检后旧原图请求 %s 不覆盖当前周期", async outcome => {
    const old = deferred<GrayImage>(), current = deferred<GrayImage>();
    vi.mocked(workspaceApi.liveImage).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    state.part = cyclePart();
    const page = render(<InspectPage />);
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledWith("cycle-1", "revisionId-A", 1));
    state = { ...state, part: cyclePart(1, "cycle-2") };
    page.rerender(<InspectPage />);
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-2", "revisionId-A", 1));
    await act(async () => current.resolve({ url: "data:,cycle-2", width: 100, height: 60 }));
    if (outcome === "resolve") await act(async () => old.resolve({ url: "data:,cycle-1", width: 100, height: 60 }));
    else await act(async () => old.reject(new Error("旧周期原图请求失败")));
    expect(screen.getByRole("img", { name: "SN 1 · k2" }).querySelector("image")).toHaveAttribute("href", "data:,cycle-2");
    expect(screen.queryByText(/旧周期原图请求失败/)).not.toBeInTheDocument();
  });

  it("布局快照尚未匹配本件时不显示旧测量、旧帧或本件原图", async () => {
    state.part = cyclePart(); state.part.recipeRevision = "new-revisionId"; measured = [cycleMeasurement()];
    render(<InspectPage />);
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看帧 k1" })).toHaveClass("s-waiting");
    expect(workspaceApi.liveImage).not.toHaveBeenCalled();
    expect(screen.getByText("点击曲线或用方向键选择测量点")).toBeVisible();
  });

  it.each(["ACQUIRE", "JUDGE", "FAULT"] as const)("%s 时不显示上一件结论", phase => {
    state = { ...state, phase, part: cyclePart(2), result: cycleResult(1) }; render(<InspectPage />);
    expect(screen.queryByText("测试样本偏移超差")).not.toBeInTheDocument();
    expect(screen.queryByText("判定结果 · SN 1")).not.toBeInTheDocument();
  });

});

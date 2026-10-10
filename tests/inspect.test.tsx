import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import InspectPage from "../src/pages/InspectPage";
import { cycleApi, useCycle, useLayout } from "../src/features/cycle/api";
import { workspaceApi } from "../src/features/workspace/api";
import { plcApi } from "../src/features/plc/api";
import { deferred, shotList, snapshot, summary, twoLines, workspaceView } from "./fixtures";
import { cycleMeasurement, cyclePart, cycleResult } from "./cycle-visual-fixtures";
import { plcConfig, plcPoint } from "./plc-fixtures";
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
const mainPanel = () => within(screen.getByRole("heading", { name: "主视图" }).closest(".panel")! as HTMLElement);

describe("在线检测切型与故障恢复", () => {
  it.each(["VALIDATE", "ACQUIRE", "DRAIN", "JUDGE", "REPORT", "RELEASE"] as const)("%s 阶段禁止切换型号", phase => {
    state.phase = phase; render(<InspectPage />); expect(manual()).toBeDisabled();
  });

  it.each(["IDLE", "FAULT"] as const)("%s 阶段切换需确认，取消无副作用", async phase => {
    state.phase = phase; render(<InspectPage />);
    await userEvent.selectOptions(manual(), "B");
    expect(screen.getByRole("dialog", { name: "切换配方" })).toBeVisible();
    expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
    await userEvent.selectOptions(manual(), "B");
    await userEvent.click(screen.getByRole("button", { name: "确认切换" }));
    expect(cycleApi.selectRecipe).toHaveBeenCalledWith("B");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("选择当前配方不打开确认，关闭与 Escape 均无副作用", async () => {
    render(<InspectPage />);
    fireEvent.change(manual(), { target: { value: "A" } });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await userEvent.selectOptions(manual(), "B");
    await userEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await userEvent.selectOptions(manual(), "B");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
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

  it("关闭失败弹窗后重选清除旧错误", async () => {
    vi.mocked(cycleApi.selectRecipe).mockRejectedValueOnce(new Error("切型失败")); render(<InspectPage />);
    await userEvent.selectOptions(manual(), "B"); await userEvent.click(screen.getByRole("button", { name: "确认切换" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("切型失败");
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    await userEvent.selectOptions(manual(), "B");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
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

  it("切型请求在卸载后失败也不会产生未处理 Promise", async () => {
    const request = deferred<void>(); vi.mocked(cycleApi.selectRecipe).mockReturnValueOnce(request.promise);
    const page = render(<InspectPage />);
    await userEvent.selectOptions(manual(), "B"); await userEvent.click(screen.getByRole("button", { name: "确认切换" }));
    page.unmount(); await act(async () => request.reject(new Error("旧切型失败")));
    expect(cycleApi.selectRecipe).toHaveBeenCalledTimes(1);
  });

  it("确认期间配方被移除时禁止切换，取消保留原配方", async () => {
    const page = render(<InspectPage />); await userEvent.selectOptions(manual(), "B");
    recipeList = [summary()]; page.rerender(<InspectPage />);
    expect(screen.getByRole("alert")).toHaveTextContent("该配方已不在配方库中");
    expect(screen.getByRole("button", { name: "确认切换" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(manual()).toHaveValue("A"); expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
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

  it("换件/故障改变后过期复位错误不显示，卸载后错误已处理", async () => {
    state.phase = "FAULT"; state.fault = "旧故障";
    const old = deferred<void>(); vi.mocked(cycleApi.reset).mockReturnValueOnce(old.promise);
    const page = render(<InspectPage />); await userEvent.click(screen.getByRole("button", { name: "复位故障" }));
    state = { ...state, fault: "新故障", since: 2, part: cyclePart(2) }; page.rerender(<InspectPage />);
    await act(async () => old.reject(new Error("旧复位响应")));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    const pending = deferred<void>(); vi.mocked(cycleApi.reset).mockReturnValueOnce(pending.promise);
    await userEvent.click(screen.getByRole("button", { name: "复位故障" })); page.unmount();
    await act(async () => pending.reject(new Error("卸载后的复位响应")));
    expect(cycleApi.reset).toHaveBeenCalledTimes(2);
  });

  it("PLC 型号来源不显示人工选择器，没有配方显示等待", () => {
    state.productSource = "plc"; layout = null; render(<InspectPage />);
    expect(screen.queryByRole("combobox", { name: "当前检测配方" })).not.toBeInTheDocument();
    expect(screen.getByText("等待 PLC 下发型号")).toBeVisible(); expect(screen.getByText("等待配方")).toBeVisible();
  });
});

describe("在线检测选帧、曲线与快照绑定", () => {
  it("选帧只改变原图和主视图，整件判定保持不变", async () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() }; measured = [cycleMeasurement()];
    render(<InspectPage />);
    expect(await screen.findByRole("heading", { name: "选中帧 k2" })).toBeVisible();
    expect(screen.getByText("测试样本偏移超差")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "查看帧 k1" }));
    expect(screen.getByRole("heading", { name: "选中帧 k1" })).toBeVisible();
    expect(screen.getByRole("button", { name: "查看帧 k1" })).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-1", "revisionId-A", 0));
    expect(screen.getByText("判定结果 · SN 1")).toBeVisible(); expect(screen.getByText("测试样本偏移超差")).toBeVisible();
    await userEvent.click(mainPanel().getByRole("button", { name: "逐拍照点" }));
    expect(mainPanel().getByLabelText("逐拍照点视图")).toBeVisible();
    expect(mainPanel().getByRole("button", { name: "逐拍照点" })).toHaveAttribute("aria-pressed", "true");
    expect(mainPanel().getByRole("button", { name: "拍照点 P1" })).toHaveAttribute("aria-pressed", "true");
    // 拍照点按段内测量点着色：P1 超差且整件判 NG，P2 合格
    expect(within(mainPanel().getByRole("button", { name: "拍照点 P1" })).getByText("NG")).toHaveClass("c-ng");
    expect(within(mainPanel().getByRole("button", { name: "拍照点 P2" })).getByText("合格")).toHaveClass("c-ok");
    await userEvent.click(mainPanel().getByRole("button", { name: "拍照点 P2" }));
    expect(screen.getByRole("heading", { name: "选中帧 k2" })).toBeVisible(); expect(screen.getByText("测试样本偏移超差")).toBeVisible();
    await userEvent.click(mainPanel().getByRole("button", { name: "整件" }));
    expect(mainPanel().getByLabelText("工件总览，选择帧查看原图")).toBeVisible();
  });

  it("总览可用键盘选帧，曲线选点跟随所属帧并可清除", async () => {
    state.part = cyclePart(); measured = [cycleMeasurement()]; render(<InspectPage />);
    fireEvent.keyDown(screen.getByRole("button", { name: "总览选择帧 k1" }), { key: "Enter" });
    expect(screen.getByRole("button", { name: "查看帧 k1" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(screen.getByRole("slider", { name: "位置曲线选点" }), { target: { value: "2" } });
    expect(screen.getByText("点 3 · P2 · J1 · s=0.00 mm · d=3.00 mm · 合格")).toBeVisible();
    expect(screen.getByRole("button", { name: "查看帧 k2" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(mainPanel().getByRole("button", { name: "逐拍照点" }));
    expect(mainPanel().getByLabelText("选中测量点 3")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "清除位置曲线选点" }));
    expect(mainPanel().queryByLabelText("选中测量点 3")).not.toBeInTheDocument();
  });

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

  it("切换到同 SN 新周期时立即清除已显示的原图", async () => {
    state.part = cyclePart();
    const page = render(<InspectPage />);
    expect(await screen.findByRole("img", { name: "SN 1 · k2" })).toBeVisible();
    vi.mocked(workspaceApi.liveImage).mockReturnValueOnce(deferred<GrayImage>().promise);
    state = { ...state, part: cyclePart(1, "cycle-2") };
    page.rerender(<InspectPage />);
    expect(screen.queryByRole("img", { name: "SN 1 · k2" })).not.toBeInTheDocument();
    expect(screen.getByText("正在读取原图…")).toBeVisible();
  });

  it.each(["shotId", "camera", "bundleId"] as const)("页面拒绝 %s 不符的测量，不显示错误来源的曲线与范围", field => {
    state.part = cyclePart(); measured = [{ ...cycleMeasurement(), [field]: "other" }];
    render(<InspectPage />);
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    expect(screen.getByText("点击曲线或用方向键选择测量点")).toBeVisible();
  });

  it("逐点错误直接显示，并分别展示本件顺序与设备原始计数", () => {
    state.part = cyclePart();
    state.part.frames[1] = { ...state.part.frames[1], status: "error", error: "单帧测量超时：P2 超过 T_proc 300 ms" };
    render(<InspectPage />);
    const shot = screen.getByRole("button", { name: "查看帧 k2" });
    expect(within(shot).getByText(/P2 超过 T_proc/)).toHaveAttribute("title", "单帧测量超时：P2 超过 T_proc 300 ms");
    const runtime = screen.getByRole("heading", { name: "选中帧 k2" }).closest(".panel")!;
    expect(within(runtime as HTMLElement).getByText("本件设备内顺序").parentElement).toHaveTextContent("2");
    expect(within(runtime as HTMLElement).getByText("设备帧计数").parentElement).toHaveTextContent("102");
    expect(within(runtime as HTMLElement).getByText("设备触发计数").parentElement).toHaveTextContent("202");
    expect(within(runtime as HTMLElement).getByText(/P2 超过 T_proc/)).toBeVisible();
  });

  it("空闲切到拍照点较少的配方时逐拍照点视图仍有效", async () => {
    const page = render(<InspectPage />);
    await userEvent.click(screen.getByRole("button", { name: "查看帧 k2" }));
    await userEvent.click(mainPanel().getByRole("button", { name: "逐拍照点" }));
    layout = workspaceView("B").layout; layout.shots = shotList(twoLines.slice(0, 1)); layout.segments = layout.segments.slice(0, 1);
    layout.points = { x: [10, 20], y: [10, 10], seg: [0, 0], k: [0, 0] };
    state = { ...state, activeRecipeId: "B" }; page.rerender(<InspectPage />);
    expect(screen.getByRole("heading", { name: "选中帧 k1" })).toBeVisible();
    expect(within(mainPanel().getByLabelText("逐拍照点视图")).getAllByRole("button")).toHaveLength(1);
    expect(mainPanel().getByRole("button", { name: "拍照点 P1" })).toHaveAttribute("aria-pressed", "true");
  });

  it("布局快照尚未匹配本件时不显示旧测量、旧帧或本件原图", async () => {
    state.part = cyclePart(); state.part.recipeRevision = "new-revisionId"; measured = [cycleMeasurement()];
    render(<InspectPage />);
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看帧 k1" })).toHaveClass("s-waiting");
    expect(workspaceApi.liveImage).not.toHaveBeenCalled();
    expect(screen.getByText("点击曲线或用方向键选择测量点")).toBeVisible();
  });

  it("旧帧原图读取结果不会覆盖重新选中的帧，加载失败可重新选帧恢复", async () => {
    state.part = cyclePart();
    const old = deferred<GrayImage>(); vi.mocked(workspaceApi.liveImage).mockReturnValueOnce(old.promise)
      .mockRejectedValueOnce(new Error("当前帧未录制")).mockResolvedValueOnce({ url: "data:,new", width: 100, height: 60 });
    render(<InspectPage />);
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenCalledWith("cycle-1", "revisionId-A", 1));
    await userEvent.click(screen.getByRole("button", { name: "查看帧 k1" }));
    expect(await screen.findByText(/当前帧未录制/)).toBeVisible();
    await act(async () => old.resolve({ url: "data:,old", width: 100, height: 60 }));
    expect(screen.getByText(/当前帧未录制/)).toBeVisible();
    expect(screen.queryByRole("img", { name: /SN/ })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "查看帧 k2" }));
    await waitFor(() => expect(screen.queryByText(/当前帧未录制/)).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "选中帧 k2" })).toBeVisible();
  });

  it.each(["ACQUIRE", "JUDGE", "FAULT"] as const)("%s 时不显示上一件结论", phase => {
    state = { ...state, phase, part: cyclePart(2), result: cycleResult(1) }; render(<InspectPage />);
    expect(screen.queryByText("测试样本偏移超差")).not.toBeInTheDocument();
    expect(screen.queryByText("判定结果 · SN 1")).not.toBeInTheDocument();
  });

  it("工件错误给出 PLC 错误码，分段结果按实际结论显示", () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: { ...cycleResult(), verdict: "ERR_INSPECT", faultCode: 95, reason: "型号不匹配", segments: [] } };
    render(<InspectPage />);
    expect(screen.getByText("PLC 2 / 95")).toBeVisible(); expect(screen.getAllByText("不可判")).toHaveLength(2);
    expect(screen.getByRole("heading", { name: "逐拍照点结果" })).toBeVisible();
  });
});

describe("在线检测信号和事件", () => {
  it("正常占满不报警，超时仍未返回时显示卡住数量，恢复后清除", () => {
    state.measurementWorkers = { capacity: 2, running: 2, timedOut: 0, availableCapacity: 0 };
    const page = render(<InspectPage />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    state = { ...state, measurementWorkers: { capacity: 2, running: 2, timedOut: 1, availableCapacity: 0 } };
    page.rerender(<InspectPage />);
    expect(screen.getByRole("alert")).toHaveTextContent("测量线程超时未返回：1 / 2，当前可用容量 0");
    expect(screen.queryByText(/当前不可布防/)).not.toBeInTheDocument();
    state = { ...state, measurementWorkers: { capacity: 2, running: 2, timedOut: 2, availableCapacity: 0 } };
    page.rerender(<InspectPage />);
    expect(screen.getByRole("alert")).toHaveTextContent("全部测量容量被占用，当前不可布防");
    state = { ...state, measurementWorkers: { capacity: 2, running: 0, timedOut: 0, availableCapacity: 2 } };
    page.rerender(<InspectPage />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("同 SN 重检进入判定时步骤条不继承旧周期错误", () => {
    state = { ...state, phase: "JUDGE", part: cyclePart(1, "cycle-2"), result: { ...cycleResult(), verdict: "ERR_INSPECT" } };
    render(<InspectPage />);
    expect(screen.getByText("判定")).toHaveClass("act");
    expect(screen.getByText("判定")).not.toHaveClass("err");
  });
  it("PLC 灯按标签显示方向、真假值和未绑定状态", async () => {
    const config = plcConfig(); config.connection.protocol = "modbusTcp";
    config.points = [ { ...plcPoint, id: "start", tags: ["partStart"] }, { ...plcPoint, id: "ready", tags: ["visionReady"] }, { ...plcPoint, id: "end", tags: ["partEnd"] } ];
    vi.mocked(plcApi.getConfig).mockResolvedValue(config);
    values = { start: { value: true, error: null, ts: 1 }, ready: { value: 1, error: null, ts: 1 }, end: { value: false, error: null, ts: 1 } };
    const page = render(<InspectPage />);
    await waitFor(() => expect(screen.getByText("partStart")).toHaveClass("on"));
    expect(screen.getByText("partStart")).not.toHaveClass("pc"); expect(screen.getByText("partEnd")).not.toHaveClass("on");
    expect(screen.getByText("visionReady")).toHaveClass("pc", "on"); expect(screen.getByText("resultAck")).toHaveClass("unbound");
    expect(screen.getByText("resultAck")).toHaveAttribute("title", "地址表中未绑定该标签");
    values = { start: { value: null, error: null, ts: 2 } }; page.rerender(<InspectPage />);
    expect(screen.getByText("partStart")).not.toHaveClass("on");
    expect(screen.queryByRole("button", { name: "运行一件" })).not.toBeInTheDocument();
  });

  it("事件按最新在前并限制 80 条，报警与统计持续可见", () => {
    logs = Array.from({ length: 82 }, (_, i) => ({ ts: 1000 + i, level: i === 81 ? "err" : "info", ev: `事件${i}`, msg: `记录 ${i}` } as LogLine));
    state = { ...state, alarms: ["触发计数跳号"], stats: { total: 10, ok: 7, ng: 2, err: 1 }, strayFrames: 3 };
    render(<InspectPage />);
    const eventPanel = screen.getByRole("heading", { name: "事件" }).closest(".panel")!;
    expect(eventPanel.querySelector(".event-log")?.children).toHaveLength(80);
    expect(eventPanel.querySelector(".event-log")?.firstElementChild).toHaveTextContent("事件81记录 81");
    expect(screen.queryByText("事件0")).not.toBeInTheDocument(); expect(screen.getByText("触发计数跳号")).toBeVisible();
    expect(screen.getByText("70.0%")).toBeVisible(); expect(screen.getByText("3")).toHaveClass("c-warn");
  });

  it("模拟工况选择与单件动作仍明确标记模拟，不产生检测结果证明", async () => {
    render(<InspectPage />);
    await userEvent.selectOptions(await screen.findByRole("combobox", { name: "模拟工况" }), "gap");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "模拟配方" }), "B");
    await userEvent.click(screen.getByRole("button", { name: "运行一件" }));
    expect(cycleApi.simStart).toHaveBeenCalledWith("B", "gap", false); expect(screen.getByText("模拟节拍")).toBeVisible();
    expect(screen.getByText("等待工件")).toBeVisible();
  });
});

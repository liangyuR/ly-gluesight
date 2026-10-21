import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import InspectPage from "../src/pages/InspectPage";
import { cycleApi, useCycle, useLayout } from "../src/features/cycle/api";
import { workspaceApi } from "../src/features/workspace/api";
import { plcApi } from "../src/features/plc/api";
import { deferred, readiness, sampleSet, siteStatus, snapshot, summary, workspaceView } from "./fixtures";
import { cycleMeasurement, cyclePart, cycleResult } from "./cycle-visual-fixtures";
import { plcConfig } from "./plc-fixtures";
import type { LogLine, Measured, Readiness, Recipe, RecipeSummary, SiteStatus, Snapshot } from "../src/features/cycle/types";
import type { PlcHandshakeState, PointValue } from "../src/features/plc/types";

vi.mock("../src/features/cycle/api", () => ({
  useCycle: vi.fn(), useRecipes: () => recipeList, useLayout: vi.fn(),
  useSiteStatus: () => site, useReadiness: () => ({ readiness: report, refresh: async () => report }), cycleApi: { selectRecipe: vi.fn(), reset: vi.fn(), offlineRun: vi.fn(), offlineStop: vi.fn() },
}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { runtimeOverview: vi.fn(), liveImage: vi.fn(), list: vi.fn(), debugRun: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getConfig: vi.fn() }, usePlcValues: () => values,
  usePlcStatus: () => ({ state: "connected", since: 1 }), subscribe: () => () => {},
  usePlcOperationState: () => ({ blockedReason: "", handshake: plcHandshake }) }));

let state: Snapshot, layout: Recipe | null, logs: LogLine[], measured: Measured[];
let recipeList: RecipeSummary[];
let site: SiteStatus;
let report: Readiness;
let values: Record<string, PointValue>;
let plcHandshake: PlcHandshakeState | null;
beforeEach(() => {
  state = snapshot(); layout = workspaceView().layout; logs = []; measured = []; values = {}; plcHandshake = null;
  recipeList = [summary(), summary(workspaceView("B"))];
  site = siteStatus();
  report = readiness();
  vi.mocked(useCycle).mockImplementation(() => ({ snapshot: state, logs, measured }));
  vi.mocked(useLayout).mockImplementation(() => layout);
  vi.mocked(cycleApi.selectRecipe).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.reset).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.offlineRun).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.offlineStop).mockReset().mockResolvedValue(undefined);
  vi.mocked(workspaceApi.list).mockReset().mockResolvedValue([workspaceView().workspace, workspaceView("B").workspace]);
  vi.mocked(workspaceApi.debugRun).mockReset().mockResolvedValue(undefined);
  vi.mocked(workspaceApi.runtimeOverview).mockReset().mockResolvedValue(null);
  vi.mocked(workspaceApi.liveImage).mockReset().mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(plcApi.getConfig).mockReset().mockResolvedValue(plcConfig());
});
const nativeSelect = (name: string, root: Pick<typeof screen, "getByLabelText"> = screen) => root.getByLabelText(name, { selector: "select" });
const manual = () => nativeSelect("当前检测配方");

describe("在线检测切型锁定", () => {
  it("切型确认期间开始接收工件，禁止切换并允许取消", async () => {
    const page = render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await userEvent.selectOptions(manual(), "B");
    state = { ...state, phase: "ACQUIRE" };
    page.rerender(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.getByRole("button", { name: "确认切换" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("暂时不能切换配方");
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(cycleApi.selectRecipe).not.toHaveBeenCalled();
  });
});

describe("在线检测原图录制失败告警", () => {
  it("快照带 recordingAlarm → 报警栏写明连续件数、最近件与原因，告警清除后撤掉", () => {
    state = { ...state, recordingAlarm: { consecutive: 3, cycleId: "cycle-9", reason: "创建原图失败：拒绝访问", since: 1 } };
    const page = render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const banner = screen.getByText(/原图录制失败/).closest("[role='status']");
    expect(banner).toHaveTextContent("连续 3 件原图未完整保存");
    expect(banner).toHaveTextContent("cycle-9");
    expect(banner).toHaveTextContent("拒绝访问");
    state = { ...state, recordingAlarm: null };
    page.rerender(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.queryByText(/原图录制失败/)).not.toBeInTheDocument();
  });
});

describe("在线检测离线产线控件", () => {
  it("真机握手待复位但复位条件未满足 → 复位按钮禁用并显示后端原因", () => {
    state = { ...state, phase: "FAULT", fault: "PLC 心跳失活" };
    plcHandshake = { phase: "fault", requestSeq: null, resultSeq: null, message: "PLC 心跳失活", resetBlockedReason: "尚未观察到 PLC 心跳翻转，不能复位" };
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.getByRole("button", { name: "复位故障" })).toBeDisabled();
    expect(screen.getByText("不能复位：尚未观察到 PLC 心跳翻转，不能复位")).toBeVisible();
  });

  it("真机模式 → 不显示离线运行控件", () => {
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.queryByRole("region", { name: "候选配方离线试运行" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "运行一件" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "离线配方" })).not.toBeInTheDocument();
  });

  it("离线模式运行一件 → 按所选配方请求正常件单件运行", async () => {
    site = siteStatus({ mode: "offline", source: "synthetic", measurement: "simulated", port: 50102 });
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await userEvent.selectOptions(nativeSelect("离线配方"), "B");
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
  it("逐拍照点包含不检帧 → 仅展示检测帧且按原始帧索引打开原图", async () => {
    layout!.shots[0].skip = true;
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() };
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const main = within(screen.getByRole("region", { name: "检测主视图" }));
    await userEvent.click(main.getByRole("button", { name: "逐拍照点" }));
    const tiles = within(main.getByLabelText("逐拍照点视图"));
    expect(tiles.queryByRole("button", { name: "拍照点 P1" })).not.toBeInTheDocument();
    expect(tiles.getAllByRole("button")).toHaveLength(1);
    await userEvent.click(tiles.getByRole("button", { name: "拍照点 P2" }));
    expect(main.getByRole("button", { name: "原图" })).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-1", "revisionId-A", 1));
  });

  it("从整件布置选择拍照点 → 返回对应原图且保留整件判定", async () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() };
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const main = within(screen.getByRole("region", { name: "检测主视图" }));
    expect(main.getByRole("button", { name: "原图" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(main.getByRole("button", { name: "整件布置" }));
    fireEvent.click(main.getByRole("button", { name: "总览选择帧 k2" }));
    expect(main.getByRole("button", { name: "原图" })).toHaveAttribute("aria-pressed", "true");
    expect(main.getByText(/P2 · CAM-1 · 视角 1 · k2/)).toBeVisible();
    expect(screen.getByText("测试样本偏移超差")).toBeVisible();
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-1", "revisionId-A", 1));
    await userEvent.click(screen.getByRole("button", { name: "返回当前拍照点" }));
    expect(screen.queryByRole("button", { name: "返回当前拍照点" })).not.toBeInTheDocument();
    expect(screen.getByText("测试样本偏移超差")).toBeVisible();
  });

  it("同 SN 重检的新周期清除旧选帧、选点、测量值和结论", async () => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: cycleResult() }; measured = [cycleMeasurement()];
    const page = render(<MemoryRouter><InspectPage /></MemoryRouter>);
    fireEvent.keyDown(screen.getByRole("slider", { name: "位置曲线选点" }), { key: "ArrowRight" });
    expect(screen.getByText(/点 2 · P1 · J1 · s=1.00/)).toBeVisible();
    state = { ...state, phase: "FAULT", fault: "新件相机错误", part: { ...cyclePart("1", "cycle-2"), frames: [cyclePart().frames[0], { ...cyclePart().frames[1], status: "waiting" }] } };
    page.rerender(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.getByText("暂无检测结果")).toBeVisible();
    expect(screen.queryByText("测试样本偏移超差")).not.toBeInTheDocument();
    expect(screen.queryByText(/点 2 · P1 · J1 · s=1.00/)).not.toBeInTheDocument();
    expect(screen.getByText(/P1 · CAM-1 · 视角 1 · k1/)).toBeVisible();
    expect(screen.queryByRole("button", { name: "返回当前拍照点" })).not.toBeInTheDocument();
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    await waitFor(() => expect(workspaceApi.liveImage).toHaveBeenLastCalledWith("cycle-2", "revisionId-A", 0));
  });

  it("布局快照尚未匹配本件时不显示旧测量、旧帧或本件原图", async () => {
    state.part = cyclePart(); state.part.recipeRevision = "new-revisionId"; measured = [cycleMeasurement()];
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.queryByText("3.00–4.50")).not.toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "拍照点" })).getByRole("button", { name: "P1 · 等待采集" })).toBeInTheDocument();
    expect(workspaceApi.liveImage).not.toHaveBeenCalled();
    expect(screen.queryByText(/点 2 ·/)).not.toBeInTheDocument();
  });
});


describe("在线检测整件判定显示", () => {
  it.each([
    { verdict: "OK_WITH_EXCURSION", code: "OK", detail: "局部超差", tone: "warn" },
    { verdict: "NG_GAP", code: "NG", detail: "断胶", tone: "ng" },
    { verdict: "ERR_INSPECT", code: "ERR", detail: "未测成", tone: "fault" },
  ] as const)("整件判 $verdict → 大字 $code 且用 $tone 语义色，合格带警告、产品 NG、检测异常三者分开", ({ verdict, code, detail, tone }) => {
    state = { ...state, phase: "REPORT", part: cyclePart(), result: { ...cycleResult(), verdict } };
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const card = screen.getByRole("region", { name: "整件判定" });
    expect(within(card).getByText(code)).toBeVisible();
    expect(within(card).getByText(detail)).toBeVisible();
    expect(card).toHaveClass(tone);
  });

  it("整件判断胶而本件有帧定位失败 → 判定卡与该帧缩略图仍挂定位失败徽标", () => {
    const part = cyclePart();
    part.frames[0] = { ...part.frames[0], gapPoints: 1, referenceFailed: [{ view: 1, reason: "定位失败" }] };
    state = { ...state, phase: "REPORT", part, result: { ...cycleResult(), verdict: "NG_GAP", plcCode: 13 } };
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(within(screen.getByRole("region", { name: "整件判定" })).getByLabelText("位置基准：定位失败")).toBeVisible();
    expect(screen.getByText("NG · 定位失败")).toBeVisible();
  });

  it("检测段判断胶 → 段结果显示中文判定而非枚举码", () => {
    const result = cycleResult();
    state = { ...state, phase: "REPORT", part: cyclePart(), result: { ...result, segments: [{ ...result.segments[0], verdict: "NG_GAP" }, result.segments[1]] } };
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const segments = within(screen.getByRole("region", { name: "检测段结果" }));
    expect(segments.getByText("断胶")).toBeVisible();
    expect(segments.queryByText("NG_GAP")).not.toBeInTheDocument();
  });
});

describe("候选配方离线试运行", () => {
  it("未发布候选请求被拒绝 → 保留真实原因并允许按同一修订重试，准备中不重复启动", async () => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet(), nextPart: "part-1" });
    const request = deferred<void>();
    vi.mocked(workspaceApi.debugRun).mockReturnValueOnce(request.promise);
    render(<MemoryRouter initialEntries={["/inspect?candidate=B"]}><InspectPage /></MemoryRouter>);
    const panel = within(screen.getByRole("region", { name: "候选配方离线试运行" }));
    const run = panel.getByRole("button", { name: "候选试运行一圈" });
    await waitFor(() => expect(run).toBeEnabled());
    expect(nativeSelect("调试候选配方", panel)).toHaveValue("B");
    await userEvent.selectOptions(nativeSelect("调试样本件", panel), "part-1");
    fireEvent.click(run);
    fireEvent.click(run);
    expect(workspaceApi.debugRun).toHaveBeenCalledTimes(1);
    expect(workspaceApi.debugRun).toHaveBeenLastCalledWith("B", workspaceView("B").workspace.revision, "part-1");
    expect(run).toBeDisabled();
    await act(async () => request.reject(new Error("候选已变化，请刷新候选")));
    expect(panel.getByRole("alert")).toHaveTextContent("候选已变化，请刷新候选");
    expect(run).toBeEnabled();
    await userEvent.click(run);
    expect(workspaceApi.debugRun).toHaveBeenCalledTimes(2);
    expect(panel.queryByRole("alert")).not.toBeInTheDocument();
    expect(cycleApi.offlineRun).not.toHaveBeenCalled();
  });

  it.each(["site", "cycle"] as const)("%s 仍在运行 → 禁止从候选入口重复启动", async running => {
    site = siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet(), running: running === "site" });
    state = snapshot(running === "cycle" ? "ACQUIRE" : "IDLE");
    render(<MemoryRouter initialEntries={["/inspect?candidate=A"]}><InspectPage /></MemoryRouter>);
    const panel = within(screen.getByRole("region", { name: "候选配方离线试运行" }));
    await waitFor(() => expect(nativeSelect("调试候选配方", panel)).toHaveValue("A"));
    expect(panel.getByRole("button", { name: "候选试运行一圈" })).toBeDisabled();
  });

  it("候选周期已生成整件结果 → 在线检测持续显示候选修订与非生产调试标记", () => {
    state = { ...state, candidateDebug: { recipeId: "A", revision: 352 }, productSource: "manual", phase: "REPORT", part: { ...cyclePart(), candidateDebug: { recipeId: "A", revision: 352 } }, result: cycleResult() };
    const page = render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const banner = screen.getAllByRole("status").find(node => node.textContent?.includes("候选配方调试结果"));
    expect(banner).toBeVisible();
    expect(banner).toHaveTextContent("A · 修订 352");
    expect(screen.queryByRole("combobox", { name: "当前检测配方" })).not.toBeInTheDocument();
    expect(screen.getByText("A · 候选修订 352")).toBeVisible();
    expect(banner).toHaveTextContent("未发布，仅用于离线测试，不作为生产验收");
    expect(screen.getByText("测试样本偏移超差")).toBeVisible();
    state = { ...state, phase: "IDLE", candidateDebug: undefined };
    page.rerender(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(screen.getByRole("combobox", { name: "当前检测配方" })).toBeVisible();
    expect(banner).toHaveTextContent("未发布，仅用于离线测试，不作为生产验收");
  });
});

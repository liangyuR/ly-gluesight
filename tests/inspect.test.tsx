import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import InspectPage from "../src/pages/InspectPage";
import { cycleApi, recipeApi, useCycle, useLayout } from "../src/features/cycle/api";
import { workspaceApi } from "../src/features/workspace/api";
import { burstImageApi } from "../src/features/workspace/burstImages";
import { plcApi } from "../src/features/plc/api";
import { burstJudgement, burstRecipe, burstSummary, burstWorkspaceView, deferred, readiness, sampleSet, siteStatus, snapshot } from "./fixtures";
import { burstPartView, burstResult } from "./cycle-visual-fixtures";
import { plcConfig } from "./plc-fixtures";
import type { LogLine, Measured, Readiness, Recipe, RecipeSummary, SiteStatus, Snapshot } from "../src/features/cycle/types";
import type { PlcHandshakeState, PointValue } from "../src/features/plc/types";

vi.mock("../src/features/cycle/api", () => ({
  useCycle: vi.fn(), useRecipes: () => recipeList, useLayout: vi.fn(),
  useSiteStatus: () => site, useReadiness: () => ({ readiness: report, refresh: async () => report }), cycleApi: { selectRecipe: vi.fn(), reset: vi.fn(), offlineRun: vi.fn(), offlineStop: vi.fn() },
  recipeApi: { doc: vi.fn() },
}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { runtimeOverview: vi.fn(), liveImage: vi.fn(), list: vi.fn(), debugRun: vi.fn() } }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { live: vi.fn(), record: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getConfig: vi.fn() }, usePlcValues: () => values,
  usePlcStatus: () => ({ state: "connected", since: 1 }), subscribe: () => () => {},
  usePlcOperationState: () => ({ blockedReason: "", handshake: plcHandshake }) }));

let state: Snapshot, layout: Recipe | null, logs: LogLine[], measured: Measured[];
let recipeList: RecipeSummary[];
let site: SiteStatus;
let report: Readiness;
let values: Record<string, PointValue>;
let plcHandshake: PlcHandshakeState | null;
const recipe = (id = "A") => burstRecipe({ id, name: `段配方 ${id}` });
const candidate = (id = "A") => burstWorkspaceView(recipe(id));
beforeEach(() => {
  state = snapshot(); layout = candidate().layout; logs = []; measured = []; values = {}; plcHandshake = null;
  recipeList = [burstSummary(recipe()), burstSummary(recipe("B"))];
  site = siteStatus();
  report = readiness();
  vi.mocked(useCycle).mockImplementation(() => ({ snapshot: state, logs, measured }));
  vi.mocked(useLayout).mockImplementation(() => layout);
  vi.mocked(cycleApi.selectRecipe).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.reset).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.offlineRun).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.offlineStop).mockReset().mockResolvedValue(undefined);
  vi.mocked(workspaceApi.list).mockReset().mockResolvedValue([candidate().workspace, candidate("B").workspace]);
  vi.mocked(workspaceApi.debugRun).mockReset().mockResolvedValue(undefined);
  vi.mocked(workspaceApi.runtimeOverview).mockReset().mockResolvedValue(null);
  vi.mocked(workspaceApi.liveImage).mockReset().mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(burstImageApi.live).mockReset().mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(recipeApi.doc).mockReset().mockImplementation(async id => recipe(id));
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

});

describe("在线检测段配方", () => {
  const burstState = (over: Partial<ReturnType<typeof burstJudgement>> = {}): Snapshot => {
    const burst = burstJudgement(over);
    return { ...snapshot("REPORT"), activeRecipeId: "S", part: burstPartView(), result: { ...burstResult(), verdict: burst.verdict, plcCode: burst.plcCode, reason: burst.reason, burst } };
  };
  const live = () => vi.mocked(burstImageApi.live).mock.calls.map(([, , camera, seq]) => [camera, seq]);
  beforeEach(() => {
    layout = burstWorkspaceView().layout; recipeList = [burstSummary()]; state = burstState();
  });

  it.each([
    ["检测不完整、胶偏未判、含未分段", {}, "OK", "整件合格", ["检测不完整", "胶偏未判", "含未分段"], ["帧数异常", "疑似提前终止"]],
    ["帧数异常、疑似提前终止", { verdict: "NG_WIDTH", plcCode: 14, reason: "溢胶：帧组 3", flags: { incomplete: false, frameCountAnomaly: true, offsetUnjudged: false, unsegmented: false, shortPart: true } },
      "NG", "胶宽超差", ["帧数异常", "疑似提前终止"], ["检测不完整", "胶偏未判", "含未分段"]],
  ] as const)("整件结果标记为%s → 判定卡只挂对应徽标，结论标签不变", (_, over, code, detail, shown, hidden) => {
    state = burstState(over as Partial<ReturnType<typeof burstJudgement>>);
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const card = within(screen.getByRole("region", { name: "整件判定" }));
    expect(card.getByText(code)).toBeVisible();
    expect(card.getByText(detail)).toBeVisible();
    for (const label of shown) expect(card.getByText(label)).toBeVisible();
    for (const label of hidden) expect(card.queryByText(label)).not.toBeInTheDocument();
  });

  it("默认跟随最新帧组；点帧组条 → 按 (相机, seq) 读各相机原图且缺帧的相机不读，返回当前帧组恢复跟随", async () => {
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await waitFor(() => expect(burstImageApi.live).toHaveBeenCalledTimes(3));
    expect(live()).toEqual([[0, 12], [1, 12], [2, 12]]);
    expect(workspaceApi.liveImage).not.toHaveBeenCalled();
    vi.mocked(burstImageApi.live).mockClear();
    await userEvent.click(screen.getByRole("button", { name: "帧组 5 · 不完整" }));
    await waitFor(() => expect(burstImageApi.live).toHaveBeenCalledTimes(2));
    expect(live()).toEqual([[0, 5], [2, 5]]);
    expect(screen.getByText("缺帧", { selector: ".wp-image-empty strong" })).toBeVisible();
    expect(screen.getByRole("region", { name: "整件判定" })).toHaveTextContent("整件合格");
    vi.mocked(burstImageApi.live).mockClear();
    await userEvent.click(screen.getByRole("button", { name: "返回当前帧组" }));
    await waitFor(() => expect(burstImageApi.live).toHaveBeenCalledTimes(3));
    expect(live()).toEqual([[0, 12], [1, 12], [2, 12]]);
    expect(screen.queryByRole("button", { name: "返回当前帧组" })).not.toBeInTheDocument();
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
    expect(workspaceApi.debugRun).toHaveBeenLastCalledWith("B", candidate("B").workspace.revision, "part-1");
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

});

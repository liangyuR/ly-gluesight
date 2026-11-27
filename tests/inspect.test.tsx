import { useState, type ReactElement } from "react";
import { act, render as renderUi, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TitlebarSlotsContext } from "../src/layout/TitlebarContent";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import InspectPage from "../src/pages/InspectPage";
import PreviousEvidence, { PreviousEvidenceProvider } from "../src/features/cycle/components/PreviousEvidence";
import { cycleApi, recipeApi, useCycle, useLayout } from "../src/features/cycle/api";
import { burstImageApi } from "../src/features/workspace/burstImages";
import { historyApi } from "../src/features/history/api";
import { plcApi } from "../src/features/plc/api";
import { burstJudgement, burstPartDetail, burstRecipe, burstSummary, burstWorkspaceView, frameMetrics, readiness, snapshot } from "./fixtures";
import { burstPartView, burstResult } from "./cycle-visual-fixtures";
import { plcConfig } from "./plc-fixtures";
import type { Readiness, Recipe, RecipeSummary, Snapshot } from "../src/features/cycle/types";
import type { PointValue } from "../src/features/plc/types";

vi.mock("../src/features/cycle/api", () => ({
  useCycle: vi.fn(), useRecipes: () => recipeList, useLayout: vi.fn(),
  useReadiness: () => ({ readiness: report, refresh: async () => report }), cycleApi: { selectRecipe: vi.fn(), setMaintenance: vi.fn() },
  recipeApi: { doc: vi.fn(), list: vi.fn() },
}));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(),  } }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { live: vi.fn(), liveView: vi.fn(), record: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ plcApi: { getConfig: vi.fn() }, usePlcValues: () => values,
  usePlcStatus: () => ({ state: "connected", since: 1 }), subscribe: () => () => {} }));

let state: Snapshot, layout: Recipe | null;
let recipeList: RecipeSummary[];
let report: Readiness;
let values: Record<string, PointValue>;
const render = (ui: ReactElement) => renderUi(ui, { wrapper: PreviousEvidenceProvider });
const recipe = (id = "A") => burstRecipe({ id, name: `段配方 ${id}` });
const candidate = (id = "A") => burstWorkspaceView(recipe(id));
beforeEach(() => {
  state = snapshot(); layout = candidate().layout; values = {};
  recipeList = [burstSummary(recipe()), burstSummary(recipe("B"))];
  report = readiness();
  vi.mocked(useCycle).mockImplementation(() => ({ snapshot: state }));
  vi.mocked(useLayout).mockImplementation(() => layout);
  vi.mocked(cycleApi.selectRecipe).mockReset().mockResolvedValue(undefined);
  vi.mocked(cycleApi.setMaintenance).mockReset().mockResolvedValue(undefined);
  vi.mocked(recipeApi.list).mockReset().mockResolvedValue({ recipes: recipeList, errors: [], unavailable: [] });
  vi.mocked(burstImageApi.live).mockReset().mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(burstImageApi.liveView).mockReset().mockResolvedValue(null);
  vi.mocked(recipeApi.doc).mockReset().mockImplementation(async id => recipe(id));
  vi.mocked(plcApi.getConfig).mockReset().mockResolvedValue(plcConfig());
});
const nativeSelect = (name: string, root: Pick<typeof screen, "getByLabelText"> = screen) => root.getByLabelText(name, { selector: "select" });
const manual = () => nativeSelect("当前检测配方");

it("历史查询期间清除回查位置 → 查询稍后成功也不离开在线页", async () => {
  let showRecord: (id: number) => void = () => {};
  const pending = new Promise<number>(resolve => { showRecord = resolve; });
  vi.spyOn(historyApi, "idByCycle").mockReturnValue(pending);

  function Review() {
    const [shown, setShown] = useState(true);
    return shown ? <PreviousEvidence evidence={{ cycleId: "archived-cycle", sn: "1", seq: 5 }} onDismiss={() => setShown(false)} />
      : <span>检测在线</span>;
  }
  const user = userEvent.setup();
  render(<MemoryRouter initialEntries={["/inspect"]}><Routes>
    <Route path="/inspect" element={<Review />} />
    <Route path="/history/:id" element={<span>历史回查</span>} />
  </Routes></MemoryRouter>);
  await user.click(screen.getByRole("button", { name: "继续查看证据" }));
  expect(screen.getByRole("button", { name: "继续查看证据" })).toBeDisabled();
  await user.click(screen.getByRole("button", { name: "清除回查位置" }));
  await act(async () => { showRecord(41); });
  expect(screen.getByText("检测在线")).toBeVisible();
});

function TitlebarHarness({ showPage = true }: { showPage?: boolean }) {
  const [details, setDetails] = useState<HTMLDivElement | null>(null);
  const [actions, setActions] = useState<HTMLDivElement | null>(null);
  return <MemoryRouter>
    <header aria-label="窗口标题栏"><div ref={setDetails} aria-label="标题栏状态" /><div ref={setActions} aria-label="标题栏操作" /></header>
    <TitlebarSlotsContext.Provider value={{ details, actions }}>
      <main aria-label="在线检测内容">{showPage && <InspectPage />}</main>
    </TitlebarSlotsContext.Provider>
  </MemoryRouter>;
}

it("在线检测挂到窗口标题栏 → 状态与操作可用，离页后清理标题栏内容", async () => {
  state = { ...state, part: { ...burstPartView("TITLEBAR-SN"), recipeId: "A" } };
  const user = userEvent.setup();
  const page = render(<TitlebarHarness />);
  const titlebar = within(screen.getByRole("banner", { name: "窗口标题栏" }));
  const content = within(screen.getByRole("main", { name: "在线检测内容" }));
  expect(nativeSelect("当前检测配方", titlebar)).toHaveValue("A");
  expect(titlebar.getByText("TITLEBAR-SN")).toBeVisible();
  expect(titlebar.getByRole("button", { name: "维护模式" })).toBeVisible();
  expect(content.queryByLabelText("当前检测配方", { selector: "select" })).toBeNull();

  await user.click(titlebar.getByRole("button", { name: /^诊断$/ }));
  expect(await screen.findByRole("dialog", { name: "检测诊断" })).toBeVisible();
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "检测诊断" })).toBeNull());

  state = { ...state, maintenance: { since: 1, reason: "标题栏维护采图" } };
  page.rerender(<TitlebarHarness />);
  await user.click(titlebar.getByRole("button", { name: /^维护模式$/ }));
  expect(await screen.findByText("原因：标题栏维护采图")).toBeVisible();
  await user.click(screen.getByRole("button", { name: /^退出维护模式$/ }));
  const maintenance = within(await screen.findByRole("dialog", { name: "退出维护模式" }));
  const exitMaintenance = maintenance.getByRole("button", { name: "确认退出" });
  await waitFor(() => expect(exitMaintenance).toBeEnabled());
  await user.click(exitMaintenance);
  expect(cycleApi.setMaintenance).toHaveBeenCalledWith(false, "");

  page.rerender(<TitlebarHarness showPage={false} />);
  expect(screen.getByLabelText("标题栏状态")).toBeEmptyDOMElement();
  expect(screen.getByLabelText("标题栏操作")).toBeEmptyDOMElement();
});

describe("在线检测切型锁定", () => {
  it("本件判定包含生产跳过 → 帧格与原图标记不判定，新工件不沿用上一件跳过范围", async () => {
    const part = burstPartView(), result = burstResult();
    part.burst.groups[0].cells[0].verdict = "NG_GAP";
    part.burst.groups[0].cells[1] = { status: "missing", verdict: null, frameCounter: null };
    part.burst.groups[5].cells[0].verdict = "NG_GAP";
    const base = result.burst!.regions[0];
    result.burst!.regions = [
      { ...base, region: { firstSeq: 1, lastSeq: 4, kind: { type: "skipped", segment: 1, pass: 1 } }, frameCount: 4 },
      { ...base, region: { ...base.region, firstSeq: 5 } },
    ];
    state = { ...state, part, result };
    const page = render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const strip = within(screen.getByRole("group", { name: "帧组" }));
    const first = strip.getByRole("button", { name: "帧组 1" });
    expect(within(first).getByRole("img", { name: "CAM-1 · 生产跳过" })).toHaveStyle({ background: "var(--outline-variant)" });
    expect(within(strip.getByRole("button", { name: "帧组 6" })).getByRole("img", { name: "CAM-1 · NG" })).toHaveStyle({ background: "var(--ng)" });
    await userEvent.click(first);
    const camera = within(await screen.findByRole("region", { name: "CAM-1 帧组 1" }));
    expect(camera.getByText("生产跳过")).toBeVisible();
    expect(camera.getByText("不参与判定")).toBeVisible();
    expect(camera.getByText("NG · 仅供诊断")).toBeVisible();
    expect(within(screen.getByRole("region", { name: "CAM-2 帧组 1" })).getByText("原图缺失；本帧组生产跳过，不参与判定。")).toBeVisible();
    state = { ...state, part: { ...part, cycleId: "next-cycle" } };
    page.rerender(<MemoryRouter><InspectPage /></MemoryRouter>);
    const next = within(screen.getByRole("group", { name: "帧组" }));
    expect(within(next.getByRole("button", { name: "帧组 1" })).getByRole("img", { name: "CAM-1 · NG" })).toBeVisible();
    expect(screen.queryByText("生产跳过 4 帧组 · 不参与判定")).toBeNull();
    expect(screen.getByText(/当前为临时测量状态/)).toBeVisible();
  });

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

describe("维护模式", () => {
  it("开启维护模式 → 原因为空不能确认，填写后以 enabled=true 与原因调用命令", async () => {
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await userEvent.click(screen.getByRole("button", { name: "维护模式" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("向 PLC 回结果码 0");
    const confirm = within(dialog).getByRole("button", { name: "确认开启" });
    await userEvent.type(within(dialog).getByLabelText(/开启原因/), "   ");
    expect(confirm).toBeDisabled();
    await userEvent.type(within(dialog).getByLabelText(/开启原因/), "新线投运采图");
    await userEvent.click(confirm);
    expect(cycleApi.setMaintenance).toHaveBeenCalledWith(true, "新线投运采图");
  });
});

describe("在线检测段配方", () => {
  const burstState = (over: Partial<ReturnType<typeof burstJudgement>> = {}): Snapshot => {
    const burst = burstJudgement(over);
    return { ...snapshot(), activeRecipeId: "S", part: burstPartView(), result: { ...burstResult(), verdict: burst.verdict, plcCode: burst.plcCode, reason: burst.reason, burst } };
  };
  const live = () => vi.mocked(burstImageApi.live).mock.calls.map(([, , camera, seq]) => [camera, seq]);
  beforeEach(() => {
    layout = burstWorkspaceView().layout; recipeList = [burstSummary()]; state = burstState();
  });

  it("工件仍锁定旧配方修订 → 整件布置与区域结果继续使用该修订的段长", async () => {
    const active = burstRecipe();
    active.segments[0].lengthMm = 111;
    layout = burstWorkspaceView(active).layout;
    const published = burstRecipe({ version: 2 });
    published.segments[0].lengthMm = 222;
    recipeList = [{ ...burstSummary(published), revisionId: "revisionId-S-new" }];
    vi.mocked(recipeApi.doc).mockResolvedValue(published);

    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await userEvent.click(screen.getByText("整件布置", { exact: true }));
    expect(within(screen.getByRole("region", { name: "相机与段" })).getByText("111 mm")).toBeVisible();
    expect(screen.getByText(/全长 221 mm/)).toBeVisible();
  });

  it("默认跟随最新帧组；点帧组条 → 按 (相机, seq) 读各相机原图且缺帧的相机不读，返回当前帧组恢复跟随", async () => {
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await waitFor(() => expect(burstImageApi.live).toHaveBeenCalledTimes(3));
    expect(live()).toEqual([[0, 12], [1, 12], [2, 12]]);
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

  it("从完整性明细定位后换件 → 在线跟随新件并保留可清除的历史回查位置", async () => {
    const previous = state.part!;
    vi.spyOn(historyApi, "idByCycle").mockResolvedValueOnce(null)
      .mockImplementation(async cycleId => cycleId === previous.cycleId ? 41 : 73);
    const detail = burstPartDetail();
    vi.spyOn(historyApi, "overview").mockResolvedValue({ ...detail, burst: null,
      summary: { ...detail.summary, id: 41, sn: previous.sn, cycleId: previous.cycleId } });
    vi.spyOn(historyApi, "neighbors").mockResolvedValue({ older: null, newer: null });
    function Destination() {
      const { pathname, search } = useLocation();
      return <><output aria-label="历史定位">{pathname}{search}</output><HistoryDetailPage /></>;
    }
    const renderApp = () => <MemoryRouter initialEntries={["/inspect"]}><Routes>
      <Route path="/inspect" element={<InspectPage />} />
      <Route path="/history/:id" element={<Destination />} />
    </Routes></MemoryRouter>;
    const user = userEvent.setup();
    const page = render(renderApp());
    const verdict = within(screen.getByRole("region", { name: "整件判定" }));
    expect(verdict.getByText("OK")).toBeVisible();
    await user.click(verdict.getByRole("button", { name: /检测完整性.*检测不完整/ }));
    const details = within(screen.getByRole("region", { name: "检测覆盖明细" }));
    expect(details.getByText(/CAM-2 · 缺帧 · 帧组 5/)).toBeVisible();
    await user.click(details.getByRole("button", { name: "定位到帧组 5" }));
    const camera = within(screen.getByRole("region", { name: "CAM-2 帧组 5" }));
    expect(camera.getByText("缺帧", { selector: ".wp-image-empty strong" })).toBeVisible();

    expect(screen.queryByRole("button", { name: "继续查看证据" })).not.toBeInTheDocument();
    state = { ...state, phase: "ACQUIRE", part: burstPartView(previous.sn, "next-cycle") };
    page.rerender(renderApp());
    expect(screen.queryByRole("region", { name: "检测覆盖明细" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "CAM-2 帧组 5" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "CAM-2 帧组 12" })).toBeVisible();

    const saved = within(screen.getByRole("region", { name: "上次定位的检测证据" }));
    expect(saved.getByText(`SN ${previous.sn} · 帧组 5 · CAM-2`)).toBeVisible();
    await user.click(saved.getByRole("button", { name: "继续查看证据" }));
    expect(await saved.findByRole("status")).toHaveTextContent("历史记录暂不可用");
    await user.click(saved.getByRole("button", { name: "继续查看证据" }));
    expect(await screen.findByLabelText("历史定位")).toHaveTextContent("/history/41?camera=1&seq=5&view=original");
    await user.click(await screen.findByRole("button", { name: "返回在线检测" }));
    expect(screen.getByRole("region", { name: "CAM-2 帧组 12" })).toBeVisible();

    const restored = within(screen.getByRole("region", { name: "上次定位的检测证据" }));
    expect(restored.getByText(`SN ${previous.sn} · 帧组 5 · CAM-2`)).toBeVisible();
    await user.click(restored.getByRole("button", { name: "继续查看证据" }));
    expect(await screen.findByLabelText("历史定位")).toHaveTextContent("/history/41?camera=1&seq=5&view=original");
    await user.click(await screen.findByRole("button", { name: "返回在线检测" }));
    await user.click(screen.getByRole("button", { name: "清除回查位置" }));
    expect(screen.queryByRole("region", { name: "上次定位的检测证据" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "CAM-2 帧组 12" })).toBeVisible();
  });

  it("长件的不完整帧组以摘要显示 → 展开分页后可定位原帧号，换件重新收起", async () => {
    const sequences = [2, 3, ...Array.from({ length: 998 }, (_, index) => 6 + index * 2)];
    const sample = burstJudgement().regions[0];
    state = burstState({ groupCount: 2000, regions: [{ ...sample, region: { ...sample.region, lastSeq: 2000 },
      frameCount: 2000, incompleteGroups: sequences }] });
    const part = state.part!;
    const template = part.burst.groups[0];
    const missing = new Set(sequences);
    part.burst.groupCount = 2000;
    part.burst.groups = Array.from({ length: 2000 }, (_, index) => {
      const seq = index + 1;
      return { ...template, seq, incomplete: missing.has(seq), cells: missing.has(seq) ? [
        template.cells[0], { status: "missing", verdict: null, frameCounter: null }, template.cells[2],
      ] : template.cells };
    });
    const user = userEvent.setup();
    const page = render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const incomplete = () => within(screen.getByRole("group", { name: /不完整帧组$/ }));
    const frameButtons = () => incomplete().queryAllByRole("button", { name: /^查看帧组 \d+$/ });
    expect(incomplete().getByText("不完整帧组 1000 个 · 2–3、6、8 等")).toBeVisible();
    expect(frameButtons()).toHaveLength(0);
    await user.click(incomplete().getByRole("button", { name: "查看首个帧组 2" }));
    expect(screen.getByRole("region", { name: "CAM-1 帧组 2" })).toBeVisible();

    await user.click(incomplete().getByRole("button", { name: "展开列表" }));
    expect(frameButtons()).toHaveLength(32);
    await user.click(incomplete().getByRole("button", { name: "第 32 页" }));
    expect(frameButtons()).toHaveLength(8);
    await user.click(incomplete().getByRole("button", { name: "查看帧组 2000" }));
    expect(screen.getByRole("region", { name: "CAM-1 帧组 2000" })).toBeVisible();

    state = { ...state, part: { ...part, cycleId: "next" }, result: { ...state.result!, cycleId: "next" } };
    page.rerender(<MemoryRouter><InspectPage /></MemoryRouter>);
    expect(frameButtons()).toHaveLength(0);
    expect(incomplete().getByRole("button", { name: "展开列表" })).toBeVisible();
  });

  it("断胶判定有单帧证据 → 从结果定位异常原图并显示对应的实测断胶长度", async () => {
    const burst = burstJudgement();
    burst.regions[0] = { ...burst.regions[0], verdict: "NG_GAP", reason: "断胶" };
    state = burstState({ ...burst, verdict: "NG_GAP", plcCode: 13 });
    state.part!.burst.groups[3].cells[1].verdict = "NG_GAP";
    vi.mocked(burstImageApi.liveView).mockImplementation(async (_cycle, _revision, camera, seq) => camera === 1 && seq === 4 ? {
      segment: 1, corridor: [], searchHalfPx: null, edges: [], partEdges: [], metrics: frameMetrics(camera, seq, { gapMaxMm: 8.4 }),
    } : null);
    const user = userEvent.setup();
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    await user.click(screen.getByRole("button", { name: /判定证据.*断胶/ }));
    const details = within(screen.getByRole("region", { name: "判定证据" }));
    expect(details.getByText("CAM-2 · 帧组 4")).toBeVisible();
    await user.click(details.getByRole("button", { name: "定位到异常帧" }));
    const frame = within(screen.getByRole("region", { name: "CAM-2 帧组 4" }));
    expect(await frame.findByText("8.40 mm")).toBeVisible();
  });

  it("PLC 写入未确认 → 保留产品 OK 并按本件展开真实写入里程碑", async () => {
    state.result!.plcWrite = { partSeq: 1844, receivedAt: 10, endedAt: 20, updatedAt: 30, state: "failed", message: "回写 PLC 失败：读回超时" };
    const user = userEvent.setup();
    render(<MemoryRouter><InspectPage /></MemoryRouter>);
    const verdict = within(screen.getByRole("region", { name: "整件判定" }));
    expect(verdict.getByText("OK")).toBeVisible();
    expect(verdict.getByText("件序号 #1844")).toBeVisible();
    await user.click(verdict.getByRole("button", { name: /PC → PLC.*写入未确认/ }));
    const details = within(screen.getByRole("region", { name: "本件 PLC 通讯" }));
    expect(details.getByText(/读回超时/)).toBeVisible();
    await user.click(details.getByRole("button", { name: "查看写入记录" }));
    const timeline = within(details.getByRole("list"));
    expect(timeline.getAllByRole("listitem").map(item => item.textContent)).toEqual([
      expect.stringContaining("PC 接到工件"), expect.stringContaining("PC 读到工件结束"), expect.stringContaining("写入未确认"),
    ]);
  });

});

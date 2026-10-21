import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { FixtureSpec } from "../src/features/cycle/types";
import type { FixtureCheck, FixtureCheckItem, FixtureRotationEvidence, FixtureTeachReport, FixtureTrial } from "../src/features/workspace/types";
import { workspaceState } from "./fixtures";

// 只替身两个协作者：候选状态（context）与后端（workspaceApi）；产线快照不替身，非桌面环境下为空即“没在生产”
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveParams: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(), fixtureCheck: vi.fn(), fixtureTrial: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter initialEntries={["/recipe/teach"]}><TeachingPage /></MemoryRouter>);
const fixture = (over: Partial<FixtureSpec> = {}): FixtureSpec => ({ rect: [60, 30, 20, 20], source: "A-image-0", searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1, ...over });
const item = (id: FixtureCheckItem["id"], passed: boolean, detail: string, level: FixtureCheckItem["level"] = "block"): FixtureCheckItem => ({ id, level, passed, detail });
const check = (checks: FixtureCheckItem[]): FixtureCheck => ({ passed: checks.every(c => c.passed || c.level === "warn"), checks, padded: null, search: [0, 0, 1, 1], lever: null, textureStd: 30, brightFraction: 0 });
const report = (checks: FixtureCheckItem[]): FixtureTeachReport => ({ passed: checks.every(c => c.passed || c.level === "warn"), checks, pose: null, lever: null, textureStd: 30, brightFraction: 0 });

/** 拍照点 P1 已保存定位框；试测带着这份报告，后端把 P1 判为 viewState */
function withSavedFixture(checks: FixtureCheckItem[], viewState: "trialPassed" | "trialFailed") {
  ws.data!.workspace.doc.shots[0].fixture = fixture();
  const trial = ws.data!.workspace.frames[0].trial!;
  trial.fixture = report(checks);
  trial.passed = viewState === "trialPassed";
  Object.assign(ws.data!.progress.shots[0].views[0], { state: viewState });
}

beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  ws.data!.workspace.doc.shots.forEach(shot => { shot.views = []; });
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  for (const fn of [workspaceApi.saveParams, workspaceApi.saveTeach, workspaceApi.fixtureCheck, workspaceApi.fixtureTrial]) vi.mocked(fn).mockReset();
  vi.mocked(workspaceApi.fixtureCheck).mockResolvedValue(check([item("inside", true, "外圈在图内"), item("corridor", true, "离走廊足够远")]));
});

describe("示教页定位特征", () => {
  it.each([
    ["外圈与走廊检查通过 → 整数像素的框进草稿", true],
    ["压走廊 → 框标红、列出原因、不进草稿", false],
  ])("拖出定位框：%s", async (_, passes) => {
    // 防的回归：rect 是 [u32; 4]，小数框会让保存失败；压走廊的框进了草稿，自动保存反复撞上配方校验
    vi.stubGlobal("DOMPoint", class { constructor(public x: number, public y: number) {} matrixTransform() { return this; } });
    Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
    Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });
    const corridor = "定位框（含 4 px 外圈）压住了拍照点 P1 图 1 的胶检测走廊";
    vi.mocked(workspaceApi.fixtureCheck).mockResolvedValue(check([item("inside", true, "外圈在图内"), item("corridor", passes, passes ? "离走廊足够远" : corridor)]));
    show();
    const image = await screen.findByRole("img", { name: /P1/ });
    await userEvent.click(screen.getByRole("button", { name: "画定位框" }));
    fireEvent.pointerDown(image, { clientX: 20.4, clientY: 10.6 });
    fireEvent.pointerMove(image, { clientX: 52.2, clientY: 40.8 });
    fireEvent.pointerUp(image);
    const proposed = { ...fixture({ rect: [20, 11, 32, 30] }), source: "" };
    await waitFor(() => expect(workspaceApi.fixtureCheck).toHaveBeenCalledWith("A", 7, 0, expect.objectContaining({ fixture: proposed })));
    if (passes) {
      await waitFor(() => expect(ws.setFrameDraft).toHaveBeenCalledWith(0, expect.objectContaining({ fixture: proposed })));
    } else {
      expect(await screen.findByText(new RegExp(corridor))).toBeVisible();
      expect(image.querySelector('[aria-label="定位框（未通过检查）"]')).not.toBeNull();
      expect(ws.setFrameDraft).not.toHaveBeenCalled();
    }
  });

  it("试测报告有阻断项 → 阻断原因列在示教图检查清单里", () => {
    // 防的回归：试测无效时看不到是哪项示教图检查没过（保存门本身在后端，见 B1、B6）
    const why = "示教图上定位没被采用：有相近的次峰（次峰比超过上限，模板不唯一）";
    withSavedFixture([item("texture", true, "纹理足够"), item("selfLocate", false, why)], "trialFailed");
    show();
    expect(within(screen.getByRole("list", { name: "示教图检查（试测）" })).getByText(why)).toBeInTheDocument();
  });

  it("试测只有杠杆臂告警 → 勾选喷嘴确认后才能保存，载荷带 fixtureReviewed", async () => {
    // 防的回归（定稿 §9）：框里有没有喷嘴工具判断不了，没人工确认也能保存；或确认没传给后端
    withSavedFixture([item("selfLocate", true, "示教图上定位被采用"), item("lever", false, "杠杆臂 L = 120 mm", "warn")], "trialPassed");
    vi.mocked(workspaceApi.saveTeach).mockResolvedValue(ws.data!);
    show();
    const save = screen.getByRole("button", { name: "保存本组示教" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(expect.stringContaining("喷嘴"));
    await userEvent.click(screen.getByRole("checkbox", { name: "已确认定位框内没有胶（含越过示教段后拐弯、其他 Pose 或其他胶路的胶），也没有喷嘴、胶枪等随相机不动的结构" }));
    await userEvent.click(screen.getByRole("button", { name: "保存本组示教" }));
    expect(workspaceApi.saveTeach).toHaveBeenCalledWith("A", 7, 0, "A-image-0", true);
  });

  it.each([
    ["没有当前修订的验证统计 → 不能确认", null],
    ["转角证据不满足前两个条件 → 不能确认", false],
    ["证据满足、选了独立证据并写了说明 → 带 rotation 保存", true],
  ] as const)("开启转角：%s", async (_, autoConditions) => {
    // 防的回归（定稿 §10 决定二，三个条件都要满足）：没有验证证据或证据不满足也能确认开转角；
    // 开转角不经显式确认，或经草稿自动保存悄悄开启
    withSavedFixture([item("selfLocate", true, "示教图上定位被采用")], "trialPassed");
    if (autoConditions !== null) {
      const rotation: FixtureRotationEvidence = { enabled: false, leverMm: 80, leverThetaP95Mm: .2, thresholdMm: .1, exceeds: true, frames: 36, observableFrames: 36, allObservable: autoConditions, autoConditions };
      ws.data!.workspace.validation!.fixture = [{ k: 0, view: 1, shotId: "P1", passed: true, samples: 12, checks: [], sigmaMm: .02, meanMm: 0, nominalMm: 0, toleranceMm: .5,
        searchUse: .1, rows: [], suggestion: null, rotation }];
    }
    vi.mocked(workspaceApi.saveParams).mockResolvedValue(ws.data!);
    show();
    await userEvent.click(screen.getByRole("button", { name: "高级：开启转角…" }));
    const dialog = screen.getByRole("dialog", { name: "开启转角（高级）" });
    const confirm = within(dialog).getByRole("button", { name: /确认开启/ });
    expect(confirm).toBeDisabled(); // 还没选独立证据、没写说明
    fireEvent.change(within(dialog).getByRole("combobox", { name: "独立证据" }).closest(".qs-select")!.querySelector("select")!, { target: { value: "twoTemplates" } });
    await userEvent.type(within(dialog).getByRole("textbox", { name: "说明（记进应用日志）" }), "两个模板转角一致");
    if (!autoConditions) {
      expect(confirm).toBeDisabled();
      return;
    }
    await userEvent.click(confirm);
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 7, 0, expect.objectContaining({
      fixture: expect.objectContaining({ angleRangeDeg: 3 }), rotation: { basis: "twoTemplates", note: "两个模板转角一致" } }));
    expect(ws.setFrameDraft).not.toHaveBeenCalledWith(0, expect.objectContaining({ fixture: expect.objectContaining({ angleRangeDeg: 3 }) }));
  });

  it("位置名义值不为 0 → 出现量具确认件编号，为空时不能试测", async () => {
    // 防的回归（定稿 §3）：名义值 ≠ 0 却没有 gaugeRef 也能试测，保存到后端才报错
    const limits = structuredClone(ws.doc!.limits);
    ws.frameDrafts = { 0: { path: [[10, 10], [20, 10]], mmPerPx: .1, limits, fixture: null } }; ws.frameDirty = true;
    show();
    expect(screen.getByRole("textbox", { name: "量具确认件编号" })).toBeInTheDocument();
    const trial = screen.getByRole("button", { name: "运行试测" });
    expect(trial).toBeDisabled();
    expect(trial).toHaveAccessibleDescription(expect.stringContaining("量具确认件编号"));
    await userEvent.type(screen.getByRole("textbox", { name: "量具确认件编号" }), "G");
    expect(ws.setFrameDraft).toHaveBeenLastCalledWith(0, expect.objectContaining({ limits: expect.objectContaining({ gaugeRef: "G" }) }));
  });

  it("定位试测结果 → 草稿框改动后清空", async () => {
    // 防的回归：旧框的定位试测结果留在页面上，被当成新框的结果
    withSavedFixture([item("selfLocate", true, "示教图上定位被采用")], "trialPassed");
    const trial: FixtureTrial = { revision: 7, k: 0, view: 1, rows: [{ label: "示教图 A-image-0", source: "teach", sampleId: null, reason: null, error: null, stations: [],
      pose: { accepted: true, reason: null, score: .98, peakRatio: .3, angle: 0, offset: [0, 0], endShiftMm: [0, 0], excludedMm: [1, 1], endLimitMm: 6 } }] };
    vi.mocked(workspaceApi.fixtureTrial).mockResolvedValue(trial);
    const view = show();
    await userEvent.click(screen.getByRole("button", { name: "定位试测" }));
    expect(await screen.findByRole("table", { name: "定位试测逐帧结果" })).toBeInTheDocument();
    ws.frameDrafts = { 0: { path: [[10, 10], [20, 10]], mmPerPx: .1, fixture: fixture({ searchRadiusPx: 40 }) } }; ws.frameDirty = true;
    view.rerender(<MemoryRouter initialEntries={["/recipe/teach"]}><TeachingPage /></MemoryRouter>);
    await waitFor(() => expect(screen.queryByRole("table", { name: "定位试测逐帧结果" })).not.toBeInTheDocument());
  });
});

import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { burstRecipe, burstWorkspaceView, captureRoundV2, frameMetrics, snapshot, workspaceState } from "./fixtures";
import { stubSvgPointer } from "./svg-pointer";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveDoc: vi.fn(), teachSegmentCorridor: vi.fn(), captureGet: vi.fn(), captureFrameImage: vi.fn(), teachPreviewFrame: vi.fn(), teachPreviewSegment: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const picture = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
function show(path = "/recipe/teach", page = <TeachingPage />) {
  return render(<MemoryRouter initialEntries={[path]}>{page}</MemoryRouter>);
}
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ id: "A" }))); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [] });
  vi.mocked(workspaceApi.image).mockResolvedValue(picture);
  vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
  vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({ recipeId: "A" }));
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.teachSegmentCorridor).mockResolvedValue(ws.data!);
});

describe("走廊示教", () => {

  // 防的回归：段走廊画了一半就切到别的段或相机参数，草稿按“相机#段”存、切走后不再显示，操作员以为丢了
  it("段走廊有未保存的修改 → 其他段与相机参数不能切换并说明先保存，当前段可继续编辑", async () => {
    ws.corridorDrafts = { "CAM-2#1": { corridor: [[10, 10], [30, 10]], mmPerPx: .1, detect: null, window: null } }; ws.frameDirty = true;
    show();
    for (const name of ["选择 CAM-1 段 1 走廊", "选择相机 CAM-2"]) {
      const other = screen.getByRole("button", { name });
      expect(other).toBeDisabled();
      expect(other).toHaveAccessibleDescription(expect.stringContaining("先保存"));
    }
    expect(screen.getByRole("button", { name: "选择 CAM-2 段 1 走廊" })).toBeEnabled();
    expect(within(screen.getByRole("group", { name: "走廊示教操作" })).getByText(/未保存的修改/)).toBeVisible();
    await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
  });

  // 防的回归：按站距不足三站的走廊能保存，lyFlow 会把这一段有胶的帧都测成无胶
  it("段走廊按站距不足三站 → 保存禁用并写明实际站数", async () => {
    ws.corridorDrafts = { "CAM-2#1": { corridor: [[10, 10], [20, 10]], mmPerPx: .1, detect: null, window: null } }; ws.frameDirty = true;
    show();
    const save = screen.getByRole("button", { name: "保存走廊" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(expect.stringContaining("只有 2 站"));
    await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
  });

  // 防的回归（D-SB-45）：像素当量随相机，段走廊草稿里没有自己的当量；相机缺当量时仍能保存段走廊，测量长度与搜索带无从换算
  it("相机缺像素当量 → 段走廊保存禁用并说明原因，不写配方", async () => {
    ws.data!.workspace.doc.inspect![1].mmPerPx = null; ws.data!.plan!.cameraTeaching[1].mmPerPx = null;
    ws.corridorDrafts = { "CAM-2#1": { corridor: [[10, 10], [30, 10]], mmPerPx: null, detect: null, window: null } }; ws.frameDirty = true;
    show();
    const save = screen.getByRole("button", { name: "保存走廊" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(expect.stringContaining("像素当量"));
    await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
    expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
  });
});

describe("相机测量参数", () => {

  // 防的回归：配方默认与相机自定义合并成一组字段后，取默认时的改动写成本相机自定义，其他相机不再跟随
  it("检测参数取配方默认时改搜索半宽 → 写入配方默认值，本相机仍跟随默认", async () => {
    show("/recipe/teach?camera=CAM-1");
    fireEvent.change(await screen.findByRole("spinbutton", { name: "搜索半宽（mm）" }), { target: { value: "6" } });
    const doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(doc.detect.searchMm).toBe(6);
    expect(doc.inspect!.find(item => item.camera === "CAM-1")!.detect).toBeNull();
  });
});

describe("站距", () => {

  // 防的回归：站距改大后已保存的走廊不足三站，配方照样保存，到试测、标定或生产时才报错或测成无胶
  it("站距改到让已保存的走廊不足三站 → 提示实际站数与可用站距，不写配方；改到够三站 → 写入", async () => {
    show("/recipe/teach?camera=CAM-1");
    fireEvent.change(await screen.findByRole("spinbutton", { name: "站距" }), { target: { value: "5" } });
    expect(screen.getByRole("alert")).toHaveTextContent(/只有 2 站.*不大于 4 mm/);
    expect(ws.setDoc).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("spinbutton", { name: "站距" }), { target: { value: "4" } });
    expect(vi.mocked(ws.setDoc).mock.lastCall![0].spacing).toBe(4);
  });
});

describe("检测窗口", () => {

  // 防的回归（D-SB-47）：长度为 0 或负数的窗口写进候选配方，自动保存被后端校验拒绝、反复失败；合法窗口没写到这台相机
  it.each(["0", "-5"])("窗口长度 %s mm → 提示长度需大于 0，不写配方；改成 20 mm → 写入这台相机的窗口", async length => {
    show("/recipe/teach?camera=CAM-1");
    fireEvent.click(await screen.findByRole("button", { name: /^逐帧定向/ }));
    fireEvent.change(await screen.findByRole("spinbutton", { name: "窗口起点" }), { target: { value: "2" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "窗口长度" }), { target: { value: length } });
    expect(screen.getByRole("alert")).toHaveTextContent("检测窗口长度需大于 0 mm");
    expect(ws.setDoc).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("spinbutton", { name: "窗口长度" }), { target: { value: "20" } });
    const doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(doc.inspect!.find(item => item.camera === "CAM-1")!.window).toEqual({ startMm: 2, lengthMm: 20 });
  });
});

describe("出胶点标定", () => {

  // 防的回归（D-SB-47）：标出胶点时点图仍按编辑点给走廊加点；出胶点写到段走廊而不是相机上，别的段用不到
  it("段开启逐帧定向、选“标出胶点”后点图 → 写入这台相机的出胶点并显示叠加，走廊不加点", async () => {
    ws.data!.workspace.doc.segments![0].heading = { toleranceDeg: 5 };
    ws.data!.workspace.doc.inspect![1].window = { startMm: 2, lengthMm: 4 };
    stubSvgPointer();
    show("/recipe/teach?camera=CAM-2&segment=1");
    const image = await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
    fireEvent.click(screen.getByLabelText("标出胶点"));
    fireEvent.pointerDown(image, { button: 0, clientX: 40, clientY: 30, pointerId: 1 });
    const doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(doc.inspect!.find(item => item.camera === "CAM-2")!.nozzle).toEqual([40, 30]);
    expect(ws.setCorridorDraft).not.toHaveBeenCalled();
    ws.data!.workspace.doc.inspect![1].nozzle = [40, 30];
    fireEvent.click(screen.getByLabelText("编辑点"));
    expect(await screen.findByLabelText("出胶点（已标定）")).toBeInTheDocument();
    expect(screen.getByLabelText("检测窗口环带")).toBeInTheDocument();
  });
});

describe("段的逐帧定向与胶偏参考", () => {
  const lastSegment = () => vi.mocked(ws.setDoc).mock.lastCall![0].segments![0];

  // 防的回归（D-SB-47）：逐帧定向开关写成别的形状，或关掉后仍留着设置（null 才是关闭、回到示教走廊）
  it.each([
    ["关闭的段打开逐帧定向", null, { toleranceDeg: 5 }],
    ["开启的段关闭逐帧定向", { toleranceDeg: 8 }, null],
  ])("%s → 写入的段 heading 为 %j", async (_, initial, expected) => {
    ws.data!.workspace.doc.segments![0].heading = initial;
    show("/recipe/teach?camera=CAM-2&segment=1");
    fireEvent.click(await screen.findByRole("switch", { name: "段 1 逐帧定向" }));
    expect(lastSegment().heading).toEqual(expected);
  });

  // 防的回归：越界或清空的方向容差写进候选配方，自动保存被后端校验拒绝、反复失败
  it("方向容差改为 0° → 提示且不写配方；改为 8° → 写入本段 heading", async () => {
    ws.data!.workspace.doc.segments![0].heading = { toleranceDeg: 5 };
    show("/recipe/teach?camera=CAM-2&segment=1");
    const tolerance = await screen.findByRole("spinbutton", { name: "方向容差" });
    fireEvent.change(tolerance, { target: { value: "0" } });
    expect(screen.getByRole("alert")).toHaveTextContent("方向容差需在 0–45° 之间");
    expect(ws.setDoc).not.toHaveBeenCalled();
    fireEvent.change(tolerance, { target: { value: "8" } });
    expect(lastSegment().heading).toEqual({ toleranceDeg: 8 });
  });

  // 防的回归（D-SB-48）：选 toEdge 没带找边参数，或改回 absoluteX 仍留着 edge（后端校验拒绝 absoluteX 带找边参数，整份配方存不进去）
  const edge = { polarity: "brightToDark" as const, searchMm: 20, contrastMin: 40, nominalMm: null };
  it.each([
    ["absoluteX 改为 toEdge → 写入默认找边参数并清空 x_ref", { measure: "absoluteX" as const, camera: "CAM-1", xRef: 0.1, spread: 0.02 }, "toEdge", { measure: "toEdge", camera: "CAM-1", xRef: null, spread: null, edge }],
    ["toEdge 改回 absoluteX → 去掉找边参数", { measure: "toEdge" as const, camera: "CAM-1", xRef: null, spread: null, edge }, "absoluteX", { measure: "absoluteX", camera: "CAM-1", xRef: null, spread: null }],
  ])("胶偏测量量 %s", async (_, initial, target, expected) => {
    ws.data!.workspace.doc.segments![0].offsetRef = initial;
    show("/recipe/teach?camera=CAM-2&segment=1");
    fireEvent.click(await screen.findByRole("button", { name: /^检测项/ }));
    const measure = (await screen.findByRole("combobox", { name: "段 1 胶偏测量量" })).closest(".qs-select")!.querySelector("select")!;
    fireEvent.change(measure, { target: { value: target } });
    expect(lastSegment().offsetRef).toStrictEqual(expected);
  });
});

describe("本段试测", () => {

  // 防的回归：试测没用示教轮次与本段（测成别的轮次或段），或试测时示教画布仍在、点图会改走廊
  it("全段试测后点一帧 → 用示教轮次试测这一帧并替换示教画布；回到示教帧恢复", async () => {
    vi.mocked(workspaceApi.teachPreviewSegment).mockResolvedValue({ round: "round-1", start: 1, end: 3,
      frames: [frameMetrics(0, 1), frameMetrics(0, 2, { headingFallback: true }), frameMetrics(0, 3, { state: "missing" })] });
    vi.mocked(workspaceApi.teachPreviewFrame).mockResolvedValue({ segment: 1, corridor: [[10, 30], [90, 30]], searchHalfPx: 4, edges: [], metrics: frameMetrics(0, 3) });
    show("/recipe/teach?camera=CAM-1&segment=1");
    await screen.findByRole("img", { name: "CAM-1 · 段 1 · 示教帧" });
    fireEvent.click(screen.getByRole("button", { name: "全段试测" }));
    expect(workspaceApi.teachPreviewSegment).toHaveBeenCalledWith("A", "CAM-1", 1, "round-1");
    fireEvent.click(await screen.findByRole("button", { name: "帧组 3：缺帧或未测" }));
    expect(await screen.findByRole("img", { name: "CAM-1 · 段 1 · 帧组 3 · 试测" })).toBeInTheDocument();
    expect(workspaceApi.teachPreviewFrame).toHaveBeenCalledWith("A", "CAM-1", 1, "round-1", 3);
    expect(screen.queryByRole("img", { name: "CAM-1 · 段 1 · 示教帧" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "回到示教帧" }));
    expect(await screen.findByRole("img", { name: "CAM-1 · 段 1 · 示教帧" })).toBeInTheDocument();
  });

  // 防的回归：走廊有未保存的修改时仍按已保存的旧走廊试测，结果与画面上的走廊对不上
  it("走廊有未保存的修改 → 全段试测禁用并说明先保存", async () => {
    ws.corridorDrafts = { "CAM-1#1": { corridor: [[10, 10], [30, 10]], mmPerPx: .1, detect: null, window: null } }; ws.frameDirty = true;
    show("/recipe/teach?camera=CAM-1&segment=1");
    expect(await screen.findByRole("button", { name: "全段试测" })).toBeDisabled();
    expect(screen.getByText("走廊有未保存的修改：保存后再试测")).toBeInTheDocument();
    expect(workspaceApi.teachPreviewSegment).not.toHaveBeenCalled();
  });
});

describe("示教帧选择", () => {

  // 防的回归：选了缺帧的帧组仍能提交，后端冻结不到图（规格 §3：缺帧只告警，但这一帧没有图可示教）；进度链接带的段没定位到，选帧落到别的段
  it("链接指定相机与段、所选帧组这台相机缺帧 → 打开即选这一段的帧，不读图、用此帧示教禁用并说明缺帧", async () => {
    ws.data!.workspace.captureReview = { rounds: { "round-1": { groupCount: 12, cuts: [4, 8], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
    show("/recipe/teach?camera=CAM-2&segment=2");
    await screen.findByText("第 2 段在示教件为帧组 5–8");
    fireEvent.change(await screen.findByRole("spinbutton", { name: "帧组序号" }), { target: { value: "5" } });
    const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-2 · 段 2" });
    await vi.waitFor(() => expect(choose).toBeDisabled());
    expect(choose).toHaveAccessibleDescription(expect.stringContaining("缺帧"));
    expect(workspaceApi.captureFrameImage).not.toHaveBeenCalledWith("round-1", "CAM-2", 5);
    expect(workspaceApi.teachSegmentCorridor).not.toHaveBeenCalled();
  });
});

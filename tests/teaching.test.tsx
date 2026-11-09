import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { burstRecipe, burstWorkspaceView, captureRoundV2, frameMetrics, pathSegment, snapshot, workspaceState } from "./fixtures";
import { stubSvgPointer } from "./svg-pointer";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveDoc: vi.fn(), teachSegmentCorridor: vi.fn(), captureGet: vi.fn(), captureFrameImage: vi.fn(), teachPreviewFrame: vi.fn(), teachPreviewSegment: vi.fn(), fitColorModel: vi.fn() } }));

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

  it("单段没有分段审查记录 → 按实际轮次帧数标记笔首及段尾跳过，无胶统计排除跳过帧", async () => {
    ws = workspaceState(burstWorkspaceView(burstRecipe({ id: "A", segments: [pathSegment(1, "straight", { expectedFrames: 12, skipTail: 1 })], passSkip: { head: 4, tail: 0 } })));
    ws.data!.workspace.teachingPiece!.passes = [12];
    vi.mocked(workspaceApi.teachPreviewSegment).mockResolvedValue({ round: "round-1", start: 1, end: 12, frames: Array.from({ length: 12 }, (_, i) => frameMetrics(0, i + 1, { presentRatio: 0 })) });
    show("/recipe/teach?camera=CAM-1&segment=1");
    expect(await screen.findByRole("button", { name: "帧组 1：生产跳过" })).toBeVisible();
    expect(screen.getByRole("button", { name: "帧组 12：生产跳过" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "全段试测" }));
    const counts = within(screen.getByLabelText("试测统计"));
    expect(await counts.findByText("无胶 7")).toBeVisible();
    expect(counts.getByText("生产跳过 5")).toBeVisible();
  });

  it("检测段两侧为免检段 → 两侧保护帧合并到生产跳过，段内其余无胶帧仍计数", async () => {
    const doc = ws.data!.workspace.doc;
    doc.segments![0].skip = true;
    doc.segments![2].skip = true;
    doc.boundaryGuard = 1;
    doc.passSkip = { head: 1, tail: 0 };
    ws.data!.workspace.teachingPiece!.passes = [4, 4, 4];
    ws.data!.workspace.captureReview = { rounds: { "round-1": { groupCount: 12, cuts: [4, 8], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
    vi.mocked(workspaceApi.teachPreviewSegment).mockResolvedValue({ round: "round-1", start: 5, end: 8, frames: Array.from({ length: 4 }, (_, i) => frameMetrics(0, i + 5, { presentRatio: 0 })) });
    show("/recipe/teach?camera=CAM-1&segment=2");
    expect(await screen.findByRole("button", { name: "帧组 5：生产跳过" })).toHaveAttribute("title", expect.stringContaining("段 1 不检测的边界保护"));
    expect(screen.getByRole("button", { name: "帧组 8：生产跳过" })).toHaveAttribute("title", expect.stringContaining("段 3 不检测的边界保护"));
    fireEvent.click(screen.getByRole("button", { name: "全段试测" }));
    const counts = within(screen.getByLabelText("试测统计"));
    expect(await counts.findByText("无胶 2")).toBeVisible();
    expect(counts.getByText("生产跳过 2")).toBeVisible();
  });

  // 防的回归：走廊草稿按站距不足三站、不会自动保存时还能切到别的段，草稿按“相机#段”存、切走后不再显示，操作员以为丢了；
  // 或者能自动保存的草稿在等保存时也把切换按钮变灰，每改一下界面就闪一次
  it.each([
    ["按站距不足三站、不会自动保存 → 其他段与相机参数不能切换并说明原因", { unsavable: true, pendingSave: false }, true],
    ["等着自动保存 → 其他段与相机参数照常可切换，不随保存变灰", { unsavable: false, pendingSave: true }, false],
  ])("段走廊草稿%s", async (_, state, locked) => {
    Object.assign(ws, state, { corridorDrafts: { "CAM-2#1": { corridor: [[10, 10], [30, 10]], mmPerPx: .1, window: null } }, frameDirty: true });
    show();
    for (const name of ["选择 CAM-1 段 1 走廊", "选择相机 CAM-2"]) {
      const other = screen.getByRole("button", { name });
      if (locked) {
        expect(other).toBeDisabled();
        expect(other).toHaveAccessibleDescription(expect.stringContaining("不会自动保存"));
      } else expect(other).toBeEnabled();
    }
    expect(screen.getByRole("button", { name: "选择 CAM-2 段 1 走廊" })).toBeEnabled();
    await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
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
    expect(screen.getByLabelText("测量区间环带")).toBeInTheDocument();
  });
});

describe("画布模式", () => {

  // 防的回归：打开示教帧默认就是“编辑点”，看图时随手一点就给走廊加了点
  it("打开示教帧默认只看 → 点图不加走廊点；选“编辑点”后点图才加点", async () => {
    stubSvgPointer();
    show("/recipe/teach?camera=CAM-2&segment=1");
    const image = await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
    expect(screen.getByRole("button", { name: "只看" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.pointerDown(image, { button: 0, clientX: 40, clientY: 30, pointerId: 1 });
    expect(ws.setCorridorDraft).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "编辑点" }));
    fireEvent.pointerDown(image, { button: 0, clientX: 40, clientY: 30, pointerId: 1 });
    expect(ws.setCorridorDraft).toHaveBeenCalled();
  });
});

describe("搜索扇区", () => {

  // 防的回归（D-SB-47）：示教帧上拖动扇区把手只转了画面，方向没写进本段的配方，测量仍按原来的半圈找胶条；
  // 扇区按相机存，拖一段的扇区同一相机其他段跟着转；或者拖动中每动一下就写配方，触发自动保存、界面跟着闪
  it("拖动段 1 的扇区把手到出胶点正下方再松手 → 拖动中不写配方，松手后段 1 的扇区方向存为 90°，同一相机段 2 不变", async () => {
    const doc = ws.data!.workspace.doc;
    for (const item of doc.segments!.slice(0, 2)) item.heading = { toleranceDeg: 5 };
    const cam1 = doc.inspect![0];
    Object.assign(cam1, { nozzle: [40, 30], window: { startMm: 2, lengthMm: 4 } });
    cam1.segments![0].sectorDeg = 0;
    cam1.segments![1].sectorDeg = 0;
    stubSvgPointer();
    show("/recipe/teach?camera=CAM-1&segment=1");
    const image = await screen.findByRole("img", { name: "CAM-1 · 段 1 · 示教帧" });
    fireEvent.pointerDown(await screen.findByRole("button", { name: "搜索扇区把手" }), { button: 0, clientX: 80, clientY: 30, pointerId: 1 });
    fireEvent.pointerMove(image, { clientX: 40, clientY: 50, pointerId: 1 });
    expect(ws.setDoc).not.toHaveBeenCalled();
    fireEvent.pointerUp(image, { pointerId: 1 });
    const saved = vi.mocked(ws.setDoc).mock.lastCall![0].inspect!.find(item => item.camera === "CAM-1")!.segments!;
    expect(saved.map(item => [item.segment, item.sectorDeg, item.corridor.length])).toEqual([[1, 90, 2], [2, 0, 2], [3, undefined, 2]]);
  });
});

describe("测量区间", () => {

  // 防的回归（D-SB-47）：面板按“跳过前 / 测到”填，配方仍存起点与长度；把“测到”当长度写进配方，实测区间整体后移；
  // 测量区间按段设置后仍写到相机上，同一相机各段共用一个区间
  it("段 1 跳过前 5 mm、测到 25 mm → 段 1 存起点 5、长度 20，相机不变；测到不大于跳过 → 提示且不写配方", async () => {
    ws.data!.workspace.doc.segments![0].heading = { toleranceDeg: 5 };
    show("/recipe/teach?camera=CAM-2&segment=1");
    const zone = await screen.findByRole("group", { name: "段 1 测量区间" });
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "跳过前" }), { target: { value: "5" } });
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "测到距出胶点" }), { target: { value: "25" } });
    const doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(doc.segments![0].heading!.window).toEqual({ startMm: 5, lengthMm: 20 });
    expect(doc.inspect!.find(item => item.camera === "CAM-2")!.window).toBeUndefined();
    const calls = vi.mocked(ws.setDoc).mock.calls.length;
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "测到距出胶点" }), { target: { value: "5" } });
    expect(within(zone).getByRole("alert")).toHaveTextContent("测量到的距离需大于跳过的距离");
    expect(ws.setDoc).toHaveBeenCalledTimes(calls);
  });

  // 防的回归（PR #274 评审）：沿用相机级区间的段第一次改动就写成段级，区间来源一变输入框被重建、失去焦点，多位数输入被打断
  it("段沿用相机级区间、改跳过前 → 写入段级区间，输入框仍是同一个", async () => {
    const doc = ws.data!.workspace.doc;
    doc.segments![0].heading = { toleranceDeg: 5 };
    doc.inspect![1].window = { startMm: 5, lengthMm: 20 };
    const { rerender } = show("/recipe/teach?camera=CAM-2&segment=1");
    const input = within(await screen.findByRole("group", { name: "段 1 测量区间" })).getByRole("spinbutton", { name: "跳过前" });
    fireEvent.change(input, { target: { value: "3" } });
    ws.doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(ws.doc.segments![0].heading!.window).toEqual({ startMm: 3, lengthMm: 22 });
    rerender(<MemoryRouter initialEntries={["/recipe/teach?camera=CAM-2&segment=1"]}><TeachingPage /></MemoryRouter>);
    expect(within(screen.getByRole("group", { name: "段 1 测量区间" })).getByRole("spinbutton", { name: "跳过前" })).toBe(input);
  });
});

describe("相机测量区间与段检测参数", () => {

  // 防的回归（D-SB-47）：相机的测量区间是这台相机各段的默认值，相机视图改了却写到段上或没写进相机，开启定向的段仍按默认 5–20 mm 测
  it("相机视图 CAM-2 测到 30 mm → CAM-2 存起点 5、长度 25，段 1 不写覆盖", async () => {
    ws.data!.workspace.doc.segments![0].heading = { toleranceDeg: 5 };
    show("/recipe/teach?camera=CAM-2");
    fireEvent.click(await screen.findByRole("button", { name: "选择相机 CAM-2" }));
    const zone = await screen.findByRole("group", { name: "CAM-2 测量区间" });
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "测到距出胶点" }), { target: { value: "30" } });
    const doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(doc.inspect!.find(item => item.camera === "CAM-2")!.window).toEqual({ startMm: 5, lengthMm: 25 });
    expect(doc.segments![0].heading!.window).toBeUndefined();
  });

  // 防的回归：段视图改搜索半宽写到配方默认或别的段上；与配方默认合并后胶宽上限不小于搜索宽度仍写进配方（后端拒存，自动保存反复失败）
  it("段 2 搜索半宽改 8 mm → 只写段 2 的覆盖；再把胶宽上限改到 16 mm → 提示且不写配方", async () => {
    const path = "/recipe/teach?camera=CAM-1&segment=2";
    const { rerender } = show(path);
    fireEvent.click(await screen.findByRole("button", { name: /^检测参数/ }));
    fireEvent.change(screen.getByRole("spinbutton", { name: "搜索半宽" }), { target: { value: "8" } });
    ws.doc = vi.mocked(ws.setDoc).mock.lastCall![0];
    expect(ws.doc.segments!.map(item => item.searchMm)).toEqual([undefined, 8, undefined]);
    expect(ws.doc.detect.searchMm).toBe(4);
    rerender(<MemoryRouter initialEntries={[path]}><TeachingPage /></MemoryRouter>);
    const calls = vi.mocked(ws.setDoc).mock.calls.length;
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽上限" }), { target: { value: "16" } });
    expect(screen.getByRole("alert")).toHaveTextContent("胶宽上限要小于搜索宽度 16.0 mm");
    expect(ws.setDoc).toHaveBeenCalledTimes(calls);
  });
});

describe("示教帧选择", () => {

  // 防的回归：选了缺帧的帧组仍能提交，后端冻结不到图（规格 §3：缺帧只告警，但这一帧没有图可示教）；进度链接带的段没定位到，选帧落到别的段
  it("链接指定相机与段、所选帧组这台相机缺帧 → 打开即选这一段的帧，不读图、用此帧示教禁用并说明缺帧", async () => {
    ws.data!.workspace.captureReview = { rounds: { "round-1": { groupCount: 12, cuts: [4, 8], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } } };
    show("/recipe/teach?camera=CAM-2&segment=2");
    await screen.findByText("第 2 段在示教件为帧组 5–8");
    fireEvent.click(await screen.findByRole("button", { name: "帧组 5 · CAM-2 · 缺帧" }));
    const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-2 · 段 2" });
    await vi.waitFor(() => expect(choose).toBeDisabled());
    expect(choose).toHaveAccessibleDescription(expect.stringContaining("缺帧"));
    expect(vi.mocked(workspaceApi.captureFrameImage).mock.calls.map(args => args.slice(0, 3))).not.toContainEqual(["round-1", "CAM-2", 5]);
    expect(workspaceApi.teachSegmentCorridor).not.toHaveBeenCalled();
  });
});

describe("测量方式与取色", () => {

  // 防的回归：彩色示教帧上找不到取色入口，或取色发错相机、段、修订，这一段一直按灰度测；取色后看不到分离度，无法判断模型能不能用
  it("彩色示教帧、按胶色测量、这一段已保存走廊 → 点“取色”按相机与段调用取色命令，显示分离度", async () => {
    vi.mocked(workspaceApi.image).mockResolvedValue({ ...picture, channels: 3 });
    ws.data!.workspace.doc.inspect![0].segments![0].measure = "color";
    const model = { glue: { mean: [0.37, -0.08], intensity: 391.7 }, background: [{ weight: 1, mean: [-0.08, 0], intensity: 394.9 }],
      quality: { separation: 4.8, glueCount: 2652, bgCount: 9552, saturatedFraction: 0.156, usable: true } };
    vi.mocked(workspaceApi.fitColorModel).mockImplementation(async () => {
      const next = structuredClone(ws.data!);
      next.workspace.doc.inspect![0].segments![0].colorModel = JSON.stringify(model);
      ws.data = next; ws.doc = next.workspace.doc;
      return next;
    });
    const path = "/recipe/teach?camera=CAM-1&segment=1";
    const { rerender } = show(path);
    fireEvent.click(await screen.findByRole("button", { name: "取色" }));
    await waitFor(() => expect(workspaceApi.fitColorModel).toHaveBeenCalledWith("A", 3, "CAM-1", 1));
    rerender(<MemoryRouter initialEntries={[path]}><TeachingPage /></MemoryRouter>);
    const stats = await screen.findByRole("group", { name: "胶色模型" });
    expect(within(stats).getByText("分离度").nextElementSibling).toHaveTextContent("4.8");
    expect(screen.getByRole("button", { name: "重新取色" })).toBeEnabled();
  });

  // 防的回归：彩色相机的段只能按胶色测，操作员改按灰度的选择没写进这台相机这一段，或留着胶色模型继续按胶色测
  it("彩色示教帧上选“按灰度” → 这一段这台相机记为按灰度并去掉胶色模型", async () => {
    vi.mocked(workspaceApi.image).mockResolvedValue({ ...picture, channels: 3 });
    ws.data!.workspace.doc.inspect![0].segments![0].colorModel = "{\"glue\":{}}";
    show("/recipe/teach?camera=CAM-1&segment=1");
    fireEvent.click(await screen.findByRole("button", { name: "按灰度" }));
    const next = vi.mocked(ws.setDoc).mock.calls.at(-1)![0] as typeof ws.doc;
    const segments = next!.inspect!.map(item => item.segments?.find(s => s.segment === 1));
    expect(segments[0]).toMatchObject({ measure: "gray" });
    expect(segments[0]).not.toHaveProperty("colorModel");
    expect(segments.slice(1).every(s => s?.measure == null)).toBe(true);
  });
});

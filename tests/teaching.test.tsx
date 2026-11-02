import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { burstRecipe, burstWorkspaceView, captureRoundV2, snapshot, workspaceState } from "./fixtures";
import { stubSvgPointer } from "./svg-pointer";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn(), recipeApi: { expertSpecs: vi.fn(async () => ({})) } }));
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

  // 防的回归：走廊草稿按站距不足三站、不会自动保存时还能切到别的段，草稿按“相机#段”存、切走后不再显示，操作员以为丢了；
  // 或者能自动保存的草稿在等保存时也把切换按钮变灰，每改一下界面就闪一次
  it.each([
    ["按站距不足三站、不会自动保存 → 其他段与相机参数不能切换并说明原因", { unsavable: true, pendingSave: false }, true],
    ["等着自动保存 → 其他段与相机参数照常可切换，不随保存变灰", { unsavable: false, pendingSave: true }, false],
  ])("段走廊草稿%s", async (_, state, locked) => {
    Object.assign(ws, state, { corridorDrafts: { "CAM-2#1": { corridor: [[10, 10], [30, 10]], mmPerPx: .1, detect: null, window: null } }, frameDirty: true });
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

  // 防的回归（D-SB-47）：示教帧上拖动扇区把手只转了画面，方向没写进这台相机的配方，测量仍按原来的半圈找胶条；
  // 或者拖动中每动一下就写配方，触发自动保存、界面跟着闪
  it("拖动扇区把手到出胶点正下方再松手 → 拖动中不写配方，松手后这台相机的扇区方向存为 90°", async () => {
    ws.data!.workspace.doc.segments![0].heading = { toleranceDeg: 5 };
    Object.assign(ws.data!.workspace.doc.inspect![1], { nozzle: [40, 30], window: { startMm: 2, lengthMm: 4 }, sectorDeg: 0 });
    stubSvgPointer();
    show("/recipe/teach?camera=CAM-2&segment=1");
    const image = await screen.findByRole("img", { name: "CAM-2 · 段 1 · 示教帧" });
    fireEvent.pointerDown(await screen.findByRole("button", { name: "搜索扇区把手" }), { button: 0, clientX: 80, clientY: 30, pointerId: 1 });
    fireEvent.pointerMove(image, { clientX: 40, clientY: 50, pointerId: 1 });
    expect(ws.setDoc).not.toHaveBeenCalled();
    fireEvent.pointerUp(image, { pointerId: 1 });
    expect(vi.mocked(ws.setDoc).mock.lastCall![0].inspect!.find(item => item.camera === "CAM-2")!.sectorDeg).toBe(90);
  });
});

describe("测量区间", () => {

  // 防的回归（D-SB-47）：面板按“跳过前 / 测到”填，配方仍存起点与长度；把“测到”当长度写进配方，实测区间整体后移
  it("跳过前 5 mm、测到 25 mm → 配方存起点 5、长度 20；测到不大于跳过 → 提示且不写配方", async () => {
    ws.data!.workspace.doc.segments![0].heading = { toleranceDeg: 5 };
    show("/recipe/teach?camera=CAM-2&segment=1");
    const zone = await screen.findByRole("group", { name: "出胶点与测量区间" });
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "跳过前" }), { target: { value: "5" } });
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "测到距出胶点" }), { target: { value: "25" } });
    expect(vi.mocked(ws.setDoc).mock.lastCall![0].inspect!.find(item => item.camera === "CAM-2")!.window).toEqual({ startMm: 5, lengthMm: 20 });
    const calls = vi.mocked(ws.setDoc).mock.calls.length;
    fireEvent.change(within(zone).getByRole("spinbutton", { name: "测到距出胶点" }), { target: { value: "5" } });
    expect(within(zone).getByRole("alert")).toHaveTextContent("测量到的距离需大于跳过的距离");
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
    expect(workspaceApi.captureFrameImage).not.toHaveBeenCalledWith("round-1", "CAM-2", 5);
    expect(workspaceApi.teachSegmentCorridor).not.toHaveBeenCalled();
  });
});

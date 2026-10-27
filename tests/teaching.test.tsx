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
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveDoc: vi.fn(), teachSegmentCorridor: vi.fn(), captureList: vi.fn(), captureGet: vi.fn(), captureFrameImage: vi.fn() } }));

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
  vi.mocked(workspaceApi.captureList).mockResolvedValue([{ roundId: "round-1", recipeId: "A", cameraIds: ["CAM-1", "CAM-2", "CAM-3"], plannedCount: 12, receivedCount: 12, state: "complete", createdAt: 1, error: null }]);
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

describe("检测窗口", () => {

  // 防的回归（D-SB-47）：长度为 0 或负数的窗口写进候选配方，自动保存被后端校验拒绝、反复失败；合法窗口没写到这台相机
  it.each(["0", "-5"])("窗口长度 %s mm → 提示长度需大于 0，不写配方；改成 20 mm → 写入这台相机的窗口", async length => {
    show("/recipe/teach?camera=CAM-1");
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

describe("示教帧选择", () => {

  // 防的回归：选了缺帧的帧组仍能提交，后端冻结不到图（规格 §3：缺帧只告警，但这一帧没有图可示教）；进度链接带的段没定位到，选帧落到别的段
  it("链接指定相机与段、所选帧组这台相机缺帧 → 打开即选这一段的帧，不读图、用此帧示教禁用并说明缺帧", async () => {
    ws.data!.workspace.captureReview = { rounds: { "round-1": { groupCount: 12, cuts: [4, 8], status: "confirmed", excludedGroups: [], segmentSignature: null, captureSignature: null, templateVersion: null } }, template: null };
    show("/recipe/teach?camera=CAM-2&segment=2");
    await screen.findByText("第 2 段在这一轮为帧组 5–8");
    fireEvent.change(await screen.findByRole("spinbutton", { name: "帧组序号" }), { target: { value: "5" } });
    const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-2 · 段 2" });
    await vi.waitFor(() => expect(choose).toBeDisabled());
    expect(choose).toHaveAccessibleDescription(expect.stringContaining("缺帧"));
    expect(workspaceApi.captureFrameImage).not.toHaveBeenCalledWith("round-1", "CAM-2", 5);
    expect(workspaceApi.teachSegmentCorridor).not.toHaveBeenCalled();
  });
});

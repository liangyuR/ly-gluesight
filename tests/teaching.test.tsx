import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import OverviewPage from "../src/features/workspace/OverviewPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { burstRecipe, burstWorkspaceView, captureRoundV2, snapshot, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveParams: vi.fn(), trial: vi.fn(), saveDoc: vi.fn(), teachCamera: vi.fn(), captureList: vi.fn(), captureGet: vi.fn(), captureFrameImage: vi.fn(), captureImage: vi.fn(), extractCenterline: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
const picture = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
function show(path = "/recipe/teach", page = <TeachingPage />) {
  return render(<MemoryRouter initialEntries={[path]}>{page}</MemoryRouter>);
}
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ id: "A" }))); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
  vi.mocked(workspaceApi.image).mockResolvedValue(picture);
  vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
  vi.mocked(workspaceApi.captureList).mockResolvedValue([{ roundId: "round-1", recipeId: "A", cameraIds: ["CAM-1", "CAM-2", "CAM-3"], plannedCount: 12, receivedCount: 12, state: "complete", createdAt: 1, error: null }]);
  vi.mocked(workspaceApi.captureGet).mockResolvedValue(captureRoundV2({ recipeId: "A" }));
  vi.mocked(workspaceApi.saveDoc).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.teachCamera).mockResolvedValue(ws.data!);
});

// 旧拍照点配方（没有段计划）仍走 G0 之前的单帧示教：按拍照组示教、保存参数并试测、按组保存（决策补充第一节：旧路径只停用不删、行为不变）
describe("单帧示教（旧拍照点配方）", () => {
  beforeEach(() => {
    ws = workspaceState();
    ws.data!.workspace.doc.shots.forEach(shot => { shot.views = []; });
  });

  it("后端判定试测已过期 → 显示后端状态与提示，不能保存本组", () => {
    const view = ws.data!.progress.shots[0].views[0];
    Object.assign(view, { state: "pendingTrial", trialStale: true, hint: "现有试测已过期，请重新保存参数并试测。" });
    ws.data!.progress.shots[0].state = "pendingTrial";
    show();
    expect(screen.queryByRole("table", { name: "逐点试测结果" })).not.toBeInTheDocument();
    expect(screen.getByText("现有试测已过期，请重新保存参数并试测。")).toBeVisible();
    expect(within(screen.getByRole("button", { name: "选择拍照组 P1" })).getByText("已过期")).toBeVisible();
    for (const name of ["保存本组示教", "保存并进入下一组"]) expect(screen.getByRole("button", { name })).toBeDisabled();
  });

  it("保存参数并试测：保存参数失败 → 说明停在保存参数一步且不试测", async () => {
    ws.frameDrafts = { 0: { path: [[10, 10], [30, 10]], mmPerPx: .1, fixture: null } }; ws.frameDirty = true;
    vi.mocked(workspaceApi.saveParams).mockRejectedValue("候选配置已更新");
    show();
    await userEvent.click(screen.getByRole("button", { name: "运行试测" }));
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 7, 0, { path: [[10, 10], [30, 10]], mmPerPx: .1, fixture: null });
    expect(ws.setError).toHaveBeenCalledWith(expect.stringContaining("保存参数未完成，未进行试测"));
    expect(workspaceApi.trial).not.toHaveBeenCalled();
  });

  it("中线有未试测的修改 → 拍照组不能切换，底栏说明先试测或清空", () => {
    ws.frameDrafts = { 0: { path: [[10, 10], [30, 10]], mmPerPx: .1, fixture: null } }; ws.frameDirty = true;
    show();
    const other = screen.getByRole("button", { name: "选择拍照组 P2" });
    expect(other).toBeDisabled();
    expect(other).toHaveAccessibleDescription(expect.stringContaining("先试测或清空"));
    expect(within(screen.getByRole("group", { name: "单帧示教操作" })).getByText(/先试测或清空后再切组/)).toBeVisible();
  });

  it("最后一组试测通过 → 可保存本组，保存并进入下一组禁用并说明已是最后一组", () => {
    show("/recipe/teach?frame=1");
    expect(screen.getByRole("button", { name: "保存本组示教" })).toBeEnabled();
    const next = screen.getByRole("button", { name: "保存并进入下一组" });
    expect(next).toBeDisabled();
    expect(next).toHaveAccessibleDescription(expect.stringContaining("最后一组"));
  });

  it("试测结论为局部超差 → 结果栏显示中文结论而不是结论代码", () => {
    ws.data!.workspace.frames[0].trial!.verdict = "OK_WITH_EXCURSION";
    show();
    const result = screen.getByLabelText("本次算法结果");
    expect(within(result).getByText("OK · 局部超差")).toBeVisible();
    expect(result).not.toHaveTextContent("OK_WITH_EXCURSION");
  });

  it.each([
    ["配方曝光与增益和采集时相同 → 不提示", 60, 6, false],
    ["配方曝光已改 → 提示采集时的值", 80, 6, true],
    ["配方增益已改 → 提示采集时的值", 60, 2, true],
  ])("%s", async (_, exposureUs, gainDb, hinted) => {
    ws.data!.workspace.doc.cameras["CAM-1"] = { exposureUs, gainDb };
    show();
    await userEvent.click(screen.getByRole("button", { name: "图像采集信息" }));
    expect(await screen.findByText("仅通过整圈重新采集")).toBeVisible();
    const hint = screen.queryByText(/采集时曝光 60 µs \/ 增益 6 dB，与配方当前值/);
    if (hinted) expect(hint).toBeVisible(); else expect(hint).toBeNull();
  });
});

describe("走廊示教", () => {
  // 防的回归：三台相机被当成一台相机的三个视图，或默认没停在待示教的相机（规格 §0：三台相机共用一个触发，每台相机一条走廊）
  it("三台计划相机 → 左栏每台相机一项并标出各自的示教状态，默认停在第一台没示教完的相机", async () => {
    show();
    const cameras = screen.getAllByRole("button", { name: /^选择相机 / });
    expect(cameras.map(button => button.getAttribute("aria-label"))).toEqual(["选择相机 CAM-1", "选择相机 CAM-2", "选择相机 CAM-3"]);
    expect(within(cameras[0]).getByText("已示教")).toBeVisible();
    expect(within(cameras[1]).getByText("待示教走廊")).toBeVisible();
    expect(within(cameras[2]).getByText("待选帧")).toBeVisible();
    expect(cameras[1]).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByRole("img", { name: "CAM-2 · 示教帧" })).toBeVisible();
    expect(workspaceApi.image).toHaveBeenCalledWith("A", "S-CAM-2-frame");
  });

  // 防的回归：走廊画了一半就切到另一台相机，草稿按相机 id 存、切走后不再显示，操作员以为丢了
  it("走廊有未保存的修改 → 其他相机不能切换并说明先保存，当前相机可继续编辑", async () => {
    ws.corridorDrafts = { "CAM-2": { corridor: [[10, 10], [30, 10]], mmPerPx: .1, detect: null } }; ws.frameDirty = true;
    show();
    const other = screen.getByRole("button", { name: "选择相机 CAM-1" });
    expect(other).toBeDisabled();
    expect(other).toHaveAccessibleDescription(expect.stringContaining("先保存"));
    expect(screen.getByRole("button", { name: "选择相机 CAM-2" })).toBeEnabled();
    expect(within(screen.getByRole("group", { name: "走廊示教操作" })).getByText(/未保存的修改/)).toBeVisible();
    await screen.findByRole("img", { name: "CAM-2 · 示教帧" });
  });

  // 防的回归：保存走廊时把草稿写到错误的相机，或漏掉像素当量就保存成“已示教”
  it("走廊草稿合法 → 点保存走廊把草稿并入这台相机的配方项整份保存，其余相机不变", async () => {
    ws.corridorDrafts = { "CAM-2": { corridor: [[10, 10], [30, 10]], mmPerPx: .1, detect: null } }; ws.frameDirty = true;
    show();
    await userEvent.click(screen.getByRole("button", { name: "保存走廊" }));
    const saved = vi.mocked(workspaceApi.saveDoc).mock.calls[0];
    expect(saved.slice(0, 2)).toEqual(["A", 3]);
    expect(saved[2].inspect).toEqual([ws.data!.workspace.doc.inspect![0], { ...ws.data!.workspace.doc.inspect![1], corridor: [[10, 10], [30, 10]], mmPerPx: .1, detect: null }, ws.data!.workspace.doc.inspect![2]]);
  });

  it("走廊草稿缺像素当量 → 保存走廊禁用并说明原因，不写配方", async () => {
    ws.corridorDrafts = { "CAM-2": { corridor: [[10, 10], [30, 10]], mmPerPx: null, detect: null } }; ws.frameDirty = true;
    show();
    const save = screen.getByRole("button", { name: "保存走廊" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(expect.stringContaining("像素当量"));
    await screen.findByRole("img", { name: "CAM-2 · 示教帧" });
    expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
  });
});

describe("示教帧选择", () => {
  // 防的回归：选帧后提交的 (轮次, 帧组) 不是画面上选的那一帧，或把这台相机已有的走廊与当量冲掉
  it("未选示教帧的相机选定轮次与帧组 → 用此帧示教按所选轮次、帧组调用，并带上这台相机已有的走廊与当量", async () => {
    show("/recipe/teach?camera=CAM-3&round=round-1");
    await screen.findByRole("region", { name: "示教帧选择" });
    const input = await screen.findByRole("spinbutton", { name: "帧组序号" });
    fireEvent.change(input, { target: { value: "7" } });
    const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-3" });
    await vi.waitFor(() => expect(choose).toBeEnabled());
    await userEvent.click(choose);
    expect(workspaceApi.teachCamera).toHaveBeenCalledWith("A", 3, "CAM-3", "round-1", 7, { corridor: [[10, 30], [90, 30]], mmPerPx: .1, detect: null });
    expect(workspaceApi.captureFrameImage).toHaveBeenCalledWith("round-1", "CAM-3", 7);
  });

  // 防的回归：选了缺帧的帧组仍能提交，后端冻结不到图（规格 §3：缺帧只告警，但这一帧没有图可示教）
  it("所选帧组这台相机缺帧 → 不读图、用此帧示教禁用并说明缺帧", async () => {
    show("/recipe/teach?camera=CAM-2&round=round-1");
    await userEvent.click(await screen.findByRole("button", { name: "重新选帧" }));
    fireEvent.change(await screen.findByRole("spinbutton", { name: "帧组序号" }), { target: { value: "5" } });
    const choose = await screen.findByRole("button", { name: "用此帧示教 CAM-2" });
    await vi.waitFor(() => expect(choose).toBeDisabled());
    expect(choose).toHaveAccessibleDescription(expect.stringContaining("缺帧"));
    expect(workspaceApi.captureFrameImage).not.toHaveBeenCalledWith("round-1", "CAM-2", 5);
    expect(workspaceApi.teachCamera).not.toHaveBeenCalled();
  });
});

describe("示教帧信息（段配方）", () => {
  it.each([
    ["配方曝光与增益和采集时相同 → 不提示", 60, 6, false],
    ["配方曝光已改 → 提示采集时的值", 80, 6, true],
    ["配方增益已改 → 提示采集时的值", 60, 2, true],
  ])("%s", async (_, exposureUs, gainDb, hinted) => {
    ws.data!.workspace.doc.cameras["CAM-2"] = { exposureUs, gainDb };
    show();
    await userEvent.click(await screen.findByRole("button", { name: "图像采集信息" }));
    expect(await screen.findByText("仅通过“重新选帧”")).toBeVisible();
    const hint = screen.queryByText(/采集时曝光 60 µs \/ 增益 6 dB，与配方当前值/);
    if (hinted) expect(hint).toBeVisible(); else expect(hint).toBeNull();
  });
});

describe("工件总览", () => {
  // 防的回归：段配方的总览仍按拍照点摆放帧框（段配方没有拍照点，帧数可达数百），或不显示相机与段
  it("段配方 → 总览画三台相机与段条、各相机示教状态", async () => {
    show("/recipe/overview", <OverviewPage />);
    const overview = screen.getByRole("region", { name: "相机与段" });
    expect(within(within(overview).getByRole("list", { name: "计划相机" })).getAllByRole("listitem")).toHaveLength(3);
    expect(within(within(overview).getByRole("list", { name: "段序列" })).getAllByRole("listitem")).toHaveLength(3);
    expect(screen.getByText("预期总数 12 ± 3")).toBeVisible();
    expect(within(screen.getByRole("list", { name: "各相机示教帧" })).getByText("待选帧")).toBeVisible();
    expect(await screen.findByRole("img", { name: "CAM-1 · 示教帧" })).toBeVisible();
  });
});

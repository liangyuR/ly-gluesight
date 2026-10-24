import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { burstRecipe, burstWorkspaceView, captureRoundV2, snapshot, workspaceState } from "./fixtures";
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveDoc: vi.fn(), teachCamera: vi.fn(), captureList: vi.fn(), captureGet: vi.fn(), captureFrameImage: vi.fn(), captureImage: vi.fn(), extractCenterline: vi.fn() } }));

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

describe("走廊示教", () => {

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

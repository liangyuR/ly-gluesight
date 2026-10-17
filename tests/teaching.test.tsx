import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { snapshot, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveParams: vi.fn(), trial: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
function show(path = "/recipe/teach") {
  return render(<MemoryRouter initialEntries={[path]}><TeachingPage /></MemoryRouter>);
}
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  ws.data!.workspace.doc.shots.forEach(shot => { shot.views = []; });
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.saveParams).mockReset();
  vi.mocked(workspaceApi.trial).mockReset();
});

describe("单帧示教", () => {
  it("后端判定试测已过期 → 显示后端状态与提示，不能保存本组", () => {
    const view = ws.data!.progress.shots[0].views[0];
    Object.assign(view, { state: "pendingTrial", trialStale: true, hint: "现有试测已过期，请重新保存参数并试测。" });
    ws.data!.progress.shots[0].state = "pendingTrial";
    show();
    expect(screen.getByText("已过期")).toBeVisible();
    expect(screen.getByText("现有试测已过期，请重新保存参数并试测。")).toBeVisible();
    expect(within(screen.getByRole("button", { name: "选择拍照组 P1" })).getByText("试测已过期")).toBeVisible();
    for (const name of ["保存本组示教", "保存并进入下一组"]) expect(screen.getByRole("button", { name })).toBeDisabled();
  });

  it("保存参数并试测：保存参数失败 → 说明停在保存参数一步且不试测", async () => {
    ws.frameDrafts = { 0: { path: [[10, 10], [30, 10]], mmPerPx: .1 } }; ws.frameDirty = true;
    vi.mocked(workspaceApi.saveParams).mockRejectedValue("候选配置已更新");
    show();
    await userEvent.click(screen.getByRole("button", { name: "保存参数并试测" }));
    expect(workspaceApi.saveParams).toHaveBeenCalledWith("A", 7, 0, { path: [[10, 10], [30, 10]], mmPerPx: .1 });
    expect(ws.setError).toHaveBeenCalledWith(expect.stringContaining("保存参数未完成，未进行试测"));
    expect(workspaceApi.trial).not.toHaveBeenCalled();
  });

  it.each([
    ["配方曝光与增益和采集时相同 → 不提示", 60, 6, false],
    ["配方曝光已改 → 提示采集时的值", 80, 6, true],
    ["配方增益已改 → 提示采集时的值", 60, 2, true],
  ])("%s", async (_, exposureUs, gainDb, hinted) => {
    ws.data!.workspace.doc.cameras["CAM-1"] = { exposureUs, gainDb };
    show();
    await userEvent.click(screen.getByText("图像采集信息"));
    const hint = screen.queryByText(/采集时曝光 60 µs \/ 增益 6 dB，与配方当前值/);
    if (hinted) expect(hint).toBeVisible(); else expect(hint).toBeNull();
  });
});

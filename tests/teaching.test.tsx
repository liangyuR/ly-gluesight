import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { useCycle } from "../src/features/cycle/api";
import { snapshot, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn() } }));

let ws: ReturnType<typeof workspaceState>;
function show(path = "/recipe/teach") {
  return render(<MemoryRouter initialEntries={[path]}><TeachingPage /></MemoryRouter>);
}
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
});

describe("单帧示教：试测与保存", () => {
  it("中线草稿未保存时不能试测；没有中线时提示先点出中线", () => {
    ws.frameDrafts = { 0: { path: [[10, 10], [30, 10]], mmPerPx: .1 } }; ws.frameDirty = true;
    const page = show();
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    expect(screen.getByText("中线或参数已改，先保存中线再试测；现有试测不用于保存或发布。")).toBeVisible();
    for (const name of ["保存本帧示教", "保存并示教下一幅"]) expect(screen.getByRole("button", { name })).toBeDisabled();
    ws.frameDrafts = {}; ws.data!.workspace.doc.shots[0].path = []; ws.data!.workspace.frames[0].trial = null;
    page.rerender(<MemoryRouter><TeachingPage /></MemoryRouter>);
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    expect(screen.getByText("在原图上从胶嘴一侧往外依次点出胶路中线，填好像素当量后保存中线。")).toBeVisible();
    expect(within(screen.getByRole("button", { name: "选择帧 k1" })).getByText("待点中线")).toBeVisible();
  });

  it("更换原图后旧试测不能保存",()=>{
    ws.data!.workspace.frames[0].image!.id="replacement-image";show();
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"保存并示教下一幅"})).toBeDisabled();
    expect(screen.getByText("试测已过期")).toBeVisible();expect(screen.getByText("现有试测已过期，请重新试测当前帧。")).toBeVisible();
  });
});

describe("多图配方示教", () => {
  it("有效 NG 允许完成示教，执行异常仍禁止完成", () => {
    ws.data!.workspace.frames[0].trial!.verdict="NG_GAP";
    const page=show();
    expect(screen.getByText("有效 · NG_GAP")).toBeVisible();
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeEnabled();
    ws.data!.workspace.frames[0].trial!.passed=false;
    page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);
    expect(screen.getByRole("button",{name:"保存本帧示教"})).toBeDisabled();
  });
});

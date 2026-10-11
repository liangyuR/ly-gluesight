import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import OverviewPage from "../src/features/workspace/OverviewPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import type { WorkspaceView } from "../src/features/workspace/types";
import { deferred, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { image: vi.fn(), saveOverview: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.saveOverview).mockResolvedValue(ws.data!);
});
const show = () => render(<MemoryRouter><OverviewPage/></MemoryRouter>);

describe("工件总览操作", () => {
  it("保存等待锁定编辑、重复提交被拒绝，失败后可重新保存",async()=>{
    const request=deferred<WorkspaceView>();vi.mocked(workspaceApi.saveOverview).mockReturnValueOnce(request.promise);show();
    const button=screen.getByRole("button",{name:"保存总览"});fireEvent.click(button);fireEvent.click(button);expect(workspaceApi.saveOverview).toHaveBeenCalledTimes(1);
    expect(button).toBeDisabled();expect(screen.getByRole("button",{name:"自动布置"})).toBeDisabled();expect(screen.getByLabelText("导入总览图")).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("button",{name:"总览选择帧 k1"}),{key:"ArrowRight"});await act(async()=>request.reject(new Error("总览保存失败")));
    expect(ws.setError).toHaveBeenCalledWith("Error: 总览保存失败");expect(button).toBeEnabled();await userEvent.click(button);
    expect(workspaceApi.saveOverview).toHaveBeenLastCalledWith("A",7,expect.objectContaining({positions:[[.25,.5],[.75,.5]]}));
  });
  it("旧保存失败不影响新配方保存的等待和提示",async()=>{
    const old=deferred<WorkspaceView>(),next=deferred<WorkspaceView>();vi.mocked(workspaceApi.saveOverview).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const page=show();fireEvent.click(screen.getByRole("button",{name:"保存总览"}));ws=workspaceState(workspaceView("B"));page.rerender(<MemoryRouter><OverviewPage/></MemoryRouter>);
    fireEvent.click(screen.getByRole("button",{name:"保存总览"}));await act(async()=>old.reject(new Error("旧保存错误")));
    expect(ws.setError).not.toHaveBeenCalled();expect(screen.getByRole("button",{name:"保存总览"})).toBeDisabled();await act(async()=>next.resolve(ws.data!));
    expect(screen.getByRole("button",{name:"保存总览"})).toBeEnabled();expect(workspaceApi.saveOverview).toHaveBeenCalledTimes(2);
  });

});

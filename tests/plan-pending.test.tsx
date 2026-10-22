import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CapturePage from "../src/features/workspace/CapturePage";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { useWorkspace } from "../src/features/workspace/context";
import { burstWorkspaceView, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {} }));

let ws: ReturnType<typeof workspaceState>;
beforeEach(() => { vi.mocked(useWorkspace).mockImplementation(() => ws); });

// 旧 / 新分流以 RecipeDoc.segments 是否为空为准。后端 WorkspaceView.plan 到 D3 才输出，
// 段配方（segments 非空）而 plan 为 null 时若按 plan 分流，会落到旧拍照点页面：那里的操作提交后被后端 validate_burst 拒绝。
describe("段配方但后端还没输出 plan（D3 之前）", () => {
  it.each([
    ["走廊示教", TeachingPage],
    ["整圈采集", CapturePage],
  ])("%s页 → 显示待后端 D3 实现的占位，不落到旧拍照点页面", (_, Page) => {
    const view = burstWorkspaceView();
    view.plan = null;
    ws = workspaceState(view);
    render(<MemoryRouter><Page /></MemoryRouter>);
    expect(screen.getByText(/待后端 D3 实现/)).toBeVisible();
  });
});

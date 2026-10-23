import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import GeometryPage from "../src/features/workspace/GeometryPage";
import { useWorkspace } from "../src/features/workspace/context";
import { burstRecipe, burstWorkspaceView, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { doc: vi.fn(() => new Promise(() => {})) } }));

let ws: ReturnType<typeof workspaceState>;
beforeEach(() => { ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws); });

it("段配方 → 配方摘要显示段数、相机数与 ΣN_exp 待标定，不显示拍照点摘要", () => {
  ws = workspaceState(burstWorkspaceView(burstRecipe({ partCalib: null })));
  render(<MemoryRouter><GeometryPage/></MemoryRouter>);
  const summary = screen.getByRole("region", { name: "配方摘要" });
  expect(summary).toHaveTextContent(/段3 · 直线 2 · 弯道 1/);
  expect(summary).toHaveTextContent(/预期总帧数待标定/);
  expect(summary).not.toHaveTextContent("尚无拍照点");
  expect(screen.getByRole("region", { name: "弯道段默认检测项" })).toBeInTheDocument();
});

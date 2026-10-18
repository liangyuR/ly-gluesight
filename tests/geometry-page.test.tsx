import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, expect, it, vi } from "vitest";
import GeometryPage from "../src/features/workspace/GeometryPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/camera", () => ({ FeasibilityCalc: () => null }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { doc: vi.fn(() => new Promise(() => {})) } }));

let ws: ReturnType<typeof workspaceState>;
beforeEach(() => { ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws); });

it("新配方尚无拍照点 → 摘要提示尚无拍照点，不显示示教 100%", () => {
  ws.data!.workspace.doc.shots = [];
  ws.data!.layout.shots = []; ws.data!.layout.segments = []; ws.data!.layout.points = { x: [], y: [], seg: [], k: [] };
  ws.data!.coverage = 100;
  render(<MemoryRouter><GeometryPage/></MemoryRouter>);
  expect(screen.getByRole("region", { name: "配方摘要" })).toHaveTextContent("尚无拍照点");
  expect(screen.queryByText(/100%/)).toBeNull();
});

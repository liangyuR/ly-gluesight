import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import { PlanSteps } from "../src/features/workspace/PlanSteps";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));

it("从走廊示教切换步骤 → 当前链接随路由更新，示教条数恢复为业务状态", async () => {
  vi.mocked(useWorkspace).mockReturnValue(workspaceState());
  render(<MemoryRouter initialEntries={["/recipe/teach?camera=CAM-2&segment=1"]}><PlanSteps /></MemoryRouter>);
  const steps = within(screen.getByRole("navigation", { name: "配方配置步骤" }));
  const teaching = steps.getByRole("link", { name: /走廊示教\s*3 \/ 9 条/ });
  expect(teaching).toHaveAttribute("aria-current", "step");
  const segments = steps.getByRole("link", { name: /段列表\s*完成/ });
  expect(segments).toHaveAttribute("href", "/recipe/segments");
  await userEvent.setup().click(segments);
  expect(segments).toHaveAttribute("aria-current", "step");
  expect(steps.getByRole("link", { name: /走廊示教\s*进行中/ })).not.toHaveAttribute("aria-current");
});

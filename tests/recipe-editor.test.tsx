import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { workspaceView } from "./fixtures";

it("候选与生产配方仅搜索半宽不同 → 只有搜索半宽标出与生产不同", () => {
  const production = workspaceView().workspace.doc;
  const candidate = { ...structuredClone(production), detect: { ...production.detect, searchMm: production.detect.searchMm + 2 } };
  render(<RecipeEditor initial={candidate} production={production} />);
  const field = (name: string) => screen.getByRole("spinbutton", { name }).closest("label")!;
  expect(within(field("搜索半宽（mm）")).getByTitle("与生产配方不同")).toBeInTheDocument();
  expect(screen.getAllByTitle("与生产配方不同")).toHaveLength(1);
});

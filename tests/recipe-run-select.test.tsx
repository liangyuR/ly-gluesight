import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import RecipeRunSelect from "../src/features/cycle/components/RecipeRunSelect";
import type { RecipeOption, RecipeSummary } from "../src/features/cycle/types";
import { burstRecipe, burstSummary, recipeOption, summary, workspaceView } from "./fixtures";

// 输入都取后端真实输出的形状：开工判定的 RecipeOption 只带 shotCount（段配方恒为 0），
// 段数与 ΣN_exp 在 cycle_recipes 的 RecipeSummary 里（旧配方 segmentCount = 0）。
const legacySummary = (): RecipeSummary => ({ ...summary(workspaceView("S")), segmentCount: 0, expectedTotal: null, toleranceTotal: 0, calibrated: false });
const labelsOf = (options: RecipeOption[], summaries: RecipeSummary[]) => {
  render(<MemoryRouter><RecipeRunSelect options={options} summaries={summaries} value="S" onChange={vi.fn()} /></MemoryRouter>);
  return [...screen.getByLabelText<HTMLSelectElement>("离线配方", { selector: "select" }).options]
    .filter(option => option.value === "S").map(option => option.textContent);
};

it.each<[string, RecipeOption, RecipeSummary[], string]>([
  ["已标定的段配方", recipeOption("S", true, { shotCount: 0 }), [burstSummary(burstRecipe())], "S · 代码 1 · 3 段 · ΣN_exp 12 ± 3"],
  ["待标定的段配方", recipeOption("S", true, { shotCount: 0 }), [burstSummary(burstRecipe({ partCalib: null }))], "S · 代码 1 · 3 段 · ΣN_exp 待标定"],
  ["旧拍照点配方", recipeOption("S", true, { shotCount: 4 }), [legacySummary()], "S · 代码 1 · N=4"],
  ["配方摘要还没取到的段配方", recipeOption("S", true, { shotCount: 0 }), [], "S · 代码 1"],
])("%s → 离线配方选项显示该配方的规模，段配方不写 N=0", (_, option, summaries, label) => {
  expect(labelsOf([option], summaries)).toEqual([label]);
});

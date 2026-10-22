import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import RecipeRunSelect from "../src/features/cycle/components/RecipeRunSelect";
import { recipeOption } from "./fixtures";

it.each([
  ["已标定的段配方", { shotCount: 0, segmentCount: 3, expectedTotal: 12, toleranceTotal: 3 }, "S · 代码 1 · 3 段 · ΣN_exp 12 ± 3"],
  ["待标定的段配方", { shotCount: 0, segmentCount: 3, expectedTotal: null, toleranceTotal: 3 }, "S · 代码 1 · 3 段 · ΣN_exp 待标定"],
  ["旧拍照点配方", { shotCount: 4 }, "S · 代码 1 · N=4"],
])("%s → 离线配方选项显示该配方的规模，段配方不显示 N=0", (_, over, label) => {
  render(<MemoryRouter><RecipeRunSelect options={[recipeOption("S", true, over)]} value="S" onChange={vi.fn()} /></MemoryRouter>);
  const select = screen.getByLabelText<HTMLSelectElement>("离线配方", { selector: "select" });
  const labels = [...select.options].map(option => option.textContent ?? "");
  expect(labels).toContain(label);
  expect(labels.some(text => text.includes("N=0"))).toBe(false);
});

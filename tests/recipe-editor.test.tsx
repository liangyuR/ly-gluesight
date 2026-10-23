import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { burstRecipe, workspaceView } from "./fixtures";

it("候选与生产配方仅搜索半宽不同 → 只有搜索半宽标出与生产不同", () => {
  const production = workspaceView().workspace.doc;
  const candidate = { ...structuredClone(production), detect: { ...production.detect, searchMm: production.detect.searchMm + 2 } };
  render(<RecipeEditor initial={candidate} production={production} />);
  const field = (name: string) => screen.getByRole("spinbutton", { name }).closest<HTMLElement>(".rcp-field")!;
  expect(within(field("搜索半宽（mm）")).getByTitle("与生产配方不同")).toBeInTheDocument();
  expect(screen.getAllByTitle("与生产配方不同")).toHaveLength(1);
});

// 段配方的编辑器：触发方式固定飞拍、不再有位置与胶宽判定，默认参数按段类型各设一套（规格 §4：胶偏只在直线段）
const kindSwitches = {
  straight: ["检胶宽", "检胶宽均匀性", "检断胶", "检溢胶", "检胶偏"],
  corner: ["检胶宽", "检断胶", "检溢胶"],
};

it.each([["直线", "straight"], ["弯道", "corner"]] as const)("段配方的%s段默认检测项 → 开关恰为该类型应有的检测项", (label, kind) => {
  render(<RecipeEditor initial={burstRecipe()} />);
  const region = within(screen.getByRole("region", { name: `${label}段默认检测项` }));
  const found = region.getAllByRole("switch");
  expect(found).toHaveLength(kindSwitches[kind].length);
  for (const name of kindSwitches[kind]) expect(found).toContain(region.getByRole("switch", { name }));
});

it("段配方 → 下拉只有极性，开关只有两类段的默认检测项（触发方式与位置、胶宽判定不进编辑器）", () => {
  render(<RecipeEditor initial={burstRecipe()} />);
  const dropdowns = screen.getAllByRole("combobox");
  expect(dropdowns).toEqual([screen.getByRole("combobox", { name: "极性" })]);
  expect(screen.getAllByRole("switch")).toHaveLength(kindSwitches.straight.length + kindSwitches.corner.length);
});

it("段配方改弯道段默认帧数容差 → 草稿里弯道容差为新值，直线段容差不变", () => {
  const changed = vi.fn();
  render(<RecipeEditor initial={burstRecipe()} onDraftChange={changed} />);
  const corner = within(screen.getByRole("region", { name: "弯道段默认检测项" }));

  fireEvent.change(corner.getByRole("spinbutton", { name: "帧数容差" }), { target: { value: "2" } });

  const draft = changed.mock.lastCall![0];
  expect([draft.segmentDefaults.straight.tolerance, draft.segmentDefaults.corner.tolerance]).toEqual([1, 2]);
});

it("段配方改标定轮数 → 草稿带新轮数，触发方式保持原值不被改写", () => {
  // 防的回归：编辑器不展示触发方式后，改参数时顺手把它写成 fly（原值由后端裁决）
  const changed = vi.fn();
  render(<RecipeEditor initial={burstRecipe({ triggerMode: "stop" })} onDraftChange={changed} />);

  fireEvent.change(screen.getByRole("spinbutton", { name: "标定轮数" }), { target: { value: "20" } });

  const draft = changed.mock.lastCall![0];
  expect([draft.calibrationRounds, draft.triggerMode]).toEqual([20, "stop"]);
});

import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { workspaceView } from "./fixtures";

it("候选与生产配方仅搜索半宽不同 → 只有搜索半宽标出与生产不同", () => {
  const production = workspaceView().workspace.doc;
  const candidate = { ...structuredClone(production), detect: { ...production.detect, searchMm: production.detect.searchMm + 2 } };
  render(<RecipeEditor initial={candidate} production={production} />);
  const field = (name: string) => screen.getByRole("spinbutton", { name }).closest<HTMLElement>(".rcp-field")!;
  expect(within(field("搜索半宽（mm）")).getByTitle("与生产配方不同")).toBeInTheDocument();
  expect(screen.getAllByTitle("与生产配方不同")).toHaveLength(1);
});

it("配方默认位置名义值不为 0 → 出现量具确认件编号并提示必填，填写后进草稿", async () => {
  // 防的回归（定稿 §3）：规则页改了名义值却没有地方填 gaugeRef，候选保存一直被后端拒绝
  const doc = workspaceView().workspace.doc;
  const changed = vi.fn();
  render(<RecipeEditor initial={doc} onDraftChange={changed} />);
  expect(screen.getByRole("alert")).toHaveTextContent("量具确认件编号");
  fireEvent.change(screen.getByRole("textbox", { name: "量具确认件编号" }), { target: { value: "GA-0315" } });
  expect(changed).toHaveBeenLastCalledWith(expect.objectContaining({ limits: expect.objectContaining({ gaugeRef: "GA-0315" }) }));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

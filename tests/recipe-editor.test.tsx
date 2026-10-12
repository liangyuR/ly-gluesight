import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { recipeApi } from "../src/features/cycle/api";
import type { RecipeDoc } from "../src/features/cycle/types";
import { deferred, workspaceView } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ recipeApi: { preview: vi.fn() } }));
beforeEach(() => {
  vi.mocked(recipeApi.preview).mockResolvedValue(workspaceView().layout);
});
const readySave = async (name = "保存候选配置") => { await waitFor(() => expect(screen.getByRole("button", { name })).toBeEnabled()); return screen.getByRole("button", { name }); };

describe("配方编辑保护", () => {
  it("保存候选期间锁定表单与重复提交，失败保留编辑值后可重试", async () => {
    const request = deferred<boolean>();
    const saveCandidate = vi.fn<(doc: RecipeDoc) => Promise<boolean>>().mockReturnValueOnce(request.promise).mockResolvedValue(true);
    render(<RecipeEditor initial={workspaceView().workspace.doc} originalId="A" saveCandidate={saveCandidate} />);
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "待保存名称" } });
    await userEvent.click(await readySave()); expect(screen.getByRole("button", { name: "保存中…" })).toBeDisabled(); expect(screen.getByRole("textbox", { name: "名称" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "保存中…" })); expect(saveCandidate).toHaveBeenCalledTimes(1);
    await act(async () => request.reject(new Error("写盘失败"))); expect(screen.getByText("Error: 写盘失败")).toBeVisible();
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("待保存名称"); await userEvent.click(await readySave());
    expect(saveCandidate).toHaveBeenLastCalledWith(expect.objectContaining({ name: "待保存名称" })); expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
  });
});

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { recipeApi } from "../src/features/cycle/api";
import type { Recipe, RecipeDoc } from "../src/features/cycle/types";
import { deferred, summary, workspaceView } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ recipeApi: { preview: vi.fn(), save: vi.fn() } }));
beforeEach(() => {
  vi.mocked(recipeApi.preview).mockResolvedValue(workspaceView().layout);
  vi.mocked(recipeApi.save).mockResolvedValue(summary());
});
function show(saveCandidate?: (doc: RecipeDoc) => Promise<boolean>, initial = workspaceView().workspace.doc, cameras = [{ id: "CAM-1", name: "相机 1" }]) {
  const onDraftChange = vi.fn(), onSaved = vi.fn();
  const page = render(<RecipeEditor initial={initial} originalId="A" cameras={cameras} onSaved={onSaved} onDraftChange={onDraftChange} saveCandidate={saveCandidate} />);
  return { ...page, onDraftChange, onSaved };
}
const readySave = async (name = "保存") => { await waitFor(() => expect(screen.getByRole("button", { name })).toBeEnabled()); return screen.getByRole("button", { name }); };
const last = (onDraftChange: ReturnType<typeof vi.fn>) => onDraftChange.mock.lastCall![0] as RecipeDoc;
const number = (name: string, value: string) => fireEvent.change(screen.getByRole("spinbutton", { name }), { target: { value } });

describe("配方的拍照点表", () => {
  it("视角只在 1–3 中选择，改变后清空该拍照点中线与像素当量，其他拍照点保持",async()=>{
    const {onDraftChange}=show();const input=screen.getByRole("combobox",{name:"拍照点 1 · 视角"});
    expect(within(input).getAllByRole("option").map(option=>(option as HTMLOptionElement).value)).toEqual(["1","2","3"]);
    await userEvent.selectOptions(input,"3");const next=last(onDraftChange);
    expect(next.schemaVersion).toBe(5);expect(next.shots[0]).toEqual({...workspaceView().workspace.doc.shots[0],view:3,path:[],mmPerPx:undefined});
    expect(Object.hasOwn(next.shots[0],"mmPerPx")).toBe(false);expect(next.shots[1]).toEqual(workspaceView().workspace.doc.shots[1]);
    expect(screen.getByRole("textbox",{name:"拍照点 1 · 编号"}).closest("tr")).toHaveTextContent("未示教");
  });

  it("保存候选锁定编号，编辑不会直接保存生产配方", async () => {
    const saveCandidate = vi.fn().mockResolvedValue(true), { onDraftChange, onSaved } = show(saveCandidate);
    expect(screen.getByRole("textbox", { name: "配方编号" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "新名称" } });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ name: "新名称" }));
    expect(saveCandidate).not.toHaveBeenCalled(); expect(recipeApi.save).not.toHaveBeenCalled();
    await userEvent.click(await readySave("保存候选配置"));
    expect(saveCandidate).toHaveBeenCalledWith(expect.objectContaining({ id: "A", name: "新名称" }));
    expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
    expect(recipeApi.save).not.toHaveBeenCalled(); expect(onSaved).not.toHaveBeenCalled();
  });

});

describe("配方保存与预览", () => {
  it("旧预览晚到不会覆盖新预览或解除当前错误", async () => {
    vi.useFakeTimers();
    try {
      const old = deferred<Recipe>();
      vi.mocked(recipeApi.preview).mockReturnValueOnce(old.promise).mockRejectedValueOnce(new Error("当前配方无效"));
      show(); await act(() => vi.advanceTimersByTimeAsync(350));
      number("站距（mm）", "0");
      await act(() => vi.advanceTimersByTimeAsync(350));
      expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
      await act(async () => old.resolve(workspaceView().layout));
      expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
      expect(screen.getByText("Error: 当前配方无效")).toBeVisible();
    } finally { vi.useRealTimers(); }
  });

  it("保存期间锁定表单与重复提交，失败保留编辑值后可重试", async () => {
    const request = deferred<ReturnType<typeof summary>>(); vi.mocked(recipeApi.save).mockReturnValueOnce(request.promise);
    const { onSaved } = show(); fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "待保存名称" } });
    await userEvent.click(await readySave()); expect(screen.getByRole("button", { name: "保存中…" })).toBeDisabled(); expect(screen.getByRole("textbox", { name: "名称" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "保存中…" })); expect(recipeApi.save).toHaveBeenCalledTimes(1);
    await act(async () => request.reject(new Error("写盘失败"))); expect(screen.getByText("Error: 写盘失败")).toBeVisible(); expect(onSaved).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("待保存名称"); await userEvent.click(await readySave()); expect(onSaved).toHaveBeenCalledWith("A");
  });

});

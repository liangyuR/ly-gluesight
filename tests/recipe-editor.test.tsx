import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { recipeApi } from "../src/features/cycle/api";
import type { RecipeDoc } from "../src/features/cycle/types";
import { deferred, summary, workspaceView } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ recipeApi: { preview: vi.fn(), save: vi.fn() } }));
beforeEach(() => {
  vi.mocked(recipeApi.preview).mockResolvedValue(workspaceView().layout);
  vi.mocked(recipeApi.save).mockResolvedValue(summary());
});
function show() {
  const onDraftChange = vi.fn(), onSaved = vi.fn();
  const page = render(<RecipeEditor initial={workspaceView().workspace.doc} originalId="A" cameras={[{ id: "CAM-1", name: "相机 1" }]} onSaved={onSaved} onDraftChange={onDraftChange} />);
  return { ...page, onDraftChange, onSaved };
}
const readySave = async (name = "保存") => { await waitFor(() => expect(screen.getByRole("button", { name })).toBeEnabled()); return screen.getByRole("button", { name }); };
const last = (onDraftChange: ReturnType<typeof vi.fn>) => onDraftChange.mock.lastCall![0] as RecipeDoc;

describe("配方编辑保护", () => {
  it("视角只在 1–3 中选择，改变后清空该拍照点中线与像素当量，其他拍照点保持",async()=>{
    const {onDraftChange}=show();const input=screen.getByRole("combobox",{name:"拍照点 1 · 视角"});
    expect(within(input).getAllByRole("option").map(option=>(option as HTMLOptionElement).value)).toEqual(["1","2","3"]);
    await userEvent.selectOptions(input,"3");const next=last(onDraftChange);
    expect(next.schemaVersion).toBe(5);expect(next.shots[0]).toEqual({...workspaceView().workspace.doc.shots[0],view:3,path:[],mmPerPx:undefined});
    expect(Object.hasOwn(next.shots[0],"mmPerPx")).toBe(false);expect(next.shots[1]).toEqual(workspaceView().workspace.doc.shots[1]);
    expect(screen.getByRole("textbox",{name:"拍照点 1 · 编号"}).closest("tr")).toHaveTextContent("未示教");
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

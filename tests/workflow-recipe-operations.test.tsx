import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defaultRecipe, initialPositions, initialState, sceneState } from "../src/features/workflow/model";
import { click, finishTask, navigate, number, panel, select, showWorkflow, stored } from "./workflow-preview-fixtures";

beforeEach(() => { sessionStorage.clear(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const row = (name: string) => screen.getByText(name, { selector: "td strong" }).closest("tr")!;
const openRecipe = (name: string) => fireEvent.click(within(row(name)).getByRole("button", { name: "编辑候选" }));
const nameRecipe = (name: string) => fireEvent.change(within(screen.getByRole("dialog")).getByRole("textbox", { name: "配方名称" }), { target: { value: name } });

describe("预览配方维护与几何操作", () => {
  it("新建/复制名称校验与取消，创建后保留原工作台，切换与重新打开保持各自参数", () => {
    showWorkflow("geometry"); number("工件宽度", 600); navigate("配方库");
    click("新建配方"); expect(screen.getByRole("button", { name: "建立配方" })).toBeDisabled();
    nameRecipe(" 工件 B · 底板 "); expect(screen.getByText("配方名称已存在")).toBeVisible();
    expect(screen.getByRole("button", { name: "建立配方" })).toBeDisabled(); click("取消");
    expect(stored().recipeLibrary).toHaveLength(2);
    click("复制当前配方"); nameRecipe("工件 A 副本"); click("建立配方");
    expect(stored().recipe).toMatchObject({ name: "工件 A 副本", width: 600, candidate: 1, production: 0 });
    expect(stored().frames.every(f => f.imageId === null && f.trial === null && !f.saved && f.backup === null)).toBe(true);
    expect(stored().recipeLibrary).toHaveLength(3);
    navigate("配方库"); expect(within(panel("当前生产配置")).getByText("未发布")).toBeVisible();
    openRecipe("工件 A · 壳体"); expect(stored().recipe).toMatchObject({ width: 600, production: 13 });
    navigate("配方库"); click("新建配方"); nameRecipe("全新壳体"); click("建立配方");
    expect(stored().recipe).toMatchObject({ name: "全新壳体", width: defaultRecipe.width, candidate: 1, production: 0 });
    navigate("配方库"); openRecipe("工件 A 副本"); expect(stored().recipe).toMatchObject({ width: 600 });
    navigate("配方库"); fireEvent.change(screen.getByRole("textbox", { name: "搜索配方" }), { target: { value: "不存在" } });
    expect(screen.getByText("没有匹配的配方")).toBeVisible();
    fireEvent.change(screen.getByRole("textbox", { name: "搜索配方" }), { target: { value: "副本" } });
    expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(2);
    expect(screen.getByText("工件 A 副本", { selector: "td strong" })).toBeVisible();
  });

  it("删除先确认，取消保留，删除当前配方后打开剩余配方，至少保留一个", () => {
    showWorkflow("recipes"); click("删除配方 工件 B · 底板"); click("取消"); expect(row("工件 B · 底板")).toBeVisible();
    click("删除配方 工件 A · 壳体"); click("确认删除"); expect(screen.queryByText("工件 A · 壳体", { selector: "td strong" })).not.toBeInTheDocument();
    expect(stored().recipe.name).toBe("工件 B · 底板"); expect(stored().recipe.production).toBe(8);
    expect(screen.getByRole("button", { name: "删除配方 工件 B · 底板" })).toBeDisabled();
    expect(stored().recipeLibrary).toHaveLength(1);
  });

  it("当前件运行时禁止新建、复制、切换与删除，候选仍可查看", () => {
    showWorkflow("recipes", sceneState("live-running"));
    expect(screen.getByRole("button", { name: "新建配方" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "复制当前配方" })).toBeDisabled();
    expect(within(row("工件 B · 底板")).getByRole("button", { name: "编辑候选" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除配方 工件 A · 壳体" })).toBeDisabled();
    click("继续配置"); expect(screen.getByRole("heading", { level: 1, name: "胶路与拍照规划" })).toBeVisible();
  });

  it.each(["工件宽度", "工件高度", "圆角半径", "物理视野宽", "物理视野高"])("修改 %s 清空六帧图像和验证，保留生产快照", label => {
    showWorkflow("geometry", sceneState("validation-pass")); const previous = stored();
    const value = Number((screen.getByRole("spinbutton", { name: label }) as HTMLInputElement).value);
    number(label, value + 1);
    expect(stored().frames.every(f => f.imageId === null && !f.saved && !f.trial)).toBe(true);
    expect(stored().validation.status).toBe("idle"); expect(stored().productionConfig).toEqual(previous.productionConfig);
  });

  it("速度、基准、容差、断胶规则编辑更新候选且保留图像；物理覆盖提示和下一步正确", () => {
    showWorkflow("geometry", sceneState("validation-pass")); const images = stored().frames.map(f => f.imageId);
    number("运动速度", 350); number("距内边基准 d", 3.2); number("距离容差", .8); number("允许断胶长度", .6);
    expect(stored().recipe).toMatchObject({ speed: 350, target: 3.2, tolerance: .8, maxGap: .6 });
    expect(stored().frames.map(f => f.imageId)).toEqual(images); expect(stored().validation.status).toBe("idle");
    number("物理视野宽", 170); expect(screen.getByText(/胶路搜索窗口覆盖不足/)).toBeVisible();
    click("下一步 · 单帧示教"); expect(screen.getByRole("heading", { level: 1, name: "单帧示教" })).toBeVisible();
  });

  it("当前生产参数运行中显示锁定；创建新候选后可编辑", () => {
    const s = sceneState("live-running"); s.recipe.candidate = s.recipe.production;
    showWorkflow("geometry", s);
    expect(screen.getByRole("spinbutton", { name: "工件宽度" })).toBeDisabled();
    expect(screen.getByText("当前生产参数暂时锁定")).toBeVisible();
    navigate("配方库"); click("继续配置"); expect(screen.getByRole("spinbutton", { name: "工件宽度" })).toBeEnabled();
    expect(stored().recipe.candidate).toBe(14);
  });
});

describe("预览六帧示教与总览", () => {
  it("六帧逐帧取样、试匹配、保存下一帧，最后进入总览，选帧同步", async () => {
    const state = initialState(); state.selectedFrame = 1; state.frames = state.frames.map(f => ({ ...f, imageId: null, trial: null, saved: false }));
    showWorkflow("teach", state);
    for (let id = 1; id <= 6; id++) {
      expect(screen.getByRole("button", { name: "选择帧 k" + id })).toHaveAttribute("aria-pressed", "true");
      expect(screen.getByRole("button", { name: "试匹配当前帧" })).toBeDisabled();
      click("取新样本"); click("试匹配当前帧");
      expect(screen.getByRole("button", { name: "选择帧 k1" })).toBeDisabled();
      expect(screen.getByRole("spinbutton", { name: "最低灰度对比" })).toBeDisabled();
      await finishTask(); click(id < 6 ? "保存并示教下一帧" : "保存并进入总览");
    }
    expect(stored().frames.every(f => f.saved && f.trial?.pass)).toBe(true);
    expect(screen.getByRole("heading", { level: 1, name: "工件总览" })).toBeVisible();
    click("总览帧 k2"); expect(screen.getByRole("img", { name: "帧 k2 的本帧样本" })).toBeVisible();
    click("进入本帧示教"); expect(screen.getByRole("button", { name: "选择帧 k2" })).toHaveAttribute("aria-pressed", "true");
  });

  it("低对比失败恢复，搜索与胶宽非法不能保存，重置参数和重新试匹配", async () => {
    showWorkflow("teach?scene=teach-empty"); select("取样情景", "低对比样本"); click("取新样本"); click("试匹配当前帧"); await finishTask();
    expect(screen.getByText("定位失败，当前样本不能保存")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    number("最低灰度对比", 10); number("搜索窗口余量", 1); click("试匹配当前帧"); await finishTask();
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    number("搜索窗口余量", 4); number("胶宽下限 w", 6); number("胶宽上限 w", 5);
    expect(screen.getByText("胶宽上限必须大于下限")).toBeVisible(); click("试匹配当前帧"); await finishTask();
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    number("胶宽下限 w", 2.5); number("胶宽上限 w", 5.5); click("试匹配当前帧"); await finishTask(); click("保存本帧示教");
    expect(stored().frames[2].saved).toBe(true); click("重置本帧参数");
    expect(stored().frames[2]).toMatchObject({ params: { search: 4, contrast: 32, minWidth: 2.5, maxWidth: 5 }, trial: null, saved: false });
    select("取样情景", "完整胶路"); click("取新样本"); click("试匹配当前帧"); await finishTask();
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeEnabled();
  });

  it("历史样本重复载入与修改后恢复原图和参数，原记录仍保留", () => {
    showWorkflow("record"); const original = stored().frames[2];
    click("用作本帧示教样本"); number("最低灰度对比", 44);
    expect(stored().frames[2].backup?.imageId).toBe(original.imageId);
    navigate("历史复测"); click("用作本帧示教样本"); click("恢复原示教图");
    expect(stored().frames[2]).toMatchObject({ imageId: original.imageId, params: original.params, source: "camera", backup: null, saved: false, trial: null });
    expect(stored().record).toBe("TJ-000184");
  });

  it("离开待试匹配页面取消过期结果", async () => {
    showWorkflow("teach"); click("试匹配当前帧"); navigate("胶路与拍照规划"); await finishTask();
    expect(stored().frames[2].trial).toBeNull();
  });

  it("显示框选取、键盘和拖动布置、边界限制、自动恢复与保存，物理参数和生产布局不变", () => {
    showWorkflow("overview"); const original = stored(); click("总览帧 k4");
    fireEvent.keyDown(screen.getByRole("button", { name: "总览帧 k2" }), { key: "Enter" });
    expect(screen.getByRole("img", { name: "帧 k2 的本帧样本" })).toBeVisible();
    fireEvent.click(screen.getByRole("checkbox", { name: "调整显示框" }));
    const selected = screen.getByRole("button", { name: "总览帧 k2" });
    fireEvent.keyDown(selected, { key: "ArrowRight" }); fireEvent.keyDown(selected, { key: "ArrowDown" });
    expect(stored().overview.positions[1]).toEqual({ x: .51, y: .22 });
    for (let i = 0; i < 100; i++) fireEvent.keyDown(selected, { key: "ArrowLeft" });
    expect(stored().overview.positions[1].x).toBe(.03);
    const map = selected.closest("svg")!;
    vi.spyOn(map, "getBoundingClientRect").mockReturnValue({ width: 800, height: 420, top: 0, left: 0, right: 800, bottom: 420, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(selected, "setPointerCapture", { value: vi.fn(), configurable: true });
    vi.stubGlobal("PointerEvent", MouseEvent);
    fireEvent.pointerDown(selected, { clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(selected, { clientX: 152, clientY: 123, pointerId: 1 }); fireEvent.pointerUp(selected);
    expect(stored().overview.positions[1].x).toBeCloseTo(.13); expect(stored().overview.positions[1].y).toBeCloseTo(.32);
    expect(stored().recipe).toEqual(original.recipe); expect(stored().productionConfig.overview).toEqual(original.productionConfig.overview);
    click("保存布局"); expect(stored().overview.saved).toBe(true);
    click("自动布置"); expect(stored().overview.positions).toEqual(initialPositions); expect(stored().overview.saved).toBe(false);
    click("保存布局并进入验证"); expect(stored().overview.saved).toBe(true); expect(screen.getByRole("heading", { level: 1, name: "验证与发布" })).toBeVisible();
  });
});

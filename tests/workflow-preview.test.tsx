import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import WorkflowPreviewPage from "../src/pages/WorkflowPreviewPage";
import { journeys, screens } from "../src/features/workflow/catalog";
import { initialState } from "../src/features/workflow/model";

const storageKey = "tujiao-workflow-preview-v3";
beforeEach(() => {
  sessionStorage.clear();
  // jsdom 无滚动布局，保留真实页面与导航，只替代浏览器方法。
  vi.spyOn(HTMLElement.prototype, "scrollTo").mockImplementation(() => {});
});
function show(path: string) {
  return render(<MemoryRouter initialEntries={[path]}><Routes><Route path="/workflow/:view?" element={<WorkflowPreviewPage />} /></Routes></MemoryRouter>);
}

describe("流程预览路由与交互", () => {
  it.each(screens)("$label 页面可直接打开，导航选中当前页面", ({ id, label }) => {
    show(`/workflow/${id}`);
    expect(screen.getByRole("heading", { level: 1, name: label })).toBeVisible();
    expect(within(screen.getByRole("navigation", { name: "操作流程导航" })).getByRole("link", { name: label })).toHaveAttribute("aria-current", "page");
    expect(document.querySelector(".wf-shell")).toHaveAttribute("data-view", id);
  });

  it("存储损坏和未知路由回到可用的初始引导", () => {
    sessionStorage.setItem(storageKey, "invalid-json"); show("/workflow/unknown");
    expect(screen.getByRole("heading", { level: 1, name: "操作流程" })).toBeVisible();
    expect(JSON.parse(sessionStorage.getItem(storageKey)!)).toMatchObject({ schema: 1, scene: "default" });
  });

  it("情景筛选切换到具体异常页，重置清理异常情景", async () => {
    show("/workflow/guide"); await userEvent.click(screen.getByRole("button", { name: "全部情景" }));
    await userEvent.type(screen.getByRole("textbox", { name: "查找情景" }), "未识别标定角点");
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getAllByRole("button")).toHaveLength(3);
    expect(within(dialog).getByRole("button", { name: "清空筛选" })).toBeEnabled();
    await userEvent.click(within(dialog).getByRole("button", { name: /未识别标定角点/ }));
    expect(screen.getByRole("heading", { level: 1, name: "飞拍工位标定" })).toBeVisible();
    expect(document.querySelector(".wf-shell")).toHaveAttribute("data-scene", "calibration-fail");
    await userEvent.click(screen.getByRole("button", { name: "重置预览数据" }));
    expect(document.querySelector(".wf-shell")).toHaveAttribute("data-scene", "default");
    expect(screen.getByRole("status")).toHaveTextContent("预览数据已恢复到初始状态。");
  });

  it("已通过的试匹配在编辑参数后失效，重新试匹配后才可保存", async () => {
    show("/workflow/teach?scene=teach-pass");
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeEnabled();
    fireEvent.change(screen.getByRole("spinbutton", { name: "最低灰度对比" }), { target: { value: "40" } });
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "试匹配当前帧" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "保存本帧示教" }));
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    const stored = JSON.parse(sessionStorage.getItem(storageKey)!);
    expect(stored.frames[2].saved).toBe(true); expect(stored.validation.status).toBe("idle");
  });

  it.each(journeys.map((journey, index) => ({ ...journey, index })))("开始 $title 路径载入对应示例，步骤可导航", ({ title, views, index }) => {
    show("/workflow/guide");
    const task = screen.getByRole("heading", { level: 3, name: title }).closest("article")!;
    fireEvent.click(within(task).getByRole("button", { name: "开始此路径" }));
    expect(screen.getByRole("heading", { level: 1, name: screens.find(s => s.id === views[0])!.label })).toBeVisible();
    const state = JSON.parse(sessionStorage.getItem(storageKey)!);
    if (index === 0) {
      expect(state.device.connected).toBe(false); expect(state.plc.ready).toBe(false); expect(state.calibration.captured).toBe(false);
      expect(state.frames.every((f: { imageId: number | null }) => f.imageId === null)).toBe(true);
    }
    if (index === 4) expect(state.recipe.mode).toBe("follow");
    fireEvent.click(screen.getByRole("link", { name: "操作流程" }));
    const steps = screen.getByRole("heading", { level: 3, name: title }).closest("article")!;
    fireEvent.click(within(steps).getByRole("button", { name: screens.find(s => s.id === views[1])!.label }));
    expect(screen.getByRole("heading", { level: 1, name: screens.find(s => s.id === views[1])!.label })).toBeVisible();
  });

  it("有效工作会话重开保持参数，嵌套损坏会话恢复为可操作页面", () => {
    const state = initialState(); state.frames[2].params.contrast = 40;
    sessionStorage.setItem(storageKey, JSON.stringify(state)); const first = show("/workflow/teach");
    expect(screen.getByRole("spinbutton", { name: "最低灰度对比" })).toHaveValue(40); first.unmount();
    const corrupted = JSON.parse(sessionStorage.getItem(storageKey)!); corrupted.frames[2].params = null;
    sessionStorage.setItem(storageKey, JSON.stringify(corrupted)); show("/workflow/teach");
    expect(screen.getByRole("spinbutton", { name: "最低灰度对比" })).toHaveValue(32);
    expect(screen.getByRole("button", { name: "取新样本" })).toBeEnabled();
  });

  it("浏览器不提供会话存储时仍可执行页面操作", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementationOnce(() => { throw new Error("storage disabled"); });
    const save = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage disabled"); });
    show("/workflow/settings"); fireEvent.change(screen.getByRole("spinbutton", { name: "原图保留天数" }), { target: { value: "60" } });
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));
    expect(screen.getByRole("status")).toHaveTextContent("本地预览设置已保存");
    expect(screen.getByRole("button", { name: "保存设置" })).toBeDisabled(); save.mockRestore();
  });
});

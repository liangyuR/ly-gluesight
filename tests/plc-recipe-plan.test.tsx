import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import S7RecipePlanPanel from "../src/features/plc/components/S7RecipePlanPanel";
import { plcApi } from "../src/features/plc/api";
import type { PlcRecipePlan } from "../src/features/plc/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/plc/api", () => ({ plcApi: { recipeChoices: vi.fn(), getOperationState: vi.fn(), recipePlan: vi.fn() } }));
const plan = (recipeId = "A"): PlcRecipePlan => ({
  protocolVersion: 1, recipeId, planVersion: 3, shotCount: 4,
  cameraSlots: ["cam1", "cam2", "cam3"], cameraShots: [0, 4, 0],
  shots: Array.from({ length: 4 }, (_, index) => ({ shotId: `shot-${index + 1}`, poseId: `pose-${index + 1}`, cameraId: "cam2" })),
});
const writeText = vi.fn();
beforeEach(() => {
  vi.mocked(plcApi.recipeChoices).mockResolvedValue([{ id: "A", name: "产品 A", version: 3 }, { id: "B", name: "产品 B", version: 2 }]);
  vi.mocked(plcApi.getOperationState).mockResolvedValue({ phase: "ACQUIRE", plcLocked: true, activeRecipeId: "A" });
  vi.mocked(plcApi.recipePlan).mockImplementation(async recipeId => plan(recipeId));
  writeText.mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
});

describe("只读配方握手计划", () => {
  it("用户请求后读取当前生产配方，保留实际槽位计数并复制原始计划", async () => {
    render(<S7RecipePlanPanel />); expect(plcApi.recipeChoices).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    expect(await screen.findByRole("button", { name: "复制计划 JSON" })).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "生产配方" })).toHaveValue("A");
    expect(plcApi.recipePlan).toHaveBeenCalledWith("A");
    const rows = screen.getAllByRole("row");
    expect(within(rows[1]).getByText("0")).toBeVisible();
    expect(within(rows[2]).getByText("4")).toBeVisible();
    expect(within(rows[3]).getByText("0")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "复制计划 JSON" }));
    expect(writeText).toHaveBeenCalledWith(JSON.stringify(plan(), null, 2));
    expect(await screen.findByRole("button", { name: "已复制 JSON" })).toBeVisible();
  });

  it("拍照点表在编号旁列出 Pose，同一 Pose 触发两台相机时都列出，不再有工件坐标中心", async () => {
    vi.mocked(plcApi.recipePlan).mockResolvedValueOnce({ ...plan(), cameraShots: [2, 1, 1], shots: [
      { shotId: "P1", poseId: "A1", cameraId: "cam1" }, { shotId: "P2", poseId: "A1", cameraId: "cam2" },
      { shotId: "P3", poseId: "A3", cameraId: "cam3" }, { shotId: "P4", poseId: "A4", cameraId: "cam1" },
    ] });
    render(<S7RecipePlanPanel />); await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    const table = await screen.findByRole("table", { name: "拍照点计划" });
    const headers = within(table).getAllByRole("columnheader").map(h => h.textContent);
    expect(headers).toEqual(["拍照点", "Pose", "相机编号"]);
    const rows = within(table).getAllByRole("row").slice(1).map(r => within(r).getAllByRole("cell").map(c => c.textContent));
    expect(rows).toEqual([["P1", "A1", "cam1"], ["P2", "A1", "cam2"], ["P3", "A3", "cam3"], ["P4", "A4", "cam1"]]);
    expect(screen.queryByText(/中心/)).not.toBeInTheDocument();
  });

  it("切换配方后忽略旧请求，不能把 A 的计划显示成 B", async () => {
    const first = deferred<PlcRecipePlan>(); vi.mocked(plcApi.recipePlan).mockReturnValueOnce(first.promise);
    render(<S7RecipePlanPanel />); await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    const select = await screen.findByRole("combobox", { name: "生产配方" });
    await userEvent.selectOptions(select, "B");
    await screen.findByRole("button", { name: "复制计划 JSON" });
    await act(async () => first.resolve(plan("A")));
    await userEvent.click(screen.getByRole("button", { name: "复制计划 JSON" }));
    expect(writeText).toHaveBeenCalledWith(JSON.stringify(plan("B"), null, 2));
  });

  it("没有当前配方时不自动猜选，选择后才生成计划", async () => {
    vi.mocked(plcApi.getOperationState).mockResolvedValueOnce({ phase: "IDLE", activeRecipeId: null });
    render(<S7RecipePlanPanel />); await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    const select = await screen.findByRole("combobox", { name: "生产配方" });
    expect(select).toHaveValue(""); expect(plcApi.recipePlan).not.toHaveBeenCalled();
    await userEvent.selectOptions(select, "B"); expect(plcApi.recipePlan).toHaveBeenCalledWith("B");
  });

  it("计划校验失败显示原因，重新读取可恢复", async () => {
    vi.mocked(plcApi.recipePlan).mockRejectedValueOnce(new Error("相机槽位不匹配"));
    render(<S7RecipePlanPanel />); await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("相机槽位不匹配");
    expect(screen.queryByRole("button", { name: "复制计划 JSON" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "刷新配方计划" }));
    expect(await screen.findByRole("button", { name: "复制计划 JSON" })).toBeVisible();
  });

  it("复制失败可重试，读取失败不会展示旧计划", async () => {
    writeText.mockRejectedValueOnce(new Error("剪贴板不可用"));
    render(<S7RecipePlanPanel />); await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    await userEvent.click(await screen.findByRole("button", { name: "复制计划 JSON" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("剪贴板不可用");
    await userEvent.click(screen.getByRole("button", { name: "复制计划 JSON" }));
    expect(await screen.findByRole("button", { name: "已复制 JSON" })).toBeVisible();
    vi.mocked(plcApi.recipeChoices).mockRejectedValueOnce(new Error("配方列表不可读"));
    await userEvent.click(screen.getByRole("button", { name: "刷新配方计划" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("配方列表不可读");
    expect(screen.queryByRole("button", { name: "已复制 JSON" })).toBeNull();
  });
});

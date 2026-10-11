import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import S7RecipePlanPanel from "../src/features/plc/components/S7RecipePlanPanel";
import { plcApi } from "../src/features/plc/api";
import type { PlcRecipePlan } from "../src/features/plc/types";
import { deferred } from "./fixtures";

vi.mock("../src/features/plc/api", () => ({ plcApi: { recipeChoices: vi.fn(), getOperationState: vi.fn(), recipePlan: vi.fn() } }));
const plan = (recipeId = "A"): PlcRecipePlan => ({
  protocolVersion: 2, recipeId, planVersion: 3, shotCount: 4,
  deviceId: "composite-camera", deviceShots: 4,
  shots: Array.from({ length: 4 }, (_, index) => ({ shotId: `shot-${index + 1}`, poseId: `pose-${index + 1}`, cameraId: "composite-camera" })),
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
  it("用户请求后读取唯一设备及触发数并复制原始计划", async () => {
    render(<S7RecipePlanPanel />); expect(plcApi.recipeChoices).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "读取配方计划" }));
    expect(await screen.findByRole("button", { name: "复制计划 JSON" })).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "生产配方" })).toHaveValue("A");
    expect(plcApi.recipePlan).toHaveBeenCalledWith("A");
    const deviceTable = screen.getByRole("table", { name: "单设备计划" });
    const rows = within(deviceTable).getAllByRole("row");
    expect(rows).toHaveLength(2);
    expect(within(rows[1]).getByText("composite-camera")).toBeVisible();
    expect(within(rows[1]).getByText("4")).toBeVisible();
    expect(screen.queryByText(/槽位|未配置|相机 2|相机 3/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "复制计划 JSON" }));
    expect(writeText).toHaveBeenCalledWith(JSON.stringify(plan(), null, 2));
    expect(await screen.findByRole("button", { name: "已复制 JSON" })).toBeVisible();
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

});

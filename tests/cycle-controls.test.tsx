import { Profiler } from "react";
import { act, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { expect, it, vi } from "vitest";
import { useMaintenance, useProductionBusy, type CycleControlState } from "../src/features/cycle/control";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

function CameraActions() {
  return <button disabled={useProductionBusy()}>相机操作</button>;
}

function MaintenanceBadge() {
  return <output aria-label="维护状态">{useMaintenance()?.reason ?? "常规生产"}</output>;
}

it("轻量状态的无关字段变化 → 只提交受影响的生产操作和维护提示", async () => {
  const mixed: CycleControlState = { productionBusy: false, plcLocked: false, plcHandshake: null, maintenance: null };
  const listeners: ((event: { payload: CycleControlState }) => void)[] = [];
  vi.mocked(listen).mockImplementation(async (_event, callback) => {
    listeners.push(callback as (event: { payload: CycleControlState }) => void);
    return () => {};
  });
  vi.mocked(invoke).mockResolvedValue(mixed);
  const actions = vi.fn(), badge = vi.fn();
  await act(async () => {
    render(<>
      <Profiler id="actions" onRender={actions}><CameraActions /></Profiler>
      <Profiler id="badge" onRender={badge}><MaintenanceBadge /></Profiler>
    </>);
  });
  actions.mockClear(); badge.mockClear();
  const publish = (payload: CycleControlState) => act(() => listeners.forEach(listener => listener({ payload })));
  const button = screen.getByRole("button", { name: "相机操作" });
  const maintenance = { since: 10, reason: "调机" };

  publish({ ...mixed, productionBusy: true, plcLocked: true });
  expect(button).toBeDisabled();
  expect(actions).toHaveBeenCalledTimes(1);
  expect(badge).not.toHaveBeenCalled();

  publish({ ...mixed, productionBusy: true, plcLocked: true, maintenance });
  expect(screen.getByLabelText("维护状态")).toHaveTextContent("调机");
  expect(actions).toHaveBeenCalledTimes(1);
  expect(badge).toHaveBeenCalledTimes(1);

  publish({ ...mixed, maintenance: { ...maintenance } });
  expect(button).toBeEnabled();
  expect(actions).toHaveBeenCalledTimes(2);
  expect(badge).toHaveBeenCalledTimes(1);
});

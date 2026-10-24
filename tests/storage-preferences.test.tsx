import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, it, vi } from "vitest";
import type { StoragePreferences } from "../src/lib/storage";
import FeasibilityCalc from "../src/features/camera/components/FeasibilityCalc";
import { deferred } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
let saved: StoragePreferences;
beforeEach(() => {
  saved = {};
  vi.mocked(invoke).mockReset().mockImplementation(async (command, args) => {
    if (command === "storage_preferences_get") return { ...saved };
    if (command === "storage_preferences_update") {
      saved = { ...saved, ...(args as { patch: StoragePreferences }).patch };
      return { ...saved };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
});



it("参数延迟读回期间输入 → 保留新值并写入资料配置，失败可重试", async () => {
  const read = deferred<StoragePreferences>();
  vi.mocked(invoke).mockImplementationOnce(() => read.promise);
  render(<FeasibilityCalc />);
  const speed = screen.getByRole("spinbutton", { name: /速度/ });
  fireEvent.change(speed, { target: { value: "123" } });
  saved = { flyshotSpeedMmS: 70, legacyImported: true };
  await act(async () => read.resolve(saved));
  await waitFor(() => expect(saved.flyshotSpeedMmS).toBe(123));
  expect(speed).toHaveValue(123);
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "storage_preferences_update") throw new Error("目录只读");
    return original(command, args);
  });
  fireEvent.change(speed, { target: { value: "150" } });
  expect(await screen.findByRole("alert")).toHaveTextContent("目录只读");
  expect(saved.flyshotSpeedMmS).toBe(123);
  vi.mocked(invoke).mockImplementation(original);
  fireEvent.click(screen.getByRole("button", { name: "重试保存" }));
  await waitFor(() => expect(saved.flyshotSpeedMmS).toBe(150));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
});

import { render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AppLayout from "../src/layout/AppLayout";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

beforeEach(() => {
  vi.mocked(listen).mockResolvedValue(() => {});
  vi.mocked(invoke).mockResolvedValue(null);
});

describe("侧栏当前页", () => {
  it.each([
    ["/recipe/teach", "配方", "配方 · 单帧示教"],
    ["/camera/calibration", "飞拍标定", "飞拍标定"],
    ["/settings", "系统设置", "系统设置"],
  ])("打开 %s → 只有「%s」标为当前页，标题为「%s」", (path, current, title) => {
    render(<MemoryRouter initialEntries={[path]}><Routes><Route element={<AppLayout />}><Route path={path} element={<div />} /></Route></Routes></MemoryRouter>);
    const sidebar = within(screen.getByRole("complementary"));
    expect(sidebar.getAllByRole("link", { current: "page" }).map(link => link.getAttribute("aria-label"))).toEqual([current]);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(title);
  });
});

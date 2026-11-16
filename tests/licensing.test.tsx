import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { LicenseProvider } from "../src/features/licensing/context";
import LicenseNotice from "../src/features/licensing/LicenseNotice";
import LicensePage from "../src/pages/LicensePage";
import type { LicenseStatus } from "../src/features/licensing/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));

const now = Date.parse("2026-11-24T00:00:00+08:00");
let status: LicenseStatus;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(now);
  status = {
    state: "expiring", allowed: true, message: "授权将在七天内到期", configured: true,
    expiresAt: "2026-12-01T00:00:00+08:00", warningAt: "2026-11-24T00:00:00+08:00",
    emergencyUntil: null, licenseId: "test-license", deviceFingerprint: "fingerprint-summary", emergencyAvailable: true,
  };
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "license_status") return { ...status };
    throw new Error(`Unexpected command: ${command}`);
  });
});

afterEach(() => vi.useRealTimers());

async function mount(page = false) {
  const result = render(<MemoryRouter><LicenseProvider><LicenseNotice />{page && <LicensePage />}</LicenseProvider></MemoryRouter>);
  await act(async () => {});
  return result;
}

it("进入七天提醒并再次打开 → 常驻提示保留，每个北京时间日期只弹一次", async () => {
  status.state = "valid";
  status.warningAt = new Date(now + 1000).toISOString();
  const page = await mount();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  status.state = "expiring";
  await act(async () => { await vi.advanceTimersByTimeAsync(1020); });
  expect(screen.getByRole("dialog", { name: "软件授权将在七天内到期" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "知道了" }));
  page.unmount();
  await mount();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByText("即将到期")).toBeInTheDocument();
  vi.setSystemTime(now + 24 * 60 * 60 * 1000);
  await act(async () => { fireEvent.focus(window); });
  expect(screen.getByRole("dialog", { name: "软件授权将在七天内到期" })).toBeInTheDocument();
});

it.each(["valid", "emergency"] as const)("%s 跨越截止时刻 → 主动刷新后端并显示停用，续期和历史入口不受遮挡", async state => {
  status.state = state;
  if (state === "emergency") status.emergencyUntil = new Date(now + 1000).toISOString();
  else status.expiresAt = new Date(now + 1000).toISOString();
  await mount(true);
  status.state = "expired";
  status.allowed = false;
  status.message = "合同已到期";
  await act(async () => { await vi.advanceTimersByTimeAsync(1020); });
  expect(screen.getAllByText("授权已到期").length).toBeGreaterThan(0);
  expect(screen.getByText(/已有数据仍可查看和导出/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "更新授权" })).toBeEnabled();
  expect(screen.getByRole("link", { name: "授权管理" })).toHaveAttribute("href", "/license");
});

it("应急密码错误后重试成功 → 失败保持停用，输入立即清空且秘密不落浏览器存储", async () => {
  status.state = "expired"; status.allowed = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "license_status") return { ...status };
    if (command === "license_emergency_unlock") {
      if ((args as { password: string }).password !== "test-only-password") throw new Error("应急密码不正确");
      return { ...status, state: "emergency", allowed: true, emergencyUntil: "2026-11-27T00:00:00+08:00" };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  await mount(true);
  const field = screen.getByLabelText(/^应急密码/);
  fireEvent.change(field, { target: { value: "wrong-password" } });
  fireEvent.change(screen.getByLabelText(/^使用原因/), { target: { value: "现场排查授权连接" } });
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "临时解锁 72 小时" })); });
  expect(field).toHaveValue("");
  expect(screen.getByText("应急密码不正确")).toBeInTheDocument();
  expect(screen.getAllByText("授权已到期").length).toBeGreaterThan(0);
  fireEvent.change(field, { target: { value: "test-only-password" } });
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "临时解锁 72 小时" })); });
  expect(field).toHaveValue("");
  expect(screen.getAllByText("现场应急放行中").length).toBeGreaterThan(0);
  expect(JSON.stringify(localStorage)).not.toContain("password");
  expect(JSON.stringify(sessionStorage)).not.toContain("password");
});

it("更新授权失败后成功 → 失败保留原期限，成功同步全局状态", async () => {
  let fail = true;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "license_status") return { ...status };
    if (command === "license_sync") {
      if (fail) throw new Error("连接授权服务失败");
      return { ...status, state: "valid", expiresAt: "2027-12-01T00:00:00+08:00" };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  await mount(true);
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "知道了" }));
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "更新授权" })); });
  expect(screen.getByText("连接授权服务失败")).toBeInTheDocument();
  expect(screen.getByText("2026/12/1 00:00:00（北京时间）")).toBeInTheDocument();
  fail = false;
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "更新授权" })); });
  expect(screen.getByText("授权有效")).toBeInTheDocument();
  expect(screen.getByText("2027/12/1 00:00:00（北京时间）")).toBeInTheDocument();
  expect(screen.queryByText("即将到期")).not.toBeInTheDocument();
});

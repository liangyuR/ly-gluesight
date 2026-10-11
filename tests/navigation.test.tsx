import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import App from "../src/App";
import ErrorBoundary from "../src/layout/ErrorBoundary";

describe("应用路由与错误恢复", () => {
  it.each(["/", "/unknown-route"])("%s 进入操作流程引导", async path => {
    render(<MemoryRouter initialEntries={[path]}><App /></MemoryRouter>);
    expect(await screen.findByRole("heading", { level: 1, name: "操作流程" })).toBeVisible();
    expect(within(screen.getByRole("navigation", { name: "操作导航" })).getByRole("link", { name: "操作流程" })).toHaveAttribute("aria-current", "page");
  });

  it("历史复测匹配具体路由，导航切到历史列表后更新选中项", async () => {
    render(<MemoryRouter initialEntries={["/history/retest"]}><App /></MemoryRouter>);
    const nav = within(screen.getByRole("navigation", { name: "操作导航" }));
    expect(await screen.findByRole("heading", { level: 1, name: "历史复测" })).toBeVisible();
    expect(nav.getByRole("link", { name: "历史复测" })).toHaveAttribute("aria-current", "page");
    expect(nav.getByRole("link", { name: "历史记录" })).not.toHaveAttribute("aria-current");
    await userEvent.click(nav.getByRole("link", { name: "历史记录" }));
    expect(screen.getByRole("heading", { level: 1, name: "历史记录" })).toBeVisible();
    expect(nav.getByRole("link", { name: "历史记录" })).toHaveAttribute("aria-current", "page");
  });

  it("页面渲染异常显示原因，修复后可重试", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    let broken = true;
    function Content() { if (broken) throw new Error("图像显示异常"); return <p>页面恢复</p>; }
    render(<ErrorBoundary><Content /></ErrorBoundary>);
    expect(screen.getByText("页面出错了")).toBeVisible(); expect(screen.getByText("图像显示异常")).toBeVisible();
    expect(errors).toHaveBeenCalled(); broken = false;
    await userEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(screen.getByText("页面恢复")).toBeVisible();
  });
});

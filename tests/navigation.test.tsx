import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import ErrorBoundary from "../src/layout/ErrorBoundary";

describe("页面错误恢复", () => {
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

import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import ContextHelp from "../src/features/cycle/components/ContextHelp";

afterEach(() => { vi.useRealTimers(); });

it("键盘查看帮助并固定展开 → 内容可读且关闭后焦点保持可操作", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true }); // Testing Library 收尾等 0 ms 定时器，假时钟须随真实时间推进
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  render(<><ContextHelp label="选帧说明">切换原图不会改变整件判定</ContextHelp><button>下一项</button></>);
  const trigger = screen.getByRole("button", { name: "选帧说明" });
  await user.tab();
  expect(trigger).toHaveFocus();
  expect(screen.getByRole("tooltip")).toHaveTextContent("切换原图不会改变整件判定");
  await user.keyboard("{Enter}");
  await user.tab();
  act(() => { vi.advanceTimersByTime(160); }); // 超过失焦后 120 ms 的收起延迟
  expect(screen.getByRole("tooltip")).toBeVisible();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
  await user.click(trigger);
  expect(screen.getByRole("tooltip")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "下一项" }));
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "下一项" })).toHaveFocus();
});

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import ContextHelp from "../src/features/cycle/components/ContextHelp";

it("键盘查看帮助并固定展开 → 内容可读且关闭后焦点保持可操作", async () => {
  const user = userEvent.setup();
  render(<><ContextHelp label="选帧说明">切换原图不会改变整件判定</ContextHelp><button>下一项</button></>);
  const trigger = screen.getByRole("button", { name: "选帧说明" });
  await user.tab();
  expect(trigger).toHaveFocus();
  expect(screen.getByRole("tooltip")).toHaveTextContent("切换原图不会改变整件判定");
  await user.keyboard("{Enter}");
  await user.tab();
  await new Promise(resolve => setTimeout(resolve, 160));
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

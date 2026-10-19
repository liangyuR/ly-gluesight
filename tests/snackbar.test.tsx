import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SnackbarHost, SnackbarProvider, snackDuration, useSnackbar } from "../src/components/Snackbar";

function Trigger({ texts }: { texts: string[] }) {
  const snack = useSnackbar();
  return <button onClick={() => texts.forEach(text => snack(text))}>{"显示 " + texts.join("、")}</button>;
}
const show = (...texts: string[]) => {
  render(<SnackbarProvider><SnackbarHost /><Trigger texts={texts} /></SnackbarProvider>);
  fireEvent.click(screen.getByRole("button", { name: "显示 " + texts.join("、") }));
};

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

it("提示到时自动消失；指针悬停期间不消失，移开后继续计时", () => {
  show("候选配置已保存");
  const message = screen.getByText("候选配置已保存");
  act(() => { vi.advanceTimersByTime(1000); });
  fireEvent.mouseEnter(message.closest(".qs-snackbar")!);
  act(() => { vi.advanceTimersByTime(30_000); });
  expect(screen.getByText("候选配置已保存")).toBeInTheDocument();
  fireEvent.mouseLeave(message.closest(".qs-snackbar")!);
  act(() => { vi.advanceTimersByTime(snackDuration("候选配置已保存") + 200); });
  expect(screen.getByRole("status")).toBeEmptyDOMElement();
});

it("同一次处理里连续发出两条提示 → 只保留最新一条，不叠在同一格", () => {
  show("候选配置已保存", "本组示教已保存");
  act(() => { vi.advanceTimersByTime(500); });
  expect(screen.getByRole("status")).toHaveTextContent(/^本组示教已保存$/);
});

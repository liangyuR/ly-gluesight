import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SnackbarHost, SnackbarProvider, snackDuration, useSnackbar } from "../src/components/Snackbar";

function Trigger({ text }: { text: string }) {
  const snack = useSnackbar();
  return <button onClick={() => snack(text)}>{"显示 " + text}</button>;
}
const show = () => render(<SnackbarProvider><SnackbarHost /><Trigger text="候选配置已保存" /><Trigger text="本组示教已保存" /></SnackbarProvider>);

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

it("提示到时自动消失；指针悬停期间不消失，移开后继续计时", () => {
  show();
  fireEvent.click(screen.getByRole("button", { name: "显示 候选配置已保存" }));
  const message = screen.getByText("候选配置已保存");
  act(() => { vi.advanceTimersByTime(1000); });
  fireEvent.mouseEnter(message.closest(".qs-snackbar")!);
  act(() => { vi.advanceTimersByTime(30_000); });
  expect(screen.getByText("候选配置已保存")).toBeInTheDocument();
  fireEvent.mouseLeave(message.closest(".qs-snackbar")!);
  act(() => { vi.advanceTimersByTime(snackDuration("候选配置已保存") + 200); });
  expect(screen.getByRole("status")).toBeEmptyDOMElement();
});

it("连续两次操作成功 → 只保留最新一条提示，不堆叠", () => {
  show();
  fireEvent.click(screen.getByRole("button", { name: "显示 候选配置已保存" }));
  fireEvent.click(screen.getByRole("button", { name: "显示 本组示教已保存" }));
  act(() => { vi.advanceTimersByTime(200); });
  expect(screen.getByRole("status")).toHaveTextContent(/^本组示教已保存$/);
});

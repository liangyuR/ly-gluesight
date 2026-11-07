import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import { expect, it, vi } from "vitest";
import { CreateProjectDialog } from "../src/features/project/ProjectDialogs";
import SwitchProjectDialog from "../src/features/project/SwitchProjectDialog";
import { workspaceState } from "./fixtures";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: () => workspaceState() }));

it.each(["切换", "创建并切换"])("开发版%s返回需要重启 → 保留当前项目提示并禁止重复切换", async mode => {
  const target = { schema: 1, id: "line-b", name: "B 线", description: "", createdAt: 0, createdBy: "", copiedFrom: null };
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "project_create") return target;
    if (command === "project_switch") return { currentProject: "line-a", nextProject: "line-b", restartRequired: true };
    return { recipes: 0, records: 0, bytes: 0 };
  });
  const close = vi.fn();
  render(<SnackbarProvider>{mode === "切换"
    ? <SwitchProjectDialog target={target} onClose={close} />
    : <CreateProjectDialog entries={[]} onClose={close} />}</SnackbarProvider>);
  if (mode === "创建并切换") {
    await userEvent.type(screen.getByRole("textbox", { name: "名称" }), "B 线");
    await userEvent.click(screen.getByRole("checkbox", { name: /创建后切换到该项目/ }));
  }
  await userEvent.click(screen.getByRole("button", { name: mode === "切换" ? "切换并重启" : "创建并切换" }));
  expect(await screen.findByText(/下次启动项目设为 line-b，当前仍运行 line-a/)).toBeVisible();
  expect(screen.getByRole("button", { name: "等待手动重启" })).toBeDisabled();
  expect(screen.queryByText("正在重启…")).not.toBeInTheDocument();
  expect(close).not.toHaveBeenCalled();
});

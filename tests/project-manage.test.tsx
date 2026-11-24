import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import { describe, expect, it, vi } from "vitest";
import { DeleteProjectDialog } from "../src/features/project/ProjectDialogs";
import type { ProjectEntry } from "../src/lib/project";
import ProjectsPage from "../src/pages/ProjectsPage";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));

it("项目当前运行或元数据不可用 → 行菜单只允许该项目可执行的操作", async () => {
  const entries: ProjectEntry[] = [
    { id: "active", current: true, error: null, meta: { schema: 1, id: "active", name: "当前工位", description: "", createdAt: 0, createdBy: "", copiedFrom: null } },
    { id: "broken", current: false, error: "project.json 无法读取", meta: null },
  ];
  vi.mocked(invoke).mockImplementation(async command => command === "project_list" ? entries : { recipes: 0, records: 0, bytes: 0 });
  render(<SnackbarProvider><ProjectsPage /></SnackbarProvider>);

  await userEvent.click(await screen.findByRole("button", { name: "项目操作：当前工位" }));
  const currentMenu = screen.getByRole("menu");
  expect(within(currentMenu).queryByRole("menuitem", { name: "切换到此项目" })).not.toBeInTheDocument();
  expect(within(currentMenu).getByRole("menuitem", { name: "删除项目" })).toHaveAttribute("aria-disabled", "true");
  expect(within(currentMenu).getByText("请先切换到其他项目")).toBeVisible();
  await userEvent.keyboard("{Escape}");

  await userEvent.click(screen.getByRole("button", { name: "项目操作：broken" }));
  const brokenMenu = screen.getByRole("menu");
  const deleteProject = within(brokenMenu).getByRole("menuitem", { name: "删除项目" });
  expect(within(brokenMenu).getAllByRole("menuitem")).toEqual([deleteProject]);
  await userEvent.click(deleteProject);
  const dialog = await screen.findByRole("dialog", { name: "删除项目" });
  expect(within(dialog).getByRole("textbox", { name: /输入项目编号“broken”/ })).toBeVisible();
});

describe("删除项目确认", () => {
  it("输入与项目名称不完全一致 → 删除不可用；完全一致 → 发出删除", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    const entry: ProjectEntry = { id: "p-20261024-01", current: false, error: null, meta: { schema: 1, id: "p-20261024-01", name: "京航 A 线", description: "", createdAt: 0, createdBy: "", copiedFrom: null } };
    const onClose = vi.fn();
    render(<SnackbarProvider><DeleteProjectDialog entry={entry} usage={{ status: "loading" }} onClose={onClose} /></SnackbarProvider>);
    const input = screen.getByRole("textbox", { name: /输入项目名称/ });
    const remove = screen.getByRole("button", { name: "删除" });
    expect(input).toHaveFocus();
    for (const typed of ["京航 A", "京航 A 线 ", "京航 a 线"]) {
      await userEvent.clear(input);
      await userEvent.type(input, typed);
      expect(remove).toBeDisabled();
    }
    await userEvent.clear(input);
    await userEvent.type(input, "京航 A 线");
    await userEvent.click(remove);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(invoke).toHaveBeenCalledWith("project_delete", { id: "p-20261024-01" });
  });
});

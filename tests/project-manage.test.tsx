import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import { describe, expect, it, vi } from "vitest";
import { DeleteProjectDialog } from "../src/features/project/ProjectDialogs";
import type { ProjectEntry, ProjectUsage } from "../src/lib/project";
import ProjectsPage from "../src/pages/ProjectsPage";
import { deferred } from "./fixtures";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));

function projectEntry(id: string, name: string, current = false): ProjectEntry {
  return { id, current, error: null, meta: { schema: 1, id, name, description: "", createdAt: 0, createdBy: "", copiedFrom: null } };
}

function projectUsage(records = 0): ProjectUsage {
  return { recipes: 0, workspaces: 0, captureRounds: 0, records, firstRecordAt: null, lastRecordAt: null, bytes: 0 };
}

it("选中当前项目或不可用项目 → 详情菜单只允许所选项目可执行的操作", async () => {
  const entries: ProjectEntry[] = [
    projectEntry("active", "当前工位", true),
    { id: "broken", current: false, error: "project.json 无法读取", meta: null },
  ];
  vi.mocked(invoke).mockImplementation(async command => command === "project_list" ? entries : projectUsage());
  render(<SnackbarProvider><ProjectsPage /></SnackbarProvider>);

  await userEvent.click(await screen.findByRole("button", { name: "项目操作：当前工位" }));
  const currentMenu = screen.getByRole("menu");
  expect(within(currentMenu).queryByRole("menuitem", { name: "切换到此项目" })).not.toBeInTheDocument();
  expect(within(currentMenu).getByRole("menuitem", { name: "删除项目" })).toHaveAttribute("aria-disabled", "true");
  expect(within(currentMenu).getByText("请先切换到其他项目")).toBeVisible();
  await userEvent.keyboard("{Escape}");

  const list = within(screen.getByRole("list", { name: "本机项目" }));
  await userEvent.click(list.getByRole("button", { name: /broken/ }));
  await userEvent.click(screen.getByRole("button", { name: "项目操作：broken" }));
  const brokenMenu = screen.getByRole("menu");
  const deleteProject = within(brokenMenu).getByRole("menuitem", { name: "删除项目" });
  expect(within(brokenMenu).getAllByRole("menuitem")).toEqual([deleteProject]);
  await userEvent.click(deleteProject);
  const dialog = await screen.findByRole("dialog", { name: "删除项目" });
  expect(within(dialog).getByRole("textbox", { name: /输入项目编号“broken”/ })).toBeVisible();
});

it("刷新时列表重排或移除已选项目 → 优先保留原编号，否则选择第一项", async () => {
  const first = projectEntry("first", "一号线");
  const second = projectEntry("second", "二号线");
  const active = projectEntry("active", "三号线", true);
  let entries = [first, second, active];
  vi.mocked(invoke).mockImplementation(async command => command === "project_list" ? entries : projectUsage());
  render(<SnackbarProvider><ProjectsPage /></SnackbarProvider>);
  const list = within(await screen.findByRole("list", { name: "本机项目" }));
  const details = within(screen.getByRole("region", { name: "项目详情" }));
  const refresh = screen.getByRole("button", { name: "刷新项目列表" });
  await userEvent.click(list.getByRole("button", { name: /二号线/ }));

  entries = [active, first, projectEntry("second", "二号线改名")];
  await userEvent.click(refresh);
  expect(await details.findByRole("heading", { name: "二号线改名" })).toBeVisible();
  expect(list.getByRole("button", { name: /三号线.*当前/ })).toHaveAttribute("aria-pressed", "false");

  entries = [active, first];
  await userEvent.click(refresh);
  expect(await details.findByRole("heading", { name: "三号线" })).toBeVisible();
  expect(list.getByRole("button", { name: /三号线.*当前/ })).toHaveAttribute("aria-pressed", "true");
});

it("前一个项目的统计晚于当前选择返回 → 详情保留所选项目的记录数", async () => {
  const first = deferred<ProjectUsage>();
  const entries = [projectEntry("first", "一号线"), projectEntry("second", "二号线")];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "project_list") return entries;
    return (args as { id: string }).id === "first" ? first.promise : projectUsage(3);
  });
  render(<SnackbarProvider><ProjectsPage /></SnackbarProvider>);
  const list = within(await screen.findByRole("list", { name: "本机项目" }));
  await userEvent.click(list.getByRole("button", { name: /二号线/ }));
  const details = within(screen.getByRole("region", { name: "项目详情" }));
  expect(await details.findByText("3 条")).toBeVisible();
  await act(async () => first.resolve(projectUsage(18)));
  expect(details.getByText("3 条")).toBeVisible();
  expect(details.queryByText("18 条")).not.toBeInTheDocument();
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

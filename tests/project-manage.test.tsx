import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import { describe, expect, it, vi } from "vitest";
import { DeleteProjectDialog } from "../src/features/project/ProjectDialogs";
import { CopyProjectDialog } from "../src/features/project/TransferDialogs";
import { defaultProjectId, importedProjectName, projectNameError } from "../src/features/project/rules";
import type { ProjectEntry } from "../src/lib/project";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));

describe("新建项目的默认编号", () => {
  it.each([
    [[], "p-20261025-01"],
    [["p-20261025-01", "p-20261025-03", "p-20261024-02"], "p-20261025-02"],
    [["P-20261025-01", "default"], "p-20261025-02"],
  ])("已占用 %j → %s", (taken, expected) => {
    expect(defaultProjectId(taken, new Date(2026, 9, 25, 23, 30))).toBe(expected);
  });
});

describe("导入项目的默认名称", () => {
  const named = (...names: string[]): ProjectEntry[] => names.map((name, i) => ({ id: `p-${i}`, current: false, error: null, meta: { schema: 1, id: `p-${i}`, name, description: "", createdAt: 0, createdBy: "", copiedFrom: null } }));
  it.each([
    [[], "京航 A 线"],
    [["京航 A 线 "], "京航 A 线（导入）"],
    [["京航 A 线", "京航 A 线（导入）"], "京航 A 线（导入 2）"],
  ])("本机已有 %j → %s，且不与本机重名", (names, expected) => {
    const entries = named(...names);
    const name = importedProjectName(" 京航 A 线", entries);
    expect(name).toBe(expected);
    expect(projectNameError(name, entries)).toBe("");
  });
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

describe("复制项目", () => {
  it("勾选包含记录 → 以来源编号、新编号与记录选项发出复制，完成后可切换到副本", async () => {
    const source = { schema: 1, id: "line-a", name: "京航 A 线", description: "三相机", createdAt: 0, createdBy: "", copiedFrom: null };
    const copy = { ...source, id: "p-20261026-01", name: "京航 A 线（副本）", copiedFrom: "line-a" };
    vi.mocked(invoke).mockResolvedValue({ meta: copy, notes: ["1 个计划版本已按本机重新分配"] });
    const onSwitch = vi.fn();
    render(<CopyProjectDialog project={source} usage={{ status: "loading" }} entries={[{ id: "line-a", current: true, error: null, meta: source }]} onClose={vi.fn()} onSwitch={onSwitch} />);
    const id = screen.getByRole("textbox", { name: /新项目编号/ });
    await userEvent.clear(id);
    await userEvent.type(id, "p-20261026-01");
    await userEvent.click(screen.getByRole("checkbox", { name: "包含检测记录与历史原图" }));
    await userEvent.click(screen.getByRole("button", { name: "复制为新项目" }));
    expect(invoke).toHaveBeenCalledWith("project_copy", { request: { sourceId: "line-a", id: "p-20261026-01", name: "京航 A 线（副本）", description: "三相机", includeRecords: true } });
    await userEvent.click(await screen.findByRole("button", { name: "切换到该项目" }));
    expect(onSwitch).toHaveBeenCalledWith(copy);
  });
});

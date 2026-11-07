import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { SnackbarProvider } from "qing-steel";
import { describe, expect, it, vi } from "vitest";
import { DeleteProjectDialog } from "../src/features/project/ProjectDialogs";
import type { ProjectEntry } from "../src/lib/project";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));

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

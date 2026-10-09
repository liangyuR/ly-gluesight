import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi } from "vitest";
import WorkflowPreviewPage from "../src/pages/WorkflowPreviewPage";
import type { View, WorkflowState } from "../src/features/workflow/model";

export const storageKey = "tujiao-workflow-preview-v3";
export function showWorkflow(view: View | string, state?: WorkflowState) {
  if (state) sessionStorage.setItem(storageKey, JSON.stringify(state));
  return render(<MemoryRouter initialEntries={["/workflow/" + view]}><Routes><Route path="/workflow/:view?" element={<WorkflowPreviewPage />} /></Routes></MemoryRouter>);
}
export const stored = (): WorkflowState => JSON.parse(sessionStorage.getItem(storageKey)!);
export const click = (name: string | RegExp) => fireEvent.click(screen.getByRole("button", { name }));
export const number = (name: string, value: number | string) => fireEvent.change(screen.getByRole("spinbutton", { name }), { target: { value: String(value) } });
export const select = (name: string, value: string) => fireEvent.change(screen.getByRole("combobox", { name }), { target: { value } });
export const navigate = (name: string) => fireEvent.click(within(screen.getByRole("navigation", { name: "操作流程导航" })).getByRole("link", { name }));
export const finishTask = (ms = 450) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });
export const panel = (title: string): HTMLElement => screen.getByRole("heading", { level: 2, name: title }).closest("section")!;

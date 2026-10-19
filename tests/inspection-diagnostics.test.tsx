import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import InspectionDiagnostics, { type DiagnosticGroup } from "../src/features/cycle/components/InspectionDiagnostics";
import RuntimeFrame from "../src/features/workspace/RuntimeFrame";
import ShotStrip from "../src/features/cycle/components/ShotStrip";
import { workspaceApi } from "../src/features/workspace/api";
import { cyclePart } from "./cycle-visual-fixtures";
import { snapshot, workspaceView } from "./fixtures";

it("本点完成但另一检测图异常 → 主区可发现且诊断保留独立结果并恢复焦点", async () => {
  vi.spyOn(workspaceApi, "liveImage").mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  const part = cyclePart();
  part.frames[0].viewResults = [
    { view: 1, verdict: "OK", error: null, score: .9, ms: 5 },
    { view: 2, verdict: "ERR_INSPECT", error: "图 2 定位失败", score: null, ms: 7 },
  ];
  function Screen() {
    const [group, setGroup] = useState<DiagnosticGroup | null>(null);
    const [selected, setSelected] = useState(1);
    return <><ShotStrip compact layout={workspaceView().layout} part={part} vis={[]} selected={selected} onSelect={setSelected} /><RuntimeFrame compact onDiagnostics={() => setGroup("capture")} part={part} layout={workspaceView().layout} k={selected} measured={[]} />
      {group && <InspectionDiagnostics group={group} onGroup={setGroup} onClose={() => setGroup(null)} snapshot={{ ...snapshot(), part }} readiness={null} layout={workspaceView().layout} part={part} selected={selected} logs={[]} />}</>;
  }
  render(<Screen />);
  const shot = within(screen.getByRole("group", { name: "拍照点" })).getByRole("button", { name: /^P1 · / });
  expect(within(shot).getByText("检测图注意 1")).toBeVisible();
  await userEvent.click(shot);
  const trigger = screen.getByRole("button", { name: "检测图异常 / 注意 1" });
  await userEvent.click(trigger);
  const drawer = within(screen.getByRole("dialog", { name: "检测诊断" }));
  expect(drawer.getByText("图 2 定位失败")).toBeVisible();
  expect(drawer.getByRole("table")).toHaveTextContent("5 ms");
  expect(drawer.getByRole("table")).toHaveTextContent("7 ms");
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
  vi.restoreAllMocks();
});

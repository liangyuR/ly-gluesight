import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import InspectionDiagnostics from "../src/features/cycle/components/InspectionDiagnostics";
import { burstPartView } from "./cycle-visual-fixtures";
import { snapshot } from "./fixtures";

it("段配方选中的帧组缺一台相机 → 采集与测量按帧组列出每台相机，缺帧的相机写缺帧而不是被略过", () => {
  const part = burstPartView();
  render(<InspectionDiagnostics group="capture" onGroup={() => {}} onClose={() => {}} snapshot={{ ...snapshot(), part }} readiness={null} layout={null} part={part} selected={0} selectedSeq={5} logs={[]} />);
  const table = within(screen.getByRole("table", { name: "本帧组各相机" }));
  const headers = table.getAllByRole("columnheader").map(header => header.textContent);
  const rows = table.getAllByRole("row").slice(1);
  const column = (name: string) => rows.map(row => row.children[headers.indexOf(name)]?.textContent);
  expect(column("相机")).toEqual(["CAM-1", "CAM-2", "CAM-3"]);
  expect(column("帧状态")).toEqual(["OK", "缺帧", "OK"]);
  expect(column("设备帧计数")).toEqual(["1005", "—", "3005"]);
  expect(screen.getByText("各相机帧数").nextElementSibling).toHaveTextContent("CAM-1 12 · CAM-2 11 · CAM-3 12");
});

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import UnrolledCurve from "../src/features/cycle/components/UnrolledCurve";
import { workspaceView } from "./fixtures";

describe("展开曲线段名", () => {
  it("段名比所在段宽 → 截断加省略号并保留完整名提示，相邻段名不再重叠", () => {
    const layout = structuredClone(workspaceView().layout);
    const names = Array.from({ length: 12 }, (_, k) => `S${String(k + 1).padStart(3, "0")}-cam2 · J1 · 图1`);
    layout.segments = names.map((name, k) => ({ ...layout.segments[0], name, first: 4 * k, count: 4 }));
    const n = names.length * 4;
    layout.points = { x: Array(n).fill(0), y: Array(n).fill(0), seg: Array.from({ length: n }, (_, j) => Math.floor(j / 4)), k: Array(n).fill(0) };
    render(<UnrolledCurve layout={layout} measured={[]} vis={[]} />);
    const label = screen.getByLabelText(`段 ${names[0]}`).querySelector("text");
    expect(label?.childNodes[0].textContent).toMatch(/^S0\S*…$/);
    expect(label?.querySelector("title")?.textContent).toBe(names[0]);
  });
});

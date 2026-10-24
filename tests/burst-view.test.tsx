import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import FrameGroupStrip from "../src/features/cycle/components/FrameGroupStrip";
import { burstCameras } from "./fixtures";
import { burstPartView } from "./cycle-visual-fixtures";

/** 默认判定的唯一区域（整条胶路未分段）改几个字段。 */

describe("段配方帧组条", () => {
  it("某台相机缺帧 → 该组的缺帧格为灰色缺帧状态、其余相机格照常，整组标不完整", () => {
    const burst = burstPartView().burst!;
    render(<FrameGroupStrip groups={burst.groups} cameras={burst.cameras} selected={null} />);
    const incomplete = screen.getByRole("button", { name: "帧组 5 · 不完整" });
    const cells = within(incomplete).getAllByRole("img");
    expect(cells.map(cell => [cell.getAttribute("aria-label"), cell.getAttribute("data-state")])).toEqual([
      [`${burstCameras[0]} · OK`, "ok"], [`${burstCameras[1]} · 缺帧`, "missing"], [`${burstCameras[2]} · OK`, "ok"],
    ]);
    const complete = within(screen.getByRole("button", { name: "帧组 4" })).getAllByRole("img");
    expect(complete.every(cell => cell.getAttribute("data-state") === "ok")).toBe(true);
  });
});

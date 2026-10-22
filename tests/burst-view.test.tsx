import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import FrameGroupStrip from "../src/features/cycle/components/FrameGroupStrip";
import RegionResultPanel from "../src/features/cycle/components/RegionResultPanel";
import type { BurstJudgement, RegionResult } from "../src/features/plan/types";
import { burstCameras, burstJudgement } from "./fixtures";
import { burstPartView } from "./cycle-visual-fixtures";

/** 默认判定的唯一区域（整条胶路未分段）改几个字段。 */
const withRegion = (over: Partial<RegionResult>): BurstJudgement => {
  const base = burstJudgement();
  return { ...base, regions: [{ ...base.regions[0], ...over }] };
};

describe("段配方区域结果", () => {
  it.each([
    ["在容差内", { frameCount: 12, countStatus: "normal" }, ["帧数正常"], ["帧数偏多", "帧数偏少"]],
    ["高于上限", { frameCount: 16, countStatus: "high" }, ["帧数偏多（16 / 12 ± 3）"], ["帧数正常"]],
    ["低于下限", { frameCount: 8, countStatus: "low" }, ["帧数偏少（8 / 12 ± 3）"], ["帧数正常"]],
  ] as const)("区域帧数%s → 按状态标注帧数与预期，结论标签不变", (_, over, shown, hidden) => {
    render(<RegionResultPanel judgement={withRegion({ ...over })} />);
    const region = within(screen.getByRole("list", { name: "区域" }));
    for (const text of shown) expect(region.getByText(new RegExp(text.replace(/[()]/g, "\\$&")))).toBeVisible();
    for (const text of hidden) expect(region.queryByText(new RegExp(text))).not.toBeInTheDocument();
    expect(region.getByText("OK")).toBeVisible();
  });

  it("整条胶路是未分段区域且有未检区间 → 区域标未分段原因与胶偏未判，列出未检区域和未检项", () => {
    render(<RegionResultPanel judgement={burstJudgement()} />);
    const regions = within(screen.getByRole("list", { name: "区域" }));
    expect(regions.getByText("段 1–3 · 未分段")).toBeVisible();
    expect(regions.getByText("分段算法未就绪")).toBeVisible();
    expect(regions.getByText("胶偏未判：未分段")).toBeVisible();
    expect(regions.getByText("不完整组 1")).toBeVisible();
    expect(within(screen.getByRole("list", { name: "未检区域" })).getAllByRole("listitem").map(li => li.textContent)).toEqual(["段 1 · 0–5 mm", "段 1 · 115–120 mm"]);
    expect(screen.getByText(/未检项（本轮没有算法）：气泡、拉丝/)).toBeVisible();
  });

  it("直线区域有胶偏统计、弯道区域标“弯道不判” → 弯道不挂胶偏未判徽标", () => {
    const base = burstJudgement(), region = base.regions[0];
    const straight: RegionResult = { ...region, region: { kind: { type: "straight", segment: 1 }, firstSeq: 1, lastSeq: 6 }, frameCount: 6, expected: null, countStatus: null,
      offset: { meanShift: 0.35, maxDev: 0.8 }, offsetUnjudged: null, incompleteGroups: [] };
    const corner: RegionResult = { ...straight, region: { kind: { type: "corner", segment: 2 }, firstSeq: 7, lastSeq: 12 }, offset: null };
    render(<RegionResultPanel judgement={{ ...base, regions: [straight, corner], uninspected: [], unchecked: [] }} />);
    const [first, second] = within(screen.getByRole("list", { name: "区域" })).getAllByRole("listitem");
    expect(within(first).getByText("均值偏移 0.35 · 最大偏差 0.80 mm")).toBeVisible();
    expect(within(second).getByText("弯道不判")).toBeVisible();
    expect(screen.queryByText(/胶偏未判/)).not.toBeInTheDocument();
  });
});

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

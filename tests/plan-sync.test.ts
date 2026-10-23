import { describe, expect, it } from "vitest";
import type { PlanExport } from "../src/features/plan/types";
import { planSyncItems } from "../src/features/workspace/PlcParams";
import { burstRecipe, pathSegment, planExport } from "./fixtures";

/** 已发布的计划：三段都标定（5 + 4 + 3 = 12），容差各 1。 */
const published = (): PlanExport => planExport(burstRecipe({
  planVersion: 3,
  segments: [pathSegment(1, "straight", { expectedFrames: 5, lengthMm: 120 }), pathSegment(2, "corner", { expectedFrames: 4 }), pathSegment(3, "straight", { expectedFrames: 3 })],
}));

describe("发布确认的 PLC 同步项（计划身份）", () => {
  it("只改容差、长度和检测区间 → 身份没变，不列任何同步项", () => {
    const plan = published();
    const changed = { ...plan, toleranceTotal: 6, segments: plan.segments.map(s => ({ ...s, tolerance: 2, lengthMm: 99, inspectRange: { aMm: 1, bMm: 2 } })) };
    expect(planSyncItems(plan, changed)).toEqual([]);
  });

  it.each<[string, (plan: PlanExport) => PlanExport, string[]]>([
    ["整件预期总帧数 12 → 14", plan => ({ ...plan, expectedTotal: 14 }), ["shotCount / deviceShots（ΣN_exp）：12 → 14"]],
    ["第 2 段由弯道改成直线", plan => ({ ...plan, segments: plan.segments.map(s => s.index === 2 ? { ...s, kind: "straight" as const } : s) }), ["段序列："]],
    ["相机顺序对调", plan => ({ ...plan, cameras: [plan.cameras[1], plan.cameras[0], plan.cameras[2]] }), ["相机顺序："]],
    ["产品代码改变", plan => ({ ...plan, productCode: plan.productCode + 1 }), ["productCode："]],
  ])("%s → 列出 planVersion 换新号和这一项，别的不列", (_, change, expected) => {
    const items = planSyncItems(published(), change(published()));
    expect(items[0]).toContain("planVersion：3 → 发布时分配新号");
    expect(items.slice(1)).toHaveLength(expected.length);
    expected.forEach((text, i) => expect(items[i + 1]).toContain(text));
  });

  it("首次发布 → 列出候选的全部身份项，待标定的整件总数写“待标定”", () => {
    const items = planSyncItems(null, { ...published(), expectedTotal: null });
    expect(items.map(item => item.split("：")[0])).toEqual(["planVersion", "shotCount / deviceShots（ΣN_exp）", "productCode", "段序列", "相机顺序"]);
    expect(items[1]).toBe("shotCount / deviceShots（ΣN_exp）：待标定");
  });
});

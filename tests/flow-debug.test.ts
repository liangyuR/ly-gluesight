import { describe, expect, it } from "vitest";
import type { GraphDoc } from "@lyflow/editor";
import { flowChanges } from "../src/features/workspace/flowDebug";

const doc = (contrastMin: number, mergeGap: number, points: string, form: string): GraphDoc => ({
  schemaVersion: 1, id: "g", name: "cam2 · 段 2",
  nodes: [{ id: "n_width", op: "glue.bead_width", params: { form } }],
  edges: [],
  params: {
    contrastMin: { type: "float", advanced: true, default: contrastMin, binds: ["n_width.contrastMin"] },
    mergeGap: { type: "float", advanced: true, default: mergeGap, binds: ["n_width.mergeGap"] },
    points: { default: points, binds: ["n_path.points"] },
  },
});

describe("流程调试的改动", () => {
  it("改了专家参数、业务图参数与节点参数 → 只有改过的专家参数可存，其余列为不保存", () => {
    const changes = flowChanges(doc(16, 8, "[[0,0],[9,0]]", "straight"), doc(24, 8, "[[0,0],[9,9]]", "curve"));
    expect(changes.expert).toEqual({ contrastMin: 24 });
    expect(changes.ignored).toEqual(["图参数 points", "节点 n_width 的 form"]);
  });
});

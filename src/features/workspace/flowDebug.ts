import type { GraphDoc } from "@lyflow/editor";
import type { ExpertParams } from "../plan/types";

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** 编辑器里的改动能存进候选的部分：`expert` 是取值与打开时不同的专家参数（顶层图参数里 `advanced: true` 的）；
 * 其余顶层图参数与节点参数的改动只在本页试跑，列在 `ignored` 里提示。 */
export interface FlowChanges { expert: ExpertParams; ignored: string[] }

export function flowChanges(opened: GraphDoc, current: GraphDoc): FlowChanges {
  const expert: ExpertParams = {};
  const ignored: string[] = [];
  for (const [name, spec] of Object.entries(current.params ?? {})) {
    const before = opened.params?.[name];
    if (before && same(before.default, spec.default)) continue;
    if (before?.advanced && typeof spec.default === "number" && Number.isFinite(spec.default)) expert[name] = spec.default;
    else ignored.push(`图参数 ${spec.label ?? name}`);
  }
  for (const node of current.nodes) {
    const before = opened.nodes.find(item => item.id === node.id);
    const keys = new Set([...Object.keys(node.params ?? {}), ...Object.keys(before?.params ?? {})]);
    for (const key of keys) if (!same(node.params?.[key], before?.params?.[key])) ignored.push(`节点 ${node.id} 的 ${key}`);
  }
  return { expert, ignored };
}

import { useEffect, useState } from "react";
import type { Readiness, ReadinessTarget, RecipeOption } from "./types";

export const targetLinks: Record<ReadinessTarget, { path: string; label: string }> = {
  plc: { path: "/plc", label: "去 PLC 通讯" },
  camera: { path: "/camera", label: "去设备与采集" },
  workspace: { path: "/recipe", label: "去配方工作台" },
  settings: { path: "/settings", label: "去系统设置" },
  offline: { path: "/offline", label: "去离线控制台" },
};

export const devLyflowHint = "开发构建：用 pnpm dev 启动会先恢复锁定版本 lyFlow 并设置 LYFLOW_CORE_DLL；恢复失败（无凭证或离线）时只告警继续启动。系统设置里已保存的核心库路径优先于该环境变量。";

/** 运行配方的显示值：运行中保持用户所选；所选配方暂不可运行时为空，不改选别的；从未选过时为第一个可运行配方。 */
export function runChoice(options: RecipeOption[] | null, chosen: string | null, running: boolean): string {
  if (running) return chosen ?? "";
  if (chosen === null) return options?.find(o => o.runnable)?.id ?? "";
  return options?.some(o => o.runnable && o.id === chosen) ? chosen : "";
}

/** 记住用户选择的运行配方（首次有可运行配方时默认第一个）。 */
export function useRunChoice(options: RecipeOption[] | null, running: boolean) {
  const [chosen, setChosen] = useState<string | null>(null);
  const fallback = chosen === null ? options?.find(o => o.runnable)?.id : undefined;
  useEffect(() => { if (fallback) setChosen(fallback); }, [fallback]);
  return [runChoice(options, chosen, running), setChosen] as const;
}

/** 在制件期间不显示开工清单；有阻塞或等待项时显示。 */
export function showChecklist(readiness: Readiness | null, busy: boolean): readiness is Readiness {
  return !!readiness && !busy && readiness.items.some(i => i.state !== "ok");
}

/** 运行被拒后的去处：所选配方不可运行时去它的去处，否则去第一个未就绪项的去处。 */
export function rejectionTarget(readiness: Readiness | null, recipeId: string): ReadinessTarget | null {
  if (!readiness) return null;
  const recipe = readiness.recipes.find(r => r.id === recipeId);
  if (recipe && !recipe.runnable) return recipe.target;
  return readiness.items.find(i => i.state !== "ok" && i.target)?.target ?? null;
}

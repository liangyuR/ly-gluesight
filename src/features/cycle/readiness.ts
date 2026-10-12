import type { Readiness, ReadinessTarget, RecipeOption } from "./types";

export const targetLinks: Record<ReadinessTarget, { path: string; label: string }> = {
  plc: { path: "/plc", label: "去 PLC 通讯" },
  camera: { path: "/camera", label: "去设备与采集" },
  workspace: { path: "/recipe", label: "去配方工作台" },
  settings: { path: "/settings", label: "去系统设置" },
  offline: { path: "/offline", label: "去离线控制台" },
};

export const devLyflowHint = "开发构建：用 pnpm dev 启动会先恢复锁定版本 lyFlow 并设置 LYFLOW_CORE_DLL；恢复失败（无凭证或离线）时只告警继续启动。系统设置里已保存的核心库路径优先于该环境变量。";

/** 已选的可运行配方仍可用就保留，否则选第一个可运行配方；没有可运行配方时为空。 */
export function pickRunnable(options: RecipeOption[], current: string): string {
  if (options.some(o => o.runnable && o.id === current)) return current;
  return options.find(o => o.runnable)?.id ?? "";
}

/** 运行被拒后的去处：所选配方不可运行时去它的去处，否则去第一个未就绪项的去处。 */
export function rejectionTarget(readiness: Readiness | null, recipeId: string): ReadinessTarget | null {
  if (!readiness) return null;
  const recipe = readiness.recipes.find(r => r.id === recipeId);
  if (recipe && !recipe.runnable) return recipe.target;
  return readiness.items.find(i => i.state !== "ok" && i.target)?.target ?? null;
}

import type { ReadinessTarget } from "./types";

export const targetLinks: Record<ReadinessTarget, { path: string; label: string }> = {
  plc: { path: "/plc", label: "去 PLC 通讯" },
  camera: { path: "/camera", label: "去相机" },
  workspace: { path: "/recipe", label: "去配方工作台" },
  settings: { path: "/settings", label: "去系统设置" },
};

export const devLyflowHint = "开发构建：先执行 pnpm deps:lyflow:dev，把锁定系列的 lyFlow 运行库复制到程序同目录的 runtime/lyflow，再 pnpm dev；联合开发用 pnpm dev:joint -LyFlowRuntime 临时指向本机运行库。";

import { invoke, isTauri } from "@tauri-apps/api/core";

export const desktopAvailable = () => isTauri();
export function desktopCall<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) return Promise.reject(new Error("此操作需要桌面后端。请在 GlueSight · 胶路智检 桌面软件中执行。"));
  return invoke<T>(command, args);
}

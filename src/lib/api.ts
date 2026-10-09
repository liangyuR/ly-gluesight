import { invoke, isTauri } from "@tauri-apps/api/core";

export interface AppInfo {
  name: string;
  version: string;
}

export interface EngineStatus {
  backend: string;
  ready: boolean;
  message: string;
  version?: string | null;
  path?: string | null;
  measuring?: boolean;
}

export async function getAppInfo(): Promise<AppInfo> {
  if (!isTauri()) return { name: "GlueSight · 胶路智检", version: "dev" };
  return invoke<AppInfo>("app_info");
}

export async function getEngineStatus(): Promise<EngineStatus> {
  if (!isTauri()) return { backend: "LyFlow", ready: false, message: "浏览器预览模式" };
  return invoke<EngineStatus>("engine_status");
}

import { invoke, isTauri } from "@tauri-apps/api/core";

export interface AppInfo {
  name: string;
  version: string;
  gitSha?: string;
  buildTime?: string;
  logDir?: string | null;
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

export async function openLogDir(): Promise<void> {
  if (!isTauri()) return;
  await invoke("app_open_log_dir");
}

export async function getEngineStatus(): Promise<EngineStatus> {
  if (!isTauri()) return { backend: "LyFlow", ready: false, message: "浏览器预览模式" };
  return invoke<EngineStatus>("engine_status");
}

/** 示教 MCP 服务（docs/integration/agent-teaching.md）：off 尚未运行或浏览器预览，listening 在监听，failed 启动失败 */
export interface AgentMcpStatus {
  state: "off" | "listening" | "failed";
  url?: string | null;
  message?: string | null;
}

export async function getAgentMcpStatus(): Promise<AgentMcpStatus> {
  if (!isTauri()) return { state: "off" };
  return invoke<AgentMcpStatus>("agent_mcp_status");
}

import { desktopCall } from "../../lib/desktop";
import type { AppLogPage, AppLogQuery, DiagnosticsRequest, DiagnosticsResult, LogUsage, Timeline } from "./types";

export const logsApi = {
  query: (query: AppLogQuery) => desktopCall<AppLogPage>("applog_query", { query }),
  usage: () => desktopCall<LogUsage>("applog_usage"),
  exportDiagnostics: (request: DiagnosticsRequest) => desktopCall<DiagnosticsResult>("diagnostics_export", { request }),
  timeline: (id: number) => desktopCall<Timeline>("history_timeline", { id }),
  findCycle: (cycleId: string) => desktopCall<number | null>("history_find_cycle", { cycleId }),
  reveal: (path: string) => desktopCall<void>("reveal_path", { path }),
};

export const levelText = { error: "错误", warn: "警告", info: "信息", debug: "调试" } as const;

export function formatBytes(bytes: number) {
  if (bytes >= 1024 ** 3) return (bytes / 1024 ** 3).toFixed(2) + " GB";
  if (bytes >= 1024 ** 2) return (bytes / 1024 ** 2).toFixed(1) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(1) + " KB";
  return bytes + " B";
}

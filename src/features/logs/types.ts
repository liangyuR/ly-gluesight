import type { HistoryScope } from "../history/types";

export type AppLogLevel = "debug" | "info" | "warn" | "error";

export interface AppLogRecord {
  ts: number;
  level: AppLogLevel;
  ev: string;
  msg: string;
  cycleId?: string;
  sn?: string;
  requestSeq?: number;
  recipeId?: string;
  shotId?: string;
  k?: number;
  camera?: string;
  view?: number;
  stage?: string;
  data?: unknown;
}

export interface AppLogQuery {
  from?: number | null;
  to?: number | null;
  levels?: AppLogLevel[];
  keyword?: string | null;
  cycleId?: string | null;
  sn?: string | null;
  offset?: number;
  limit?: number;
}

export interface AppLogPage {
  total: number;
  items: AppLogRecord[];
  files: number;
  bytes: number;
}

export interface LogUsage {
  dir: string;
  files: number;
  bytes: number;
  keepDays: number;
  maxTotalBytes: number;
  segmentBytes: number;
}

export type ImageMode = "none" | "abnormal" | "all";

export interface DiagnosticsRequest {
  from: number;
  to: number;
  scope?: HistoryScope | null;
  historyIds?: number[];
  images: ImageMode;
  maxImageParts: number;
}

export interface DiagnosticsResult {
  path: string;
  bytes: number;
  logs: number;
  plcLogs: number;
  parts: number;
  imageParts: number;
  images: number;
  notes: string[];
}

export interface TimelineEntry {
  ts: number;
  source: "app" | "plc";
  level: AppLogLevel;
  ev: string;
  msg: string;
  stage: string | null;
  k: number | null;
  shotId: string | null;
  camera: string | null;
  data: unknown;
}

export interface Timeline {
  cycleId: string;
  requestSeq: number | null;
  start: number | null;
  end: number | null;
  stages: { key: string; label: string; ms: number | null }[];
  entries: TimelineEntry[];
  truncated: boolean;
  notes: string[];
}

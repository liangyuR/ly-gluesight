import { PROJECT_ID_PATTERN, type ProjectEntry, type ProjectUsage } from "../../lib/project";

export type UsageState = { status: "loading" } | { status: "ok"; usage: ProjectUsage } | { status: "error"; error: string };

const pad = (n: number) => String(n).padStart(2, "0");

export function defaultProjectId(taken: Iterable<string>, now = new Date()): string {
  const day = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`;
  const used = new Set(Array.from(taken, id => id.toLowerCase()));
  for (let n = 1; ; n++) {
    const id = `p-${day}-${pad(n)}`;
    if (!used.has(id.toLowerCase())) return id;
  }
}

export function projectIdError(id: string, entries: ProjectEntry[]): string {
  if (!id) return "请填写项目编号";
  if (!PROJECT_ID_PATTERN.test(id)) return "编号只能包含字母、数字、- 和 _";
  if (entries.some(entry => entry.id.toLowerCase() === id.toLowerCase())) return "编号已被其他项目使用";
  return "";
}

export function projectNameError(name: string, entries: ProjectEntry[], self?: string): string {
  const trimmed = name.trim();
  if (!trimmed) return "请填写项目名称";
  if (entries.some(entry => entry.id !== self && entry.meta?.name.trim() === trimmed)) return "名称已被其他项目使用";
  return "";
}

export function importedProjectName(sourceName: string, entries: ProjectEntry[], label = "导入"): string {
  const base = sourceName.trim();
  const taken = new Set(entries.flatMap(entry => entry.meta ? [entry.meta.name.trim()] : []));
  if (!taken.has(base)) return base;
  for (let n = 1; ; n++) {
    const name = n === 1 ? `${base}（${label}）` : `${base}（${label} ${n}）`;
    if (!taken.has(name)) return name;
  }
}

export const projectLabel = (entry: ProjectEntry) => entry.meta?.name ?? entry.id;

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024, unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export function formatDate(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function formatDateTime(ts: number): string {
  const d = new Date(ts);
  return `${formatDate(ts)} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function recordsText(usage: ProjectUsage): string {
  if (!usage.records) return "0 条";
  const range = usage.firstRecordAt !== null && usage.lastRecordAt !== null ? `（${formatDate(usage.firstRecordAt)} – ${formatDate(usage.lastRecordAt)}）` : "";
  return `${usage.records} 条${range}`;
}

export function usageText(state: UsageState | undefined, format: (usage: ProjectUsage) => string): string {
  if (!state || state.status === "loading") return "计算中";
  if (state.status === "error") return `统计失败：${state.error}`;
  return format(state.usage);
}

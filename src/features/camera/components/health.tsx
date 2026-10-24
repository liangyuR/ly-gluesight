import { useEffect, useState, type CSSProperties } from "react";
import { StatusBadge } from "qing-steel";
import type { CameraConfig, CameraStatus } from "../types";

export type Tone = "ok" | "warn" | "fault" | "off";

export const sourceText = { mvs: "海康 MVS", sim: "合成图（离线）", sampleSet: "样本集（离线）" } as const;

/** 总览与详情共用的相机状态：未绑定、已释放是要人处理的黄色；未就绪是紫色系统故障。 */
export function cameraHealth(config: CameraConfig, status: CameraStatus | null): { tone: Tone; label: string } {
  if (config.source === "mvs" && !config.serial) return { tone: "warn", label: "未绑定" };
  if (status?.released) return { tone: "warn", label: "已释放给 MVS" };
  if (status?.ready) return { tone: "ok", label: "已连接" };
  if (!status) return { tone: "off", label: "读取中" };
  return { tone: "fault", label: "未就绪" };
}

export function StateBadge({ tone, label }: { tone: Tone; label: string }) {
  return <StatusBadge tone={tone}>{label}</StatusBadge>;
}

export function cameraColor(i: number) {
  return { "--cam-color": `var(--cam-${(i % 8) + 1})` } as CSSProperties;
}

export function useNow(ms: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), ms); return () => clearInterval(t); }, [ms]);
  return now;
}

export function ago(ms: number) {
  if (ms < 2000) return "刚刚";
  if (ms < 60_000) return `${Math.floor(ms / 1000)} 秒前`;
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)} 分钟前`;
  return `${Math.floor(ms / 3_600_000)} 小时前`;
}

export function clock(ts: number, ms = false) {
  const d = new Date(ts), p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}${ms ? "." + p(d.getMilliseconds(), 3) : ""}`;
}

export const count = (n: number | null | undefined) => n == null ? "—" : n.toLocaleString("en-US").replace(/,/g, " ");

export function triggerSummary(c: CameraConfig) {
  if (c.source !== "mvs") return sourceText[c.source];
  if (c.triggerSource === "Software") return "Software 软触发";
  return `Line0 · 高电平 · ${c.frameRate} fps · 滤波 ${c.debouncerUs} µs`;
}

import type { Verdict } from "../cycle/types";

export const verdictLabel: Record<Verdict, string> = {
  OK: "OK",
  OK_WITH_EXCURSION: "OK · 局部超差",
  NG_POSITION: "NG · 位置超差",
  NG_WIDTH: "NG · 胶宽超差",
  NG_ABSOLUTE: "NG · 超绝对限",
  NG_GAP: "NG · 断胶",
  ERR_INSPECT: "ERR · 未测成",
};

/** 检测记录里的触发方式。 */
export function triggerModeLabel(mode: string | null | undefined) {
  return mode === "stop" ? "停稳拍" : "飞拍";
}

export function verdictClass(v: Verdict) {
  return v === "OK" ? "vt-ok" : v === "OK_WITH_EXCURSION" ? "vt-exc" : v === "ERR_INSPECT" ? "vt-err" : "vt-ng";
}

export const verdictGroups: { key: string; label: string; verdicts: Verdict[] }[] = [
  { key: "ok", label: "OK", verdicts: ["OK"] },
  { key: "excursion", label: "局部超差", verdicts: ["OK_WITH_EXCURSION"] },
  { key: "ng", label: "NG", verdicts: ["NG_POSITION", "NG_WIDTH", "NG_ABSOLUTE", "NG_GAP"] },
  { key: "err", label: "ERR", verdicts: ["ERR_INSPECT"] },
];

export function formatTime(ts: number) {
  const d = new Date(ts);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
// 旧判定文本的“帧 N”使用从 0 开始的编号，界面统一显示 k1。
export function displayReason(reason: string) {
  return reason.replace(/帧 (\d+)/g, (_, k: string) => "k" + (Number(k) + 1));
}

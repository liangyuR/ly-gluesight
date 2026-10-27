import { Alert, Button, Card, StatusBadge } from "qing-steel";
import { productionBusy } from "../phase";
import type { ReadinessLoadStatus } from "../api";
import type { Readiness, Snapshot } from "../types";
import ReadinessChecklist from "./ReadinessChecklist";

const statusTone = { fault: "fault", busy: "info", waiting: "warn", unknown: "warn", blocked: "ng", ready: "ok" } as const;

export default function InspectionReadiness({ snapshot, readiness, status: loadStatus, error, onDiagnostics, offlineSample = false, resetBlockedReason, resetPendingCheck }: {
  offlineSample?: boolean;
  snapshot: Snapshot | null;
  readiness: Readiness | null;
  status?: ReadinessLoadStatus;
  error?: string | null;
  onDiagnostics?: () => void;
  resetBlockedReason?: string;
  resetPendingCheck?: boolean;
}) {
  if (offlineSample && readiness) {
    const items = readiness.items.filter(item => item.key !== "recipe");
    readiness = { ...readiness, items, ready: items.length > 0 && items.every(item => item.state === "ok") };
  }
  const busy = productionBusy(snapshot);
  const faulted = snapshot?.phase === "FAULT";
  const blocked = readiness?.items.filter(item => item.state === "blocked").length ?? 0;
  const waiting = readiness?.items.some(item => item.state === "waiting") ?? false;
  const alarms = [...new Set(snapshot?.alarms ?? [])];
  const failed = loadStatus === "error";
  const unavailable = loadStatus === "loading" || !readiness;
  const nextAbnormal = failed || (!!readiness && (!readiness.ready || blocked > 0 || waiting));
  const readyCount = readiness?.items.filter(item => item.state === "ok").length ?? 0;
  const pending = readiness?.items.filter(item => item.state !== "ok") ?? [];
  const expanded = faulted || blocked > 0;
  const inline = onDiagnostics && !expanded ? pending.slice(0, 1) : null;
  const hidden = inline ? pending.length - inline.length : 0;
  const entry = [hidden > 0 && `另有 ${hidden} 项未就绪`, readyCount > 0 && `已就绪 ${readyCount} 项`, alarms.length > 0 && `诊断详情 ${alarms.length} 条`].filter(Boolean).join(" · ");
  const status = faulted ? "fault" : busy ? "busy" : failed ? "waiting" : !snapshot || unavailable ? "unknown" : blocked > 0 ? "blocked" : waiting ? "waiting" : readiness?.ready ? "ready" : "blocked";
  const label = status === "unknown" ? "正在确认设备状态"
    : status === "busy" ? nextAbnormal ? "本件检测中／下一件条件异常" : "本件检测中"
    : failed && !faulted ? offlineSample ? "离线设备状态更新失败" : "开工条件更新失败"
    : status === "waiting" ? "等待就绪条件"
    : status === "ready" ? offlineSample ? "离线设备条件已通过" : "就绪，等待工件"
    : blocked > 0 ? `${offlineSample ? "离线设备异常" : "无法开工"} · ${blocked} 项阻塞`
    : faulted ? offlineSample ? "离线设备故障" : "无法开工 · 设备故障" : "暂未就绪";

  return <Card className={`inspection-readiness device-${status}${expanded ? " readiness-expanded" : ""}`} role="region" aria-label="设备状态">
    <div className="readiness-summary">
      <h2 className="panel-title">设备状态</h2>
      <StatusBadge tone={statusTone[status]}>{label}</StatusBadge>
    </div>
    {offlineSample && <p className="muted">候选示教与样本匹配在启动试运行时检查；生产发布条件不用于候选调试。</p>}
    {faulted && <Alert tone="fault">
      {snapshot.fault || "设备故障，请检查设备并复位"}
      {resetBlockedReason && (snapshot.fault?.includes(resetBlockedReason) ? <> · 当前不能复位</> : <> · 不能复位：{resetBlockedReason}</>)}
      {resetPendingCheck && <> · 复位条件核对中</>}
    </Alert>}
    {failed && <Alert tone="warn">
      {busy || faulted ? "开工条件更新失败；" : ""}{readiness ? "下列条件为上次报告，仅供参考" : "暂时无法确认开工条件"}
      {error && <details><summary>查看更新错误</summary><p>{error}</p></details>}
    </Alert>}
    {readiness && <ReadinessChecklist readiness={inline ? { ...readiness, items: inline } : readiness} embedded includeReady={!onDiagnostics} />}
    {onDiagnostics && entry && <Button variant="text" size="sm" className="readiness-diagnostics-entry" onClick={onDiagnostics}>{entry}</Button>}
    {!onDiagnostics && alarms.length > 0 && <details className="readiness-details readiness-diagnostics">
      <summary>诊断详情（{alarms.length}）</summary>
      <ul className="readiness-alarm-list">{alarms.map(alarm => <li key={alarm}>{alarm}</li>)}</ul>
    </details>}
  </Card>;
}

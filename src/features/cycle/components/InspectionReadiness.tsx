import { productionBusy } from "../phase";
import type { Readiness, Snapshot } from "../types";
import ReadinessChecklist from "./ReadinessChecklist";

export default function InspectionReadiness({ snapshot, readiness }: { snapshot: Snapshot | null; readiness: Readiness | null }) {
  const busy = productionBusy(snapshot);
  const faulted = snapshot?.phase === "FAULT";
  const blocked = readiness?.items.filter(item => item.state === "blocked").length ?? 0;
  const waiting = readiness?.items.some(item => item.state === "waiting") ?? false;
  const alarms = [...new Set(snapshot?.alarms ?? [])];
  const status = !snapshot ? "unknown" : faulted ? "blocked" : busy ? "busy" : blocked > 0 ? "blocked" : waiting ? "waiting" : readiness?.ready ? "ready" : readiness ? "blocked" : "unknown";
  const label = status === "unknown" ? "正在确认设备状态"
    : status === "busy" ? "工件检测中"
    : status === "waiting" ? "等待就绪条件"
    : status === "ready" ? "就绪，等待工件"
    : blocked > 0 ? `无法开工 · ${blocked} 项阻塞`
    : faulted ? "无法开工 · 设备故障" : "暂未就绪";

  return <section className={`panel inspection-readiness device-${status}`} aria-label="设备状态">
    <div className="readiness-summary">
      <h2 className="panel-title">设备状态</h2>
      <strong className="readiness-summary-state">{label}</strong>
    </div>
    {faulted && <p className="readiness-fault">{snapshot.fault || "设备故障，请检查设备并复位"}</p>}
    {!busy && readiness && <ReadinessChecklist readiness={readiness} embedded />}
    {alarms.length > 0 && <details className="readiness-details readiness-diagnostics">
      <summary>诊断详情（{alarms.length}）</summary>
      <ul className="readiness-alarm-list">{alarms.map(alarm => <li key={alarm}>{alarm}</li>)}</ul>
    </details>}
  </section>;
}

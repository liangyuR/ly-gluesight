import Modal from "../../plc/components/Modal";
import { verdictLabel } from "../../history";
import type { LogLine, PartView, Readiness, Recipe, Snapshot } from "../types";
import { CycleStepper, SignalLamps } from "./CycleStatus";
import ReadinessChecklist from "./ReadinessChecklist";
import ContextHelp from "./ContextHelp";

export type DiagnosticGroup = "readiness" | "capture" | "plc" | "events";
const groups: [DiagnosticGroup, string][] = [["readiness", "开工检查"], ["capture", "采集与测量"], ["plc", "PLC 信号"], ["events", "事件"]];

export default function InspectionDiagnostics({ group, onGroup, onClose, snapshot, readiness, readinessError, layout, part, selected, logs, offlineSample = false }: {
  offlineSample?: boolean;
  group: DiagnosticGroup; onGroup: (group: DiagnosticGroup) => void; onClose: () => void;
  snapshot: Snapshot | null; readiness: Readiness | null; readinessError?: string | null;
  layout: Recipe | null; part: PartView | null; selected: number; logs: LogLine[];
}) {
  const frame = part?.frames[selected];
  const alarms = [...new Set(snapshot?.alarms ?? [])];
  return <Modal title="检测诊断" className="inspection-drawer" width={520} onClose={onClose}>
    <nav className="diagnostic-tabs" aria-label="诊断分组">{groups.map(([key, label]) => <button key={key} className={`btn small${group === key ? " active" : ""}`} aria-pressed={group === key} onClick={() => onGroup(key)}>{label}</button>)}</nav>
    <section className="diagnostic-content" aria-label={groups.find(([key]) => key === group)?.[1]}>
      {group === "readiness" && <>
        {offlineSample && <p className="notice">{readiness?.candidateDebug ? `下方检查候选 ${readiness.candidateDebug.recipeId} · 修订 ${readiness.candidateDebug.revision}；通过不代表生产发布。` : "下方为已发布配方的开工报告；候选离线试运行不要求生产发布，启动时另行检查。"}</p>}
        {readinessError && <p className="c-warn">开工条件更新失败，旧报告仅供参考：{readinessError}</p>}
        {readiness ? <ReadinessChecklist readiness={readiness} /> : <p className="muted">尚未取得开工报告</p>}
        {snapshot?.fault && <p className="c-err">{snapshot.fault}</p>}
        <h3>原始告警</h3>
        {alarms.length ? <ul className="diagnostic-alarms">{alarms.map(alarm => <li key={alarm}>{alarm}</li>)}</ul> : <p className="muted">暂无告警</p>}
        <dl className="diagnostic-kv"><dt>配方修订</dt><dd>{layout?.revisionId ?? "—"}</dd><dt>工件周期</dt><dd>{part?.cycleId ?? "—"}</dd></dl>
        <CycleStepper snapshot={snapshot} />
      </>}
      {group === "capture" && <>
        <h3>拍照点 {layout?.shots[selected]?.poseId ?? "—"} · k{selected + 1}</h3>
        <dl className="diagnostic-kv">
          <dt>本件设备内顺序</dt><dd>{frame?.ordinal ?? "—"}</dd><dt>设备帧计数</dt><dd>{frame?.frameCounter ?? "—"}</dd><dt>设备触发计数</dt><dd>{frame?.triggerCounter ?? "—"}</dd>
          <dt>本件触发 / 收到</dt><dd>{part ? `${part.triggers} / ${part.received}` : "—"}</dd><dt>处理队列</dt><dd>{snapshot?.part?.queue ?? 0}</dd>
          <dt>测量线程</dt><dd>{snapshot ? `${snapshot.measurementWorkers.running} / ${snapshot.measurementWorkers.capacity}` : "—"}</dd><dt>超时线程</dt><dd>{snapshot?.measurementWorkers.timedOut ?? 0}</dd><dt>可用容量</dt><dd>{snapshot?.measurementWorkers.availableCapacity ?? "—"}</dd><dt>游离帧</dt><dd>{snapshot?.strayFrames ?? 0}</dd>
        </dl>
        <div className="panel-head"><h3>独立检测图结果</h3><ContextHelp label="多检测图判定说明">本点任意选中图 OK 则本点 OK；其余图的 NG 和检测异常仍保留。采集异常独立处理，本点结果不代表整件判定。</ContextHelp></div>
        {frame?.viewResults?.length ? <div className="table-wrap"><table className="table"><thead><tr><th>检测图</th><th>结果</th><th>耗时</th><th>异常明细</th></tr></thead><tbody>{frame.viewResults.map(view => <tr key={view.view}><td>{view.view}</td><td>{verdictLabel[view.verdict]}</td><td>{view.ms == null ? "—" : `${view.ms} ms`}</td><td>{view.error ?? "—"}</td></tr>)}</tbody></table></div> : <p className="muted">暂无独立检测图结果</p>}
        {frame?.error && <p className="c-err">{frame.error}</p>}
      </>}
      {group === "plc" && <><h3>握手信号</h3><SignalLamps /><CycleStepper snapshot={snapshot} /><p className="muted">信号来自当前 PLC 点位；未绑定标签不代表信号为低。</p></>}
      {group === "events" && <div className="event-log">{logs.length ? logs.slice(-200).reverse().map((line, i) => <div key={`${line.ts}-${i}`} className={`l-${line.level}`}><time className="tm">{new Date(line.ts).toLocaleTimeString("zh-CN", { hour12: false })}.{String(line.ts % 1000).padStart(3, "0")}</time><span className="ev">{line.ev}</span>{line.msg}</div>) : <p className="muted">暂无事件</p>}</div>}
    </section>
  </Modal>;
}

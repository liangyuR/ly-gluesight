import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { PackageOpen } from "lucide-react";
import { formatTs } from "../plc/time";
import { errorText } from "../../lib/errors";
import { levelText, logsApi } from "./api";
import DiagnosticsDialog from "./DiagnosticsDialog";
import type { Timeline } from "./types";

const stageTone: Record<string, string> = { partStart: "info", armed: "info", partEnd: "info", result: "ok", released: "ok", fault: "ng", reset: "warn" };

function relative(ms: number) {
  return (ms >= 0 ? "+" : "") + (Math.abs(ms) >= 10_000 ? (ms / 1000).toFixed(2) + " s" : ms + " ms");
}

export default function CycleTimeline({ historyId, cycleId, ts, selected, onSelectShot }: {
  historyId: number; cycleId: string | null; ts: number; selected: number; onSelectShot: (k: number) => void;
}) {
  const [timeline, setTimeline] = useState<Timeline | null>(null);
  const [error, setError] = useState("");
  const [detail, setDetail] = useState(true);
  const [plc, setPlc] = useState(true);
  const [onlyShot, setOnlyShot] = useState(false);
  const [exporting, setExporting] = useState(false);
  useEffect(() => {
    let alive = true;
    setTimeline(null); setError("");
    if (!cycleId) return;
    logsApi.timeline(historyId).then(t => alive && setTimeline(t)).catch(e => alive && setError(errorText(e)));
    return () => { alive = false; };
  }, [historyId, cycleId]);
  const start = timeline?.entries.find(e => e.stage === "partStart")?.ts ?? timeline?.start ?? ts;
  const rows = useMemo(() => (timeline?.entries ?? []).filter(e => (detail || e.level !== "debug") && (plc || e.source !== "plc") && (!onlyShot || e.k === selected)),
    [timeline, detail, plc, onlyShot, selected]);
  const errors = timeline?.entries.filter(e => e.level === "error").length ?? 0;
  return <section className="panel wp-panel timeline-panel" aria-label="本件事件时间线">
    <div className="wp-panel-head"><div><h2>本件事件时间线</h2><p className="muted">按 cycleId 关联的运行日志与同时段 PLC 信号变化；点拍照点可在上方查看对应原图</p></div>
      <div className="row">
        {cycleId && <Link className="btn small" to={`/logs?cycleId=${encodeURIComponent(cycleId)}`}>在运行日志中查看</Link>}
        <button className="btn small" onClick={() => setExporting(true)}><PackageOpen size={14} />导出本件诊断包</button>
      </div>
    </div>
    {!cycleId ? <p className="muted">本记录没有 cycleId，无法关联运行日志。</p> : error ? <p className="c-warn">{error}</p> : !timeline ? <p className="muted">正在读取本件日志…</p> : <>
      <div className="timeline-stages">
        {timeline.requestSeq != null && <span className="timeline-stage"><span>PLC 请求序号</span><b className="mono">{timeline.requestSeq}</b></span>}
        {timeline.stages.map(s => <span key={s.key} className={"timeline-stage" + (s.key === "total" ? " total" : "")}><span>{s.label}</span><b className="mono">{s.ms == null ? "—" : s.ms + " ms"}</b></span>)}
        {errors > 0 && <span className="timeline-stage error"><span>错误</span><b className="mono">{errors}</b></span>}
      </div>
      {timeline.notes.map((note, i) => <p key={i} className="c-warn">{note}</p>)}
      <div className="filter-row">
        <label className="check"><input type="checkbox" checked={detail} onChange={e => setDetail(e.target.checked)} />逐帧明细（调试级）</label>
        <label className="check"><input type="checkbox" checked={plc} onChange={e => setPlc(e.target.checked)} />PLC 信号</label>
        <label className="check"><input type="checkbox" checked={onlyShot} onChange={e => setOnlyShot(e.target.checked)} />只看选中拍照点 k{selected + 1}</label>
        <span className="spacer" /><span className="muted">{rows.length} / {timeline.entries.length} 条</span>
      </div>
      <div className="table-wrap timeline-table">
        <table className="table log-table app-log-table" aria-label="本件事件">
          <thead><tr><th>相对</th><th>时间</th><th>来源</th><th>级别</th><th>事件</th><th>拍照点</th><th>内容</th></tr></thead>
          <tbody>{rows.map((e, i) => <tr key={i} className={(e.stage && stageTone[e.stage] ? "timeline-mark " : "") + (e.level === "error" ? "log-row-error" : "") + (e.k != null && e.k === selected ? " timeline-selected" : "")}>
            <td className="mono nowrap">{relative(e.ts - start)}</td>
            <td className="mono nowrap">{formatTs(e.ts).slice(11)}</td>
            <td><span className={"tag" + (e.source === "plc" ? " subtle" : "")}>{e.source === "plc" ? "PLC" : "运行"}</span></td>
            <td><span className={`badge level-${e.level}`}>{levelText[e.level]}</span></td>
            <td className="nowrap">{e.ev}</td>
            <td className="nowrap">{e.k != null ? <button className="link mono" aria-label={`查看拍照点 k${e.k + 1}`} onClick={() => onSelectShot(e.k!)}>k{e.k + 1}{e.shotId ? " · " + e.shotId : ""}</button> : ""}</td>
            <td className="msg"><span className="log-msg" title={e.msg}>{e.msg}</span></td>
          </tr>)}{!rows.length && <tr><td colSpan={7} className="muted center">没有符合条件的事件</td></tr>}</tbody>
        </table>
      </div>
    </>}
    {exporting && <DiagnosticsDialog title="导出本件诊断包" historyIds={[historyId]} from={(timeline?.start ?? ts) - 60_000} to={(timeline?.end ?? ts) + 60_000} onClose={() => setExporting(false)} />}
  </section>;
}

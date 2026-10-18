import { useState } from "react";
import { verdictClass, verdictLabel } from "../meta";
import { REPLAY_TOLERANCE_MM, type ShotDiff, type ShotSide } from "../replay";
import "../replay.css";

const verdicts = (vs: ShotSide["verdicts"], empty = "—") => vs.some(v => v.verdict)
  ? <span className="replay-verdicts">{vs.map(({ view, verdict }) => <span key={view} className={verdict ? "vt " + verdictClass(verdict) : "muted"}>{vs.length > 1 ? `图 ${view} · ` : ""}{verdict ? verdictLabel[verdict] : "—"}</span>)}</span>
  : <span className="muted">{empty}</span>;
const side = (s: ShotSide) => <>{verdicts(s.verdicts)}
  <span className="muted"> · 有胶 {s.ok} · 缺胶 {s.gap}{s.inv ? ` · 未测成 ${s.inv}` : ""}</span></>;
const mm = (v: number | null) => v == null ? "—" : v.toFixed(3);

export default function ReplayDiff({ diffs, source, selected, onSelect }: {
  diffs: ShotDiff[] | null; source: "rules" | "raw" | "original"; selected: number; onSelect: (k: number) => void;
}) {
  const [onlyDiff, setOnlyDiff] = useState(true);
  if (!diffs) return <p className="c-warn">候选的拍照点或测点布局与原记录不同，无法逐点对应，只比较整件结论。</p>;
  const checked = diffs.filter(d => !d.skip);
  const differing = checked.filter(d => d.differs);
  const rows = onlyDiff ? differing : checked;
  const skipped = diffs.length - checked.length;
  const title = source === "original" ? "原包重跑" : source === "raw" ? "候选复测" : "候选重判";
  return <div className="replay-diff">
    <div className="wp-actions">
      <span className={differing.length ? "replay-summary differs" : "replay-summary same"} role="status">
        {differing.length ? `${differing.length} / ${checked.length} 个拍照点与原结果不同` : `${checked.length} 个拍照点全部与原结果一致`}
        {source !== "rules" && `（偏移、胶宽差 ≤ ${REPLAY_TOLERANCE_MM} mm 视为一致）`}
      </span>
      {skipped > 0 && <span className="muted">另有 {skipped} 个不检拍照点未列出</span>}
      <span className="spacer" />
      <label className="check"><input type="checkbox" checked={onlyDiff} onChange={e => setOnlyDiff(e.target.checked)} />只看差异</label>
    </div>
    <div className="table-wrap replay-table"><table className="table" aria-label="逐拍照点比对">
      <thead><tr><th>拍照点</th><th>原结果</th><th>{title}</th><th>状态变化</th><th>最大偏移差 mm</th><th>最大胶宽差 mm</th><th>结论</th></tr></thead>
      <tbody>{rows.map(d => <tr key={d.k} className={(d.differs ? "replay-differs" : "") + (d.k === selected ? " replay-selected" : "")}>
        <td><button className="link mono" aria-label={`比对查看拍照点 k${d.k + 1}`} onClick={() => onSelect(d.k)}>k{d.k + 1} · {d.shotId}</button><div className="muted">{d.camera}</div></td>
        <td>{side(d.original)}{d.original.error && <div className="c-err">{d.original.error}</div>}</td>
        <td>{d.remeasured ? side(d.replay) : verdicts(d.replay.verdicts, "未重测")}{d.replay.error && <div className="c-err">{d.replay.error}</div>}</td>
        <td className="mono">{d.remeasured ? d.changed.length : "—"}</td>
        <td className="mono">{mm(d.maxDd)}</td>
        <td className="mono">{mm(d.maxDw)}</td>
        <td>{d.differs ? <b className="c-warn">不同</b> : <span className="c-ok">一致</span>}</td>
      </tr>)}{!rows.length && <tr><td colSpan={7} className="muted center">{onlyDiff ? "没有差异" : "没有拍照点"}</td></tr>}</tbody>
    </table></div>
  </div>;
}
